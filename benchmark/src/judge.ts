import path from "node:path";

import { formatCommand, runProcess } from "./command.ts";
import { writeJson, writeText } from "./fs-utils.ts";
import type { BenchmarkConfig, CheckResult, JudgeResult, LoadedTask, ModelConfig } from "./types.ts";

export interface JudgeRunInput {
  benchmark: BenchmarkConfig;
  task: LoadedTask;
  model: ModelConfig;
  workspaceDir: string;
  runDir: string;
  diff: string;
  changedFiles: string[];
  checks: CheckResult[];
  claudeStdout: string;
  claudeStderr: string;
  timeoutMs: number;
  secrets: string[];
}

export async function runJudge(input: JudgeRunInput): Promise<JudgeResult> {
  const modelId = process.env.JUDGE_MODEL_ID || input.benchmark.judge.model;
  const prompt = buildJudgePrompt(input);
  const promptPath = path.join(input.runDir, "judge-prompt.md");
  const rawOutputPath = path.join(input.runDir, "judge-output.txt");
  await writeText(promptPath, prompt);

  const env = {
    ANTHROPIC_BASE_URL: process.env.BYTEFUTURE_BASE_URL || input.benchmark.claude.baseUrl,
    ANTHROPIC_API_KEY: process.env.ANTHROPIC_API_KEY || "",
    ANTHROPIC_CUSTOM_MODEL_OPTION: modelId,
    ANTHROPIC_MODEL: modelId,
    CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS: input.benchmark.claude.disableExperimentalBetas ? "1" : undefined,
  };
  const args = [
    "--bare",
    "-p",
    prompt,
    "--settings",
    input.benchmark.claude.projectSettingsFile,
    "--model",
    modelId,
    "--output-format",
    "json",
  ];

  const command = await runProcess("claude", args, {
    cwd: input.workspaceDir,
    env,
    timeoutMs: Math.min(input.timeoutMs, 10 * 60 * 1000),
    secrets: input.secrets,
  });

  await writeText(rawOutputPath, `${formatCommand("claude", args)}\n\nSTDOUT\n${command.stdout}\n\nSTDERR\n${command.stderr}`);

  if (command.exitCode !== 0 || command.timedOut) {
    const failed: JudgeResult = {
      enabled: true,
      modelId,
      score: null,
      passed: false,
      correctness: null,
      maintainability: null,
      scopeControl: null,
      hasUnrelatedChanges: false,
      findings: [
        {
          severity: "major",
          category: "judge",
          message: command.timedOut ? "Judge timed out." : `Judge command exited with code ${command.exitCode}.`,
        },
      ],
      rawOutputPath: "judge-output.txt",
      error: command.timedOut ? "Judge timed out." : `Judge command exited with code ${command.exitCode}.`,
    };
    await writeJson(path.join(input.runDir, "judge.json"), failed);
    return failed;
  }

  try {
    const parsed = parseJudgeJson(command.stdout);
    const normalized = normalizeJudge(parsed, modelId, input.benchmark.judge.minimumScore);
    normalized.rawOutputPath = "judge-output.txt";
    await writeJson(path.join(input.runDir, "judge.json"), normalized);
    return normalized;
  } catch (error) {
    const failed: JudgeResult = {
      enabled: true,
      modelId,
      score: null,
      passed: false,
      correctness: null,
      maintainability: null,
      scopeControl: null,
      hasUnrelatedChanges: false,
      findings: [
        {
          severity: "major",
          category: "judge",
          message: error instanceof Error ? error.message : "Judge output could not be parsed.",
        },
      ],
      rawOutputPath: "judge-output.txt",
      error: error instanceof Error ? error.message : "Judge output could not be parsed.",
    };
    await writeJson(path.join(input.runDir, "judge.json"), failed);
    return failed;
  }
}

export function skippedJudge(modelId: string): JudgeResult {
  return {
    enabled: false,
    modelId,
    score: null,
    passed: true,
    correctness: null,
    maintainability: null,
    scopeControl: null,
    hasUnrelatedChanges: false,
    findings: [],
  };
}

export function normalizeJudge(raw: unknown, modelId: string, minimumScore: number): JudgeResult {
  const value = unwrapJudgePayload(raw);
  const score = numberOrNull(value.score);
  const correctness = numberOrNull(value.correctness);
  const maintainability = numberOrNull(value.maintainability);
  const scopeControl = numberOrNull(value.scopeControl);
  const hasUnrelatedChanges = Boolean(value.hasUnrelatedChanges);
  const findings = Array.isArray(value.findings) ? value.findings : [];
  const hasCriticalFinding = findings.some((finding) => {
    const severity = String(finding?.severity ?? "").toLowerCase();
    const category = String(finding?.category ?? "").toLowerCase();
    return severity === "critical" || (severity === "major" && category === "correctness");
  });
  const passed = Boolean(value.passed) && !hasUnrelatedChanges && !hasCriticalFinding && (score === null || score >= minimumScore);

  return {
    enabled: true,
    modelId,
    score,
    passed,
    correctness,
    maintainability,
    scopeControl,
    hasUnrelatedChanges,
    findings: findings.map((finding) => ({
      severity: String(finding?.severity ?? "minor"),
      category: String(finding?.category ?? "general"),
      message: String(finding?.message ?? ""),
    })),
  };
}

function buildJudgePrompt(input: JudgeRunInput): string {
  const payload = {
    task: input.task.config,
    testedModel: {
      id: input.model.id,
      providerModelId: input.model.model,
    },
    changedFiles: input.changedFiles,
    checks: input.checks.map((check) => ({
      name: check.name,
      command: check.command,
      exitCode: check.exitCode,
      passed: check.passed,
      durationMs: check.durationMs,
      timedOut: check.timedOut ?? false,
    })),
  };

  return [
    "You are judging a Claude Code benchmark run. Return only valid JSON matching this schema:",
    '{"score":4,"passed":true,"correctness":4,"maintainability":4,"scopeControl":5,"hasUnrelatedChanges":false,"findings":[{"severity":"minor","category":"maintainability","message":"..."}]}',
    "",
    "Judge criteria:",
    "- Correctness relative to the task prompt and deterministic check results.",
    "- Simplicity, maintainability, and whether the diff overfits tests.",
    "- Scope control. Mark hasUnrelatedChanges true for unjustified changes outside allowed paths.",
    "- Fail if score is below the configured minimum, if severe correctness issues exist, or unrelated changes exist.",
    "",
    `Minimum passing score: ${input.benchmark.judge.minimumScore}`,
    "",
    "Task prompt:",
    fence(input.task.prompt, "markdown"),
    "",
    "Task metadata, changed files, and deterministic checks:",
    fence(JSON.stringify(payload, null, 2), "json"),
    "",
    "Git diff:",
    fence(truncate(input.diff, 80_000), "diff"),
    "",
    "Claude stdout excerpt:",
    fence(truncate(input.claudeStdout, 12_000), "text"),
    "",
    "Claude stderr excerpt:",
    fence(truncate(input.claudeStderr, 12_000), "text"),
  ].join("\n");
}

function parseJudgeJson(stdout: string): unknown {
  const parsed = parsePossibleJson(stdout);
  const unwrapped = unwrapClaudeJson(parsed);
  if (typeof unwrapped === "string") {
    return parsePossibleJson(unwrapped);
  }
  return unwrapped;
}

function parsePossibleJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    const fenced = text.match(/```(?:json)?\s*([\s\S]*?)```/i);
    if (fenced) {
      return JSON.parse(fenced[1]);
    }
    const first = text.indexOf("{");
    const last = text.lastIndexOf("}");
    if (first >= 0 && last > first) {
      return JSON.parse(text.slice(first, last + 1));
    }
    throw new Error("Judge output did not contain parseable JSON.");
  }
}

function unwrapClaudeJson(value: unknown): unknown {
  if (!value || typeof value !== "object") return value;
  const object = value as Record<string, unknown>;
  return object.result ?? object.content ?? object.message ?? value;
}

function unwrapJudgePayload(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object") {
    throw new Error("Judge payload is not an object.");
  }
  const object = value as Record<string, unknown>;
  if (typeof object.result === "string") {
    return parsePossibleJson(object.result) as Record<string, unknown>;
  }
  return object;
}

function numberOrNull(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function fence(value: string, language: string): string {
  return `\`\`\`${language}\n${value.replace(/```/g, "`\\`\\`")}\n\`\`\``;
}

function truncate(value: string, maxChars: number): string {
  if (value.length <= maxChars) return value;
  return `${value.slice(0, maxChars)}\n[truncated ${value.length - maxChars} characters]`;
}
