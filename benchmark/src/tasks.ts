import path from "node:path";

import { listDirectories, readText } from "./fs-utils.ts";
import { parseYaml } from "./yaml.ts";
import type { LoadedTask, TaskConfig } from "./types.ts";

export async function loadTasks(tasksDir: string): Promise<LoadedTask[]> {
  const taskDirs = (await listDirectories(tasksDir)).sort();
  const tasks: LoadedTask[] = [];

  for (const taskDir of taskDirs) {
    const taskPath = path.join(taskDir, "task.yml");
    const taskConfig = parseYaml(await readText(taskPath)) as TaskConfig;
    validateTask(taskConfig, taskPath);

    const fixtureDir = path.join(taskDir, taskConfig.fixturePath);
    const promptPath = path.join(taskDir, taskConfig.promptFile);
    tasks.push({
      config: taskConfig,
      taskDir,
      fixtureDir,
      promptPath,
      prompt: await readText(promptPath),
    });
  }

  return tasks;
}

export function selectTasks(tasks: LoadedTask[], selector = "all"): LoadedTask[] {
  if (selector === "all") return tasks;
  const requested = new Set(selector.split(",").map((value) => value.trim()).filter(Boolean));
  const selected = tasks.filter((task) => requested.has(task.config.id));
  const found = new Set(selected.map((task) => task.config.id));
  const missing = [...requested].filter((taskId) => !found.has(taskId));
  if (missing.length > 0) {
    throw new Error(`Unknown task id(s): ${missing.join(", ")}`);
  }
  return selected;
}

function validateTask(task: TaskConfig, filePath: string): void {
  const requiredStringFields = ["id", "title", "description", "fixturePath", "promptFile"] as const;
  for (const field of requiredStringFields) {
    if (typeof task[field] !== "string" || task[field].length === 0) {
      throw new Error(`Task ${filePath} is missing required field: ${field}`);
    }
  }
  if (!Array.isArray(task.setup)) {
    task.setup = [];
  }
  if (!Array.isArray(task.checks)) {
    throw new Error(`Task ${filePath} must define checks`);
  }
  if (!task.success || !Array.isArray(task.success.requiredChecks)) {
    throw new Error(`Task ${filePath} must define success.requiredChecks`);
  }
}
