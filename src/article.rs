use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::fs_utils::{find_files, read_json, write_text};
use crate::types::{CompletionStatus, RunResult};
use crate::util::now_iso;

/// Per-model rollup used by the summary and comparison tables.
#[derive(Debug, Clone)]
pub struct ModelAggregate {
    pub model_id: String,
    pub provider_model_id: String,
    pub total: usize,
    pub passed: usize,
    pub avg_judge_score: Option<f64>,
    pub avg_input_tokens: Option<f64>,
    pub avg_output_tokens: Option<f64>,
    pub avg_tokens: Option<f64>,
    pub avg_cost_usd: Option<f64>,
    pub avg_latency_ms: Option<f64>,
}

/// Where an article was written and how many runs fed it.
#[derive(Debug, Clone)]
pub struct GenerateSummary {
    pub output_path: PathBuf,
    pub run_count: usize,
}

/// Groups runs by model and computes per-model averages.
pub fn aggregate_by_model(runs: &[RunResult]) -> Vec<ModelAggregate> {
    use std::collections::BTreeMap;

    let mut groups: BTreeMap<&str, Vec<&RunResult>> = BTreeMap::new();
    for run in runs {
        groups.entry(run.model_id.as_str()).or_default().push(run);
    }

    groups
        .into_iter()
        .map(|(model_id, model_runs)| {
            let judge_scores: Vec<f64> = model_runs.iter().filter_map(|r| r.judge.score).collect();
            let token_field = |pick: fn(&RunResult) -> Option<i64>| -> Vec<f64> {
                model_runs
                    .iter()
                    .filter_map(|&r| pick(r))
                    .map(|v| v as f64)
                    .collect()
            };
            let input_tokens = token_field(|r| r.tokens.as_ref().and_then(|t| t.input));
            let output_tokens = token_field(|r| r.tokens.as_ref().and_then(|t| t.output));
            let token_totals = token_field(|r| r.tokens.as_ref().and_then(|t| t.total));
            let costs: Vec<f64> = model_runs
                .iter()
                .filter_map(|r| r.tokens.as_ref().and_then(|t| t.estimated_cost_usd))
                .collect();
            let latencies: Vec<f64> = model_runs.iter().map(|r| r.duration_ms as f64).collect();

            ModelAggregate {
                model_id: model_id.to_string(),
                provider_model_id: model_runs
                    .first()
                    .map(|r| r.provider_model_id.clone())
                    .unwrap_or_else(|| model_id.to_string()),
                total: model_runs.len(),
                passed: model_runs
                    .iter()
                    .filter(|r| r.completion.status == CompletionStatus::Passed)
                    .count(),
                avg_judge_score: average(&judge_scores),
                avg_input_tokens: average(&input_tokens),
                avg_output_tokens: average(&output_tokens),
                avg_tokens: average(&token_totals),
                avg_cost_usd: average(&costs),
                avg_latency_ms: average(&latencies),
            }
        })
        .collect()
}

/// Renders the full benchmark article as Markdown.
pub fn render_article(title: &str, runs: &[RunResult]) -> String {
    let models = aggregate_by_model(runs);
    let mut tasks: Vec<&str> = runs.iter().map(|r| r.task_id.as_str()).collect();
    tasks.sort();
    tasks.dedup();

    let tested_models = if models.is_empty() {
        "No completed run results were found.".to_string()
    } else {
        render_model_list(&models)
    };
    let task_list = if tasks.is_empty() {
        "No task results were found.".to_string()
    } else {
        tasks
            .iter()
            .map(|t| format!("- `{t}`"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    [
        format!("# {title}"),
        String::new(),
        format!("Generated at: {}", now_iso()),
        String::new(),
        "## Introduction".to_string(),
        String::new(),
        "This guide compares Claude Code-style Rust development tasks across models routed through `models.bytefuture.ai`. Each run records command output, deterministic checks, git diff, latency, judge score, and Token Station usage when a backend dump is available.".to_string(),
        String::new(),
        "## Methodology".to_string(),
        String::new(),
        "- Every model receives the same task prompt and an isolated copy of the fixture.".to_string(),
        "- The runner initializes git before model execution and captures the resulting diff.".to_string(),
        "- Completion is determined by required deterministic checks and, when enabled, an LLM judge.".to_string(),
        "- Token usage is imported after execution by matching Token Station dump records to model IDs and execution time windows.".to_string(),
        String::new(),
        "## Tested Models".to_string(),
        String::new(),
        tested_models,
        String::new(),
        "## Tasks".to_string(),
        String::new(),
        task_list,
        String::new(),
        "## Summary".to_string(),
        String::new(),
        render_summary_table(&models),
        String::new(),
        "## Per-Task Results".to_string(),
        String::new(),
        render_per_task_tables(runs),
        String::new(),
        "## Token And Latency Comparison".to_string(),
        String::new(),
        render_token_latency_table(&models),
        String::new(),
        "## Three-Run Stability".to_string(),
        String::new(),
        render_stability_table(runs),
        String::new(),
        "## Judge Scores".to_string(),
        String::new(),
        render_judge_table(&models),
        String::new(),
        "## Quality Notes And Failure Modes".to_string(),
        String::new(),
        render_quality_notes(runs),
        String::new(),
        "## Reproducible Commands".to_string(),
        String::new(),
        "```bash".to_string(),
        "cargo run --release -- benchmark --tasks all --models all".to_string(),
        "cargo run --release -- benchmark --tasks fix-failing-test --models openai-gpt-5-5,minimax-m2-7,glm-5".to_string(),
        "cargo run --release -- import-token-dump --input benchmark/reports/token-station-usage.json --runs benchmark/runs".to_string(),
        "cargo run --release -- generate-article --input benchmark/runs --output benchmark/reports/article.md".to_string(),
        "```".to_string(),
        String::new(),
        "## Limitations".to_string(),
        String::new(),
        "- This is a practical engineering benchmark, not an academic benchmark.".to_string(),
        "- Results depend on the exact task suite, fixture state, gateway behavior, and Claude Code version.".to_string(),
        "- Token usage is only populated when a Token Station backend dump can be matched confidently.".to_string(),
        "- Judge scores should be treated as structured review evidence, not as a replacement for human audit.".to_string(),
        String::new(),
        "## Conclusion".to_string(),
        String::new(),
        render_conclusion(&models),
        String::new(),
    ]
    .join("\n")
}

fn format_number(value: Option<f64>, digits: usize) -> String {
    match value {
        Some(v) if v.is_finite() => group_decimal(v, digits),
        _ => "n/a".to_string(),
    }
}

fn format_duration(value_ms: Option<f64>) -> String {
    match value_ms {
        Some(v) if v.is_finite() => {
            let total_seconds = (v / 1000.0).round() as i64;
            let minutes = total_seconds / 60;
            let seconds = total_seconds % 60;
            if minutes > 0 {
                format!("{minutes}m {seconds}s")
            } else {
                format!("{seconds}s")
            }
        }
        _ => "n/a".to_string(),
    }
}

/// Formats `value` with fixed decimal `digits` and thousands separators, like
/// JavaScript's `Number.toLocaleString("en-US", ...)`.
fn group_decimal(value: f64, digits: usize) -> String {
    let formatted = format!("{value:.digits$}");
    let negative = formatted.starts_with('-');
    let unsigned = formatted.trim_start_matches('-');
    let (int_part, frac_part) = match unsigned.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (unsigned, None),
    };

    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&group_thousands(int_part));
    if let Some(frac) = frac_part {
        out.push('.');
        out.push_str(frac);
    }
    out
}

fn group_thousands(int_part: &str) -> String {
    let len = int_part.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in int_part.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn note_for(model: &ModelAggregate) -> String {
    if model.total == 0 {
        String::new()
    } else if model.passed == model.total {
        "Completed every recorded run".to_string()
    } else if model.passed == 0 {
        "No recorded passes".to_string()
    } else {
        "Mixed completion; inspect task-level failures".to_string()
    }
}

fn render_model_list(models: &[ModelAggregate]) -> String {
    models
        .iter()
        .map(|m| format!("- `{}` ({})", m.model_id, m.provider_model_id))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_summary_table(models: &[ModelAggregate]) -> String {
    if models.is_empty() {
        return "No run data is available yet.".to_string();
    }
    let mut lines = vec![
        "| Model | Pass Rate | Avg Judge Score | Avg Total Tokens | Avg Latency | Notes |"
            .to_string(),
        "| --- | ---: | ---: | ---: | ---: | --- |".to_string(),
    ];
    for m in models {
        lines.push(format!(
            "| {} | {}/{} | {} | {} | {} | {} |",
            m.model_id,
            m.passed,
            m.total,
            format_number(m.avg_judge_score, 1),
            format_number(m.avg_tokens, 0),
            format_duration(m.avg_latency_ms),
            note_for(m)
        ));
    }
    lines.join("\n")
}

fn render_per_task_tables(runs: &[RunResult]) -> String {
    let mut tasks: Vec<&str> = runs.iter().map(|r| r.task_id.as_str()).collect();
    tasks.sort();
    tasks.dedup();
    if tasks.is_empty() {
        return "No per-task data is available yet.".to_string();
    }

    tasks
        .iter()
        .map(|task_id| {
            let mut lines = vec![
                format!("### {task_id}"),
                String::new(),
                "| Model | Run | Status | Required Checks | Judge | Latency |".to_string(),
                "| --- | ---: | --- | --- | ---: | ---: |".to_string(),
            ];
            for run in runs.iter().filter(|r| r.task_id == **task_id) {
                let required = run
                    .checks
                    .iter()
                    .map(|c| format!("{}:{}", c.name, if c.passed { "pass" } else { "fail" }))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!(
                    "| {} | {} | {} | {} | {} | {} |",
                    run.model_id,
                    run.run_index,
                    run.completion.status.as_str(),
                    required,
                    format_number(run.judge.score, 1),
                    format_duration(Some(run.duration_ms as f64))
                ));
            }
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_token_latency_table(models: &[ModelAggregate]) -> String {
    if models.is_empty() {
        return "No token or latency data is available yet.".to_string();
    }
    let mut lines = vec![
        "| Model | Avg Input Tokens | Avg Output Tokens | Avg Total Tokens | Avg Cost USD | Avg Latency |".to_string(),
        "| --- | ---: | ---: | ---: | ---: | ---: |".to_string(),
    ];
    for m in models {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} |",
            m.model_id,
            format_number(m.avg_input_tokens, 0),
            format_number(m.avg_output_tokens, 0),
            format_number(m.avg_tokens, 0),
            format_number(m.avg_cost_usd, 4),
            format_duration(m.avg_latency_ms)
        ));
    }
    lines.join("\n")
}

fn render_stability_table(runs: &[RunResult]) -> String {
    if runs.is_empty() {
        return "No stability data is available yet.".to_string();
    }
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<&RunResult>> = BTreeMap::new();
    for run in runs {
        groups
            .entry(format!("{}:{}", run.model_id, run.task_id))
            .or_default()
            .push(run);
    }

    let mut lines = vec![
        "| Model | Task | Passes | Runs | Statuses |".to_string(),
        "| --- | --- | ---: | ---: | --- |".to_string(),
    ];
    for group in groups.values() {
        let first = group[0];
        let passes = group
            .iter()
            .filter(|r| r.completion.status == CompletionStatus::Passed)
            .count();
        let statuses = group
            .iter()
            .map(|r| r.completion.status.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            first.model_id,
            first.task_id,
            passes,
            group.len(),
            statuses
        ));
    }
    lines.join("\n")
}

fn render_judge_table(models: &[ModelAggregate]) -> String {
    if models.is_empty() {
        return "No judge data is available yet.".to_string();
    }
    let mut lines = vec![
        "| Model | Avg Judge Score | Pass Rate |".to_string(),
        "| --- | ---: | ---: |".to_string(),
    ];
    for m in models {
        lines.push(format!(
            "| {} | {} | {}/{} |",
            m.model_id,
            format_number(m.avg_judge_score, 1),
            m.passed,
            m.total
        ));
    }
    lines.join("\n")
}

fn render_quality_notes(runs: &[RunResult]) -> String {
    let mut findings = Vec::new();
    for run in runs {
        for finding in &run.judge.findings {
            findings.push(format!(
                "- `{}` on `{}` run {}: {}/{}: {}",
                run.model_id,
                run.task_id,
                run.run_index,
                finding.severity,
                finding.category,
                finding.message
            ));
        }
    }
    if findings.is_empty() {
        return "No judge findings were recorded. Review diffs in run artifacts before publishing stronger claims.".to_string();
    }
    findings.join("\n")
}

fn render_conclusion(models: &[ModelAggregate]) -> String {
    if models.is_empty() {
        return "Run the benchmark to produce evidence-backed recommendations.".to_string();
    }
    if models.iter().all(|model| model.passed == 0) {
        return "No model produced a passing run in this run set. Treat the results as failure diagnostics rather than a model recommendation.".to_string();
    }
    let best = models
        .iter()
        .max_by(|a, b| {
            let pass_rate = |m: &ModelAggregate| m.passed as f64 / (m.total.max(1) as f64);
            pass_rate(a)
                .partial_cmp(&pass_rate(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.avg_judge_score
                        .unwrap_or(0.0)
                        .partial_cmp(&b.avg_judge_score.unwrap_or(0.0))
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        })
        .expect("non-empty");
    format!(
        "In this run set, `{}` had the strongest aggregate completion signal with {}/{} passing runs. Treat this as task-suite evidence and compare it with token, latency, and diff quality before making deployment decisions.",
        best.model_id, best.passed, best.total
    )
}

/// Loads, renders, and writes the article. Returns where it was written and how
/// many runs contributed.
pub fn generate_article(
    runs_dir: &Path,
    output_path: &Path,
    title: &str,
) -> Result<GenerateSummary> {
    let runs = load_run_results(runs_dir)?;
    generate_article_for_runs(&runs, output_path, title)
}

/// Renders and writes an article from an explicit run set. Benchmark execution
/// uses this so stale artifacts from previous invocations do not leak into the
/// current report.
pub fn generate_article_for_runs(
    runs: &[RunResult],
    output_path: &Path,
    title: &str,
) -> Result<GenerateSummary> {
    let markdown = render_article(title, runs);
    write_text(output_path, &markdown)?;
    Ok(GenerateSummary {
        output_path: output_path.to_path_buf(),
        run_count: runs.len(),
    })
}

/// Loads every `result.json` under `runs_dir`, sorted by run id.
pub fn load_run_results(runs_dir: &Path) -> Result<Vec<RunResult>> {
    let files = find_files(runs_dir, "result.json")?;
    let mut runs: Vec<RunResult> = Vec::with_capacity(files.len());
    for file in &files {
        runs.push(read_json(file)?);
    }
    runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Artifacts, ClaudeRunMeta, Completion, HumanAudit, JudgeResult, TokenUsage};

    fn run(
        model_id: &str,
        task_id: &str,
        status: CompletionStatus,
        score: Option<f64>,
        total_tokens: Option<i64>,
        duration_ms: u64,
    ) -> RunResult {
        RunResult {
            run_id: format!("{task_id}-{model_id}"),
            task_id: task_id.into(),
            model_id: model_id.into(),
            provider_model_id: format!("prov/{model_id}"),
            run_index: 1,
            provider: "models.bytefuture.ai".into(),
            started_at: "2026-06-04T10:00:00.000Z".into(),
            finished_at: "2026-06-04T10:03:00.000Z".into(),
            duration_ms,
            claude_exit_code: Some(0),
            claude: ClaudeRunMeta {
                session_id: None,
                output_format: "json".into(),
                command_strategy: vec![],
            },
            checks: vec![],
            completion: Completion {
                status,
                reason: "r".into(),
            },
            tokens: Some(TokenUsage {
                source: "s".into(),
                correlation: "c".into(),
                dump_file: "d".into(),
                input: total_tokens,
                output: None,
                cache_creation_input: None,
                cache_read_input: None,
                total: total_tokens,
                estimated_cost_usd: None,
            }),
            judge: JudgeResult {
                enabled: true,
                model_id: "m".into(),
                score,
                passed: matches!(status, CompletionStatus::Passed),
                correctness: None,
                maintainability: None,
                scope_control: None,
                has_unrelated_changes: false,
                findings: vec![],
                raw_output_path: None,
                error: None,
            },
            artifacts: Artifacts {
                stdout: String::new(),
                stderr: String::new(),
                claude_output: String::new(),
                diff: String::new(),
                workspace: String::new(),
                checks: String::new(),
                model_config: String::new(),
            },
            changed_files: vec![],
            warnings: vec![],
            human_audit: HumanAudit {
                required_for_mvp: false,
                score: None,
                notes: String::new(),
            },
        }
    }

    #[test]
    fn format_number_groups_thousands_and_fixes_digits() {
        assert_eq!(format_number(Some(15200.0), 0), "15,200");
        assert_eq!(format_number(Some(4.1), 1), "4.1");
        assert_eq!(format_number(None, 0), "n/a");
    }

    #[test]
    fn format_duration_renders_minutes_and_seconds() {
        assert_eq!(format_duration(Some(192000.0)), "3m 12s");
        assert_eq!(format_duration(Some(48000.0)), "48s");
        assert_eq!(format_duration(None), "n/a");
    }

    #[test]
    fn aggregate_groups_by_model_with_pass_counts_and_averages() {
        let runs = vec![
            run(
                "deepseek-v4-flash",
                "fix",
                CompletionStatus::Passed,
                Some(4.0),
                Some(15000),
                180000,
            ),
            run(
                "deepseek-v4-flash",
                "api",
                CompletionStatus::Failed,
                Some(2.0),
                Some(13000),
                120000,
            ),
        ];
        let agg = aggregate_by_model(&runs);
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].total, 2);
        assert_eq!(agg[0].passed, 1);
        assert_eq!(agg[0].avg_judge_score, Some(3.0));
        assert_eq!(agg[0].avg_tokens, Some(14000.0));
    }

    #[test]
    fn render_article_includes_title_models_and_cargo_commands() {
        let runs = vec![run(
            "deepseek-v4-flash",
            "fix-failing-test",
            CompletionStatus::Passed,
            Some(4.0),
            Some(15000),
            180000,
        )];
        let markdown = render_article("My Benchmark", &runs);
        assert!(markdown.contains("# My Benchmark"));
        assert!(markdown.contains("deepseek-v4-flash"));
        assert!(markdown.contains("## Summary"));
        assert!(markdown.contains("cargo run --release -- benchmark"));
        assert!(!markdown.contains("npm run"));
    }

    #[test]
    fn generate_article_for_runs_uses_only_explicit_runs() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("article.md");
        let runs = vec![run(
            "deepseek-v4-flash",
            "fix-failing-test",
            CompletionStatus::Passed,
            Some(4.0),
            Some(15000),
            180000,
        )];

        let summary = generate_article_for_runs(&runs, &output, "Current").unwrap();
        let markdown = std::fs::read_to_string(output).unwrap();

        assert_eq!(summary.run_count, 1);
        assert!(markdown.contains("deepseek-v4-flash"));
        assert!(!markdown.contains("gpt-oss-20b"));
    }

    #[test]
    fn render_conclusion_does_not_pick_a_winner_when_all_runs_fail() {
        let models = aggregate_by_model(&[run(
            "nemotron-3-super",
            "fix-failing-test",
            CompletionStatus::Failed,
            None,
            None,
            1000,
        )]);

        let conclusion = render_conclusion(&models);
        assert!(conclusion.contains("No model produced a passing run"));
        assert!(!conclusion.contains("strongest aggregate"));
    }
}
