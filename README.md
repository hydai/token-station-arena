# Token Station Arena

Automated Claude Code benchmark tooling for running the same Rust coding tasks across multiple models through `models.bytefuture.ai`, importing Token Station usage dumps, and generating an evidence-backed Markdown article.

## Quick Start

```bash
cp .env.example .env
npm run benchmark -- --tasks all --models all --dry-run
npm run benchmark -- --tasks fix-failing-test --models deepseek-v4-flash,kimi-k2-5 --runs 1 --skip-judge
npm run import-token-dump -- --input benchmark/reports/token-station-usage.json --runs benchmark/runs
npm run generate-article -- --input benchmark/runs --output benchmark/reports/article.md
```

Real benchmark runs require:

- `claude` available on `PATH`.
- `ANTHROPIC_API_KEY` set to a ByteFuture API key.
- Rust/Cargo for fixture setup and deterministic checks.

## Benchmark Layout

- `benchmark/config/models.yml`: canonical model list.
- `benchmark/config/benchmark.yml`: run, judge, Token Station, and article settings.
- `benchmark/tasks/*`: task definitions, prompts, and Rust fixtures.
- `benchmark/runs/*`: per-run workspaces and artifacts.
- `benchmark/reports/*`: generated reports and Token Station dump imports.

Generated run artifacts include stdout/stderr, Claude JSON output, check outputs, git diff, judge output, and `result.json`.
