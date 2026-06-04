import path from "node:path";

import { loadBenchmarkConfig, getProjectPaths, resolveProjectPath } from "../src/config.ts";
import { runChecks } from "../src/evaluator.ts";
import { readJson, writeJson } from "../src/fs-utils.ts";
import { loadTasks } from "../src/tasks.ts";
import type { RunResult } from "../src/types.ts";

const paths = getProjectPaths();

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const runId = valueAfter(args, "--run-id");
  if (!runId) {
    throw new Error("Usage: npm run evaluate -- --run-id <runId>");
  }

  const config = await loadBenchmarkConfig(paths);
  const runsDir = resolveProjectPath(paths.rootDir, config.outputDir);
  const runDir = path.join(runsDir, runId);
  const resultPath = path.join(runDir, "result.json");
  const result = await readJson<RunResult>(resultPath);
  const task = (await loadTasks(paths.tasksDir)).find((candidate) => candidate.config.id === result.taskId);
  if (!task) throw new Error(`Task not found for result: ${result.taskId}`);

  result.checks = await runChecks({
    task,
    workspaceDir: path.join(runDir, "workspace"),
    runDir,
    timeoutMs: config.timeoutSeconds * 1000,
    secrets: [process.env.ANTHROPIC_API_KEY ?? ""].filter(Boolean),
  });
  await writeJson(resultPath, result);
  console.log(`Re-ran ${result.checks.length} check(s) for ${runId}.`);
}

function valueAfter(args: string[], flag: string): string | null {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] ?? null : null;
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
