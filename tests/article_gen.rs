//! End-to-end test for article generation: write run results to disk, render
//! the article, and verify the output file.

use std::fs;

use serde_json::json;
use token_station_arena::article::generate_article;

fn result_json(run_id: &str, model_id: &str, task_id: &str, status: &str) -> serde_json::Value {
    json!({
        "runId": run_id,
        "taskId": task_id,
        "modelId": model_id,
        "providerModelId": format!("prov/{model_id}"),
        "runIndex": 1,
        "provider": "anthropic-compatible-gateway",
        "startedAt": "2026-06-04T10:00:00.000Z",
        "finishedAt": "2026-06-04T10:03:00.000Z",
        "durationMs": 180000,
        "claudeExitCode": 0,
        "claude": { "sessionId": null, "outputFormat": "json", "commandStrategy": [] },
        "checks": [],
        "completion": { "status": status, "reason": "r" },
        "tokens": {
            "input": 12000, "output": 3000, "cacheCreationInput": 0,
            "cacheReadInput": 0, "total": 15000, "estimatedCostUsd": 0.08
        },
        "judge": {
            "enabled": true, "modelId": "m", "score": 4, "passed": true, "correctness": 4,
            "maintainability": 4, "scopeControl": 5, "hasUnrelatedChanges": false, "findings": []
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

#[test]
fn generate_article_writes_markdown_from_run_results() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    for (i, status) in ["passed", "failed"].iter().enumerate() {
        let run_dir = runs.join(format!("run-{i}"));
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("result.json"),
            result_json(
                &format!("r{i}"),
                "deepseek-v4-flash",
                "fix-failing-test",
                status,
            )
            .to_string(),
        )
        .unwrap();
    }

    let output = dir.path().join("article.md");
    let summary = generate_article(&runs, &output, "Bench Title").unwrap();
    assert_eq!(summary.run_count, 2);

    let markdown = fs::read_to_string(&output).unwrap();
    assert!(markdown.contains("# Bench Title"));
    assert!(markdown.contains("deepseek-v4-flash"));
    assert!(
        markdown.contains("1/2"),
        "expected a 1/2 pass rate in:\n{markdown}"
    );
    assert!(markdown.contains("cargo run --release"));
    assert!(!markdown.contains("npm run"));
}
