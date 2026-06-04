import { readFileSync } from "node:fs";

const source = readFileSync("crates/catalog-core/src/pricing.rs", "utf8");
const duplicateExpression = /u32::from\(discount_percent\.min\(100\)\)\s*\/\s*100/g;
const helper = /fn\s+discount_amount_cents\s*\(/;
const duplicateCount = [...source.matchAll(duplicateExpression)].length;

if (!helper.test(source)) {
  console.error("Expected a shared helper named discount_amount_cents.");
  process.exit(1);
}

if (duplicateCount > 1) {
  console.error(`Expected discount percentage arithmetic in one helper, found ${duplicateCount} occurrences.`);
  process.exit(1);
}
