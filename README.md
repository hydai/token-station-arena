# Token Station Arena

Automated Claude Code benchmark tooling for running the same Rust coding tasks across multiple models through `models.bytefuture.ai`, importing Token Station usage dumps, and generating an evidence-backed Markdown article.

The MVP includes:

- A Rust benchmark runner built as a single binary (`tokio`, `clap`, `serde`).
- Canonical ByteFuture model configuration.
- Three Rust benchmark tasks with isolated fixtures.
- Deterministic checks with `cargo test`, `cargo check`, `cargo clippy`, and task-specific scripts.
- Optional LLM judge scoring.
- Token Station backend dump import.
- Markdown article generation.

## Requirements

- Rust and Cargo (1.85 or newer).
- `git`.
- `claude` available on `PATH` for real benchmark runs.
- A ByteFuture API key exposed as `ANTHROPIC_API_KEY`.

## Setup

Create a local environment file:

```bash
cp .env.example .env
```

Edit `.env` and set at least:

```bash
ANTHROPIC_API_KEY=<your-bytefuture-api-key>
BYTEFUTURE_BASE_URL=https://models.bytefuture.ai
```

This project reads environment variables from the shell. Before running real benchmarks, export the file into your shell session:

```bash
set -a
source .env
set +a
```

## Build And Validate

Build the optimized binary and run the test suite:

```bash
cargo build --release
cargo test
```

Preview the benchmark plan without calling Claude or ByteFuture:

```bash
cargo run --release -- benchmark --tasks all --models all --runs 1 --dry-run
```

After `cargo build --release`, you can invoke the binary directly as `target/release/token-station-arena <command>`.

## Run Benchmarks

Run one task against a small model subset:

```bash
cargo run --release -- benchmark --tasks fix-failing-test --models deepseek-v4-flash,kimi-k2-5 --runs 1
```

Run all configured tasks against all enabled models:

```bash
cargo run --release -- benchmark --tasks all --models all
```

Skip the LLM judge when you only want deterministic checks:

```bash
cargo run --release -- benchmark --tasks add-api-endpoint --models nemotron-3-super --runs 1 --skip-judge
```

Useful options:

| Option | Example | Meaning |
| --- | --- | --- |
| `--tasks` | `all` or `fix-failing-test,refactor-pricing` | Select task IDs. |
| `--models` | `all` or `deepseek-v4-flash,kimi-k2-5` | Select model IDs or provider model IDs. |
| `--runs` | `3` | Number of independent runs per task/model pair. |
| `--timeout` | `1800` | Timeout per Claude run, in seconds. |
| `--skip-judge` | | Skip LLM judge scoring. |
| `--skip-article` | | Do not regenerate the article after benchmark completion. |
| `--dry-run` | | Print the planned run matrix and command strategy only. |
| `--verbose` | | Print the prepared Claude invocation, full `claude -p` prompt, and failed-check stdout/stderr details. |
| `--token-dump` | `benchmark/reports/token-station-usage.json` | Token Station dump path to import after execution, if present. |

## What The Runner Does

For every selected task/model/run combination, the runner:

1. Copies the task fixture into `benchmark/runs/<run-id>/workspace`.
2. Initializes a git baseline for diff capture.
3. Runs task setup commands such as `cargo fetch`.
4. Calls `claude --bare -p <task prompt>` through `models.bytefuture.ai`.
5. Loads the fixture-local `.claude/settings.json`.
6. Captures stdout, stderr, Claude JSON output, git diff, changed files, and timings.
7. Runs deterministic checks from `task.yml`.
8. Runs the LLM judge unless disabled.
9. Writes `result.json`.
10. Regenerates the Markdown article unless disabled.

Secrets such as `ANTHROPIC_API_KEY` are redacted from command output artifacts.

## Tasks

Current task IDs:

- `fix-failing-test`: fix a Rust pricing bug so tests pass.
- `add-api-endpoint`: add `GET /products/top?limit=<n>` to an Axum API.
- `refactor-pricing`: remove duplicated discount logic while preserving behavior.

Each task lives under `benchmark/tasks/<task-id>/`:

- `task.yml`: setup commands, checks, required checks, and judge policy.
- `prompt.md`: prompt passed to Claude.
- `fixture/`: isolated Rust workspace copied for each run.
- `fixture/.claude/settings.json`: Claude Code permission policy for that fixture.

Each fixture is its own Cargo workspace; the orchestrator excludes `benchmark/tasks` and `benchmark/runs` from its own workspace so `cargo build` never compiles them.

## Models

Models are configured in `benchmark/config/models.yml`.

Enabled canonical model IDs:

- `gpt-oss-20b`
- `gpt-oss-120b`
- `deepseek-v4-flash`
- `deepseek-v4-pro`
- `kimi-k2-5`
- `nemotron-3-super`
- `nemotron-3-nano`

You can disable a model by setting `enabled: false`.

## Artifacts

Run artifacts are written under `benchmark/runs/<run-id>/`:

```text
result.json
prompt.md
stdout.txt
stderr.txt
claude-output.json
command-strategy.json
diff.patch
judge.json
judge-output.txt
checks/
workspace/
```

`benchmark/runs/*` and generated reports are ignored by git. `.gitkeep` files keep the directories present.

## Re-run Evaluation Or Judge

Re-run deterministic checks for an existing run:

```bash
cargo run --release -- evaluate --run-id <run-id>
```

Re-run judge scoring for an existing run:

```bash
cargo run --release -- judge --run-id <run-id>
```

## Import Token Station Usage

After benchmark execution, export or dump Token Station usage from the backend to JSON, then import it:

```bash
cargo run --release -- import-token-dump --input benchmark/reports/token-station-usage.json --runs benchmark/runs
```

The importer matches usage records to benchmark runs by:

- execution time window,
- benchmark model ID or provider model ID,
- configured padding from `benchmark/config/benchmark.yml`.

If no confident match is found, the run keeps token fields empty and receives a warning instead of guessing.

The importer accepts common dump shapes such as:

```json
{
  "records": [
    {
      "model": "deepseek/deepseek-v4-flash",
      "createdAt": "2026-06-04T10:00:00Z",
      "usage": {
        "input_tokens": 12000,
        "output_tokens": 3200,
        "total_tokens": 15200,
        "cost_usd": 0.08
      }
    }
  ]
}
```

## Generate The Article

Generate or refresh the Markdown report:

```bash
cargo run --release -- generate-article --input benchmark/runs --output benchmark/reports/article.md
```

The article includes methodology, tested models, task list, a summary table, per-task results, token and latency comparison, three-run stability, judge scores, quality notes, reproducible commands, limitations, and a conclusion.

## Configuration Files

- `benchmark/config/benchmark.yml`: run counts, timeouts, output directories, Claude settings, judge model, Token Station settings, and article output path.
- `benchmark/config/models.yml`: model list and provider model IDs.
- `.env.example`: required environment variables.

## License

Apache-2.0. See `LICENSE`.

## Development Notes

Run checks before committing changes:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run --release -- benchmark --tasks all --models all --runs 1 --dry-run
```

When adding a new task:

1. Create `benchmark/tasks/<task-id>/task.yml`.
2. Create `benchmark/tasks/<task-id>/prompt.md`.
3. Add a small Rust fixture under `benchmark/tasks/<task-id>/fixture`.
4. Include fixture-local `.claude/settings.json`.
5. Make sure `task.yml` defines deterministic checks and `success.requiredChecks`.

When adding a new model, update `benchmark/config/models.yml` and verify it appears in:

```bash
cargo run --release -- benchmark --tasks all --models all --dry-run
```
