# Spec: Automated Multi-Model Claude Code Benchmark and Article Generator

## 1. Overview

This project builds an automated system that runs the same developer tasks across multiple LLMs through `models.bytefuture.ai`, using `claude -p` as the execution interface. The system evaluates whether each task was completed successfully, records Claude Code JSON statistics such as tokens, cost, and duration, and generates a publishable technical article that compares model behavior with reproducible evidence.

The primary output is not a simple announcement that ByteFuture supports a model such as Nemotron. The output is a useful developer guide that shows how different models perform on realistic Claude Code workflows, including `gpt-oss-20b`, `gpt-oss-120b`, DeepSeek models, Kimi, and Nemotron 3 series models.

## 2. Goals

- Run the same coding tasks across selected models using `claude -p`.
- Use `models.bytefuture.ai` as the model provider for every experiment.
- Measure task completion with deterministic checks and an LLM judge.
- Detect unrelated or excessive code changes with an LLM judge.
- Record tokens, cost, API timing, TTFT, turn count, command output, generated diffs, and test results.
- Generate a Markdown report that can be edited into a public article.
- Make the benchmark reproducible from a single command.

## 3. Non-Goals

- This project does not attempt to benchmark every open-weight or open-source LLM in the first version.
- This project does not claim that models have identical quality unless the evidence supports that claim.
- This project does not replace formal academic benchmarking.
- This project does not require a web dashboard for the MVP.
- This project does not require human-only subjective review in the first release.
- This project does not compare provider variants of the same model in the MVP.

## 4. Target Audience

- Developers evaluating which models to use with Claude Code-style workflows.
- ByteFuture users who want practical examples for `models.bytefuture.ai`.
- Internal marketing, developer relations, and engineering teams who need evidence-backed content.
- Users who care about model cost, token count, latency, and real task completion.

## 5. Core Narrative

The generated article should answer:

> Can developers complete the same Claude Code task using different models through `models.bytefuture.ai`, and how do those models compare on completion, tokens, latency, and quality?

The article should avoid vague claims such as:

> Model X has the same quality with fewer tokens.

Instead, it should use evidence-based claims:

> In this task suite, Model X completed 4 of 5 tasks, used 32% fewer tokens than Model Y, and passed all deterministic checks on the completed tasks.

## 6. MVP Scope

The first version should include:

- 3 to 5 coding tasks.
- 7 canonical models.
- Three independent runs per task per model.
- Deterministic completion checks.
- LLM judge scoring for correctness, maintainability, and unrelated changes.
- Token, cost, and latency measurement from `claude -p --output-format json`.
- Generated git diff capture.
- Markdown report generation.

Canonical first-run model IDs from the ByteFuture catalog:

- `groq/gpt-oss-20b`
- `groq/gpt-oss-120b`
- `deepseek/deepseek-v4-flash`
- `deepseek/deepseek-v4-pro`
- `kimi/kimi-k2.5`
- `deepinfra/nemotron-3-super`
- `deepinfra/nemotron-3-nano`

Recommended first task types:

- Fix a failing Rust test.
- Add a small Rust API endpoint in an Axum-based fixture.
- Refactor duplicated Rust business logic.
- Add Rust unit tests or integration tests for an existing module.
- Generate or improve Rust developer documentation.

The tasks should use public toy Rust fixtures, but the work should resemble real engineering tasks. Fixtures must be small enough to publish, yet realistic enough to require repository exploration, implementation, and verification. The first fixture set should be a small Rust workspace with a library crate, an Axum API crate, and unit/integration tests.

## 7. System Architecture

The system consists of five main components:

1. `task-suite`
   - Stores benchmark task definitions.
   - Provides setup commands, prompt text, success checks, and cleanup instructions.

2. `model-runner`
   - Invokes `claude -p` for each task/model pair.
   - Passes model configuration through `models.bytefuture.ai`.
   - Captures stdout, stderr, exit code, start time, end time, and workspace diff.

3. `completion-evaluator`
   - Determines whether the model completed the task.
   - Runs configured checks such as tests, typecheck, lint, or custom scripts.
   - Runs an LLM judge after deterministic checks.
   - Scores correctness, implementation quality, and whether the diff contains unrelated changes.
   - Produces a structured evaluation result.

4. `statistics-recorder`
   - Parses usage data from `claude -p --output-format json`.
   - Records input tokens, output tokens, cache tokens, total tokens, estimated cost, `duration_ms`, API duration, TTFT, and turn count when available.

5. `article-generator`
   - Converts benchmark results into a Markdown guide.
   - Produces summary tables, task-level details, model-level observations, and reproducible commands.

The benchmark orchestration tooling should be implemented in Rust as a single binary crate. It uses `tokio` for subprocess management (running `claude`, `cargo`, and `git` with timeout enforcement and output capture), `clap` for the command-line interface, and `serde` for configuration parsing and result serialization. The five components above map to Rust modules under `src/`. The benchmark fixtures themselves are separate Rust projects (Cargo workspaces) that are copied into an isolated working directory for each run and are never compiled as part of the orchestrator crate.

## 8. Repository Structure

```text
Cargo.toml
Cargo.lock
src/
  main.rs            # CLI entry point; dispatches subcommands
  cli.rs             # clap argument definitions and subcommand handlers
  types.rs           # serde data model (camelCase) and status enums
  config.rs          # load benchmark.yml and models.yml; project paths
  models.rs          # model selection
  tasks.rs           # task loading, selection, and validation
  command.rs         # tokio subprocess execution: timeout, output cap, redaction
  evaluator.rs       # deterministic checks and completion classification
  judge.rs           # LLM judge invocation and response normalization
  article.rs         # Markdown article generation
  fs_utils.rs        # filesystem helpers
  runner.rs          # benchmark orchestration loop
tests/               # integration tests
benchmark/
  config/
    models.yml
    benchmark.yml
  tasks/
    fix-failing-test/
      task.yml
      prompt.md
      fixture/         # self-contained Rust Cargo workspace
        .claude/
          settings.json
    add-api-endpoint/
      task.yml
      prompt.md
      fixture/
        .claude/
          settings.json
  runs/
    .gitkeep
  reports/
    .gitkeep
```

The final structure may vary depending on the host repository, but the implementation should keep task definitions, run artifacts, and generated reports clearly separated.

Because each fixture under `benchmark/tasks/*/fixture/` is its own Cargo workspace, the orchestrator's root `Cargo.toml` must exclude `benchmark/tasks` and `benchmark/runs` from its workspace so that building the orchestrator never attempts to compile fixture or run-artifact crates.

## 9. Configuration

### 9.1 Model Config

`config/models.yml` defines the models to test.

```yaml
models:
  - id: gpt-oss-20b
    displayName: GPT OSS 20B
    provider: models.bytefuture.ai
    model: groq/gpt-oss-20b
    claudeModelStrategy: custom-model-option
    enabled: true

  - id: gpt-oss-120b
    displayName: GPT OSS 120B
    provider: models.bytefuture.ai
    model: groq/gpt-oss-120b
    claudeModelStrategy: custom-model-option
    enabled: true

  - id: deepseek-v4-flash
    displayName: DeepSeek V4 Flash
    provider: models.bytefuture.ai
    model: deepseek/deepseek-v4-flash
    claudeModelStrategy: custom-model-option
    enabled: true

  - id: deepseek-v4-pro
    displayName: DeepSeek V4 Pro
    provider: models.bytefuture.ai
    model: deepseek/deepseek-v4-pro
    claudeModelStrategy: custom-model-option
    enabled: true

  - id: kimi-k2-5
    displayName: Kimi K2.5
    provider: models.bytefuture.ai
    model: kimi/kimi-k2.5
    claudeModelStrategy: custom-model-option
    enabled: true

  - id: nemotron-3-super
    displayName: Nemotron 3 Super
    provider: models.bytefuture.ai
    model: deepinfra/nemotron-3-super
    claudeModelStrategy: custom-model-option
    enabled: true

  - id: nemotron-3-nano
    displayName: Nemotron 3 Nano
    provider: models.bytefuture.ai
    model: deepinfra/nemotron-3-nano
    claudeModelStrategy: custom-model-option
    enabled: true
```

### 9.2 Benchmark Config

`config/benchmark.yml` defines global run behavior.

```yaml
benchmark:
  runsPerTaskModel: 3
  timeoutSeconds: 1800
  outputDir: runs
  reportDir: reports
  claude:
    baseUrl: https://bec.bytefuture.ai/v1
    outputFormat: json
    projectSettingsFile: .claude/settings.json
    disableExperimentalBetas: true
  judge:
    enabled: true
    provider: models.bytefuture.ai
    model: anthropic/claude-opus-4-6
    minimumScore: 4
  article:
    title: "Comparing Claude Code Tasks Across Models on ByteFuture"
    outputFile: reports/article.md
```

### 9.3 Task Config

Each task has a `task.yml`.

```yaml
id: fix-failing-test
title: Fix a failing unit test
description: Repair the implementation so the provided test suite passes.
fixturePath: fixture
promptFile: prompt.md
setup:
  - cargo fetch
checks:
  - name: unit-tests
    command: cargo test
  - name: typecheck
    command: cargo check
  - name: clippy
    command: cargo clippy --all-targets -- -D warnings
success:
  requiredChecks:
    - unit-tests
    - typecheck
    - clippy
judge:
  rubric:
    correctness: 0-5
    maintainability: 0-5
    scopeControl: 0-5
  unrelatedChangePolicy: fail
  allowedChangePaths:
    - src/**
    - tests/**
    - Cargo.toml
    - Cargo.lock
    - README.md
```

Each Rust fixture should include a project-local `.claude/settings.json` that manages permissions for Claude Code. The runner should load this file for each task run instead of hard-coding permission flags.

Example fixture settings:

```json
{
  "permissions": {
    "allow": [
      "Read",
      "Edit",
      "Bash(cargo fetch)",
      "Bash(cargo test)",
      "Bash(cargo check)",
      "Bash(cargo clippy --all-targets -- -D warnings)",
      "Bash(git diff *)",
      "Bash(git status *)"
    ],
    "deny": [
      "Bash(git push *)",
      "Bash(cargo publish *)"
    ]
  }
}
```

## 10. Execution Flow

1. Load benchmark configuration.
2. Load all enabled models.
3. Load selected tasks.
4. For each task/model pair, repeat three independent runs:
   - Copy the task fixture into an isolated working directory.
   - Initialize git state for diff capture.
   - Run setup commands.
   - Execute `claude -p` with the task prompt and model configuration.
   - Load the fixture's `.claude/settings.json` for Claude Code permissions.
  - Capture stdout, stderr, exit code, Claude JSON statistics, and generated files.
   - Run evaluation checks.
   - Run the LLM judge on the prompt, diff, check results, and repository-specific task definition.
   - Capture git diff.
   - Write a structured run result.
5. Aggregate run results.
6. Generate the Markdown article.

## 11. Claude Command Execution

Claude Code supports routing requests through a custom Anthropic-compatible gateway. The runner should configure Claude Code with `ANTHROPIC_BASE_URL=https://bec.bytefuture.ai`, authenticate with the ByteFuture API key, and pass the selected model ID through Claude Code model selection.

The runner should execute a command equivalent to:

```bash
ANTHROPIC_BASE_URL=https://bec.bytefuture.ai \
ANTHROPIC_API_KEY="<bytefuture-api-key>" \
ANTHROPIC_AUTH_TOKEN="<bytefuture-api-key>" \
ANTHROPIC_CUSTOM_MODEL_OPTION="<bytefuture-model-id>" \
ANTHROPIC_MODEL="<bytefuture-model-id>" \
CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1 \
claude --bare -p "<task prompt>" \
  --settings .claude/settings.json \
  --model "<bytefuture-model-id>" \
  --output-format json
```

The preferred strategy is:

1. Use `ANTHROPIC_BASE_URL` to route Claude Code requests to `bec.bytefuture.ai`; Claude Code appends Anthropic API paths like `/v1/messages` itself, so OpenAI-style `/v1` base URLs must be normalized before invocation.
2. Use `ANTHROPIC_AUTH_TOKEN` for ByteFuture gateway authentication because it requires `Authorization: Bearer <key>`. The runner may derive this from `ANTHROPIC_API_KEY` for compatibility.
3. Use `--model` and `ANTHROPIC_MODEL` with the exact ByteFuture model ID.
4. Use `ANTHROPIC_CUSTOM_MODEL_OPTION` so Claude Code does not reject gateway-specific model IDs.
5. Use `--settings .claude/settings.json` to load the Rust fixture's project-local permission policy.
6. Use `--output-format json` so the runner can capture Claude Code session metadata.
7. Use `--bare` for reproducible scripted runs while explicitly loading the settings file required by the fixture.
8. Use `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` if the gateway rejects Anthropic-specific beta headers or tool-schema fields.

ByteFuture model catalog checks should use the public `/api/models` endpoint. Do not rely on Claude Code gateway discovery for this benchmark unless ByteFuture also exposes a compatible `/v1/models` endpoint.

The runner must log the exact command strategy used, while redacting secrets.

Required environment variables should be documented in `.env.example`, for example:

```bash
ANTHROPIC_AUTH_TOKEN=
BYTEFUTURE_BASE_URL=https://bec.bytefuture.ai/v1
TOKEN_STATION_DUMP_PATH=reports/token-station-usage.json
JUDGE_MODEL_ID=anthropic/claude-opus-4-6
```

## 12. Run Result Schema

Each task/model run should produce a JSON file.

```json
{
  "runId": "2026-06-04T14-00-00Z_fix-failing-test_deepseek_001",
  "taskId": "fix-failing-test",
  "modelId": "deepseek-v4-flash",
  "providerModelId": "deepseek/deepseek-v4-flash",
  "runIndex": 1,
  "provider": "models.bytefuture.ai",
  "startedAt": "2026-06-04T14:00:00Z",
  "finishedAt": "2026-06-04T14:03:12Z",
  "durationMs": 192000,
  "claudeExitCode": 0,
  "claude": {
    "sessionId": "claude-session-id",
    "outputFormat": "json"
  },
  "checks": [
    {
      "name": "unit-tests",
      "command": "cargo test",
      "exitCode": 0,
      "passed": true,
      "durationMs": 12000
    }
  ],
  "completion": {
    "status": "passed",
    "reason": "All required checks passed."
  },
  "tokens": {
    "source": "token-station-backend-dump",
    "correlation": "execution-time-window",
    "dumpFile": "reports/token-station-usage.json",
    "input": 12000,
    "output": 3200,
    "cacheCreationInput": 0,
    "cacheReadInput": 0,
    "total": 15200,
    "estimatedCostUsd": 0.08
  },
  "judge": {
    "enabled": true,
    "modelId": "anthropic/claude-opus-4-6",
    "score": 4,
    "passed": true,
    "correctness": 4,
    "maintainability": 4,
    "scopeControl": 5,
    "hasUnrelatedChanges": false,
    "findings": []
  },
  "artifacts": {
    "stdout": "stdout.txt",
    "stderr": "stderr.txt",
    "diff": "diff.patch",
    "workspace": "workspace/"
  },
  "humanAudit": {
    "requiredForMvp": false,
    "score": null,
    "notes": ""
  }
}
```

## 13. Completion Evaluation

The evaluator should classify each run into one of these statuses:

- `passed`: all required checks passed and the LLM judge passed the run.
- `partial`: some progress was made, but at least one required check or judge requirement failed.
- `failed`: the model did not produce a usable result.
- `timeout`: the run exceeded the configured timeout.
- `error`: the runner failed due to infrastructure or configuration issues.

The MVP should prioritize deterministic evaluation:

- Test pass/fail.
- Typecheck pass/fail.
- Lint pass/fail.
- Expected file existence.
- Expected API response from local test script.

The LLM judge is required in the MVP. It should evaluate:

- Correctness relative to the task prompt.
- Whether deterministic failures indicate an incomplete implementation.
- Simplicity and maintainability of the diff.
- Scope control.
- Whether the model changed unrelated files.
- Whether the implementation appears to overfit the tests.

The judge must receive:

- Original task prompt.
- Task metadata and success criteria.
- Full git diff.
- List of changed files.
- Deterministic check results.
- Relevant stdout/stderr excerpts.

The judge must return structured JSON:

```json
{
  "score": 4,
  "passed": true,
  "correctness": 4,
  "maintainability": 4,
  "scopeControl": 5,
  "hasUnrelatedChanges": false,
  "findings": [
    {
      "severity": "minor",
      "category": "maintainability",
      "message": "The fix is correct but could simplify one branch."
    }
  ]
}
```

A run should fail judge evaluation when:

- `hasUnrelatedChanges` is `true`.
- `score` is below the configured minimum.
- The judge identifies a severe correctness issue.
- The model changes files outside `allowedChangePaths` without task justification.

## 14. Claude JSON Statistics

Every benchmark run uses `claude -p --output-format json`. The runner stores the raw JSON in `claude-output.json` and extracts structured statistics into `result.json`.

Required per-run fields:

- `startedAt`.
- `finishedAt`.
- `durationMs`, preferring Claude JSON `duration_ms` and falling back to subprocess wall-clock duration.
- `modelId`.
- `providerModelId`.
- `runId`.
- `tokens.input`, `tokens.output`, cache token fields, `tokens.total`, and `tokens.estimatedCostUsd` when available.
- `claude.statistics.durationMs`.
- `claude.statistics.durationApiMs`.
- `claude.statistics.ttftMs`.
- `claude.statistics.timeToRequestMs`.
- `claude.statistics.numTurns`.
- `claude.statistics.terminalReason`.
- `claude.statistics.stopReason`.

Token totals should prefer the per-model `modelUsage` breakdown. If `modelUsage` is absent, the runner should fall back to top-level `usage`.

## 15. Article Generator

The generated article should be Markdown and include:

- Title.
- Short introduction.
- Explanation of the benchmark methodology.
- List of tested models.
- List of tasks.
- Summary table.
- Per-task result table.
- Token and latency comparison.
- Three-run stability comparison.
- Judge score comparison.
- Notes on quality and failure modes.
- Reproducible commands.
- Limitations.
- Conclusion and recommendations.

Example summary table:

```markdown
| Model | Pass Rate | Avg Judge Score | Avg Total Tokens | Avg Latency | Notes |
| --- | ---: | ---: | ---: | ---: | --- |
| DeepSeek | 12/15 | 4.1 | 15,200 | 3m 12s | Strong completion, moderate token use |
| Kimi | 10/15 | 3.8 | 12,800 | 2m 48s | Low token use, missed one edge case |
| Nemotron 3 | 12/15 | 4.0 | 17,900 | 3m 40s | Reliable but more verbose |
```

The article should be written as a practical guide, not as a press release.

## 16. CLI Requirements

The MVP should expose commands similar to:

```bash
cargo run --release -- benchmark --tasks all --models all
cargo run --release -- benchmark --tasks fix-failing-test --models deepseek-v4-flash,kimi-k2-5,nemotron-3-super
cargo run --release -- judge --run-id <runId>
cargo run --release -- generate-article --input benchmark/runs --output benchmark/reports/article.md
```

After `cargo build --release`, the compiled binary at `target/release/token-station-arena` exposes the same subcommands directly. `cargo test` runs the unit and integration test suite.

Useful options:

- `--tasks`
- `--models`
- `--runs`
- `--timeout`
- `--skip-judge`
- `--skip-article`
- `--output`
- `--dry-run`

## 17. Artifacts

Each run should save:

- Prompt.
- Model configuration snapshot.
- Claude stdout.
- Claude stderr.
- Evaluation command outputs.
- Git diff.
- Token usage data.
- Claude Code JSON output.
- LLM judge output.
- Final result JSON.

Artifacts should be organized by run ID.

```text
runs/
  2026-06-04T14-00-00Z_fix-failing-test_deepseek_001/
    result.json
    prompt.md
    stdout.txt
    stderr.txt
    claude-output.json
    diff.patch
    judge.json
    checks/
      unit-tests.stdout.txt
      unit-tests.stderr.txt
    workspace/
```

## 18. Security and Privacy

- Never write API keys into logs.
- Redact known secret environment variables from stdout and stderr.
- Do not include private repository code in public reports unless explicitly approved.
- Keep generated articles free of confidential logs.
- Store run artifacts locally by default.
- Make external uploads opt-in.

## 19. Failure Handling

The runner should continue when one task/model pair fails, unless `--fail-fast` is enabled.

Failures should be recorded as structured results instead of crashing the entire benchmark.

Common failure cases:

- `claude` command not found.
- ByteFuture API authentication failed.
- Model unavailable.
- Claude JSON output missing token or timing fields.
- Task setup failed.
- Evaluation command failed.
- Timeout.

## 20. Acceptance Criteria

The MVP is complete when:

- A user can configure the seven canonical model IDs from `models.bytefuture.ai`:
  - `groq/gpt-oss-20b`
  - `groq/gpt-oss-120b`
  - `deepseek/deepseek-v4-flash`
  - `deepseek/deepseek-v4-pro`
  - `kimi/kimi-k2.5`
  - `deepinfra/nemotron-3-super`
  - `deepinfra/nemotron-3-nano`
- A user can define at least three benchmark tasks.
- Benchmark fixtures are Rust projects organized as a small workspace with a library crate, an Axum API crate, and unit/integration tests.
- The benchmark orchestration tooling is implemented in Rust as a single binary crate.
- The orchestrator builds with `cargo build --release` and passes `cargo clippy --all-targets -- -D warnings`.
- The benchmark runs each task/model pair three independent times.
- A single command can run all task/model combinations.
- Each run produces structured JSON results.
- Each run captures stdout, stderr, duration, checks, and git diff.
- Each Rust fixture manages Claude Code permissions through `.claude/settings.json`.
- Each run is evaluated by deterministic checks and an LLM judge.
- Required deterministic checks include `cargo test`, `cargo check`, and `cargo clippy --all-targets -- -D warnings`.
- The LLM judge reports score, correctness, maintainability, scope control, and unrelated-change findings.
- Each run records token, cost, and timing statistics from Claude JSON output when available.
- The system generates a Markdown article from the run results.
- The article includes completion, judge score, token, latency, and three-run stability comparisons.
- The workflow can be reproduced from documented commands.

## 21. Suggested Implementation Plan

### Phase 1: Local Runner

- Scaffold the Cargo binary crate with the module layout above.
- Implement the `serde`-based config, model, and task loaders.
- Implement the `tokio` subprocess core in `command.rs` (timeout, output cap, secret redaction).
- Implement isolated task workspace creation.
- Implement `claude -p` invocation.
- Capture logs and diffs.
- Write initial `result.json`.

### Phase 2: Evaluator

- Add task setup commands.
- Add task check commands.
- Add completion classification.
- Persist check outputs.

### Phase 3: LLM Judge

- Implement judge prompt and JSON schema.
- Run judge on prompt, task metadata, diff, and check results.
- Detect unrelated changes and low-quality fixes.
- Persist `judge.json`.

### Phase 4: Claude JSON Statistics

- Implement Claude JSON statistics parsing.
- Prefer `modelUsage` for token and cost data; fall back to top-level `usage`.
- Store token, cost, duration, API duration, TTFT, and turn count in `result.json`.

### Phase 5: Article Generation

- Aggregate run results.
- Generate Markdown tables.
- Generate model notes and task notes.
- Aggregate three-run pass rates and stability.
- Add limitations and reproducibility section.

### Phase 6: Hardening

- Add timeout handling.
- Add secret redaction.
- Add dry-run mode.
- Add retries for infrastructure failures.
- Add documentation and examples.

## 22. Risks

- Some ByteFuture model IDs may not match Claude Code model validation unless custom model configuration is used.
- LLM judge results can be biased or inconsistent.
- Three runs per model/task are better than one run but still not statistically exhaustive.
- Some models may optimize for short output but fail hidden requirements.

## 23. Mitigations

- Start with deterministic task checks.
- Record exact prompts, commands, and artifacts.
- Use `ANTHROPIC_CUSTOM_MODEL_OPTION` and `--model` for gateway-specific model IDs.
- Preserve exact Claude JSON output alongside normalized statistics.
- Label first results as a practical benchmark guide, not a comprehensive leaderboard.
- Run each task/model pair three times.
- Keep claims narrow and evidence-backed.
- Use structured LLM judge outputs and preserve judge artifacts.

## 24. Future Enhancements

- Run each task/model pair more than three times.
- Add human review calibration for the LLM judge.
- Add a web dashboard.
- Add historical trend tracking.
- Add cost-normalized ranking.
- Add automatic charts.
- Add GitHub Actions support for scheduled benchmark runs.
- Publish anonymized benchmark data.

## 25. Finalized Decisions

- Token, cost, and timing data come from `claude -p --output-format json`.
- The first public fixture set will use Rust: a small workspace with a library crate, an Axum API crate, and unit/integration tests.
- The benchmark runner will be implemented in Rust as a single binary crate using `tokio`, `clap`, and `serde`.
- YAML configuration is parsed with `serde_yaml_ng`; subprocess timeouts use `tokio::time::timeout` with a `SIGTERM`-then-`SIGKILL` escalation via `nix`.
- Rust fixture permissions will be managed through project-local `.claude/settings.json` files.
- The LLM judge is `anthropic/claude-opus-4-6`.
- The judge pass threshold is `4`.
- `cargo clippy --all-targets -- -D warnings` is a required check.

## 26. Claude Code Integration References

Implementation should follow the official Claude Code documentation:

- Claude Code environment variables: `ANTHROPIC_BASE_URL` can override the API endpoint and route through a proxy or gateway.
- Claude Code model configuration: `ANTHROPIC_CUSTOM_MODEL_OPTION` allows a custom model ID accepted by the gateway and skips normal model ID validation.
- Claude Code CLI reference: `claude -p` runs non-interactively, `--model` overrides the model for the session, and `--output-format json` returns structured output.
- Claude Code programmatic usage: `--bare` is recommended for scripted calls, and JSON output includes session metadata plus usage/cost metadata when available.

Reference URLs:

- https://code.claude.com/docs/en/env-vars
- https://code.claude.com/docs/en/model-config
- https://code.claude.com/docs/en/cli-usage
- https://code.claude.com/docs/en/headless
- https://code.claude.com/docs/en/llm-gateway
