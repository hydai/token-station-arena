import { runBenchmarkCli } from "../src/runner.ts";

runBenchmarkCli(process.argv.slice(2)).catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
