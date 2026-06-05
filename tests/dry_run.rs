//! Exercises the benchmark runner's planning path end to end against the real
//! repository config. Dry-run returns before any `claude`/`cargo` invocation or
//! API-key check, so it validates loading, selection, and plan output only.

use token_station_arena::runner::{run_benchmark, BenchmarkArgs};

#[tokio::test]
async fn dry_run_loads_real_config_and_plans_without_calling_claude() {
    let args = BenchmarkArgs {
        tasks: Some("all".to_string()),
        models: Some("deepseek-v4-flash,kimi-k2-5".to_string()),
        runs: Some(1),
        dry_run: true,
        ..BenchmarkArgs::default()
    };

    run_benchmark(&args)
        .await
        .expect("dry run over the real config should succeed");
}

#[tokio::test]
async fn dry_run_rejects_unknown_model() {
    let args = BenchmarkArgs {
        models: Some("does-not-exist".to_string()),
        dry_run: true,
        ..BenchmarkArgs::default()
    };

    let error = run_benchmark(&args).await.unwrap_err();
    assert!(error.to_string().contains("does-not-exist"));
}
