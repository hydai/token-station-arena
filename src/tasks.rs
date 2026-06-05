use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::types::{LoadedTask, TaskConfig};

/// Parses and validates a single `task.yml` document.
pub fn parse_task(yaml: &str) -> Result<TaskConfig> {
    let task: TaskConfig = serde_yaml_ng::from_str(yaml).context("parse task.yml")?;
    validate_task(&task)?;
    Ok(task)
}

/// Validates that required string fields are present and non-empty. (Missing
/// fields are already rejected by deserialization; this catches empty values.)
pub fn validate_task(task: &TaskConfig) -> Result<()> {
    let required = [
        ("id", &task.id),
        ("title", &task.title),
        ("description", &task.description),
        ("fixturePath", &task.fixture_path),
        ("promptFile", &task.prompt_file),
    ];
    for (field, value) in required {
        if value.is_empty() {
            bail!("task is missing required field: {field}");
        }
    }
    Ok(())
}

/// Selects tasks by id. `"all"` keeps every task; otherwise a comma-separated
/// list of ids. An unknown id is an error.
pub fn select_tasks(tasks: &[LoadedTask], selector: &str) -> Result<Vec<LoadedTask>> {
    if selector == "all" {
        return Ok(tasks.to_vec());
    }

    let requested: Vec<&str> = selector
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let selected: Vec<LoadedTask> = tasks
        .iter()
        .filter(|t| requested.contains(&t.config.id.as_str()))
        .cloned()
        .collect();

    let found: std::collections::HashSet<&str> =
        selected.iter().map(|t| t.config.id.as_str()).collect();
    let missing: Vec<&str> = requested
        .iter()
        .copied()
        .filter(|r| !found.contains(r))
        .collect();
    if !missing.is_empty() {
        bail!("Unknown task id(s): {}", missing.join(", "));
    }

    Ok(selected)
}

/// Loads every task directory under `tasks_dir`, sorted by directory name.
pub fn load_tasks(tasks_dir: impl AsRef<Path>) -> Result<Vec<LoadedTask>> {
    let tasks_dir = tasks_dir.as_ref();
    let mut task_dirs: Vec<PathBuf> = std::fs::read_dir(tasks_dir)
        .with_context(|| format!("read tasks dir {}", tasks_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|entry| entry.path())
        .collect();
    task_dirs.sort();

    let mut tasks = Vec::new();
    for task_dir in task_dirs {
        let task_path = task_dir.join("task.yml");
        let text = std::fs::read_to_string(&task_path)
            .with_context(|| format!("read {}", task_path.display()))?;
        let config = parse_task(&text)?;
        let fixture_dir = task_dir.join(&config.fixture_path);
        let prompt_path = task_dir.join(&config.prompt_file);
        let prompt = std::fs::read_to_string(&prompt_path)
            .with_context(|| format!("read {}", prompt_path.display()))?;
        tasks.push(LoadedTask {
            config,
            task_dir,
            fixture_dir,
            prompt_path,
            prompt,
        });
    }
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
id: fix-failing-test
title: Fix a failing Rust unit test
description: Repair the pricing implementation so the test suite passes.
fixturePath: fixture
promptFile: prompt.md
setup:
  - cargo fetch
checks:
  - name: unit-tests
    command: cargo test
  - name: clippy
    command: cargo clippy --all-targets -- -D warnings
success:
  requiredChecks:
    - unit-tests
    - clippy
judge:
  unrelatedChangePolicy: fail
  allowedChangePaths:
    - crates/**
    - Cargo.toml
"#;

    #[test]
    fn parses_task_with_checks_and_judge_policy() {
        let task = parse_task(SAMPLE).unwrap();
        assert_eq!(task.id, "fix-failing-test");
        assert_eq!(task.fixture_path, "fixture");
        assert_eq!(task.prompt_file, "prompt.md");
        assert_eq!(task.setup, ["cargo fetch"]);
        assert_eq!(task.checks.len(), 2);
        assert_eq!(task.checks[0].name, "unit-tests");
        assert_eq!(task.checks[0].command, "cargo test");
        assert_eq!(task.success.required_checks, ["unit-tests", "clippy"]);
        let judge = task.judge.expect("judge policy present");
        assert_eq!(judge.unrelated_change_policy.as_deref(), Some("fail"));
        assert_eq!(
            judge.allowed_change_paths.unwrap(),
            ["crates/**", "Cargo.toml"]
        );
    }

    #[test]
    fn setup_defaults_to_empty_and_judge_is_optional() {
        let yaml = r#"
id: t
title: T
description: d
fixturePath: fixture
promptFile: prompt.md
checks:
  - name: c
    command: cargo check
success:
  requiredChecks:
    - c
"#;
        let task = parse_task(yaml).unwrap();
        assert!(task.setup.is_empty());
        assert!(task.judge.is_none());
    }

    #[test]
    fn empty_required_string_field_is_rejected() {
        let yaml = r#"
id: ""
title: T
description: d
fixturePath: fixture
promptFile: prompt.md
checks:
  - name: c
    command: cargo check
success:
  requiredChecks:
    - c
"#;
        let err = parse_task(yaml).unwrap_err();
        assert!(err.to_string().contains("id"), "error was: {err}");
    }

    fn loaded(id: &str) -> LoadedTask {
        let yaml = SAMPLE.replacen("fix-failing-test", id, 1);
        LoadedTask {
            config: parse_task(&yaml).unwrap(),
            task_dir: PathBuf::from("/tasks").join(id),
            fixture_dir: PathBuf::from("/tasks").join(id).join("fixture"),
            prompt_path: PathBuf::from("/tasks").join(id).join("prompt.md"),
            prompt: "do the thing".to_string(),
        }
    }

    #[test]
    fn select_all_returns_every_task() {
        let tasks = vec![loaded("a"), loaded("b")];
        let selected = select_tasks(&tasks, "all").unwrap();
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn select_unknown_task_is_an_error() {
        let tasks = vec![loaded("a")];
        let err = select_tasks(&tasks, "missing").unwrap_err();
        assert!(err.to_string().contains("missing"));
    }
}
