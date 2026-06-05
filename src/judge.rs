use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::command::{format_command, run_process, RunOptions};
use crate::fs_utils::{write_json, write_text};
use crate::types::{
    BenchmarkConfig, CheckResult, JudgeFinding, JudgeResult, LoadedTask, ModelConfig,
};

/// A disabled (skipped) judge verdict, which counts as a pass.
pub fn skipped_judge(model_id: &str) -> JudgeResult {
    JudgeResult {
        enabled: false,
        model_id: model_id.to_string(),
        score: None,
        passed: true,
        correctness: None,
        maintainability: None,
        scope_control: None,
        has_unrelated_changes: false,
        findings: vec![],
        raw_output_path: None,
        error: None,
    }
}

/// Normalizes a parsed judge payload into a [`JudgeResult`], applying the
/// pass/fail policy (score threshold, unrelated changes, critical findings).
pub fn normalize_judge(raw: &Value, model_id: &str, minimum_score: f64) -> Result<JudgeResult> {
    let value = unwrap_judge_payload(raw)?;

    let score = number_or_null(value.get("score"));
    let correctness = number_or_null(value.get("correctness"));
    let maintainability = number_or_null(value.get("maintainability"));
    let scope_control = number_or_null(value.get("scopeControl"));
    let has_unrelated_changes = value
        .get("hasUnrelatedChanges")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let raw_findings = value
        .get("findings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let has_critical_finding = raw_findings.iter().any(|finding| {
        let severity = finding
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let category = finding
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        severity == "critical" || (severity == "major" && category == "correctness")
    });

    let passed_flag = value
        .get("passed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let passed = passed_flag
        && !has_unrelated_changes
        && !has_critical_finding
        && score.map(|s| s >= minimum_score).unwrap_or(true);

    let findings = raw_findings
        .iter()
        .map(|finding| JudgeFinding {
            severity: finding
                .get("severity")
                .and_then(Value::as_str)
                .unwrap_or("minor")
                .to_string(),
            category: finding
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("general")
                .to_string(),
            message: finding
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
        .collect();

    Ok(JudgeResult {
        enabled: true,
        model_id: model_id.to_string(),
        score,
        passed,
        correctness,
        maintainability,
        scope_control,
        has_unrelated_changes,
        findings,
        raw_output_path: None,
        error: None,
    })
}

/// Extracts the judge JSON object from Claude's (possibly wrapped or fenced)
/// stdout.
pub fn parse_judge_json(stdout: &str) -> Result<Value> {
    let parsed = parse_possible_json(stdout)?;
    let unwrapped = unwrap_claude_json(&parsed);
    if let Some(text) = unwrapped.as_str() {
        return parse_possible_json(text);
    }
    Ok(unwrapped)
}

/// Builds the judge prompt sent to Claude.
#[allow(clippy::too_many_arguments)]
pub fn build_judge_prompt(
    task: &LoadedTask,
    model: &ModelConfig,
    changed_files: &[String],
    checks: &[CheckResult],
    diff: &str,
    claude_stdout: &str,
    claude_stderr: &str,
    minimum_score: f64,
) -> String {
    let payload = json!({
        "task": task.config,
        "testedModel": { "id": model.id, "providerModelId": model.model },
        "changedFiles": changed_files,
        "checks": checks
            .iter()
            .map(|check| json!({
                "name": check.name,
                "command": check.command,
                "exitCode": check.exit_code,
                "passed": check.passed,
                "durationMs": check.duration_ms,
                "timedOut": check.timed_out.unwrap_or(false),
            }))
            .collect::<Vec<_>>(),
    });
    let payload_json = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());

    [
        "You are judging a Claude Code benchmark run. Return only valid JSON matching this schema:".to_string(),
        r#"{"score":4,"passed":true,"correctness":4,"maintainability":4,"scopeControl":5,"hasUnrelatedChanges":false,"findings":[{"severity":"minor","category":"maintainability","message":"..."}]}"#.to_string(),
        String::new(),
        "Judge criteria:".to_string(),
        "- Correctness relative to the task prompt and deterministic check results.".to_string(),
        "- Simplicity, maintainability, and whether the diff overfits tests.".to_string(),
        "- Scope control. Mark hasUnrelatedChanges true for unjustified changes outside allowed paths.".to_string(),
        "- Fail if score is below the configured minimum, if severe correctness issues exist, or unrelated changes exist.".to_string(),
        String::new(),
        format!("Minimum passing score: {minimum_score}"),
        String::new(),
        "Task prompt:".to_string(),
        fence(&task.prompt, "markdown"),
        String::new(),
        "Task metadata, changed files, and deterministic checks:".to_string(),
        fence(&payload_json, "json"),
        String::new(),
        "Git diff:".to_string(),
        fence(&truncate(diff, 80_000), "diff"),
        String::new(),
        "Claude stdout excerpt:".to_string(),
        fence(&truncate(claude_stdout, 12_000), "text"),
        String::new(),
        "Claude stderr excerpt:".to_string(),
        fence(&truncate(claude_stderr, 12_000), "text"),
    ]
    .join("\n")
}

/// Inputs needed to run the LLM judge for a single benchmark run.
pub struct JudgeRunInput<'a> {
    pub benchmark: &'a BenchmarkConfig,
    pub task: &'a LoadedTask,
    pub model: &'a ModelConfig,
    pub workspace_dir: &'a Path,
    pub run_dir: &'a Path,
    pub diff: &'a str,
    pub changed_files: &'a [String],
    pub checks: &'a [CheckResult],
    pub claude_stdout: &'a str,
    pub claude_stderr: &'a str,
    pub timeout: Duration,
    pub secrets: &'a [String],
}

/// Runs the LLM judge by invoking `claude` with the judge model and parsing the
/// structured verdict. Persists `judge-prompt.md`, `judge-output.txt`, and
/// `judge.json` under the run directory.
pub async fn run_judge(input: &JudgeRunInput<'_>) -> JudgeResult {
    let model_id =
        env_nonempty("JUDGE_MODEL_ID").unwrap_or_else(|| input.benchmark.judge.model.clone());
    let prompt = build_judge_prompt(
        input.task,
        input.model,
        input.changed_files,
        input.checks,
        input.diff,
        input.claude_stdout,
        input.claude_stderr,
        input.benchmark.judge.minimum_score,
    );

    if let Err(error) = write_text(input.run_dir.join("judge-prompt.md"), &prompt) {
        return failed_judge(&model_id, format!("Failed to write judge prompt: {error}"));
    }

    let base_url = env_nonempty("BYTEFUTURE_BASE_URL")
        .unwrap_or_else(|| input.benchmark.claude.base_url.clone());
    let mut env = vec![
        ("ANTHROPIC_BASE_URL".to_string(), base_url),
        (
            "ANTHROPIC_API_KEY".to_string(),
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        ),
        (
            "ANTHROPIC_CUSTOM_MODEL_OPTION".to_string(),
            model_id.clone(),
        ),
        ("ANTHROPIC_MODEL".to_string(), model_id.clone()),
    ];
    if input.benchmark.claude.disable_experimental_betas {
        env.push((
            "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS".to_string(),
            "1".to_string(),
        ));
    }

    let args = vec![
        "--bare".to_string(),
        "-p".to_string(),
        prompt,
        "--settings".to_string(),
        input.benchmark.claude.project_settings_file.clone(),
        "--model".to_string(),
        model_id.clone(),
        "--output-format".to_string(),
        "json".to_string(),
    ];
    let options = RunOptions {
        cwd: input.workspace_dir.to_path_buf(),
        env,
        timeout: Some(input.timeout.min(Duration::from_secs(600))),
        secrets: input.secrets.to_vec(),
    };

    let outcome = run_process("claude", &args, &options).await;
    let _ = write_text(
        input.run_dir.join("judge-output.txt"),
        &format!(
            "{}\n\nSTDOUT\n{}\n\nSTDERR\n{}",
            format_command("claude", &args),
            outcome.stdout,
            outcome.stderr
        ),
    );

    let result = if outcome.exit_code != Some(0) || outcome.timed_out {
        let message = if outcome.timed_out {
            "Judge timed out.".to_string()
        } else {
            let code = outcome
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "null".to_string());
            format!("Judge command exited with code {code}.")
        };
        failed_judge(&model_id, message)
    } else {
        match parse_judge_json(&outcome.stdout).and_then(|value| {
            normalize_judge(&value, &model_id, input.benchmark.judge.minimum_score)
        }) {
            Ok(mut normalized) => {
                normalized.raw_output_path = Some("judge-output.txt".to_string());
                normalized
            }
            Err(error) => failed_judge(&model_id, error.to_string()),
        }
    };

    let _ = write_json(input.run_dir.join("judge.json"), &result);
    result
}

fn failed_judge(model_id: &str, message: String) -> JudgeResult {
    JudgeResult {
        enabled: true,
        model_id: model_id.to_string(),
        score: None,
        passed: false,
        correctness: None,
        maintainability: None,
        scope_control: None,
        has_unrelated_changes: false,
        findings: vec![JudgeFinding {
            severity: "major".to_string(),
            category: "judge".to_string(),
            message: message.clone(),
        }],
        raw_output_path: Some("judge-output.txt".to_string()),
        error: Some(message),
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn number_or_null(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn unwrap_judge_payload(value: &Value) -> Result<Value> {
    if !value.is_object() {
        bail!("Judge payload is not an object.");
    }
    if let Some(result) = value.get("result").and_then(Value::as_str) {
        return parse_possible_json(result);
    }
    Ok(value.clone())
}

fn unwrap_claude_json(value: &Value) -> Value {
    if let Some(object) = value.as_object() {
        for key in ["result", "content", "message"] {
            if let Some(found) = object.get(key) {
                if !found.is_null() {
                    return found.clone();
                }
            }
        }
    }
    value.clone()
}

fn parse_possible_json(text: &str) -> Result<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return Ok(value);
    }
    if let Some(fenced) = extract_fenced(text) {
        if let Ok(value) = serde_json::from_str::<Value>(&fenced) {
            return Ok(value);
        }
    }
    if let (Some(first), Some(last)) = (text.find('{'), text.rfind('}')) {
        if last > first {
            if let Ok(value) = serde_json::from_str::<Value>(&text[first..=last]) {
                return Ok(value);
            }
        }
    }
    bail!("Judge output did not contain parseable JSON.")
}

fn extract_fenced(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
    let body = &after[body_start..];
    let end = body.find("```")?;
    Some(body[..end].trim().to_string())
}

fn fence(value: &str, language: &str) -> String {
    let escaped = value.replace("```", "`\\`\\`");
    format!("```{language}\n{escaped}\n```")
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars).collect();
    let remaining = value.chars().count() - max_chars;
    format!("{truncated}\n[truncated {remaining} characters]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SuccessConfig, TaskConfig};
    use serde_json::json;
    use std::path::PathBuf;

    fn make_task(prompt: &str) -> LoadedTask {
        LoadedTask {
            config: TaskConfig {
                id: "fix".into(),
                title: "Fix".into(),
                description: "d".into(),
                fixture_path: "fixture".into(),
                prompt_file: "prompt.md".into(),
                setup: vec![],
                checks: vec![],
                success: SuccessConfig {
                    required_checks: vec!["unit-tests".into()],
                },
                judge: None,
                expected_files: None,
            },
            task_dir: PathBuf::from("/t"),
            fixture_dir: PathBuf::from("/t/fixture"),
            prompt_path: PathBuf::from("/t/prompt.md"),
            prompt: prompt.to_string(),
        }
    }

    fn make_model() -> ModelConfig {
        ModelConfig {
            id: "deepseek-v4-flash".into(),
            display_name: "DeepSeek".into(),
            provider: "models.bytefuture.ai".into(),
            model: "deepseek/deepseek-v4-flash".into(),
            claude_model_strategy: "custom-model-option".into(),
            enabled: true,
        }
    }

    #[test]
    fn skipped_judge_is_disabled_but_passes() {
        let judge = skipped_judge("m");
        assert!(!judge.enabled);
        assert!(judge.passed);
        assert!(judge.score.is_none());
    }

    #[test]
    fn normalize_accepts_a_clean_passing_payload() {
        let raw = json!({
            "passed": true, "score": 4, "correctness": 4, "maintainability": 4,
            "scopeControl": 5, "hasUnrelatedChanges": false, "findings": []
        });
        let judge = normalize_judge(&raw, "m", 4.0).unwrap();
        assert!(judge.passed);
        assert_eq!(judge.score, Some(4.0));
        assert_eq!(judge.scope_control, Some(5.0));
    }

    #[test]
    fn normalize_fails_when_score_below_minimum() {
        let raw = json!({"passed": true, "score": 3, "hasUnrelatedChanges": false, "findings": []});
        assert!(!normalize_judge(&raw, "m", 4.0).unwrap().passed);
    }

    #[test]
    fn normalize_fails_on_unrelated_changes() {
        let raw = json!({"passed": true, "score": 5, "hasUnrelatedChanges": true, "findings": []});
        assert!(!normalize_judge(&raw, "m", 4.0).unwrap().passed);
    }

    #[test]
    fn normalize_fails_on_critical_finding() {
        let raw = json!({
            "passed": true, "score": 5, "hasUnrelatedChanges": false,
            "findings": [{"severity": "critical", "category": "correctness", "message": "x"}]
        });
        assert!(!normalize_judge(&raw, "m", 4.0).unwrap().passed);
    }

    #[test]
    fn normalize_fails_on_major_correctness_finding() {
        let raw = json!({
            "passed": true, "score": 5, "hasUnrelatedChanges": false,
            "findings": [{"severity": "major", "category": "correctness", "message": "x"}]
        });
        assert!(!normalize_judge(&raw, "m", 4.0).unwrap().passed);
    }

    #[test]
    fn normalize_passes_with_minor_finding_and_keeps_it() {
        let raw = json!({
            "passed": true, "score": 4, "hasUnrelatedChanges": false,
            "findings": [{"severity": "minor", "category": "style", "message": "tidy"}]
        });
        let judge = normalize_judge(&raw, "m", 4.0).unwrap();
        assert!(judge.passed);
        assert_eq!(judge.findings.len(), 1);
        assert_eq!(judge.findings[0].severity, "minor");
    }

    #[test]
    fn normalize_unwraps_a_result_string_payload() {
        let raw = json!({
            "result": "{\"passed\":true,\"score\":4,\"hasUnrelatedChanges\":false,\"findings\":[]}"
        });
        assert!(normalize_judge(&raw, "m", 4.0).unwrap().passed);
    }

    #[test]
    fn parse_judge_json_reads_a_plain_object() {
        let value = parse_judge_json(r#"{"passed":true,"score":4}"#).unwrap();
        assert_eq!(value["score"], 4);
    }

    #[test]
    fn parse_judge_json_unwraps_claude_result_with_a_fenced_block() {
        let inner = "```json\n{\"passed\":true,\"score\":5}\n```";
        let wrapper = json!({"result": format!("Verdict:\n{inner}\n"), "session_id": "abc"});
        let stdout = serde_json::to_string(&wrapper).unwrap();
        let value = parse_judge_json(&stdout).unwrap();
        assert_eq!(value["score"], 5);
        assert_eq!(value["passed"], true);
    }

    #[test]
    fn parse_judge_json_extracts_object_from_surrounding_prose() {
        let value = parse_judge_json("blah {\"passed\":false,\"score\":2} trailing").unwrap();
        assert_eq!(value["score"], 2);
    }

    #[test]
    fn build_judge_prompt_includes_task_prompt_and_minimum_score() {
        let prompt = build_judge_prompt(
            &make_task("Fix the failing pricing test."),
            &make_model(),
            &["crates/catalog-core/src/pricing.rs".into()],
            &[],
            "diff-marker-12345",
            "out",
            "err",
            4.0,
        );
        assert!(prompt.contains("Fix the failing pricing test."));
        assert!(prompt.contains("Minimum passing score: 4"));
        assert!(prompt.contains("diff-marker-12345"));
    }
}
