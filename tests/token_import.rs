//! End-to-end test for Token Station dump import: write a real `result.json`
//! and a dump to disk, run the importer, and verify the merge.

use std::fs;

use serde_json::json;
use token_station_arena::fs_utils::read_json;
use token_station_arena::token_station::import_token_dump;
use token_station_arena::types::RunResult;

fn sample_result(provider_model_id: &str, started: &str, finished: &str) -> serde_json::Value {
    json!({
        "runId": "2026-06-04T10-00-00-000Z_fix_deepseek_001",
        "taskId": "fix-failing-test",
        "modelId": "deepseek-v4-flash",
        "providerModelId": provider_model_id,
        "runIndex": 1,
        "provider": "models.bytefuture.ai",
        "startedAt": started,
        "finishedAt": finished,
        "durationMs": 120000,
        "claudeExitCode": 0,
        "claude": { "sessionId": null, "outputFormat": "json", "commandStrategy": [] },
        "checks": [],
        "completion": { "status": "passed", "reason": "ok" },
        "tokens": null,
        "judge": {
            "enabled": false, "modelId": "m", "score": null, "passed": true,
            "correctness": null, "maintainability": null, "scopeControl": null,
            "hasUnrelatedChanges": false, "findings": []
        },
        "artifacts": {
            "stdout": "stdout.txt", "stderr": "stderr.txt", "claudeOutput": "claude-output.json",
            "diff": "diff.patch", "workspace": "workspace/", "checks": "checks/",
            "modelConfig": "model-config.json"
        },
        "changedFiles": [],
        "warnings": [],
        "humanAudit": { "requiredForMvp": false, "score": null, "notes": "" }
    })
}

fn write_run(runs_dir: &std::path::Path, result: &serde_json::Value) -> std::path::PathBuf {
    let run_dir = runs_dir.join("run-001");
    fs::create_dir_all(&run_dir).unwrap();
    let path = run_dir.join("result.json");
    fs::write(&path, result.to_string()).unwrap();
    path
}

#[test]
fn import_merges_matched_usage_into_run_result() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    let result_path = write_run(
        &runs,
        &sample_result(
            "deepseek/deepseek-v4-flash",
            "2026-06-04T10:00:00.000Z",
            "2026-06-04T10:02:00.000Z",
        ),
    );

    let dump_path = dir.path().join("dump.json");
    let dump = json!({"records": [{
        "model": "deepseek/deepseek-v4-flash",
        "createdAt": "2026-06-04T10:01:00Z",
        "usage": {"input_tokens": 12000, "output_tokens": 3200, "total_tokens": 15200, "cost_usd": 0.08}
    }]});
    fs::write(&dump_path, dump.to_string()).unwrap();

    let summary = import_token_dump(&dump_path, &runs, 60).unwrap();
    assert_eq!(summary.updated, 1);
    assert_eq!(summary.unmatched, 0);
    assert_eq!(summary.run_files, 1);

    let updated: RunResult = read_json(&result_path).unwrap();
    let tokens = updated.tokens.expect("tokens populated");
    assert_eq!(tokens.input, Some(12000));
    assert_eq!(tokens.total, Some(15200));
    assert_eq!(tokens.estimated_cost_usd, Some(0.08));
}

#[test]
fn import_warns_and_nulls_when_no_record_matches() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    let result_path = write_run(
        &runs,
        &sample_result(
            "deepseek/deepseek-v4-flash",
            "2026-06-04T10:00:00.000Z",
            "2026-06-04T10:02:00.000Z",
        ),
    );

    let dump_path = dir.path().join("dump.json");
    // A day later — outside any padded window.
    let dump = json!({"records": [{
        "model": "deepseek/deepseek-v4-flash",
        "createdAt": "2026-06-05T10:01:00Z",
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }]});
    fs::write(&dump_path, dump.to_string()).unwrap();

    let summary = import_token_dump(&dump_path, &runs, 60).unwrap();
    assert_eq!(summary.unmatched, 1);

    let updated: RunResult = read_json(&result_path).unwrap();
    assert!(updated
        .tokens
        .expect("tokens set to null usage")
        .input
        .is_none());
    assert!(updated
        .warnings
        .iter()
        .any(|w| w.contains("No Token Station usage record matched")));
}
