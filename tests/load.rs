//! Integration tests that load the real benchmark configuration and tasks from
//! the repository, exercising the loaders end to end against on-disk data.

use token_station_arena::config::{load_benchmark_config, load_models, project_paths};
use token_station_arena::tasks::load_tasks;

#[test]
fn loads_the_seven_enabled_models() {
    let paths = project_paths(".");
    let models = load_models(&paths).expect("load models.yml");
    let enabled: Vec<&str> = models
        .iter()
        .filter(|m| m.enabled)
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(
        enabled.len(),
        7,
        "expected 7 enabled models, got {enabled:?}"
    );
    assert!(models.iter().any(|m| m.model == "groq/gpt-oss-20b"));
    assert!(models
        .iter()
        .any(|m| m.model == "deepinfra/nemotron-3-nano"));
}

#[test]
fn loads_the_real_benchmark_config() {
    let paths = project_paths(".");
    let cfg = load_benchmark_config(&paths).expect("load benchmark.yml");
    assert_eq!(cfg.runs_per_task_model, 3);
    assert_eq!(cfg.timeout_seconds, 1800);
    assert_eq!(cfg.judge.model, "anthropic/claude-opus-4-6");
    assert_eq!(cfg.token_station.match_window_padding_seconds, Some(60));
}

#[test]
fn loads_all_three_tasks_with_required_checks_and_prompts() {
    let paths = project_paths(".");
    let tasks = load_tasks(&paths.tasks_dir).expect("load tasks");
    let ids: Vec<&str> = tasks.iter().map(|t| t.config.id.as_str()).collect();
    assert!(ids.contains(&"fix-failing-test"), "ids were {ids:?}");
    assert!(ids.contains(&"add-api-endpoint"), "ids were {ids:?}");
    assert!(ids.contains(&"refactor-pricing"), "ids were {ids:?}");

    for task in &tasks {
        assert!(
            !task.config.success.required_checks.is_empty(),
            "task {} has no required checks",
            task.config.id
        );
        assert!(
            !task.prompt.trim().is_empty(),
            "task {} has an empty prompt",
            task.config.id
        );
    }
}
