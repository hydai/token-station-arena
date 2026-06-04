import assert from "node:assert/strict";
import test from "node:test";

import { parseYaml } from "../src/yaml.ts";

test("parseYaml reads nested maps and arrays used by benchmark configs", () => {
  const parsed = parseYaml(`
models:
  - id: deepseek-v4-flash
    enabled: true
    nested:
      checks:
        - cargo test
        - cargo check
  - id: kimi-k2-5
    enabled: false
`) as { models: Array<{ id: string; enabled: boolean; nested?: { checks: string[] } }> };

  assert.equal(parsed.models.length, 2);
  assert.equal(parsed.models[0]!.id, "deepseek-v4-flash");
  assert.equal(parsed.models[0]!.enabled, true);
  assert.deepEqual(parsed.models[0]!.nested?.checks, ["cargo test", "cargo check"]);
  assert.equal(parsed.models[1]!.enabled, false);
});
