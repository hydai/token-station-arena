import { runGenerateArticleCli } from "../src/cli.ts";

runGenerateArticleCli(process.argv.slice(2)).catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
