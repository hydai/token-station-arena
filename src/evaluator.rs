use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::command::{run_shell_command, RunOptions};
use crate::fs_utils::{ensure_dir, write_text};
use crate::types::{CheckResult, Completion, CompletionStatus, JudgeResult, LoadedTask};

/// Runs every deterministic check command for a task in the workspace, writing
/// each command's stdout/stderr under `<run_dir>/checks/` and returning a
/// structured result per check.
pub async fn run_checks(
    task: &LoadedTask,
    workspace_dir: &Path,
    run_dir: &Path,
    timeout: Duration,
    secrets: &[String],
) -> Result<Vec<CheckResult>> {
    let checks_dir = run_dir.join("checks");
    ensure_dir(&checks_dir)?;

    let mut results = Vec::with_capacity(task.config.checks.len());
    for check in &task.config.checks {
        let check_timeout = check
            .timeout_seconds
            .map(Duration::from_secs)
            .unwrap_or(timeout);
        let options = RunOptions {
            cwd: workspace_dir.to_path_buf(),
            timeout: Some(check_timeout),
            secrets: secrets.to_vec(),
            ..RunOptions::default()
        };
        let outcome = run_shell_command(&check.command, &options).await;

        write_text(
            checks_dir.join(format!("{}.stdout.txt", check.name)),
            &outcome.stdout,
        )?;
        write_text(
            checks_dir.join(format!("{}.stderr.txt", check.name)),
            &outcome.stderr,
        )?;

        results.push(CheckResult {
            name: check.name.clone(),
            command: check.command.clone(),
            exit_code: outcome.exit_code,
            passed: outcome.exit_code == Some(0) && !outcome.timed_out,
            duration_ms: outcome.duration_ms,
            stdout_path: Some(format!("checks/{}.stdout.txt", check.name)),
            stderr_path: Some(format!("checks/{}.stderr.txt", check.name)),
            timed_out: Some(outcome.timed_out),
        });
    }
    Ok(results)
}

/// Everything `classify_completion` needs to decide a run's status.
pub struct CompletionInputs<'a> {
    pub task: &'a LoadedTask,
    pub checks: &'a [CheckResult],
    pub judge: &'a JudgeResult,
    pub claude_exit_code: Option<i32>,
    pub claude_timed_out: bool,
    pub changed_files: &'a [String],
    pub infrastructure_error: Option<String>,
}

/// Classifies a run into one of the five completion statuses.
///
/// The decision is a small precedence ladder (highest precedence first):
///
/// 1. An `infrastructure_error` short-circuits to `Error` (the runner itself
///    failed — setup, copy, etc.), using the error text as the reason.
/// 2. If Claude itself timed out, the status is `Timeout`.
/// 3. Otherwise look at the *required* checks (`task.config.success.required_checks`):
///    - If none of them failed and the judge is enabled but did NOT pass, the
///      run is `Partial` (the code works but the judge rejected it — surface
///      `judge_failure_reason`).
///    - If none of them failed, the run is `Passed`. The reason differs based on
///      whether the judge was enabled (accepted) or skipped.
///    - If some required check failed but there was *progress* (any file changed
///      or any check passed), it is `Partial`, naming the failed checks.
///    - If a required check failed with no progress and Claude exited non-zero,
///      it is `Failed` (mention the exit code).
///    - Otherwise it is `Failed` (no usable change produced).
///
/// The `judge_failure_reason` helper (below) explains a judge rejection.
pub fn classify_completion(inputs: &CompletionInputs) -> Completion {
    if let Some(error) = &inputs.infrastructure_error {
        return Completion {
            status: CompletionStatus::Error,
            reason: error.clone(),
        };
    }

    if inputs.claude_timed_out {
        return Completion {
            status: CompletionStatus::Timeout,
            reason: "Claude execution exceeded the configured timeout.".to_string(),
        };
    }

    let required: std::collections::HashSet<&str> = inputs
        .task
        .config
        .success
        .required_checks
        .iter()
        .map(String::as_str)
        .collect();
    let failed_required: Vec<&str> = inputs
        .checks
        .iter()
        .filter(|check| required.contains(check.name.as_str()) && !check.passed)
        .map(|check| check.name.as_str())
        .collect();

    if failed_required.is_empty() {
        if inputs.judge.enabled && !inputs.judge.passed {
            return Completion {
                status: CompletionStatus::Partial,
                reason: format!(
                    "Required checks passed, but judge failed: {}",
                    judge_failure_reason(inputs.judge)
                ),
            };
        }
        let reason = if inputs.judge.enabled {
            "All required checks passed and judge accepted the run."
        } else {
            "All required checks passed; judge was skipped."
        };
        return Completion {
            status: CompletionStatus::Passed,
            reason: reason.to_string(),
        };
    }

    let made_progress = !inputs.changed_files.is_empty();
    if made_progress {
        return Completion {
            status: CompletionStatus::Partial,
            reason: format!("Failed required check(s): {}.", failed_required.join(", ")),
        };
    }

    if inputs.claude_exit_code != Some(0) {
        let code = inputs
            .claude_exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "null".to_string());
        return Completion {
            status: CompletionStatus::Failed,
            reason: format!("Claude exited with code {code}; no usable change was produced."),
        };
    }

    Completion {
        status: CompletionStatus::Failed,
        reason: format!(
            "No usable change was produced; failed required check(s): {}.",
            failed_required.join(", ")
        ),
    }
}

/// Explains why the judge did not pass, in priority order.
fn judge_failure_reason(judge: &JudgeResult) -> String {
    if let Some(error) = &judge.error {
        return error.clone();
    }
    if judge.has_unrelated_changes {
        return "unrelated changes detected".to_string();
    }
    if let Some(score) = judge.score {
        return format!("score {}", format_score(score));
    }
    "judge did not return a passing result".to_string()
}

/// Formats a judge score without a trailing `.0` for whole numbers, matching the
/// original JavaScript number formatting (e.g. `2`, not `2.0`).
fn format_score(score: f64) -> String {
    if score.fract() == 0.0 {
        format!("{}", score as i64)
    } else {
        format!("{score}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CheckConfig, SuccessConfig, TaskConfig};
    use std::path::PathBuf;

    fn make_task(check_cmds: &[(&str, &str)], required: &[&str]) -> LoadedTask {
        let checks = check_cmds
            .iter()
            .map(|(name, command)| CheckConfig {
                name: name.to_string(),
                command: command.to_string(),
                timeout_seconds: None,
            })
            .collect();
        LoadedTask {
            config: TaskConfig {
                id: "t".into(),
                title: "t".into(),
                description: "d".into(),
                fixture_path: "fixture".into(),
                prompt_file: "prompt.md".into(),
                setup: vec![],
                checks,
                success: SuccessConfig {
                    required_checks: required.iter().map(|s| s.to_string()).collect(),
                },
                judge: None,
                expected_files: None,
            },
            task_dir: PathBuf::from("/t"),
            fixture_dir: PathBuf::from("/t/fixture"),
            prompt_path: PathBuf::from("/t/prompt.md"),
            prompt: "p".into(),
        }
    }

    fn check_result(name: &str, passed: bool) -> CheckResult {
        CheckResult {
            name: name.into(),
            command: "x".into(),
            exit_code: Some(if passed { 0 } else { 1 }),
            passed,
            duration_ms: 1,
            stdout_path: None,
            stderr_path: None,
            timed_out: Some(false),
        }
    }

    fn judge(enabled: bool, passed: bool, score: Option<f64>, unrelated: bool) -> JudgeResult {
        JudgeResult {
            enabled,
            model_id: "m".into(),
            score,
            passed,
            correctness: None,
            maintainability: None,
            scope_control: None,
            has_unrelated_changes: unrelated,
            findings: vec![],
            raw_output_path: None,
            error: None,
        }
    }

    fn inputs<'a>(
        task: &'a LoadedTask,
        checks: &'a [CheckResult],
        judge: &'a JudgeResult,
        claude_exit_code: Option<i32>,
        changed_files: &'a [String],
    ) -> CompletionInputs<'a> {
        CompletionInputs {
            task,
            checks,
            judge,
            claude_exit_code,
            claude_timed_out: false,
            changed_files,
            infrastructure_error: None,
        }
    }

    #[test]
    fn infrastructure_error_yields_error_status() {
        let task = make_task(&[], &["unit-tests"]);
        let j = judge(false, true, None, false);
        let mut i = inputs(&task, &[], &j, Some(0), &[]);
        i.infrastructure_error = Some("setup failed".into());
        let c = classify_completion(&i);
        assert_eq!(c.status, CompletionStatus::Error);
        assert_eq!(c.reason, "setup failed");
    }

    #[test]
    fn claude_timeout_yields_timeout_status() {
        let task = make_task(&[], &["unit-tests"]);
        let j = judge(true, true, None, false);
        let mut i = inputs(&task, &[], &j, None, &[]);
        i.claude_timed_out = true;
        assert_eq!(classify_completion(&i).status, CompletionStatus::Timeout);
    }

    #[test]
    fn required_pass_with_judge_accept_is_passed() {
        let task = make_task(&[], &["unit-tests"]);
        let checks = vec![check_result("unit-tests", true)];
        let j = judge(true, true, Some(5.0), false);
        let c = classify_completion(&inputs(&task, &checks, &j, Some(0), &[]));
        assert_eq!(c.status, CompletionStatus::Passed);
        assert!(c.reason.contains("judge accepted"), "reason: {}", c.reason);
    }

    #[test]
    fn required_pass_with_judge_skipped_is_passed() {
        let task = make_task(&[], &["unit-tests"]);
        let checks = vec![check_result("unit-tests", true)];
        let j = judge(false, true, None, false);
        let c = classify_completion(&inputs(&task, &checks, &j, Some(0), &[]));
        assert_eq!(c.status, CompletionStatus::Passed);
        assert!(c.reason.contains("skipped"), "reason: {}", c.reason);
    }

    #[test]
    fn required_pass_but_judge_fails_is_partial_with_score_reason() {
        let task = make_task(&[], &["unit-tests"]);
        let checks = vec![check_result("unit-tests", true)];
        let j = judge(true, false, Some(2.0), false);
        let c = classify_completion(&inputs(&task, &checks, &j, Some(0), &[]));
        assert_eq!(c.status, CompletionStatus::Partial);
        assert!(c.reason.contains("judge failed"), "reason: {}", c.reason);
        assert!(c.reason.contains("score 2"), "reason: {}", c.reason);
    }

    #[test]
    fn failed_required_with_progress_is_partial() {
        let task = make_task(&[], &["unit-tests"]);
        let checks = vec![check_result("unit-tests", false)];
        let j = judge(true, true, None, false);
        let changed = vec!["src/lib.rs".to_string()];
        let c = classify_completion(&inputs(&task, &checks, &j, Some(0), &changed));
        assert_eq!(c.status, CompletionStatus::Partial);
        assert!(c.reason.contains("unit-tests"), "reason: {}", c.reason);
    }

    #[test]
    fn failed_required_no_progress_nonzero_exit_is_failed_with_code() {
        let task = make_task(&[], &["unit-tests"]);
        let checks = vec![check_result("unit-tests", false)];
        let j = judge(true, true, None, false);
        let c = classify_completion(&inputs(&task, &checks, &j, Some(1), &[]));
        assert_eq!(c.status, CompletionStatus::Failed);
        assert!(
            c.reason.contains("exited with code 1"),
            "reason: {}",
            c.reason
        );
    }

    #[test]
    fn passed_baseline_checks_do_not_count_as_model_progress() {
        let task = make_task(&[], &["unit-tests", "typecheck"]);
        let checks = vec![
            check_result("unit-tests", false),
            check_result("typecheck", true),
        ];
        let j = judge(true, true, None, false);
        let c = classify_completion(&inputs(&task, &checks, &j, Some(1), &[]));
        assert_eq!(c.status, CompletionStatus::Failed);
        assert!(
            c.reason.contains("exited with code 1"),
            "reason: {}",
            c.reason
        );
    }

    #[test]
    fn failed_required_no_progress_zero_exit_is_failed() {
        let task = make_task(&[], &["unit-tests"]);
        let checks = vec![check_result("unit-tests", false)];
        let j = judge(true, true, None, false);
        let c = classify_completion(&inputs(&task, &checks, &j, Some(0), &[]));
        assert_eq!(c.status, CompletionStatus::Failed);
        assert!(
            c.reason.contains("No usable change"),
            "reason: {}",
            c.reason
        );
    }

    #[tokio::test]
    async fn run_checks_records_each_command_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let run_dir = dir.path().join("run");
        let task = make_task(&[("ok", "true"), ("bad", "false")], &["ok"]);

        let results = run_checks(&task, &workspace, &run_dir, Duration::from_secs(30), &[])
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "ok");
        assert!(results[0].passed);
        assert!(!results[1].passed);
        assert_eq!(results[1].exit_code, Some(1));
        assert!(run_dir.join("checks/ok.stdout.txt").exists());
        assert_eq!(
            results[0].stdout_path.as_deref(),
            Some("checks/ok.stdout.txt")
        );
    }
}
