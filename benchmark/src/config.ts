import path from "node:path";

import { readText } from "./fs-utils.ts";
import { parseYaml } from "./yaml.ts";
import type { BenchmarkConfig, ModelConfig } from "./types.ts";

export interface ProjectPaths {
  rootDir: string;
  benchmarkDir: string;
  configDir: string;
  tasksDir: string;
}

export function getProjectPaths(rootDir = process.cwd()): ProjectPaths {
  const benchmarkDir = path.join(rootDir, "benchmark");
  return {
    rootDir,
    benchmarkDir,
    configDir: path.join(benchmarkDir, "config"),
    tasksDir: path.join(benchmarkDir, "tasks"),
  };
}

export async function loadBenchmarkConfig(paths = getProjectPaths()): Promise<BenchmarkConfig> {
  const filePath = path.join(paths.configDir, "benchmark.yml");
  const parsed = parseYaml(await readText(filePath)) as { benchmark?: BenchmarkConfig };
  if (!parsed.benchmark) {
    throw new Error(`Missing benchmark root in ${filePath}`);
  }
  return parsed.benchmark;
}

export async function loadModels(paths = getProjectPaths()): Promise<ModelConfig[]> {
  const filePath = path.join(paths.configDir, "models.yml");
  const parsed = parseYaml(await readText(filePath)) as { models?: ModelConfig[] };
  if (!Array.isArray(parsed.models)) {
    throw new Error(`Missing models array in ${filePath}`);
  }
  return parsed.models;
}

export function resolveProjectPath(rootDir: string, configuredPath: string): string {
  if (path.isAbsolute(configuredPath)) {
    return configuredPath;
  }
  return path.join(rootDir, configuredPath);
}
