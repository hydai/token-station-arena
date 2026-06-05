use std::path::Path;

use anyhow::{bail, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Map, Value};

use crate::fs_utils::{find_files, read_json, write_json};
use crate::types::{RunResult, TokenUsage};

/// One normalized usage row extracted from a Token Station dump.
#[derive(Debug, Clone)]
pub struct UsageRecord {
    pub model_id: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub input: i64,
    pub output: i64,
    pub cache_creation_input: i64,
    pub cache_read_input: i64,
    pub total: i64,
    pub estimated_cost_usd: Option<f64>,
}

/// Summary of an import pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub updated: usize,
    pub unmatched: usize,
    pub run_files: usize,
}

/// Extracts usage rows from a dump, tolerating many key spellings and shapes.
pub fn extract_usage_records(dump: &Value) -> Result<Vec<UsageRecord>> {
    let rows = normalize_rows(dump)?;
    Ok(rows
        .into_iter()
        .map(|raw| {
            let usage = raw.get("usage").and_then(Value::as_object);
            let input = first_i64(
                raw,
                usage,
                &[
                    "inputTokens",
                    "input_tokens",
                    "prompt_tokens",
                    "input",
                    "promptTokens",
                ],
            )
            .unwrap_or(0);
            let output = first_i64(
                raw,
                usage,
                &[
                    "outputTokens",
                    "output_tokens",
                    "completion_tokens",
                    "output",
                    "completionTokens",
                ],
            )
            .unwrap_or(0);
            let cache_creation_input = first_i64(
                raw,
                usage,
                &[
                    "cacheCreationInputTokens",
                    "cache_creation_input_tokens",
                    "cacheCreationInput",
                ],
            )
            .unwrap_or(0);
            let cache_read_input = first_i64(
                raw,
                usage,
                &[
                    "cacheReadInputTokens",
                    "cache_read_input_tokens",
                    "cacheReadInput",
                ],
            )
            .unwrap_or(0);
            let total = first_i64(raw, usage, &["totalTokens", "total_tokens", "total"])
                .unwrap_or(input + output + cache_creation_input + cache_read_input);
            let estimated_cost_usd = first_f64(
                raw,
                usage,
                &[
                    "estimatedCostUsd",
                    "estimated_cost_usd",
                    "costUsd",
                    "cost_usd",
                ],
            );
            let model_id = first_string(
                raw,
                &[
                    "providerModelId",
                    "provider_model_id",
                    "modelId",
                    "model_id",
                    "model",
                    "modelName",
                ],
            );
            let timestamp = first_date(
                raw,
                &[
                    "startedAt",
                    "started_at",
                    "createdAt",
                    "created_at",
                    "timestamp",
                    "time",
                    "requestStartedAt",
                ],
            );
            UsageRecord {
                model_id,
                timestamp,
                input,
                output,
                cache_creation_input,
                cache_read_input,
                total,
                estimated_cost_usd,
            }
        })
        .collect())
}

/// Returns the records whose model id and timestamp fall within the run's
/// execution window (padded by `padding_seconds`).
pub fn match_records<'a>(
    result: &RunResult,
    records: &'a [UsageRecord],
    padding_seconds: u64,
) -> Vec<&'a UsageRecord> {
    let (Some(start), Some(finish)) = (
        parse_timestamp_str(&result.started_at),
        parse_timestamp_str(&result.finished_at),
    ) else {
        return Vec::new();
    };
    let padding = chrono::Duration::seconds(padding_seconds as i64);
    let low = start - padding;
    let high = finish + padding;
    let candidates = [result.provider_model_id.as_str(), result.model_id.as_str()];

    records
        .iter()
        .filter(|record| {
            let (Some(timestamp), Some(model_id)) = (record.timestamp, record.model_id.as_deref())
            else {
                return false;
            };
            timestamp >= low && timestamp <= high && candidates.contains(&model_id)
        })
        .collect()
}

/// Sums a set of matched usage records into a [`TokenUsage`].
pub fn sum_usage(records: &[&UsageRecord], dump_file: &str) -> TokenUsage {
    let costs: Vec<f64> = records
        .iter()
        .filter_map(|r| r.estimated_cost_usd)
        .collect();
    TokenUsage {
        source: "token-station-backend-dump".to_string(),
        correlation: "execution-time-window".to_string(),
        dump_file: dump_file.to_string(),
        input: Some(records.iter().map(|r| r.input).sum()),
        output: Some(records.iter().map(|r| r.output).sum()),
        cache_creation_input: Some(records.iter().map(|r| r.cache_creation_input).sum()),
        cache_read_input: Some(records.iter().map(|r| r.cache_read_input).sum()),
        total: Some(records.iter().map(|r| r.total).sum()),
        estimated_cost_usd: if costs.is_empty() {
            None
        } else {
            Some(costs.iter().sum())
        },
    }
}

/// Imports a Token Station dump and merges matched usage into every run result.
pub fn import_token_dump(
    input_path: &Path,
    runs_dir: &Path,
    padding_seconds: u64,
) -> Result<ImportSummary> {
    let dump: Value = read_json(input_path)?;
    let records = extract_usage_records(&dump)?;
    let run_files = find_files(runs_dir, "result.json")?;
    let dump_file = input_path.to_string_lossy().to_string();

    let mut updated = 0;
    let mut unmatched = 0;
    for result_path in &run_files {
        let mut result: RunResult = read_json(result_path)?;
        let matched = match_records(&result, &records, padding_seconds);
        if matched.is_empty() {
            result.tokens = Some(null_token_usage(&dump_file));
            result.warnings.push(
                "No Token Station usage record matched this run by model and execution time window."
                    .to_string(),
            );
            unmatched += 1;
        } else {
            result.tokens = Some(sum_usage(&matched, &dump_file));
            updated += 1;
        }
        write_json(result_path, &result)?;
    }

    Ok(ImportSummary {
        updated,
        unmatched,
        run_files: run_files.len(),
    })
}

fn normalize_rows(dump: &Value) -> Result<Vec<&Map<String, Value>>> {
    if let Some(array) = dump.as_array() {
        return Ok(array.iter().filter_map(Value::as_object).collect());
    }
    if let Some(object) = dump.as_object() {
        for key in ["records", "usage", "rows", "data"] {
            if let Some(array) = object.get(key).and_then(Value::as_array) {
                return Ok(array.iter().filter_map(Value::as_object).collect());
            }
        }
    }
    bail!("Token Station dump must be an array or an object with records, usage, rows, or data.")
}

fn null_token_usage(dump_file: &str) -> TokenUsage {
    TokenUsage {
        source: "token-station-backend-dump".to_string(),
        correlation: "execution-time-window".to_string(),
        dump_file: dump_file.to_string(),
        input: None,
        output: None,
        cache_creation_input: None,
        cache_read_input: None,
        total: None,
        estimated_cost_usd: None,
    }
}

fn first_i64(
    obj: &Map<String, Value>,
    nested: Option<&Map<String, Value>>,
    keys: &[&str],
) -> Option<i64> {
    for key in keys {
        if let Some(value) = obj.get(*key).and_then(value_to_i64) {
            return Some(value);
        }
        if let Some(value) = nested.and_then(|n| n.get(*key)).and_then(value_to_i64) {
            return Some(value);
        }
    }
    None
}

fn first_f64(
    obj: &Map<String, Value>,
    nested: Option<&Map<String, Value>>,
    keys: &[&str],
) -> Option<f64> {
    for key in keys {
        if let Some(value) = obj.get(*key).and_then(value_to_f64) {
            return Some(value);
        }
        if let Some(value) = nested.and_then(|n| n.get(*key)).and_then(value_to_f64) {
            return Some(value);
        }
    }
    None
}

fn first_string(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = obj.get(*key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn first_date(obj: &Map<String, Value>, keys: &[&str]) -> Option<DateTime<Utc>> {
    for key in keys {
        if let Some(value) = obj.get(*key) {
            if let Some(timestamp) = parse_timestamp_value(value) {
                return Some(timestamp);
            }
        }
    }
    None
}

fn value_to_i64(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(f) = value.as_f64().filter(|f| f.is_finite()) {
        return Some(f as i64);
    }
    value
        .as_str()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|f| f.is_finite())
        .map(|f| f as i64)
}

fn value_to_f64(value: &Value) -> Option<f64> {
    if let Some(f) = value.as_f64().filter(|f| f.is_finite()) {
        return Some(f);
    }
    value
        .as_str()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|f| f.is_finite())
}

fn parse_timestamp_value(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(millis) = value.as_i64() {
        return Utc.timestamp_millis_opt(millis).single();
    }
    value.as_str().and_then(parse_timestamp_str)
}

fn parse_timestamp_str(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Artifacts, ClaudeRunMeta, Completion, CompletionStatus, HumanAudit, JudgeResult,
    };
    use serde_json::json;

    fn ts(value: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    fn record(model: &str, timestamp: &str, input: i64, output: i64, total: i64) -> UsageRecord {
        UsageRecord {
            model_id: Some(model.to_string()),
            timestamp: ts(timestamp),
            input,
            output,
            cache_creation_input: 0,
            cache_read_input: 0,
            total,
            estimated_cost_usd: Some(0.01),
        }
    }

    fn run_result(
        model_id: &str,
        provider_model_id: &str,
        started: &str,
        finished: &str,
    ) -> RunResult {
        RunResult {
            run_id: "r".into(),
            task_id: "t".into(),
            model_id: model_id.into(),
            provider_model_id: provider_model_id.into(),
            run_index: 1,
            provider: "models.bytefuture.ai".into(),
            started_at: started.into(),
            finished_at: finished.into(),
            duration_ms: 0,
            claude_exit_code: Some(0),
            claude: ClaudeRunMeta {
                session_id: None,
                output_format: "json".into(),
                command_strategy: vec![],
            },
            checks: vec![],
            completion: Completion {
                status: CompletionStatus::Passed,
                reason: "ok".into(),
            },
            tokens: None,
            judge: JudgeResult {
                enabled: false,
                model_id: "m".into(),
                score: None,
                passed: true,
                correctness: None,
                maintainability: None,
                scope_control: None,
                has_unrelated_changes: false,
                findings: vec![],
                raw_output_path: None,
                error: None,
            },
            artifacts: Artifacts {
                stdout: "stdout.txt".into(),
                stderr: "stderr.txt".into(),
                claude_output: "claude-output.json".into(),
                diff: "diff.patch".into(),
                workspace: "workspace/".into(),
                checks: "checks/".into(),
                model_config: "model-config.json".into(),
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
    fn extract_reads_common_records_shape() {
        let dump = json!({"records":[{
            "model": "deepseek/deepseek-v4-flash",
            "createdAt": "2026-06-04T10:00:00Z",
            "usage": {"input_tokens": 100, "output_tokens": 50, "total_tokens": 150, "cost_usd": 0.01}
        }]});
        let records = extract_usage_records(&dump).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].model_id.as_deref(),
            Some("deepseek/deepseek-v4-flash")
        );
        assert_eq!(records[0].input, 100);
        assert_eq!(records[0].output, 50);
        assert_eq!(records[0].total, 150);
        assert_eq!(records[0].estimated_cost_usd, Some(0.01));
        assert!(records[0].timestamp.is_some());
    }

    #[test]
    fn extract_accepts_array_camelcase_with_total_fallback() {
        let dump = json!([{
            "providerModelId": "kimi/kimi-k2.5",
            "timestamp": "2026-06-04T10:00:00Z",
            "inputTokens": 10, "outputTokens": 5
        }]);
        let records = extract_usage_records(&dump).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model_id.as_deref(), Some("kimi/kimi-k2.5"));
        assert_eq!(records[0].total, 15);
    }

    #[test]
    fn extract_errors_on_unsupported_shape() {
        assert!(extract_usage_records(&json!({"nope": 1})).is_err());
    }

    #[test]
    fn match_records_filters_by_window_and_model() {
        let records = vec![
            record(
                "deepseek/deepseek-v4-flash",
                "2026-06-04T10:01:00Z",
                100,
                50,
                150,
            ),
            record(
                "deepseek/deepseek-v4-flash",
                "2026-06-04T12:00:00Z",
                1,
                1,
                2,
            ),
            record("other/model", "2026-06-04T10:01:00Z", 1, 1, 2),
        ];
        let result = run_result(
            "deepseek-v4-flash",
            "deepseek/deepseek-v4-flash",
            "2026-06-04T10:00:00.000Z",
            "2026-06-04T10:02:00.000Z",
        );
        let matched = match_records(&result, &records, 60);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].total, 150);
    }

    #[test]
    fn sum_usage_totals_counts_and_costs() {
        let r1 = record("m", "2026-06-04T10:00:00Z", 10, 5, 18);
        let r2 = UsageRecord {
            estimated_cost_usd: Some(0.02),
            ..record("m", "2026-06-04T10:00:00Z", 20, 5, 25)
        };
        let usage = sum_usage(&[&r1, &r2], "dump.json");
        assert_eq!(usage.input, Some(30));
        assert_eq!(usage.total, Some(43));
        assert_eq!(usage.estimated_cost_usd, Some(0.03));
        assert_eq!(usage.source, "token-station-backend-dump");
    }
}
