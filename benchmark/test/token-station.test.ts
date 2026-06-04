import assert from "node:assert/strict";
import test from "node:test";

import { extractUsageRecords } from "../src/token-station.ts";

test("extractUsageRecords accepts common Token Station dump shapes", () => {
  const records = extractUsageRecords({
    records: [
      {
        model: "deepseek/deepseek-v4-flash",
        createdAt: "2026-06-04T10:00:00Z",
        usage: {
          input_tokens: 100,
          output_tokens: 50,
          total_tokens: 150,
          cost_usd: 0.01,
        },
      },
    ],
  });

  assert.equal(records.length, 1);
  assert.equal(records[0]!.modelId, "deepseek/deepseek-v4-flash");
  assert.equal(records[0]!.input, 100);
  assert.equal(records[0]!.output, 50);
  assert.equal(records[0]!.total, 150);
  assert.equal(records[0]!.estimatedCostUsd, 0.01);
});
