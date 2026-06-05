use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::article::generate_article;
use crate::command::{format_command, redact_env, run_process, run_shell_command, RunOptions};
use crate::config::{load_benchmark_config, load_models, project_paths, resolve_project_path};
use crate::evaluator::{classify_completion, run_checks, CompletionInputs};
use crate::fs_utils::{copy_dir, ensure_dir, path_exists, read_text, write_json, write_text};
use crate::judge::{run_judge, skipped_judge, JudgeRunInput};
use crate::models::select_models;
use crate::tasks::{load_tasks, select_tasks};
use crate::token_station::import_token_dump;
use crate::types::{
    Artifacts, BenchmarkConfig, ClaudeRunMeta, CommandResult, Completion, CompletionStatus,
    HumanAudit, LoadedTask, ModelConfig, RunResult,
};
use crate::util::now_iso;

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

/// Options controlling a benchmark run, mirroring the CLI flags.
#[derive(Debug, Clone, Default)]
pub struct BenchmarkArgs {
    pub tasks: Option<String>,
    pub models: Option<String>,
    pub runs: Option<u32>,
    pub timeout: Option<u64>,
    pub token_dump: Option<String>,
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
    let output_dir = resolve_project_path(&paths.root_dir, &benchmark.output_dir);
    let report_dir = resolve_project_path(&paths.root_dir, &benchmark.report_dir);

    println!(
        "Selected {} task(s), {} model(s), {} run(s) each.",
        tasks.len(),
        models.len(),
        runs_per_task_model
    );
    println!("Run artifacts: {}", output_dir.display());

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

    if std::env::var("ANTHROPIC_API_KEY")
        .unwrap_or_default()
        .is_empty()
    {
        bail!("ANTHROPIC_API_KEY is required for real benchmark runs. Use --dry-run to inspect the plan without calling Claude.");
    }

    ensure_dir(&output_dir)?;
    ensure_dir(&report_dir)?;

    let timeout = Duration::from_secs(timeout_seconds);
    for task in &tasks {
        for model in &models {
            for run_index in 1..=runs_per_task_model {
                let result = run_single_benchmark(
                    &benchmark,
                    task,
                    model,
                    run_index,
                    &output_dir,
                    timeout,
                    args.skip_judge,
                    args.verbose,
                )
                .await?;
                println!(
                    "{}: {} ({})",
                    result.run_id,
                    result.completion.status.as_str(),
                    result.completion.reason
                );
                if args.verbose {
                    print_verbose_run_failure_details(&output_dir.join(&result.run_id), &result);
                }
            }
        }
    }

    let token_dump = args
        .token_dump
        .clone()
        .unwrap_or_else(|| benchmark.token_station.dump_path.clone());
    let token_dump_path = resolve_project_path(&paths.root_dir, &token_dump);
    if benchmark.token_station.enabled && path_exists(&token_dump_path) {
        let padding = benchmark
            .token_station
            .match_window_padding_seconds
            .unwrap_or(60);
        let summary = import_token_dump(&token_dump_path, &output_dir, padding)?;
        println!(
            "Token import updated {}/{} run(s).",
            summary.updated, summary.run_files
        );
    }

    if !args.skip_article {
        let article_path = resolve_project_path(&paths.root_dir, &benchmark.article.output_file);
        let summary = generate_article(&output_dir, &article_path, &benchmark.article.title)?;
        println!(
            "Generated {} from {} run(s).",
            summary.output_path.display(),
            summary.run_count
        );
    }

    Ok(())
}

async fn run_single_benchmark(
    benchmark: &BenchmarkConfig,
    task: &LoadedTask,
    model: &ModelConfig,
    run_index: u32,
    output_dir: &Path,
    timeout: Duration,
    skip_judge: bool,
    verbose: bool,
) -> Result<RunResult> {
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

    let claude = run_claude(
        benchmark,
        task,
        model,
        &workspace_dir,
        timeout,
        &secrets,
        &run_id,
        verbose,
    )
    .await;
    write_text(run_dir.join("stdout.txt"), &claude.stdout)?;
    write_text(run_dir.join("stderr.txt"), &claude.stderr)?;
    write_json(
        run_dir.join("claude-output.json"),
        &parse_claude_output(&claude.stdout),
    )?;
    write_json(
        run_dir.join("command-strategy.json"),
        &json!({
            "command": format_command("claude", claude.args.as_deref().unwrap_or(&[])),
            "env": redact_env(&claude_env(benchmark, model)),
        }),
    )?;

    let checks = run_checks(task, &workspace_dir, &run_dir, timeout, &secrets).await?;
    let diff = capture_diff(&workspace_dir, &secrets).await;
    let changed_files = capture_changed_files(&workspace_dir, &secrets).await;
    write_text(run_dir.join("diff.patch"), &diff)?;

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
        duration_ms: claude.duration_ms,
        claude_exit_code: claude.exit_code,
        claude: ClaudeRunMeta {
            session_id: extract_claude_session_id(&claude.stdout),
            output_format: benchmark.claude.output_format.clone(),
            command_strategy: command_strategy_labels(),
        },
        checks,
        completion,
        tokens: None,
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

async fn run_claude(
    benchmark: &BenchmarkConfig,
    task: &LoadedTask,
    model: &ModelConfig,
    workspace_dir: &Path,
    timeout: Duration,
    secrets: &[String],
    run_id: &str,
    verbose: bool,
) -> CommandResult {
    let env: Vec<(String, String)> = claude_env(benchmark, model).into_iter().collect();
    let args = vec![
        "--bare".to_string(),
        "-p".to_string(),
        task.prompt.clone(),
        "--settings".to_string(),
        benchmark.claude.project_settings_file.clone(),
        "--model".to_string(),
        model.model.clone(),
        "--output-format".to_string(),
        benchmark.claude.output_format.clone(),
    ];
    if verbose {
        print_verbose_claude_invocation(run_id, model, &args);
    }
    run_process(
        "claude",
        &args,
        &RunOptions {
            cwd: workspace_dir.to_path_buf(),
            env,
            timeout: Some(timeout),
            secrets: secrets.to_vec(),
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
    let mut env = BTreeMap::new();
    env.insert("ANTHROPIC_BASE_URL".to_string(), base_url);
    env.insert(
        "ANTHROPIC_API_KEY".to_string(),
        std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
    );
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
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .into_iter()
        .collect()
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
        "ANTHROPIC_BASE_URL=<BYTEFUTURE_BASE_URL> ANTHROPIC_API_KEY=[REDACTED] ANTHROPIC_CUSTOM_MODEL_OPTION=<provider-model-id> ANTHROPIC_MODEL=<provider-model-id> {disable}claude --bare -p <task prompt> --settings .claude/settings.json --model <provider-model-id> --output-format json"
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
}
