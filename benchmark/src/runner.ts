import path from "node:path";

import { generateArticle } from "./article.ts";
import { formatCommand, redactEnv, runProcess, runShellCommand } from "./command.ts";
import { getProjectPaths, loadBenchmarkConfig, loadModels, resolveProjectPath } from "./config.ts";
import { classifyCompletion, runChecks } from "./evaluator.ts";
import { copyDir, ensureDir, pathExists, readText, relativeArtifact, writeJson, writeText } from "./fs-utils.ts";
import { runJudge, skippedJudge } from "./judge.ts";
import { selectModels } from "./models.ts";
import { loadTasks, selectTasks } from "./tasks.ts";
import { importTokenDump } from "./token-station.ts";
import type { BenchmarkConfig, CliOptions, CommandResult, LoadedTask, ModelConfig, RunResult } from "./types.ts";

export async function runBenchmarkCli(args: string[]): Promise<void> {
  const { parseCliOptions } = await import("./cli.ts");
  const options = parseCliOptions(args);
  await runBenchmark(options);
}

export async function runBenchmark(options: CliOptions): Promise<void> {
  const paths = getProjectPaths();
  const benchmark = await loadBenchmarkConfig(paths);
  const allModels = await loadModels(paths);
  const allTasks = await loadTasks(paths.tasksDir);
  const models = selectModels(allModels, options.models ?? "all");
  const tasks = selectTasks(allTasks, options.tasks ?? "all");
  const runsPerTaskModel = options.runs ?? benchmark.runsPerTaskModel;
  const timeoutSeconds = options.timeout ?? benchmark.timeoutSeconds;
  const outputDir = resolveProjectPath(paths.rootDir, benchmark.outputDir);
  const reportDir = resolveProjectPath(paths.rootDir, benchmark.reportDir);

  console.log(`Selected ${tasks.length} task(s), ${models.length} model(s), ${runsPerTaskModel} run(s) each.`);
  console.log(`Run artifacts: ${path.relative(paths.rootDir, outputDir)}`);

  if (options.dryRun) {
    printDryRunPlan({ benchmark, tasks, models, runsPerTaskModel, timeoutSeconds });
    return;
  }

  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) {
    throw new Error("ANTHROPIC_API_KEY is required for real benchmark runs. Use --dry-run to inspect the plan without calling Claude.");
  }

  await ensureDir(outputDir);
  await ensureDir(reportDir);

  const results: RunResult[] = [];
  for (const task of tasks) {
    for (const model of models) {
      for (let runIndex = 1; runIndex <= runsPerTaskModel; runIndex += 1) {
        const result = await runSingleBenchmark({
          benchmark,
          task,
          model,
          runIndex,
          outputDir,
          timeoutMs: timeoutSeconds * 1000,
          skipJudge: options.skipJudge ?? false,
        });
        results.push(result);
        console.log(`${result.runId}: ${result.completion.status} (${result.completion.reason})`);
      }
    }
  }

  const tokenDump = options.tokenDump ?? benchmark.tokenStation.dumpPath;
  const tokenDumpPath = resolveProjectPath(paths.rootDir, tokenDump);
  if (benchmark.tokenStation.enabled && (await pathExists(tokenDumpPath))) {
    const importResult = await importTokenDump({
      inputPath: tokenDumpPath,
      runsDir: outputDir,
      paddingSeconds: benchmark.tokenStation.matchWindowPaddingSeconds,
    });
    console.log(`Token import updated ${importResult.updated}/${importResult.runFiles} run(s).`);
  }

  if (!options.skipArticle) {
    const articlePath = resolveProjectPath(paths.rootDir, benchmark.article.outputFile);
    const article = await generateArticle({
      runsDir: outputDir,
      outputPath: articlePath,
      title: benchmark.article.title,
    });
    console.log(`Generated ${path.relative(paths.rootDir, article.outputPath)} from ${article.runCount} run(s).`);
  }
}

export async function runSingleBenchmark(params: {
  benchmark: BenchmarkConfig;
  task: LoadedTask;
  model: ModelConfig;
  runIndex: number;
  outputDir: string;
  timeoutMs: number;
  skipJudge: boolean;
}): Promise<RunResult> {
  const runId = buildRunId(params.task.config.id, params.model.id, params.runIndex);
  const runDir = path.join(params.outputDir, runId);
  const workspaceDir = path.join(runDir, "workspace");
  const checksDir = path.join(runDir, "checks");
  const secrets = [process.env.ANTHROPIC_API_KEY ?? ""].filter(Boolean);

  await ensureDir(runDir);
  await ensureDir(checksDir);
  await copyDir(params.task.fixtureDir, workspaceDir);
  await writeText(path.join(runDir, "prompt.md"), params.task.prompt);
  await writeJson(path.join(runDir, "model-config.json"), params.model);

  const setupResult = await prepareWorkspace(params.task, workspaceDir, params.timeoutMs, secrets);
  if (setupResult) {
    const result = buildInfrastructureErrorResult({
      runId,
      task: params.task,
      model: params.model,
      runIndex: params.runIndex,
      benchmark: params.benchmark,
      runDir,
      workspaceDir,
      startedAt: setupResult.startedAt,
      finishedAt: setupResult.finishedAt,
      durationMs: setupResult.durationMs,
      error: `Setup failed while running "${setupResult.command}".`,
    });
    await writeJson(path.join(runDir, "result.json"), result);
    return result;
  }

  const claude = await runClaude({
    benchmark: params.benchmark,
    task: params.task,
    model: params.model,
    workspaceDir,
    timeoutMs: params.timeoutMs,
    secrets,
  });

  await writeText(path.join(runDir, "stdout.txt"), claude.stdout);
  await writeText(path.join(runDir, "stderr.txt"), claude.stderr);
  await writeJson(path.join(runDir, "claude-output.json"), parseClaudeOutput(claude.stdout));
  await writeJson(path.join(runDir, "command-strategy.json"), {
    command: formatCommand(claude.command, claude.args ?? []),
    env: redactEnv({
      ANTHROPIC_BASE_URL: process.env.BYTEFUTURE_BASE_URL || params.benchmark.claude.baseUrl,
      ANTHROPIC_API_KEY: process.env.ANTHROPIC_API_KEY || "",
      ANTHROPIC_CUSTOM_MODEL_OPTION: params.model.model,
      ANTHROPIC_MODEL: params.model.model,
      CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS: params.benchmark.claude.disableExperimentalBetas ? "1" : undefined,
    }),
  });

  const checks = await runChecks({
    task: params.task,
    workspaceDir,
    runDir,
    timeoutMs: params.timeoutMs,
    secrets,
  });
  const diff = await captureDiff(workspaceDir, secrets);
  const changedFiles = await captureChangedFiles(workspaceDir, secrets);
  await writeText(path.join(runDir, "diff.patch"), diff);

  const judge =
    params.skipJudge || !params.benchmark.judge.enabled
      ? skippedJudge(params.benchmark.judge.model)
      : await runJudge({
          benchmark: params.benchmark,
          task: params.task,
          model: params.model,
          workspaceDir,
          runDir,
          diff,
          changedFiles,
          checks,
          claudeStdout: claude.stdout,
          claudeStderr: claude.stderr,
          timeoutMs: params.timeoutMs,
          secrets,
        });

  const completion = classifyCompletion({
    task: params.task,
    checks,
    judge,
    claudeExitCode: claude.exitCode,
    claudeTimedOut: claude.timedOut,
    changedFiles,
  });

  const result: RunResult = {
    runId,
    taskId: params.task.config.id,
    modelId: params.model.id,
    providerModelId: params.model.model,
    runIndex: params.runIndex,
    provider: params.model.provider,
    startedAt: claude.startedAt,
    finishedAt: claude.finishedAt,
    durationMs: claude.durationMs,
    claudeExitCode: claude.exitCode,
    claude: {
      sessionId: extractClaudeSessionId(claude.stdout),
      outputFormat: params.benchmark.claude.outputFormat,
      commandStrategy: [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
        "--bare",
        "--settings",
        "--model",
        "--output-format json",
      ],
    },
    checks,
    completion,
    tokens: null,
    judge,
    artifacts: {
      stdout: "stdout.txt",
      stderr: "stderr.txt",
      claudeOutput: "claude-output.json",
      diff: "diff.patch",
      workspace: "workspace/",
      checks: "checks/",
      modelConfig: "model-config.json",
    },
    changedFiles,
    warnings: [],
    humanAudit: {
      requiredForMvp: false,
      score: null,
      notes: "",
    },
  };

  await writeJson(path.join(runDir, "result.json"), result);
  return result;
}

async function prepareWorkspace(
  task: LoadedTask,
  workspaceDir: string,
  timeoutMs: number,
  secrets: string[],
): Promise<CommandResult | null> {
  const initCommands = [
    "git init",
    "git config user.email benchmark@example.invalid",
    "git config user.name Benchmark Runner",
  ];

  for (const command of initCommands) {
    const result = await runShellCommand(command, {
      cwd: workspaceDir,
      timeoutMs,
      secrets,
    });
    if (result.exitCode !== 0) return result;
  }

  for (const command of task.config.setup) {
    const result = await runShellCommand(command, {
      cwd: workspaceDir,
      timeoutMs,
      secrets,
    });
    if (result.exitCode !== 0) return result;
  }

  const baselineCommands = ["git add .", "git commit -m baseline"];
  for (const command of baselineCommands) {
    const result = await runShellCommand(command, {
      cwd: workspaceDir,
      timeoutMs,
      secrets,
    });
    if (result.exitCode !== 0) return result;
  }

  return null;
}

async function runClaude(params: {
  benchmark: BenchmarkConfig;
  task: LoadedTask;
  model: ModelConfig;
  workspaceDir: string;
  timeoutMs: number;
  secrets: string[];
}): Promise<CommandResult> {
  const env = {
    ANTHROPIC_BASE_URL: process.env.BYTEFUTURE_BASE_URL || params.benchmark.claude.baseUrl,
    ANTHROPIC_API_KEY: process.env.ANTHROPIC_API_KEY || "",
    ANTHROPIC_CUSTOM_MODEL_OPTION: params.model.model,
    ANTHROPIC_MODEL: params.model.model,
    CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS: params.benchmark.claude.disableExperimentalBetas ? "1" : undefined,
  };
  const args = [
    "--bare",
    "-p",
    params.task.prompt,
    "--settings",
    params.benchmark.claude.projectSettingsFile,
    "--model",
    params.model.model,
    "--output-format",
    params.benchmark.claude.outputFormat,
  ];

  return runProcess("claude", args, {
    cwd: params.workspaceDir,
    env,
    timeoutMs: params.timeoutMs,
    secrets: params.secrets,
  });
}

async function captureDiff(workspaceDir: string, secrets: string[]): Promise<string> {
  const result = await runShellCommand("git diff --binary", {
    cwd: workspaceDir,
    timeoutMs: 60_000,
    secrets,
  });
  return result.stdout || result.stderr;
}

async function captureChangedFiles(workspaceDir: string, secrets: string[]): Promise<string[]> {
  const result = await runShellCommand("git status --short", {
    cwd: workspaceDir,
    timeoutMs: 60_000,
    secrets,
  });
  return result.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => line.slice(3).trim())
    .sort();
}

function parseClaudeOutput(stdout: string): unknown {
  try {
    return JSON.parse(stdout);
  } catch {
    return {
      parseError: "Claude stdout was not valid JSON.",
      stdoutArtifact: "stdout.txt",
    };
  }
}

function extractClaudeSessionId(stdout: string): string | null {
  try {
    const value = JSON.parse(stdout) as Record<string, unknown>;
    for (const key of ["session_id", "sessionId", "id"]) {
      if (typeof value[key] === "string") return value[key];
    }
  } catch {
    return null;
  }
  return null;
}

function buildRunId(taskId: string, modelId: string, runIndex: number): string {
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  return `${timestamp}_${sanitize(taskId)}_${sanitize(modelId)}_${String(runIndex).padStart(3, "0")}`;
}

function sanitize(value: string): string {
  return value.replace(/[^A-Za-z0-9_-]+/g, "-");
}

function printDryRunPlan(params: {
  benchmark: BenchmarkConfig;
  tasks: LoadedTask[];
  models: ModelConfig[];
  runsPerTaskModel: number;
  timeoutSeconds: number;
}): void {
  const total = params.tasks.length * params.models.length * params.runsPerTaskModel;
  console.log(`Dry run only. Planned runs: ${total}`);
  console.log(`Timeout per Claude run: ${params.timeoutSeconds}s`);
  console.log("Command strategy:");
  console.log(
    [
      "ANTHROPIC_BASE_URL=<BYTEFUTURE_BASE_URL>",
      "ANTHROPIC_API_KEY=[REDACTED]",
      "ANTHROPIC_CUSTOM_MODEL_OPTION=<provider-model-id>",
      "ANTHROPIC_MODEL=<provider-model-id>",
      params.benchmark.claude.disableExperimentalBetas ? "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1" : "",
      "claude --bare -p <task prompt> --settings .claude/settings.json --model <provider-model-id> --output-format json",
    ]
      .filter(Boolean)
      .join(" "),
  );
  for (const task of params.tasks) {
    console.log(`Task: ${task.config.id} (${relativeArtifact(process.cwd(), task.fixtureDir)})`);
  }
  for (const model of params.models) {
    console.log(`Model: ${model.id} -> ${model.model}`);
  }
}

function buildInfrastructureErrorResult(params: {
  runId: string;
  task: LoadedTask;
  model: ModelConfig;
  runIndex: number;
  benchmark: BenchmarkConfig;
  runDir: string;
  workspaceDir: string;
  startedAt: string;
  finishedAt: string;
  durationMs: number;
  error: string;
}): RunResult {
  return {
    runId: params.runId,
    taskId: params.task.config.id,
    modelId: params.model.id,
    providerModelId: params.model.model,
    runIndex: params.runIndex,
    provider: params.model.provider,
    startedAt: params.startedAt,
    finishedAt: params.finishedAt,
    durationMs: params.durationMs,
    claudeExitCode: null,
    claude: {
      sessionId: null,
      outputFormat: params.benchmark.claude.outputFormat,
      commandStrategy: [],
    },
    checks: [],
    completion: {
      status: "error",
      reason: params.error,
    },
    tokens: null,
    judge: skippedJudge(params.benchmark.judge.model),
    artifacts: {
      stdout: "stdout.txt",
      stderr: "stderr.txt",
      claudeOutput: "claude-output.json",
      diff: "diff.patch",
      workspace: "workspace/",
      checks: "checks/",
      modelConfig: "model-config.json",
    },
    changedFiles: [],
    warnings: [params.error],
    humanAudit: {
      requiredForMvp: false,
      score: null,
      notes: "",
    },
  };
}
