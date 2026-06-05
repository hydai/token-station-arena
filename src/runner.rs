use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::article::generate_article_for_runs;
use crate::command::{format_command, redact_env, run_process, run_shell_command, RunOptions};
use crate::config::{load_benchmark_config, load_models, project_paths, resolve_project_path};
use crate::evaluator::{classify_completion, run_checks, CompletionInputs};
use crate::fs_utils::{
    copy_dir, ensure_dir, path_exists, read_json, read_text, write_json, write_text,
};
use crate::judge::{run_judge, skipped_judge, JudgeRunInput};
use crate::models::select_models;
use crate::tasks::{load_tasks, select_tasks};
use crate::types::{
    Artifacts, BenchmarkConfig, ClaudeRunMeta, ClaudeRunStatistics, CommandResult, Completion,
    CompletionStatus, HumanAudit, LoadedTask, ModelConfig, RunResult, TokenUsage,
};
use crate::util::{anthropic_base_url_for_claude, now_iso};

const VERBOSE_OUTPUT_LIMIT_CHARS: usize = 24 * 1024;

/// Replaces every run of characters outside `[A-Za-z0-9_-]` with a single dash.
fn sanitize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_dash_run = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            in_dash_run = false;
        } else if !in_dash_run {
            out.push('-');
            in_dash_run = true;
        }
    }
    out
}

/// Builds a sortable run id: `<timestamp>_<task>_<model>_<NNN>`, with `:` and
/// `.` in the timestamp replaced by `-` so it is filesystem-safe.
pub fn format_run_id(timestamp_iso: &str, task_id: &str, model_id: &str, run_index: u32) -> String {
    let timestamp = timestamp_iso.replace([':', '.'], "-");
    format!(
        "{timestamp}_{}_{}_{run_index:03}",
        sanitize(task_id),
        sanitize(model_id)
    )
}

/// Pulls a Claude session id out of the JSON stdout, if present.
fn extract_claude_session_id(stdout: &str) -> Option<String> {
    let value: Value = serde_json::from_str(stdout).ok()?;
    for key in ["session_id", "sessionId", "id"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return Some(text.to_string());
        }
    }
    None
}

/// Parses Claude's stdout as JSON, or returns a structured parse-error marker.
fn parse_claude_output(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|_| {
        json!({
            "parseError": "Claude stdout was not valid JSON.",
            "stdoutArtifact": "stdout.txt",
        })
    })
}

fn claude_api_error(output: &Value) -> Option<String> {
    let is_error = output
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let status = output.get("api_error_status").and_then(Value::as_i64);
    if !is_error && status.is_none() {
        return None;
    }

    let detail = output
        .get("result")
        .and_then(Value::as_str)
        .or_else(|| output.get("error").and_then(Value::as_str))
        .unwrap_or("Claude returned an API error.");
    let prefix = status
        .map(|code| format!("Claude API error {code}"))
        .unwrap_or_else(|| "Claude API error".to_string());
    Some(format!("{prefix}: {detail}"))
}

fn extract_claude_statistics(output: &Value) -> Option<ClaudeRunStatistics> {
    if output.get("parseError").is_some() {
        return None;
    }

    let statistics = ClaudeRunStatistics {
        duration_ms: first_u64(output, &["duration_ms", "durationMs"]),
        duration_api_ms: first_u64(output, &["duration_api_ms", "durationApiMs"]),
        ttft_ms: first_u64(output, &["ttft_ms", "ttftMs"]),
        time_to_request_ms: first_u64(output, &["time_to_request_ms", "timeToRequestMs"]),
        num_turns: first_u64(output, &["num_turns", "numTurns"]),
        total_cost_usd: first_f64(output, &["total_cost_usd", "totalCostUsd"]),
        terminal_reason: first_string(output, &["terminal_reason", "terminalReason"]),
        stop_reason: first_string(output, &["stop_reason", "stopReason"]),
    };

    if statistics.duration_ms.is_none()
        && statistics.duration_api_ms.is_none()
        && statistics.ttft_ms.is_none()
        && statistics.time_to_request_ms.is_none()
        && statistics.num_turns.is_none()
        && statistics.total_cost_usd.is_none()
        && statistics.terminal_reason.is_none()
        && statistics.stop_reason.is_none()
    {
        None
    } else {
        Some(statistics)
    }
}

fn extract_claude_token_usage(output: &Value) -> Option<TokenUsage> {
    if output.get("parseError").is_some() {
        return None;
    }

    if let Some(model_usage) = output.get("modelUsage").and_then(Value::as_object) {
        let mut totals = UsageAccumulator::default();
        for usage in model_usage.values().filter_map(Value::as_object) {
            totals.add_input(first_i64_from_object(
                usage,
                &["inputTokens", "input_tokens", "input"],
            ));
            totals.add_output(first_i64_from_object(
                usage,
                &["outputTokens", "output_tokens", "output"],
            ));
            totals.add_cache_creation(first_i64_from_object(
                usage,
                &[
                    "cacheCreationInputTokens",
                    "cache_creation_input_tokens",
                    "cacheCreationInput",
                ],
            ));
            totals.add_cache_read(first_i64_from_object(
                usage,
                &[
                    "cacheReadInputTokens",
                    "cache_read_input_tokens",
                    "cacheReadInput",
                ],
            ));
            totals.add_cost(first_f64_from_object(
                usage,
                &["costUSD", "costUsd", "cost_usd"],
            ));
        }
        if totals.has_usage() {
            return Some(totals.into_token_usage(
                "claude-output-json",
                "direct-claude-output",
                "claude-output.json",
            ));
        }
    }

    let usage = output.get("usage").and_then(Value::as_object)?;
    let cache_creation_nested = usage.get("cache_creation").and_then(Value::as_object);
    let mut totals = UsageAccumulator::default();
    totals.add_input(first_i64_from_object(
        usage,
        &["input_tokens", "inputTokens", "input"],
    ));
    totals.add_output(first_i64_from_object(
        usage,
        &["output_tokens", "outputTokens", "output"],
    ));
    totals.add_cache_creation(
        first_i64_from_object(
            usage,
            &[
                "cache_creation_input_tokens",
                "cacheCreationInputTokens",
                "cacheCreationInput",
            ],
        )
        .or_else(|| {
            cache_creation_nested.and_then(|nested| {
                first_i64_from_object(
                    nested,
                    &[
                        "ephemeral_1h_input_tokens",
                        "ephemeral_5m_input_tokens",
                        "ephemeral1hInputTokens",
                        "ephemeral5mInputTokens",
                    ],
                )
            })
        }),
    );
    totals.add_cache_read(first_i64_from_object(
        usage,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cacheReadInput",
        ],
    ));
    totals.add_cost(first_f64(output, &["total_cost_usd", "totalCostUsd"]));

    if totals.has_usage() {
        Some(totals.into_token_usage(
            "claude-output-json",
            "direct-claude-output",
            "claude-output.json",
        ))
    } else {
        None
    }
}

#[derive(Default)]
struct UsageAccumulator {
    input: i64,
    output: i64,
    cache_creation_input: i64,
    cache_read_input: i64,
    estimated_cost_usd: f64,
    has_input: bool,
    has_output: bool,
    has_cache_creation: bool,
    has_cache_read: bool,
    has_cost: bool,
}

impl UsageAccumulator {
    fn add_input(&mut self, value: Option<i64>) {
        if let Some(value) = value {
            self.input += value;
            self.has_input = true;
        }
    }

    fn add_output(&mut self, value: Option<i64>) {
        if let Some(value) = value {
            self.output += value;
            self.has_output = true;
        }
    }

    fn add_cache_creation(&mut self, value: Option<i64>) {
        if let Some(value) = value {
            self.cache_creation_input += value;
            self.has_cache_creation = true;
        }
    }

    fn add_cache_read(&mut self, value: Option<i64>) {
        if let Some(value) = value {
            self.cache_read_input += value;
            self.has_cache_read = true;
        }
    }

    fn add_cost(&mut self, value: Option<f64>) {
        if let Some(value) = value {
            self.estimated_cost_usd += value;
            self.has_cost = true;
        }
    }

    fn has_usage(&self) -> bool {
        self.has_input
            || self.has_output
            || self.has_cache_creation
            || self.has_cache_read
            || self.has_cost
    }

    fn into_token_usage(self, source: &str, correlation: &str, dump_file: &str) -> TokenUsage {
        let total = if self.has_input
            || self.has_output
            || self.has_cache_creation
            || self.has_cache_read
        {
            Some(self.input + self.output + self.cache_creation_input + self.cache_read_input)
        } else {
            None
        };
        TokenUsage {
            source: source.to_string(),
            correlation: correlation.to_string(),
            dump_file: dump_file.to_string(),
            input: self.has_input.then_some(self.input),
            output: self.has_output.then_some(self.output),
            cache_creation_input: self.has_cache_creation.then_some(self.cache_creation_input),
            cache_read_input: self.has_cache_read.then_some(self.cache_read_input),
            total,
            estimated_cost_usd: self.has_cost.then_some(self.estimated_cost_usd),
        }
    }
}

fn first_u64(output: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(value) = output.get(*key).and_then(value_to_u64) {
            return Some(value);
        }
    }
    None
}

fn first_i64_from_object(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(value) = object.get(*key).and_then(value_to_i64) {
            return Some(value);
        }
    }
    None
}

fn first_f64(output: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(value) = output.get(*key).and_then(value_to_f64) {
            return Some(value);
        }
    }
    None
}

fn first_f64_from_object(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(value) = object.get(*key).and_then(value_to_f64) {
            return Some(value);
        }
    }
    None
}

fn first_string(output: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = output.get(*key).and_then(Value::as_str) {
            return Some(value.to_string());
        }
    }
    None
}

fn value_to_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
}

fn value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

fn value_to_f64(value: &Value) -> Option<f64> {
    value.as_f64().filter(|value| value.is_finite())
}

/// Options controlling a benchmark run, mirroring the CLI flags.
#[derive(Debug, Clone, Default)]
pub struct BenchmarkArgs {
    pub tasks: Option<String>,
    pub models: Option<String>,
    pub runs: Option<u32>,
    pub timeout: Option<u64>,
    pub jobs: Option<usize>,
    pub skip_judge: bool,
    pub skip_article: bool,
    pub dry_run: bool,
    pub verbose: bool,
}

/// Runs the full benchmark: every selected task × model × run, then optional
/// token import and article generation.
pub async fn run_benchmark(args: &BenchmarkArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("determine current directory")?;
    let paths = project_paths(&cwd);
    let benchmark = load_benchmark_config(&paths)?;
    let all_models = load_models(&paths)?;
    let all_tasks = load_tasks(&paths.tasks_dir)?;
    let models = select_models(&all_models, args.models.as_deref().unwrap_or("all"))?;
    let tasks = select_tasks(&all_tasks, args.tasks.as_deref().unwrap_or("all"))?;
    let runs_per_task_model = args.runs.unwrap_or(benchmark.runs_per_task_model);
    let timeout_seconds = args.timeout.unwrap_or(benchmark.timeout_seconds);
    let jobs = args.jobs.or(benchmark.jobs).unwrap_or(1);
    if jobs == 0 {
        bail!("--jobs must be at least 1.");
    }
    let output_dir = resolve_project_path(&paths.root_dir, &benchmark.output_dir);
    let report_dir = resolve_project_path(&paths.root_dir, &benchmark.report_dir);

    println!(
        "Selected {} task(s), {} model(s), {} run(s) each.",
        tasks.len(),
        models.len(),
        runs_per_task_model
    );
    println!("Run artifacts: {}", output_dir.display());
    println!("Parallel jobs: {jobs}");

    if args.dry_run {
        print_dry_run_plan(
            &benchmark,
            &tasks,
            &models,
            runs_per_task_model,
            timeout_seconds,
        );
        return Ok(());
    }

    if benchmark_auth_secret().is_none() {
        bail!("ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN, or BYTEFUTURE_AUTH_TOKEN is required for real benchmark runs. Use --dry-run to inspect the plan without calling Claude.");
    }

    ensure_dir(&output_dir)?;
    ensure_dir(&report_dir)?;

    let timeout = Duration::from_secs(timeout_seconds);
    let run_plans = build_run_plans(&tasks, &models, runs_per_task_model);
    let current_run_ids = run_benchmark_plans(RunPlansInput {
        benchmark: &benchmark,
        run_plans,
        output_dir: &output_dir,
        timeout,
        skip_judge: args.skip_judge,
        verbose: args.verbose,
        jobs,
    })
    .await?;

    if !args.skip_article {
        let article_path = resolve_project_path(&paths.root_dir, &benchmark.article.output_file);
        let article_runs = load_current_run_results(&output_dir, &current_run_ids)?;
        let summary =
            generate_article_for_runs(&article_runs, &article_path, &benchmark.article.title)?;
        println!(
            "Generated {} from {} run(s).",
            summary.output_path.display(),
            summary.run_count
        );
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct RunPlan {
    task: LoadedTask,
    model: ModelConfig,
    run_index: u32,
}

fn build_run_plans(
    tasks: &[LoadedTask],
    models: &[ModelConfig],
    runs_per_task_model: u32,
) -> Vec<RunPlan> {
    let mut run_plans = Vec::new();
    for task in tasks {
        for model in models {
            for run_index in 1..=runs_per_task_model {
                run_plans.push(RunPlan {
                    task: task.clone(),
                    model: model.clone(),
                    run_index,
                });
            }
        }
    }
    run_plans
}

struct RunPlansInput<'a> {
    benchmark: &'a BenchmarkConfig,
    run_plans: Vec<RunPlan>,
    output_dir: &'a Path,
    timeout: Duration,
    skip_judge: bool,
    verbose: bool,
    jobs: usize,
}

async fn run_benchmark_plans(input: RunPlansInput<'_>) -> Result<Vec<String>> {
    if input.jobs == 1 {
        return run_benchmark_plans_serial(&input).await;
    }
    run_benchmark_plans_parallel(input).await
}

async fn run_benchmark_plans_serial(input: &RunPlansInput<'_>) -> Result<Vec<String>> {
    let mut current_run_ids = Vec::new();
    for plan in &input.run_plans {
        let result = run_single_benchmark(SingleBenchmarkInput {
            benchmark: input.benchmark,
            task: &plan.task,
            model: &plan.model,
            run_index: plan.run_index,
            output_dir: input.output_dir,
            timeout: input.timeout,
            skip_judge: input.skip_judge,
            verbose: input.verbose,
        })
        .await?;
        print_run_result(input.output_dir, &result, input.verbose);
        current_run_ids.push(result.run_id.clone());
    }
    Ok(current_run_ids)
}

async fn run_benchmark_plans_parallel(input: RunPlansInput<'_>) -> Result<Vec<String>> {
    let mut pending = input.run_plans.into_iter();
    let mut running = tokio::task::JoinSet::new();
    let mut active = 0usize;
    let mut current_run_ids = Vec::new();

    loop {
        while active < input.jobs {
            let Some(plan) = pending.next() else {
                break;
            };
            spawn_benchmark_plan(
                &mut running,
                input.benchmark,
                plan,
                input.output_dir,
                input.timeout,
                input.skip_judge,
                input.verbose,
            );
            active += 1;
        }

        if active == 0 {
            break;
        }

        let joined = running
            .join_next()
            .await
            .context("benchmark worker set ended unexpectedly")?;
        active -= 1;
        let result = joined.context("benchmark worker panicked")??;
        print_run_result(input.output_dir, &result, input.verbose);
        current_run_ids.push(result.run_id.clone());
    }

    Ok(current_run_ids)
}

fn spawn_benchmark_plan(
    running: &mut tokio::task::JoinSet<Result<RunResult>>,
    benchmark: &BenchmarkConfig,
    plan: RunPlan,
    output_dir: &Path,
    timeout: Duration,
    skip_judge: bool,
    verbose: bool,
) {
    let benchmark = benchmark.clone();
    let output_dir = output_dir.to_path_buf();
    running.spawn(async move {
        run_single_benchmark(SingleBenchmarkInput {
            benchmark: &benchmark,
            task: &plan.task,
            model: &plan.model,
            run_index: plan.run_index,
            output_dir: &output_dir,
            timeout,
            skip_judge,
            verbose,
        })
        .await
    });
}

fn print_run_result(output_dir: &Path, result: &RunResult, verbose: bool) {
    println!(
        "{}: {} in {} ({})",
        result.run_id,
        result.completion.status.as_str(),
        format_duration_ms(result.duration_ms),
        result.completion.reason
    );
    if verbose {
        print_verbose_run_failure_details(&output_dir.join(&result.run_id), result);
    }
}

fn load_current_run_results(output_dir: &Path, run_ids: &[String]) -> Result<Vec<RunResult>> {
    let mut runs = Vec::with_capacity(run_ids.len());
    for run_id in run_ids {
        runs.push(read_json(output_dir.join(run_id).join("result.json"))?);
    }
    Ok(runs)
}

struct SingleBenchmarkInput<'a> {
    benchmark: &'a BenchmarkConfig,
    task: &'a LoadedTask,
    model: &'a ModelConfig,
    run_index: u32,
    output_dir: &'a Path,
    timeout: Duration,
    skip_judge: bool,
    verbose: bool,
}

async fn run_single_benchmark(input: SingleBenchmarkInput<'_>) -> Result<RunResult> {
    let benchmark = input.benchmark;
    let task = input.task;
    let model = input.model;
    let run_index = input.run_index;
    let output_dir = input.output_dir;
    let timeout = input.timeout;
    let skip_judge = input.skip_judge;
    let verbose = input.verbose;
    let run_id = build_run_id(&task.config.id, &model.id, run_index);
    let run_dir = output_dir.join(&run_id);
    let workspace_dir = run_dir.join("workspace");
    let secrets = secret_list();

    ensure_dir(&run_dir)?;
    ensure_dir(run_dir.join("checks"))?;
    copy_dir(&task.fixture_dir, &workspace_dir)?;
    write_text(run_dir.join("prompt.md"), &task.prompt)?;
    write_json(run_dir.join("model-config.json"), model)?;

    if let Some(failed) = prepare_workspace(task, &workspace_dir, timeout, &secrets).await {
        if verbose {
            print_verbose_command_result("Workspace setup failed", &failed);
        }
        let result =
            build_infrastructure_error_result(&run_id, task, model, run_index, benchmark, &failed);
        write_json(run_dir.join("result.json"), &result)?;
        return Ok(result);
    }

    let claude = run_claude(ClaudeInvocation {
        benchmark,
        task,
        model,
        workspace_dir: &workspace_dir,
        timeout,
        secrets: &secrets,
        run_id: &run_id,
        verbose,
    })
    .await;
    write_text(run_dir.join("stdout.txt"), &claude.stdout)?;
    write_text(run_dir.join("stderr.txt"), &claude.stderr)?;
    let claude_output = parse_claude_output(&claude.stdout);
    let claude_tokens = extract_claude_token_usage(&claude_output);
    let claude_statistics = extract_claude_statistics(&claude_output);
    let result_duration_ms = claude_statistics
        .as_ref()
        .and_then(|statistics| statistics.duration_ms)
        .unwrap_or(claude.duration_ms);
    write_json(run_dir.join("claude-output.json"), &claude_output)?;
    write_json(
        run_dir.join("command-strategy.json"),
        &json!({
            "command": format_command("claude", claude.args.as_deref().unwrap_or(&[])),
            "env": redact_env(&claude_env(benchmark, model)),
        }),
    )?;

    let diff = capture_diff(&workspace_dir, &secrets).await;
    let changed_files = capture_changed_files(&workspace_dir, &secrets).await;
    write_text(run_dir.join("diff.patch"), &diff)?;

    if let Some(error) = claude_api_error(&claude_output) {
        let result = build_claude_error_result(ClaudeErrorResultInput {
            run_id: &run_id,
            task,
            model,
            run_index,
            benchmark,
            claude: &claude,
            duration_ms: result_duration_ms,
            tokens: claude_tokens,
            statistics: claude_statistics,
            changed_files,
            reason: error,
        });
        write_json(run_dir.join("result.json"), &result)?;
        return Ok(result);
    }

    let checks = run_checks(task, &workspace_dir, &run_dir, timeout, &secrets).await?;

    let judge = if skip_judge || !benchmark.judge.enabled {
        skipped_judge(&benchmark.judge.model)
    } else {
        run_judge(&JudgeRunInput {
            benchmark,
            task,
            model,
            workspace_dir: &workspace_dir,
            run_dir: &run_dir,
            diff: &diff,
            changed_files: &changed_files,
            checks: &checks,
            claude_stdout: &claude.stdout,
            claude_stderr: &claude.stderr,
            timeout,
            secrets: &secrets,
        })
        .await
    };

    let completion = classify_completion(&CompletionInputs {
        task,
        checks: &checks,
        judge: &judge,
        claude_exit_code: claude.exit_code,
        claude_timed_out: claude.timed_out,
        changed_files: &changed_files,
        infrastructure_error: None,
    });

    let result = RunResult {
        run_id: run_id.clone(),
        task_id: task.config.id.clone(),
        model_id: model.id.clone(),
        provider_model_id: model.model.clone(),
        run_index,
        provider: model.provider.clone(),
        started_at: claude.started_at.clone(),
        finished_at: claude.finished_at.clone(),
        duration_ms: result_duration_ms,
        claude_exit_code: claude.exit_code,
        claude: ClaudeRunMeta {
            session_id: extract_claude_session_id(&claude.stdout),
            output_format: benchmark.claude.output_format.clone(),
            command_strategy: command_strategy_labels(),
            statistics: claude_statistics,
        },
        checks,
        completion,
        tokens: claude_tokens,
        judge,
        artifacts: default_artifacts(),
        changed_files,
        warnings: vec![],
        human_audit: HumanAudit {
            required_for_mvp: false,
            score: None,
            notes: String::new(),
        },
    };

    write_json(run_dir.join("result.json"), &result)?;
    Ok(result)
}

/// Initializes git, runs task setup, and commits a baseline. Returns the failed
/// command if any step exits non-zero.
async fn prepare_workspace(
    task: &LoadedTask,
    workspace_dir: &Path,
    timeout: Duration,
    secrets: &[String],
) -> Option<CommandResult> {
    let options = || RunOptions {
        cwd: workspace_dir.to_path_buf(),
        timeout: Some(timeout),
        secrets: secrets.to_vec(),
        ..RunOptions::default()
    };

    let init = [
        "git init",
        "git config user.email benchmark@example.invalid",
        "git config user.name Benchmark Runner",
    ];
    for command in init {
        let result = run_shell_command(command, &options()).await;
        if result.exit_code != Some(0) {
            return Some(result);
        }
    }
    for command in &task.config.setup {
        let result = run_shell_command(command, &options()).await;
        if result.exit_code != Some(0) {
            return Some(result);
        }
    }
    for command in ["git add .", "git commit -m baseline"] {
        let result = run_shell_command(command, &options()).await;
        if result.exit_code != Some(0) {
            return Some(result);
        }
    }
    None
}

struct ClaudeInvocation<'a> {
    benchmark: &'a BenchmarkConfig,
    task: &'a LoadedTask,
    model: &'a ModelConfig,
    workspace_dir: &'a Path,
    timeout: Duration,
    secrets: &'a [String],
    run_id: &'a str,
    verbose: bool,
}

async fn run_claude(input: ClaudeInvocation<'_>) -> CommandResult {
    let env: Vec<(String, String)> = claude_env(input.benchmark, input.model)
        .into_iter()
        .collect();
    let args = vec![
        "--bare".to_string(),
        "-p".to_string(),
        input.task.prompt.clone(),
        "--settings".to_string(),
        input.benchmark.claude.project_settings_file.clone(),
        "--model".to_string(),
        input.model.model.clone(),
        "--output-format".to_string(),
        input.benchmark.claude.output_format.clone(),
    ];
    if input.verbose {
        print_verbose_claude_invocation(input.run_id, input.model, &args);
    }
    run_process(
        "claude",
        &args,
        &RunOptions {
            cwd: input.workspace_dir.to_path_buf(),
            env,
            timeout: Some(input.timeout),
            secrets: input.secrets.to_vec(),
        },
    )
    .await
}

fn print_verbose_claude_invocation(run_id: &str, model: &ModelConfig, args: &[String]) {
    let prompt = args
        .windows(2)
        .find(|window| window[0] == "-p")
        .map(|window| window[1].as_str())
        .unwrap_or("");
    let command_args = args_with_prompt_placeholder(args);

    println!("Verbose Claude invocation for {run_id}:");
    println!("Model: {} -> {}", model.id, model.model);
    println!("Command: {}", format_command("claude", &command_args));
    println!("claude -p payload:");
    println!("-----BEGIN CLAUDE PROMPT-----");
    print!("{prompt}");
    if !prompt.ends_with('\n') {
        println!();
    }
    println!("-----END CLAUDE PROMPT-----");
}

fn args_with_prompt_placeholder(args: &[String]) -> Vec<String> {
    let mut rendered = Vec::with_capacity(args.len());
    let mut replace_next = false;
    for arg in args {
        if replace_next {
            rendered.push("<prompt printed below>".to_string());
            replace_next = false;
            continue;
        }
        rendered.push(arg.clone());
        replace_next = arg == "-p";
    }
    rendered
}

fn print_verbose_run_failure_details(run_dir: &Path, result: &RunResult) {
    if result.completion.status == CompletionStatus::Passed {
        return;
    }

    let failed_checks: Vec<_> = result.checks.iter().filter(|check| !check.passed).collect();
    let has_claude_output = path_exists(run_dir.join(&result.artifacts.stdout))
        || path_exists(run_dir.join(&result.artifacts.stderr));
    let should_print_claude = (result.claude_exit_code != Some(0)
        || result.completion.status == CompletionStatus::Timeout)
        && has_claude_output;
    let should_print_judge = result.judge.enabled && !result.judge.passed;
    let should_print_warnings = !result.warnings.is_empty();

    if failed_checks.is_empty()
        && !should_print_claude
        && !should_print_judge
        && !should_print_warnings
    {
        return;
    }

    println!("Verbose failure details for {}:", result.run_id);
    if !failed_checks.is_empty() {
        for check in failed_checks {
            println!(
                "Failed check: {} (exit: {}, timed out: {}, duration: {}ms)",
                check.name,
                format_exit_code(check.exit_code),
                check.timed_out.unwrap_or(false),
                check.duration_ms
            );
            println!("Command: {}", check.command);
            print_verbose_artifact(run_dir, "stdout", check.stdout_path.as_deref());
            print_verbose_artifact(run_dir, "stderr", check.stderr_path.as_deref());
        }
    }

    if should_print_claude {
        println!(
            "Claude process: exit: {}, duration: {}ms",
            format_exit_code(result.claude_exit_code),
            result.duration_ms
        );
        print_verbose_artifact(
            run_dir,
            "claude stdout",
            Some(result.artifacts.stdout.as_str()),
        );
        print_verbose_artifact(
            run_dir,
            "claude stderr",
            Some(result.artifacts.stderr.as_str()),
        );
    }

    if should_print_judge {
        println!(
            "Judge failed: model={}, score={}",
            result.judge.model_id,
            result
                .judge
                .score
                .map(|score| score.to_string())
                .unwrap_or_else(|| "null".to_string())
        );
        if let Some(error) = &result.judge.error {
            println!("Judge error: {error}");
        }
        for finding in &result.judge.findings {
            println!(
                "Judge finding [{}:{}]: {}",
                finding.severity, finding.category, finding.message
            );
        }
    }

    if should_print_warnings {
        for warning in &result.warnings {
            println!("Warning: {warning}");
        }
    }
}

fn print_verbose_artifact(run_dir: &Path, label: &str, relative_path: Option<&str>) {
    let Some(relative_path) = relative_path else {
        println!("{label}: <not captured>");
        return;
    };
    let path = run_dir.join(relative_path);
    match read_text(&path) {
        Ok(text) if text.is_empty() => {
            println!("{label} ({relative_path}): <empty>");
        }
        Ok(text) => {
            println!("{label} ({relative_path}):");
            println!("-----BEGIN {label}-----");
            print!("{}", truncate_verbose_output(&text));
            if !text.ends_with('\n') {
                println!();
            }
            println!("-----END {label}-----");
        }
        Err(error) => {
            println!("{label} ({relative_path}): <failed to read: {error}>");
        }
    }
}

fn print_verbose_command_result(label: &str, result: &CommandResult) {
    println!("{label}:");
    println!("Command: {}", result.command);
    println!(
        "Exit: {}, timed out: {}, duration: {}ms",
        format_exit_code(result.exit_code),
        result.timed_out,
        result.duration_ms
    );
    print_verbose_stream("stdout", &result.stdout);
    print_verbose_stream("stderr", &result.stderr);
}

fn print_verbose_stream(label: &str, text: &str) {
    if text.is_empty() {
        println!("{label}: <empty>");
        return;
    }
    println!("{label}:");
    println!("-----BEGIN {label}-----");
    print!("{}", truncate_verbose_output(text));
    if !text.ends_with('\n') {
        println!();
    }
    println!("-----END {label}-----");
}

fn format_exit_code(exit_code: Option<i32>) -> String {
    exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn truncate_verbose_output(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= VERBOSE_OUTPUT_LIMIT_CHARS {
        return text.to_string();
    }
    let omitted = char_count - VERBOSE_OUTPUT_LIMIT_CHARS;
    let tail: String = text.chars().skip(omitted).collect();
    format!("[output truncated; omitted {omitted} chars, showing last {VERBOSE_OUTPUT_LIMIT_CHARS} chars]\n{tail}")
}

async fn capture_diff(workspace_dir: &Path, secrets: &[String]) -> String {
    let result = run_shell_command(
        "git diff --binary",
        &RunOptions {
            cwd: workspace_dir.to_path_buf(),
            timeout: Some(Duration::from_secs(60)),
            secrets: secrets.to_vec(),
            ..RunOptions::default()
        },
    )
    .await;
    if result.stdout.is_empty() {
        result.stderr
    } else {
        result.stdout
    }
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        return format!("{duration_ms}ms");
    }
    let total_seconds = duration_ms as f64 / 1_000.0;
    if total_seconds < 60.0 {
        return format!("{total_seconds:.1}s");
    }
    let minutes = duration_ms / 60_000;
    let seconds = (duration_ms % 60_000) / 1_000;
    format!("{minutes}m {seconds}s")
}

async fn capture_changed_files(workspace_dir: &Path, secrets: &[String]) -> Vec<String> {
    let result = run_shell_command(
        "git status --short",
        &RunOptions {
            cwd: workspace_dir.to_path_buf(),
            timeout: Some(Duration::from_secs(60)),
            secrets: secrets.to_vec(),
            ..RunOptions::default()
        },
    )
    .await;
    // `git status --short` lines are `XY <path>`: drop the two status columns and
    // the separating space (index 3 onward), then trim.
    let mut files: Vec<String> = result
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| line.get(3..).map(|path| path.trim().to_string()))
        .filter(|path| !path.is_empty())
        .collect();
    files.sort();
    files
}

fn claude_env(benchmark: &BenchmarkConfig, model: &ModelConfig) -> BTreeMap<String, String> {
    let base_url = std::env::var("BYTEFUTURE_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| benchmark.claude.base_url.clone());
    let anthropic_base_url = anthropic_base_url_for_claude(&base_url);
    let mut env = BTreeMap::new();
    env.insert("ANTHROPIC_BASE_URL".to_string(), anthropic_base_url);
    env.insert(
        "ANTHROPIC_API_KEY".to_string(),
        anthropic_api_key().unwrap_or_default(),
    );
    if should_send_auth_token(&base_url) {
        env.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            anthropic_auth_token().unwrap_or_default(),
        );
    }
    env.insert(
        "ANTHROPIC_CUSTOM_MODEL_OPTION".to_string(),
        model.model.clone(),
    );
    env.insert("ANTHROPIC_MODEL".to_string(), model.model.clone());
    if benchmark.claude.disable_experimental_betas {
        env.insert(
            "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS".to_string(),
            "1".to_string(),
        );
    }
    env
}

fn command_strategy_labels() -> Vec<String> {
    [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
        "--bare",
        "--settings",
        "--model",
        "--output-format json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_artifacts() -> Artifacts {
    Artifacts {
        stdout: "stdout.txt".to_string(),
        stderr: "stderr.txt".to_string(),
        claude_output: "claude-output.json".to_string(),
        diff: "diff.patch".to_string(),
        workspace: "workspace/".to_string(),
        checks: "checks/".to_string(),
        model_config: "model-config.json".to_string(),
    }
}

fn secret_list() -> Vec<String> {
    let mut secrets = Vec::new();
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "BYTEFUTURE_AUTH_TOKEN",
    ] {
        if let Some(value) = env_nonempty(key) {
            if !secrets.contains(&value) {
                secrets.push(value);
            }
        }
    }
    secrets
}

fn benchmark_auth_secret() -> Option<String> {
    anthropic_api_key().or_else(anthropic_auth_token)
}

fn anthropic_api_key() -> Option<String> {
    env_nonempty("ANTHROPIC_API_KEY")
        .or_else(|| env_nonempty("BYTEFUTURE_AUTH_TOKEN"))
        .or_else(|| env_nonempty("ANTHROPIC_AUTH_TOKEN"))
}

fn anthropic_auth_token() -> Option<String> {
    env_nonempty("ANTHROPIC_AUTH_TOKEN")
        .or_else(|| env_nonempty("BYTEFUTURE_AUTH_TOKEN"))
        .or_else(|| env_nonempty("ANTHROPIC_API_KEY"))
}

fn should_send_auth_token(base_url: &str) -> bool {
    base_url.contains("bytefuture.ai")
        || env_nonempty("ANTHROPIC_AUTH_TOKEN").is_some()
        || env_nonempty("BYTEFUTURE_AUTH_TOKEN").is_some()
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn build_run_id(task_id: &str, model_id: &str, run_index: u32) -> String {
    format_run_id(&now_iso(), task_id, model_id, run_index)
}

fn build_infrastructure_error_result(
    run_id: &str,
    task: &LoadedTask,
    model: &ModelConfig,
    run_index: u32,
    benchmark: &BenchmarkConfig,
    failed: &CommandResult,
) -> RunResult {
    let reason = format!("Setup failed while running \"{}\".", failed.command);
    RunResult {
        run_id: run_id.to_string(),
        task_id: task.config.id.clone(),
        model_id: model.id.clone(),
        provider_model_id: model.model.clone(),
        run_index,
        provider: model.provider.clone(),
        started_at: failed.started_at.clone(),
        finished_at: failed.finished_at.clone(),
        duration_ms: failed.duration_ms,
        claude_exit_code: None,
        claude: ClaudeRunMeta {
            session_id: None,
            output_format: benchmark.claude.output_format.clone(),
            command_strategy: vec![],
            statistics: None,
        },
        checks: vec![],
        completion: Completion {
            status: CompletionStatus::Error,
            reason: reason.clone(),
        },
        tokens: None,
        judge: skipped_judge(&benchmark.judge.model),
        artifacts: default_artifacts(),
        changed_files: vec![],
        warnings: vec![reason],
        human_audit: HumanAudit {
            required_for_mvp: false,
            score: None,
            notes: String::new(),
        },
    }
}

struct ClaudeErrorResultInput<'a> {
    run_id: &'a str,
    task: &'a LoadedTask,
    model: &'a ModelConfig,
    run_index: u32,
    benchmark: &'a BenchmarkConfig,
    claude: &'a CommandResult,
    duration_ms: u64,
    tokens: Option<TokenUsage>,
    statistics: Option<ClaudeRunStatistics>,
    changed_files: Vec<String>,
    reason: String,
}

fn build_claude_error_result(input: ClaudeErrorResultInput<'_>) -> RunResult {
    RunResult {
        run_id: input.run_id.to_string(),
        task_id: input.task.config.id.clone(),
        model_id: input.model.id.clone(),
        provider_model_id: input.model.model.clone(),
        run_index: input.run_index,
        provider: input.model.provider.clone(),
        started_at: input.claude.started_at.clone(),
        finished_at: input.claude.finished_at.clone(),
        duration_ms: input.duration_ms,
        claude_exit_code: input.claude.exit_code,
        claude: ClaudeRunMeta {
            session_id: extract_claude_session_id(&input.claude.stdout),
            output_format: input.benchmark.claude.output_format.clone(),
            command_strategy: command_strategy_labels(),
            statistics: input.statistics,
        },
        checks: vec![],
        completion: Completion {
            status: CompletionStatus::Error,
            reason: input.reason.clone(),
        },
        tokens: input.tokens,
        judge: skipped_judge(&input.benchmark.judge.model),
        artifacts: default_artifacts(),
        changed_files: input.changed_files,
        warnings: vec![input.reason],
        human_audit: HumanAudit {
            required_for_mvp: false,
            score: None,
            notes: String::new(),
        },
    }
}

fn print_dry_run_plan(
    benchmark: &BenchmarkConfig,
    tasks: &[LoadedTask],
    models: &[ModelConfig],
    runs_per_task_model: u32,
    timeout_seconds: u64,
) {
    let total = tasks.len() * models.len() * runs_per_task_model as usize;
    println!("Dry run only. Planned runs: {total}");
    println!("Timeout per Claude run: {timeout_seconds}s");
    println!("Command strategy:");
    let disable = if benchmark.claude.disable_experimental_betas {
        "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1 "
    } else {
        ""
    };
    println!(
        "ANTHROPIC_BASE_URL=<normalized BYTEFUTURE_BASE_URL> ANTHROPIC_API_KEY=[REDACTED] ANTHROPIC_AUTH_TOKEN=[REDACTED] ANTHROPIC_CUSTOM_MODEL_OPTION=<provider-model-id> ANTHROPIC_MODEL=<provider-model-id> {disable}claude --bare -p <task prompt> --settings .claude/settings.json --model <provider-model-id> --output-format json"
    );
    for task in tasks {
        println!("Task: {} ({})", task.config.id, task.fixture_dir.display());
    }
    for model in models {
        println!("Model: {} -> {}", model.id, model.model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SuccessConfig, TaskConfig};
    use serde_json::json;

    #[test]
    fn sanitize_replaces_disallowed_runs_with_a_single_dash() {
        assert_eq!(
            sanitize("deepseek/deepseek-v4-flash"),
            "deepseek-deepseek-v4-flash"
        );
        assert_eq!(sanitize("a b.c"), "a-b-c");
        assert_eq!(sanitize("ok_id-1"), "ok_id-1");
    }

    #[test]
    fn format_run_id_is_sortable_and_has_no_colons_or_dots() {
        let id = format_run_id(
            "2026-06-04T14:03:12.000Z",
            "fix-failing-test",
            "deepseek-v4-flash",
            1,
        );
        assert_eq!(
            id,
            "2026-06-04T14-03-12-000Z_fix-failing-test_deepseek-v4-flash_001"
        );
    }

    #[test]
    fn format_duration_ms_keeps_subsecond_precision_and_minutes() {
        assert_eq!(format_duration_ms(250), "250ms");
        assert_eq!(format_duration_ms(1_234), "1.2s");
        assert_eq!(format_duration_ms(65_432), "1m 5s");
    }

    #[test]
    fn build_run_plans_expands_tasks_models_and_runs_in_order() {
        let tasks = vec![loaded_task("task-a"), loaded_task("task-b")];
        let models = vec![model("model-a"), model("model-b")];

        let plans = build_run_plans(&tasks, &models, 2);

        let keys: Vec<_> = plans
            .iter()
            .map(|plan| {
                (
                    plan.task.config.id.as_str(),
                    plan.model.id.as_str(),
                    plan.run_index,
                )
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                ("task-a", "model-a", 1),
                ("task-a", "model-a", 2),
                ("task-a", "model-b", 1),
                ("task-a", "model-b", 2),
                ("task-b", "model-a", 1),
                ("task-b", "model-a", 2),
                ("task-b", "model-b", 1),
                ("task-b", "model-b", 2),
            ]
        );
    }

    #[test]
    fn extract_session_id_reads_known_keys() {
        assert_eq!(
            extract_claude_session_id(r#"{"session_id":"abc"}"#),
            Some("abc".to_string())
        );
        assert_eq!(
            extract_claude_session_id(r#"{"id":"xyz"}"#),
            Some("xyz".to_string())
        );
        assert_eq!(extract_claude_session_id("not json"), None);
    }

    #[test]
    fn parse_claude_output_falls_back_on_invalid_json() {
        let parsed = parse_claude_output("not json");
        assert_eq!(parsed["parseError"], "Claude stdout was not valid JSON.");

        let ok = parse_claude_output(r#"{"a":1}"#);
        assert_eq!(ok["a"], 1);
    }

    #[test]
    fn claude_api_error_extracts_status_and_message() {
        let parsed = parse_claude_output(
            r#"{"is_error":true,"api_error_status":404,"result":"model unavailable"}"#,
        );
        assert_eq!(
            claude_api_error(&parsed).as_deref(),
            Some("Claude API error 404: model unavailable")
        );

        let ok = parse_claude_output(r#"{"type":"result","is_error":false}"#);
        assert!(claude_api_error(&ok).is_none());
    }

    #[test]
    fn extracts_claude_statistics_from_json_result() {
        let output = json!({
            "duration_ms": 1200,
            "duration_api_ms": 900,
            "ttft_ms": 250,
            "time_to_request_ms": 12,
            "num_turns": 3,
            "total_cost_usd": 0.42,
            "terminal_reason": "completed",
            "stop_reason": "end_turn"
        });

        let statistics = extract_claude_statistics(&output).expect("statistics parsed");

        assert_eq!(statistics.duration_ms, Some(1200));
        assert_eq!(statistics.duration_api_ms, Some(900));
        assert_eq!(statistics.ttft_ms, Some(250));
        assert_eq!(statistics.num_turns, Some(3));
        assert_eq!(statistics.total_cost_usd, Some(0.42));
        assert_eq!(statistics.terminal_reason.as_deref(), Some("completed"));
    }

    #[test]
    fn extracts_token_usage_from_model_usage_breakdown() {
        let output = json!({
            "total_cost_usd": 0.07,
            "usage": {
                "input_tokens": 1,
                "output_tokens": 2
            },
            "modelUsage": {
                "provider/model-a": {
                    "inputTokens": 100,
                    "outputTokens": 20,
                    "cacheCreationInputTokens": 3,
                    "cacheReadInputTokens": 4,
                    "costUSD": 0.05
                },
                "provider/model-b": {
                    "inputTokens": 10,
                    "outputTokens": 2,
                    "costUSD": 0.02
                }
            }
        });

        let tokens = extract_claude_token_usage(&output).expect("tokens parsed");

        assert_eq!(tokens.source, "claude-output-json");
        assert_eq!(tokens.input, Some(110));
        assert_eq!(tokens.output, Some(22));
        assert_eq!(tokens.cache_creation_input, Some(3));
        assert_eq!(tokens.cache_read_input, Some(4));
        assert_eq!(tokens.total, Some(139));
        assert_eq!(tokens.estimated_cost_usd, Some(0.07));
    }

    #[test]
    fn extracts_token_usage_from_top_level_usage_when_breakdown_missing() {
        let output = json!({
            "total_cost_usd": 0.03,
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "cache_creation_input_tokens": 5,
                "cache_read_input_tokens": 7
            }
        });

        let tokens = extract_claude_token_usage(&output).expect("tokens parsed");

        assert_eq!(tokens.input, Some(100));
        assert_eq!(tokens.output, Some(20));
        assert_eq!(tokens.total, Some(132));
        assert_eq!(tokens.estimated_cost_usd, Some(0.03));
    }

    #[test]
    fn args_with_prompt_placeholder_replaces_prompt_value() {
        let args = vec![
            "--bare".to_string(),
            "-p".to_string(),
            "fix the bug".to_string(),
            "--model".to_string(),
            "model-id".to_string(),
        ];

        assert_eq!(
            args_with_prompt_placeholder(&args),
            vec![
                "--bare",
                "-p",
                "<prompt printed below>",
                "--model",
                "model-id"
            ]
        );
    }

    #[test]
    fn truncate_verbose_output_keeps_short_text() {
        assert_eq!(truncate_verbose_output("short"), "short");
    }

    #[test]
    fn truncate_verbose_output_keeps_tail_of_long_text() {
        let text = "a".repeat(VERBOSE_OUTPUT_LIMIT_CHARS + 3) + "tail";
        let truncated = truncate_verbose_output(&text);

        assert!(truncated.starts_with("[output truncated; omitted "));
        assert!(truncated.ends_with("tail"));
        assert!(!truncated.ends_with(&text));
    }

    fn loaded_task(id: &str) -> LoadedTask {
        LoadedTask {
            config: TaskConfig {
                id: id.to_string(),
                title: id.to_string(),
                description: String::new(),
                fixture_path: "fixture".to_string(),
                prompt_file: "prompt.md".to_string(),
                setup: vec![],
                checks: vec![],
                success: SuccessConfig {
                    required_checks: vec![],
                },
                judge: None,
                expected_files: None,
            },
            task_dir: std::path::PathBuf::from(id),
            fixture_dir: std::path::PathBuf::from("fixture"),
            prompt_path: std::path::PathBuf::from("prompt.md"),
            prompt: String::new(),
        }
    }

    fn model(id: &str) -> ModelConfig {
        ModelConfig {
            id: id.to_string(),
            display_name: id.to_string(),
            provider: "models.bytefuture.ai".to_string(),
            model: format!("provider/{id}"),
            claude_model_strategy: "custom-model-option".to_string(),
            enabled: true,
        }
    }
}
