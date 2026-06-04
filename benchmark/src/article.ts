import path from "node:path";

import { findFiles, readJson, writeText } from "./fs-utils.ts";
import type { RunResult } from "./types.ts";

interface ModelAggregate {
  modelId: string;
  providerModelId: string;
  total: number;
  passed: number;
  avgJudgeScore: number | null;
  avgInputTokens: number | null;
  avgOutputTokens: number | null;
  avgTokens: number | null;
  avgCostUsd: number | null;
  avgLatencyMs: number | null;
}

export async function generateArticle(params: {
  runsDir: string;
  outputPath: string;
  title: string;
}): Promise<{ outputPath: string; runCount: number }> {
  const runs = await loadRunResults(params.runsDir);
  const markdown = renderArticle(params.title, runs);
  await writeText(params.outputPath, markdown);
  return {
    outputPath: params.outputPath,
    runCount: runs.length,
  };
}

export async function loadRunResults(runsDir: string): Promise<RunResult[]> {
  const files = await findFiles(runsDir, "result.json");
  const runs = await Promise.all(files.map((file) => readJson<RunResult>(file)));
  return runs.sort((a, b) => a.runId.localeCompare(b.runId));
}

export function renderArticle(title: string, runs: RunResult[]): string {
  const models = aggregateByModel(runs);
  const tasks = [...new Set(runs.map((run) => run.taskId))].sort();
  const generatedAt = new Date().toISOString();

  return [
    `# ${title}`,
    "",
    `Generated at: ${generatedAt}`,
    "",
    "## Introduction",
    "",
    "This guide compares Claude Code-style Rust development tasks across models routed through `models.bytefuture.ai`. Each run records command output, deterministic checks, git diff, latency, judge score, and Token Station usage when a backend dump is available.",
    "",
    "## Methodology",
    "",
    "- Every model receives the same task prompt and an isolated copy of the fixture.",
    "- The runner initializes git before model execution and captures the resulting diff.",
    "- Completion is determined by required deterministic checks and, when enabled, an LLM judge.",
    "- Token usage is imported after execution by matching Token Station dump records to model IDs and execution time windows.",
    "",
    "## Tested Models",
    "",
    models.length > 0 ? renderModelList(models) : "No completed run results were found.",
    "",
    "## Tasks",
    "",
    tasks.length > 0 ? tasks.map((taskId) => `- \`${taskId}\``).join("\n") : "No task results were found.",
    "",
    "## Summary",
    "",
    renderSummaryTable(models),
    "",
    "## Per-Task Results",
    "",
    renderPerTaskTables(runs),
    "",
    "## Token And Latency Comparison",
    "",
    renderTokenLatencyTable(models),
    "",
    "## Three-Run Stability",
    "",
    renderStabilityTable(runs),
    "",
    "## Judge Scores",
    "",
    renderJudgeTable(models),
    "",
    "## Quality Notes And Failure Modes",
    "",
    renderQualityNotes(runs),
    "",
    "## Reproducible Commands",
    "",
    "```bash",
    "npm run benchmark -- --tasks all --models all",
    "npm run benchmark -- --tasks fix-failing-test --models deepseek-v4-flash,kimi-k2-5,nemotron-3-super",
    "npm run import-token-dump -- --input benchmark/reports/token-station-usage.json --runs benchmark/runs",
    "npm run generate-article -- --input benchmark/runs --output benchmark/reports/article.md",
    "```",
    "",
    "## Limitations",
    "",
    "- This is a practical engineering benchmark, not an academic benchmark.",
    "- Results depend on the exact task suite, fixture state, gateway behavior, and Claude Code version.",
    "- Token usage is only populated when a Token Station backend dump can be matched confidently.",
    "- Judge scores should be treated as structured review evidence, not as a replacement for human audit.",
    "",
    "## Conclusion",
    "",
    renderConclusion(models),
    "",
  ].join("\n");
}

function aggregateByModel(runs: RunResult[]): ModelAggregate[] {
  const groups = new Map<string, RunResult[]>();
  for (const run of runs) {
    const key = run.modelId;
    groups.set(key, [...(groups.get(key) ?? []), run]);
  }

  return [...groups.entries()]
    .map(([modelId, modelRuns]) => {
      const judgeScores = modelRuns.map((run) => run.judge.score).filter((score) => score !== null);
      const inputTokens = modelRuns.map((run) => run.tokens?.input ?? null).filter((total) => total !== null);
      const outputTokens = modelRuns.map((run) => run.tokens?.output ?? null).filter((total) => total !== null);
      const tokenTotals = modelRuns.map((run) => run.tokens?.total ?? null).filter((total) => total !== null);
      const costs = modelRuns.map((run) => run.tokens?.estimatedCostUsd ?? null).filter((total) => total !== null);
      const latencies = modelRuns.map((run) => run.durationMs).filter((duration) => Number.isFinite(duration));
      return {
        modelId,
        providerModelId: modelRuns[0]?.providerModelId ?? modelId,
        total: modelRuns.length,
        passed: modelRuns.filter((run) => run.completion.status === "passed").length,
        avgJudgeScore: average(judgeScores),
        avgInputTokens: average(inputTokens),
        avgOutputTokens: average(outputTokens),
        avgTokens: average(tokenTotals),
        avgCostUsd: average(costs),
        avgLatencyMs: average(latencies),
      };
    })
    .sort((a, b) => a.modelId.localeCompare(b.modelId));
}

function renderModelList(models: ModelAggregate[]): string {
  return models.map((model) => `- \`${model.modelId}\` (${model.providerModelId})`).join("\n");
}

function renderSummaryTable(models: ModelAggregate[]): string {
  if (models.length === 0) return "No run data is available yet.";
  return [
    "| Model | Pass Rate | Avg Judge Score | Avg Total Tokens | Avg Latency | Notes |",
    "| --- | ---: | ---: | ---: | ---: | --- |",
    ...models.map((model) =>
      [
        `| ${model.modelId}`,
        `${model.passed}/${model.total}`,
        formatNumber(model.avgJudgeScore, 1),
        formatNumber(model.avgTokens, 0),
        formatDuration(model.avgLatencyMs),
        noteFor(model),
        "|",
      ].join(" | "),
    ),
  ].join("\n");
}

function renderPerTaskTables(runs: RunResult[]): string {
  const tasks = [...new Set(runs.map((run) => run.taskId))].sort();
  if (tasks.length === 0) return "No per-task data is available yet.";

  return tasks
    .map((taskId) => {
      const taskRuns = runs.filter((run) => run.taskId === taskId);
      return [
        `### ${taskId}`,
        "",
        "| Model | Run | Status | Required Checks | Judge | Latency |",
        "| --- | ---: | --- | --- | ---: | ---: |",
        ...taskRuns.map((run) => {
          const required = run.checks.map((check) => `${check.name}:${check.passed ? "pass" : "fail"}`).join(", ");
          return `| ${run.modelId} | ${run.runIndex} | ${run.completion.status} | ${required} | ${formatNumber(run.judge.score, 1)} | ${formatDuration(run.durationMs)} |`;
        }),
      ].join("\n");
    })
    .join("\n\n");
}

function renderTokenLatencyTable(models: ModelAggregate[]): string {
  if (models.length === 0) return "No token or latency data is available yet.";
  return [
    "| Model | Avg Input Tokens | Avg Output Tokens | Avg Total Tokens | Avg Cost USD | Avg Latency |",
    "| --- | ---: | ---: | ---: | ---: | ---: |",
    ...models.map((model) => `| ${model.modelId} | ${formatNumber(model.avgInputTokens, 0)} | ${formatNumber(model.avgOutputTokens, 0)} | ${formatNumber(model.avgTokens, 0)} | ${formatNumber(model.avgCostUsd, 4)} | ${formatDuration(model.avgLatencyMs)} |`),
  ].join("\n");
}

function renderStabilityTable(runs: RunResult[]): string {
  if (runs.length === 0) return "No stability data is available yet.";
  const groups = new Map<string, RunResult[]>();
  for (const run of runs) {
    const key = `${run.modelId}:${run.taskId}`;
    groups.set(key, [...(groups.get(key) ?? []), run]);
  }
  return [
    "| Model | Task | Passes | Runs | Statuses |",
    "| --- | --- | ---: | ---: | --- |",
    ...[...groups.values()].map((group) => {
      const first = group[0]!;
      return `| ${first.modelId} | ${first.taskId} | ${group.filter((run) => run.completion.status === "passed").length} | ${group.length} | ${group
        .map((run) => run.completion.status)
        .join(", ")} |`;
    }),
  ].join("\n");
}

function renderJudgeTable(models: ModelAggregate[]): string {
  if (models.length === 0) return "No judge data is available yet.";
  return [
    "| Model | Avg Judge Score | Pass Rate |",
    "| --- | ---: | ---: |",
    ...models.map((model) => `| ${model.modelId} | ${formatNumber(model.avgJudgeScore, 1)} | ${model.passed}/${model.total} |`),
  ].join("\n");
}

function renderQualityNotes(runs: RunResult[]): string {
  const findings = runs.flatMap((run) =>
    run.judge.findings.map((finding) => `- \`${run.modelId}\` on \`${run.taskId}\` run ${run.runIndex}: ${finding.severity}/${finding.category}: ${finding.message}`),
  );
  if (findings.length === 0) {
    return "No judge findings were recorded. Review diffs in run artifacts before publishing stronger claims.";
  }
  return findings.join("\n");
}

function renderConclusion(models: ModelAggregate[]): string {
  if (models.length === 0) {
    return "Run the benchmark to produce evidence-backed recommendations.";
  }
  const best = [...models].sort((a, b) => b.passed / b.total - a.passed / a.total || (b.avgJudgeScore ?? 0) - (a.avgJudgeScore ?? 0))[0]!;
  return `In this run set, \`${best.modelId}\` had the strongest aggregate completion signal with ${best.passed}/${best.total} passing runs. Treat this as task-suite evidence and compare it with token, latency, and diff quality before making deployment decisions.`;
}

function noteFor(model: ModelAggregate): string {
  if (model.total === 0) return "";
  if (model.passed === model.total) return "Completed every recorded run";
  if (model.passed === 0) return "No recorded passes";
  return "Mixed completion; inspect task-level failures";
}

function average(values: number[]): number | null {
  if (values.length === 0) return null;
  return values.reduce((acc, value) => acc + value, 0) / values.length;
}

function formatNumber(value: number | null, digits: number): string {
  if (value === null || Number.isNaN(value)) return "n/a";
  return value.toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

function formatDuration(valueMs: number | null): string {
  if (valueMs === null || Number.isNaN(valueMs)) return "n/a";
  const totalSeconds = Math.round(valueMs / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes > 0 ? `${minutes}m ${seconds}s` : `${seconds}s`;
}
