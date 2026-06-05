use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::article::generate_article;
use crate::config::{load_benchmark_config, load_models, project_paths, resolve_project_path};
use crate::evaluator::run_checks;
use crate::fs_utils::{read_json, read_text, write_json};
use crate::judge::{run_judge, JudgeRunInput};
use crate::runner::{run_benchmark, BenchmarkArgs};
use crate::tasks::load_tasks;
use crate::token_station::import_token_dump;
use crate::types::RunResult;

#[derive(Parser)]
#[command(name = "token-station-arena", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run benchmarks across selected tasks and models.
    Benchmark(BenchmarkCommand),
    /// Re-run the LLM judge for an existing run.
    Judge {
        #[arg(long)]
        run_id: String,
    },
    /// Re-run the deterministic checks for an existing run.
    Evaluate {
        #[arg(long)]
        run_id: String,
    },
    /// Import a Token Station dump into existing run results.
    ImportTokenDump {
        #[arg(long)]
        input: Option<String>,
        #[arg(long)]
        runs: Option<String>,
    },
    /// Generate the Markdown article from run results.
    GenerateArticle {
        #[arg(long)]
        input: Option<String>,
        #[arg(long)]
        output: Option<String>,
    },
}

#[derive(Args)]
struct BenchmarkCommand {
    #[arg(long)]
    tasks: Option<String>,
    #[arg(long)]
    models: Option<String>,
    #[arg(long)]
    runs: Option<u32>,
    #[arg(long)]
    timeout: Option<u64>,
    #[arg(long)]
    token_dump: Option<String>,
    #[arg(long)]
    skip_judge: bool,
    #[arg(long)]
    skip_article: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    verbose: bool,
}

/// Parses CLI arguments and dispatches to the matching subcommand.
pub async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Benchmark(command) => {
            run_benchmark(&BenchmarkArgs {
                tasks: command.tasks,
                models: command.models,
                runs: command.runs,
                timeout: command.timeout,
                token_dump: command.token_dump,
                skip_judge: command.skip_judge,
                skip_article: command.skip_article,
                dry_run: command.dry_run,
                verbose: command.verbose,
            })
            .await
        }
        Command::Judge { run_id } => judge_command(&run_id).await,
        Command::Evaluate { run_id } => evaluate_command(&run_id).await,
        Command::ImportTokenDump { input, runs } => import_command(input, runs),
        Command::GenerateArticle { input, output } => generate_command(input, output),
    }
}

fn secret_list() -> Vec<String> {
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .into_iter()
        .collect()
}

async fn judge_command(run_id: &str) -> Result<()> {
    let paths = project_paths(std::env::current_dir().context("determine current directory")?);
    let benchmark = load_benchmark_config(&paths)?;
    let run_dir = resolve_project_path(&paths.root_dir, &benchmark.output_dir).join(run_id);
    let result_path = run_dir.join("result.json");
    let mut result: RunResult = read_json(&result_path)?;

    let tasks = load_tasks(&paths.tasks_dir)?;
    let task = tasks
        .iter()
        .find(|t| t.config.id == result.task_id)
        .with_context(|| format!("task not found for result: {}", result.task_id))?;
    let models = load_models(&paths)?;
    let model = models
        .iter()
        .find(|m| m.id == result.model_id)
        .with_context(|| format!("model not found for result: {}", result.model_id))?;

    let diff = read_text(run_dir.join("diff.patch")).unwrap_or_default();
    let claude_stdout = read_text(run_dir.join("stdout.txt")).unwrap_or_default();
    let claude_stderr = read_text(run_dir.join("stderr.txt")).unwrap_or_default();

    result.judge = run_judge(&JudgeRunInput {
        benchmark: &benchmark,
        task,
        model,
        workspace_dir: &run_dir.join("workspace"),
        run_dir: &run_dir,
        diff: &diff,
        changed_files: &result.changed_files,
        checks: &result.checks,
        claude_stdout: &claude_stdout,
        claude_stderr: &claude_stderr,
        timeout: Duration::from_secs(benchmark.timeout_seconds),
        secrets: &secret_list(),
    })
    .await;
    write_json(&result_path, &result)?;

    println!(
        "Judge {} for {run_id}.",
        if result.judge.passed {
            "passed"
        } else {
            "failed"
        }
    );
    Ok(())
}

async fn evaluate_command(run_id: &str) -> Result<()> {
    let paths = project_paths(std::env::current_dir().context("determine current directory")?);
    let benchmark = load_benchmark_config(&paths)?;
    let run_dir = resolve_project_path(&paths.root_dir, &benchmark.output_dir).join(run_id);
    let result_path = run_dir.join("result.json");
    let mut result: RunResult = read_json(&result_path)?;

    let tasks = load_tasks(&paths.tasks_dir)?;
    let task = tasks
        .iter()
        .find(|t| t.config.id == result.task_id)
        .with_context(|| format!("task not found for result: {}", result.task_id))?;

    result.checks = run_checks(
        task,
        &run_dir.join("workspace"),
        &run_dir,
        Duration::from_secs(benchmark.timeout_seconds),
        &secret_list(),
    )
    .await?;
    write_json(&result_path, &result)?;

    println!("Re-ran {} check(s) for {run_id}.", result.checks.len());
    Ok(())
}

fn import_command(input: Option<String>, runs: Option<String>) -> Result<()> {
    let paths = project_paths(std::env::current_dir().context("determine current directory")?);
    let benchmark = load_benchmark_config(&paths)?;
    let input_path = resolve_project_path(
        &paths.root_dir,
        input
            .as_deref()
            .unwrap_or(&benchmark.token_station.dump_path),
    );
    let runs_dir = resolve_project_path(
        &paths.root_dir,
        runs.as_deref().unwrap_or(&benchmark.output_dir),
    );
    let padding = benchmark
        .token_station
        .match_window_padding_seconds
        .unwrap_or(60);

    let summary = import_token_dump(&input_path, &runs_dir, padding)?;
    println!(
        "Token import updated {}/{} run(s); unmatched: {}.",
        summary.updated, summary.run_files, summary.unmatched
    );
    Ok(())
}

fn generate_command(input: Option<String>, output: Option<String>) -> Result<()> {
    let paths = project_paths(std::env::current_dir().context("determine current directory")?);
    let benchmark = load_benchmark_config(&paths)?;
    let input_dir = resolve_project_path(
        &paths.root_dir,
        input.as_deref().unwrap_or(&benchmark.output_dir),
    );
    let output_path = resolve_project_path(
        &paths.root_dir,
        output.as_deref().unwrap_or(&benchmark.article.output_file),
    );

    let summary = generate_article(&input_dir, &output_path, &benchmark.article.title)?;
    println!(
        "Generated {} from {} run(s).",
        summary.output_path.display(),
        summary.run_count
    );
    Ok(())
}
