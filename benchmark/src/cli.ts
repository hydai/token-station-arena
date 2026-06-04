import path from "node:path";

import { generateArticle } from "./article.ts";
import { loadBenchmarkConfig, loadModels, getProjectPaths, resolveProjectPath } from "./config.ts";
import { importTokenDump } from "./token-station.ts";
import type { CliOptions } from "./types.ts";

export function parseCliOptions(args: string[]): CliOptions {
  const options: CliOptions = {};

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]!;
    if (!arg.startsWith("--")) {
      continue;
    }

    const key = arg.slice(2);
    const next = args[index + 1];
    const hasValue = next !== undefined && !next.startsWith("--");
    const value = hasValue ? next : "true";
    if (hasValue) index += 1;

    switch (key) {
      case "tasks":
        options.tasks = value;
        break;
      case "models":
        options.models = value;
        break;
      case "runs":
        if (/^\d+$/.test(value)) {
          options.runs = Number.parseInt(value, 10);
        } else {
          options.runsDir = value;
        }
        break;
      case "timeout":
        options.timeout = Number.parseInt(value, 10);
        break;
      case "token-dump":
        options.tokenDump = value;
        break;
      case "skip-judge":
        options.skipJudge = value === "true";
        break;
      case "skip-article":
        options.skipArticle = value === "true";
        break;
      case "dry-run":
        options.dryRun = value === "true";
        break;
      case "output":
        options.output = value;
        break;
      case "input":
        options.input = value;
        break;
      case "run-id":
        options.runId = value;
        break;
      case "runs-dir":
        options.runsDir = value;
        break;
      default:
        throw new Error(`Unknown option: --${key}`);
    }
  }

  return options;
}

export async function runGenerateArticleCli(args: string[]): Promise<void> {
  const options = parseCliOptions(args);
  const paths = getProjectPaths();
  const config = await loadBenchmarkConfig(paths);
  const input = resolveProjectPath(paths.rootDir, options.input ?? config.outputDir);
  const output = resolveProjectPath(paths.rootDir, options.output ?? config.article.outputFile);
  const result = await generateArticle({
    runsDir: input,
    outputPath: output,
    title: config.article.title,
  });
  console.log(`Generated ${path.relative(paths.rootDir, result.outputPath)} from ${result.runCount} run(s).`);
}

export async function runImportTokenDumpCli(args: string[]): Promise<void> {
  const options = parseCliOptions(args);
  const paths = getProjectPaths();
  const config = await loadBenchmarkConfig(paths);
  const input = resolveProjectPath(paths.rootDir, options.input ?? options.tokenDump ?? config.tokenStation.dumpPath);
  const runsDir = resolveProjectPath(paths.rootDir, options.runsDir ?? config.outputDir);
  const result = await importTokenDump({
    inputPath: input,
    runsDir,
    paddingSeconds: config.tokenStation.matchWindowPaddingSeconds,
  });
  console.log(`Token import updated ${result.updated}/${result.runFiles} run(s); unmatched: ${result.unmatched}.`);
}

export async function printConfigSummaryCli(): Promise<void> {
  const paths = getProjectPaths();
  const [config, models] = await Promise.all([loadBenchmarkConfig(paths), loadModels(paths)]);
  console.log(
    JSON.stringify(
      {
        benchmark: config,
        enabledModels: models.filter((model) => model.enabled).map((model) => model.id),
      },
      null,
      2,
    ),
  );
}
