# Token Station Arena

Automated Claude Code benchmark tooling for running the same Rust coding tasks across multiple models through a configurable Anthropic-compatible gateway and generating an evidence-backed Markdown article.

The MVP includes:

- A Rust benchmark runner built as a single binary (`tokio`, `clap`, `serde`).
- Configurable gateway and model configuration.
- Three Rust benchmark tasks with isolated fixtures.
- Deterministic checks with `cargo test`, `cargo check`, `cargo clippy`, and task-specific scripts.
- Optional LLM judge scoring.
- Token, cost, and timing extraction from `claude -p --output-format json`.
- Markdown article generation.

## Requirements

- Rust and Cargo (1.85 or newer).
- `git`.
- `claude` available on `PATH` for real benchmark runs.
- An API key or bearer token for your Anthropic-compatible gateway, exposed as `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY`.

## Setup

Create a local environment file:

```bash
cp .env.example .env
```

Edit `.env` and set at least:

```bash
ANTHROPIC_AUTH_TOKEN=<your-gateway-api-key>
ANTHROPIC_BASE_URL=https://gateway.example
```

`ANTHROPIC_API_KEY` is also accepted for direct Anthropic-style API key auth. When `ANTHROPIC_AUTH_TOKEN` is set, the runner forwards it to Claude Code so gateways that require `Authorization: Bearer <key>` work without vendor-specific logic.

Custom gateway endpoint:

```bash
ANTHROPIC_BASE_URL=https://custom-gateway.example
ANTHROPIC_AUTH_TOKEN=<your-api-key>
```

Set `ANTHROPIC_BASE_URL` to the gateway root, without `/v1`; Claude Code appends Anthropic paths such as `/v1/messages` itself.

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

Preview the benchmark plan without calling Claude or a remote gateway:

```bash
cargo run --release -- benchmark --tasks all --models all --runs 1 --dry-run
```

After `cargo build --release`, you can invoke the binary directly as `target/release/token-station-arena <command>`.

## Run Benchmarks

Run one task against a small model subset:

```bash
cargo run --release -- benchmark --tasks fix-failing-test --models openai-gpt-5-5,minimax-m2-7 --runs 1
```

Run all configured tasks against all enabled models:

```bash
cargo run --release -- benchmark --tasks all --models all
```

Run several task/model pairs in parallel:

```bash
cargo run --release -- benchmark --tasks all --models all --jobs 3
```

Skip the LLM judge when you only want deterministic checks:

```bash
cargo run --release -- benchmark --tasks add-api-endpoint --models glm-5 --runs 1 --skip-judge
```

Useful options:

| Option | Example | Meaning |
| --- | --- | --- |
| `--tasks` | `all` or `fix-failing-test,refactor-pricing` | Select task IDs. |
| `--models` | `all` or `deepseek-v4-flash,kimi-k2-5` | Select model IDs or provider model IDs. |
| `--runs` | `3` | Number of independent runs per task/model pair. |
| `--timeout` | `1800` | Timeout per Claude run, in seconds. |
| `--jobs` | `3` | Number of task/model runs to execute concurrently; overrides `benchmark.jobs` and must be at least `1`. |
| `--skip-judge` | | Skip LLM judge scoring. |
| `--skip-article` | | Do not regenerate the article after benchmark completion. |
| `--dry-run` | | Print the planned run matrix and command strategy only. |
| `--verbose` | | Print the prepared Claude invocation, full `claude -p` prompt, and failed-check stdout/stderr details. |

## Claude Invocation

Each benchmark run invokes Claude Code with:

```text
claude --bare -p <task prompt> --settings .claude/settings.json --model <provider-model-id> --output-format json
```

The runner also sets these environment variables for the subprocess:

| Variable | Meaning |
| --- | --- |
| `ANTHROPIC_BASE_URL` | Normalized gateway base URL for Claude Code. |
| `ANTHROPIC_API_KEY` | API key forwarded from `ANTHROPIC_API_KEY`, or from `ANTHROPIC_AUTH_TOKEN` when only bearer auth is configured. |
| `ANTHROPIC_AUTH_TOKEN` | Bearer token forwarded when `ANTHROPIC_AUTH_TOKEN` is set. |
| `ANTHROPIC_CUSTOM_MODEL_OPTION` | Provider model ID from `benchmark/config/models.yml`. |
| `ANTHROPIC_MODEL` | Same provider model ID, matching Claude Code model selection. |
| `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` | Set when `benchmark.claude.disableExperimentalBetas` is true. |

`--output-format json` is required. The runner parses `usage`, `modelUsage`, `total_cost_usd`, `duration_ms`, `duration_api_ms`, `ttft_ms`, `time_to_request_ms`, `num_turns`, `terminal_reason`, and `stop_reason` from `claude-output.json`.

## What The Runner Does

For every selected task/model/run combination, the runner:

1. Copies the task fixture into `benchmark/runs/<run-id>/workspace`.
2. Initializes a git baseline for diff capture.
3. Runs task setup commands such as `cargo fetch`.
4. Calls `claude --bare -p <task prompt> --settings .claude/settings.json --model <provider-model-id> --output-format json` through the configured gateway.
5. Loads the fixture-local `.claude/settings.json`.
6. Captures stdout, stderr, Claude JSON output, git diff, and changed files.
7. Extracts Claude Code JSON statistics, including token counts, cost, `duration_ms`, API time, TTFT, and turn count.
8. Runs deterministic checks from `task.yml`.
9. Runs the LLM judge unless disabled.
10. Writes `result.json`.
11. Regenerates the Markdown article unless disabled.

Secrets such as `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are redacted from command output artifacts.

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

The checked-in model list is a sample. Update `provider`, `model`, and `benchmark/config/benchmark.yml`'s `claude.baseUrl` to match the gateway and model IDs you want to test.

Enabled model IDs:

- `openai-gpt-5-5`
- `minimax-m2-7`
- `glm-5`
- `qwen3-32b`
- `gpt-oss-120b`
- `nemotron-3-ultra`

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

## Claude JSON Statistics

`result.json` stores token and cost data in `tokens` and runtime metadata in `claude.statistics`.

Token totals prefer the `modelUsage` breakdown from `claude-output.json`; if it is absent, the runner falls back to top-level `usage`. `tokens.total` is calculated as input + output + cache creation input + cache read input. `tokens.estimatedCostUsd` is read from the per-model `costUSD` breakdown or top-level `total_cost_usd`.

Timing prefers Claude Code's JSON `duration_ms`. The subprocess wall-clock duration is kept only as a fallback when JSON timing is unavailable. The article also reports `duration_api_ms`, `ttft_ms`, and `num_turns` when present.

## Generate The Article

Generate or refresh the Markdown report:

```bash
cargo run --release -- generate-article --input benchmark/runs --output benchmark/reports/article.md
```

The article includes methodology, tested models, task list, a summary table, per-task results, token and latency comparison, three-run stability, judge scores, quality notes, reproducible commands, limitations, and a conclusion.

## Configuration Files

- `benchmark/config/benchmark.yml`: run counts, timeouts, output directories, Claude settings, judge model, and article output path.
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
