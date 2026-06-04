import path from "node:path";

import { findFiles, readJson, writeJson } from "./fs-utils.ts";
import type { RunResult, TokenUsage } from "./types.ts";

interface UsageRecord {
  raw: Record<string, unknown>;
  modelId: string | null;
  timestamp: Date | null;
  input: number;
  output: number;
  cacheCreationInput: number;
  cacheReadInput: number;
  total: number;
  estimatedCostUsd: number | null;
}

export async function importTokenDump(params: {
  inputPath: string;
  runsDir: string;
  paddingSeconds?: number;
}): Promise<{ updated: number; unmatched: number; runFiles: number }> {
  const dump = await readJson<unknown>(params.inputPath);
  const records = extractUsageRecords(dump);
  const runFiles = await findFiles(params.runsDir, "result.json");
  let updated = 0;
  let unmatched = 0;

  for (const resultPath of runFiles) {
    const result = await readJson<RunResult>(resultPath);
    const matched = matchRecords(result, records, params.paddingSeconds ?? 60);
    if (matched.length === 0) {
      result.tokens = nullTokenUsage(params.inputPath);
      result.warnings = [...(result.warnings ?? []), "No Token Station usage record matched this run by model and execution time window."];
      unmatched += 1;
    } else {
      result.tokens = sumUsage(matched, params.inputPath);
      updated += 1;
    }
    await writeJson(resultPath, result);
  }

  return { updated, unmatched, runFiles: runFiles.length };
}

export function extractUsageRecords(dump: unknown): UsageRecord[] {
  const rows = normalizeRows(dump);
  return rows.map((raw) => {
    const usage = objectAt(raw, "usage");
    const input = firstNumber(raw, usage, ["inputTokens", "input_tokens", "prompt_tokens", "input", "promptTokens"]);
    const output = firstNumber(raw, usage, ["outputTokens", "output_tokens", "completion_tokens", "output", "completionTokens"]);
    const cacheCreationInput = firstNumber(raw, usage, ["cacheCreationInputTokens", "cache_creation_input_tokens", "cacheCreationInput"], 0);
    const cacheReadInput = firstNumber(raw, usage, ["cacheReadInputTokens", "cache_read_input_tokens", "cacheReadInput"], 0);
    const total = firstNumber(raw, usage, ["totalTokens", "total_tokens", "total"], input + output + cacheCreationInput + cacheReadInput);
    const estimatedCostUsd = firstNumberOrNull(raw, usage, ["estimatedCostUsd", "estimated_cost_usd", "costUsd", "cost_usd"]);

    return {
      raw,
      modelId: firstString(raw, ["providerModelId", "provider_model_id", "modelId", "model_id", "model", "modelName"]),
      timestamp: firstDate(raw, ["startedAt", "started_at", "createdAt", "created_at", "timestamp", "time", "requestStartedAt"]),
      input,
      output,
      cacheCreationInput,
      cacheReadInput,
      total,
      estimatedCostUsd,
    };
  });
}

function matchRecords(result: RunResult, records: UsageRecord[], paddingSeconds: number): UsageRecord[] {
  const start = new Date(result.startedAt).getTime() - paddingSeconds * 1000;
  const finish = new Date(result.finishedAt).getTime() + paddingSeconds * 1000;
  const modelCandidates = new Set([result.providerModelId, result.modelId]);

  return records.filter((record) => {
    if (!record.timestamp || !record.modelId) return false;
    const time = record.timestamp.getTime();
    return time >= start && time <= finish && modelCandidates.has(record.modelId);
  });
}

function sumUsage(records: UsageRecord[], dumpFile: string): TokenUsage {
  const input = sum(records, "input");
  const output = sum(records, "output");
  const cacheCreationInput = sum(records, "cacheCreationInput");
  const cacheReadInput = sum(records, "cacheReadInput");
  const total = sum(records, "total");
  const estimatedCostUsdValues = records.map((record) => record.estimatedCostUsd).filter((value) => value !== null);

  return {
    source: "token-station-backend-dump",
    correlation: "execution-time-window",
    dumpFile: path.normalize(dumpFile),
    input,
    output,
    cacheCreationInput,
    cacheReadInput,
    total,
    estimatedCostUsd:
      estimatedCostUsdValues.length > 0 ? estimatedCostUsdValues.reduce((acc, value) => acc + value, 0) : null,
  };
}

function nullTokenUsage(dumpFile: string): TokenUsage {
  return {
    source: "token-station-backend-dump",
    correlation: "execution-time-window",
    dumpFile: path.normalize(dumpFile),
    input: null,
    output: null,
    cacheCreationInput: null,
    cacheReadInput: null,
    total: null,
    estimatedCostUsd: null,
  };
}

function normalizeRows(dump: unknown): Record<string, unknown>[] {
  if (Array.isArray(dump)) return dump.filter(isRecord);
  if (isRecord(dump)) {
    for (const key of ["records", "usage", "rows", "data"]) {
      const value = dump[key];
      if (Array.isArray(value)) return value.filter(isRecord);
    }
  }
  throw new Error("Token Station dump must be an array or an object with records, usage, rows, or data.");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function objectAt(object: Record<string, unknown>, key: string): Record<string, unknown> {
  return isRecord(object[key]) ? object[key] : {};
}

function firstString(object: Record<string, unknown>, keys: string[]): string | null {
  for (const key of keys) {
    const value = object[key];
    if (typeof value === "string" && value.length > 0) return value;
  }
  return null;
}

function firstDate(object: Record<string, unknown>, keys: string[]): Date | null {
  for (const key of keys) {
    const value = object[key];
    if (typeof value === "string" || typeof value === "number") {
      const date = new Date(value);
      if (!Number.isNaN(date.getTime())) return date;
    }
  }
  return null;
}

function firstNumber(
  object: Record<string, unknown>,
  nested: Record<string, unknown>,
  keys: string[],
  fallback = 0,
): number {
  return firstNumberOrNull(object, nested, keys) ?? fallback;
}

function firstNumberOrNull(
  object: Record<string, unknown>,
  nested: Record<string, unknown>,
  keys: string[],
): number | null {
  for (const key of keys) {
    for (const candidate of [object[key], nested[key]]) {
      if (typeof candidate === "number" && Number.isFinite(candidate)) return candidate;
      if (typeof candidate === "string" && candidate.trim() !== "" && Number.isFinite(Number(candidate))) return Number(candidate);
    }
  }
  return null;
}

function sum(records: UsageRecord[], field: "input" | "output" | "cacheCreationInput" | "cacheReadInput" | "total"): number {
  return records.reduce((acc, record) => acc + record[field], 0);
}
