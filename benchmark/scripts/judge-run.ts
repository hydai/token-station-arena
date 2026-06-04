import path from "node:path";

import { loadBenchmarkConfig, loadModels, getProjectPaths, resolveProjectPath } from "../src/config.ts";
import { readJson, readText, writeJson } from "../src/fs-utils.ts";
import { runJudge } from "../src/judge.ts";
import { loadTasks } from "../src/tasks.ts";
import type { RunResult } from "../src/types.ts";

const paths = getProjectPaths();

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const runId = valueAfter(args, "--run-id");
  if (!runId) {
    throw new Error("Usage: npm run judge -- --run-id <runId>");
  }

  const benchmark = await loadBenchmarkConfig(paths);
  const runsDir = resolveProjectPath(paths.rootDir, benchmark.outputDir);
  const runDir = path.join(runsDir, runId);
  const resultPath = path.join(runDir, "result.json");
  const result = await readJson<RunResult>(resultPath);
  const task = (await loadTasks(paths.tasksDir)).find((candidate) => candidate.config.id === result.taskId);
  const model = (await loadModels(paths)).find((candidate) => candidate.id === result.modelId);
  if (!task) throw new Error(`Task not found for result: ${result.taskId}`);
  if (!model) throw new Error(`Model not found for result: ${result.modelId}`);

  result.judge = await runJudge({
    benchmark,
    task,
    model,
    workspaceDir: path.join(runDir, "workspace"),
    runDir,
    diff: await readText(path.join(runDir, "diff.patch")),
    changedFiles: result.changedFiles,
    checks: result.checks,
    claudeStdout: await readText(path.join(runDir, "stdout.txt")),
    claudeStderr: await readText(path.join(runDir, "stderr.txt")),
    timeoutMs: benchmark.timeoutSeconds * 1000,
    secrets: [process.env.ANTHROPIC_API_KEY ?? ""].filter(Boolean),
  });
  await writeJson(resultPath, result);
  console.log(`Judge ${result.judge.passed ? "passed" : "failed"} for ${runId}.`);
}

function valueAfter(args: string[], flag: string): string | null {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] ?? null : null;
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
