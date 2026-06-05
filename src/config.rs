use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::types::{BenchmarkConfig, ModelConfig};

#[derive(Deserialize)]
struct ModelsFile {
    models: Vec<ModelConfig>,
}

#[derive(Deserialize)]
struct BenchmarkFile {
    benchmark: BenchmarkConfig,
}

/// Resolved locations of the benchmark data directories, relative to a root.
pub struct ProjectPaths {
    pub root_dir: PathBuf,
    pub benchmark_dir: PathBuf,
    pub config_dir: PathBuf,
    pub tasks_dir: PathBuf,
}

pub fn project_paths(root_dir: impl AsRef<Path>) -> ProjectPaths {
    let root = root_dir.as_ref().to_path_buf();
    let benchmark = root.join("benchmark");
    ProjectPaths {
        config_dir: benchmark.join("config"),
        tasks_dir: benchmark.join("tasks"),
        benchmark_dir: benchmark,
        root_dir: root,
    }
}

pub fn parse_models(yaml: &str) -> Result<Vec<ModelConfig>> {
    let file: ModelsFile = serde_yaml_ng::from_str(yaml).context("parse models.yml")?;
    Ok(file.models)
}

pub fn parse_benchmark_config(yaml: &str) -> Result<BenchmarkConfig> {
    let file: BenchmarkFile = serde_yaml_ng::from_str(yaml).context("parse benchmark.yml")?;
    Ok(file.benchmark)
}

pub fn resolve_project_path(root_dir: impl AsRef<Path>, configured: &str) -> PathBuf {
    let configured = Path::new(configured);
    if configured.is_absolute() {
        configured.to_path_buf()
    } else {
        root_dir.as_ref().join(configured)
    }
}

pub fn load_models(paths: &ProjectPaths) -> Result<Vec<ModelConfig>> {
    let path = paths.config_dir.join("models.yml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    parse_models(&text)
}

pub fn load_benchmark_config(paths: &ProjectPaths) -> Result<BenchmarkConfig> {
    let path = paths.config_dir.join("benchmark.yml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    parse_benchmark_config(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_models_file_with_camelcase_fields() {
        let yaml = r#"
models:
  - id: gpt-oss-20b
    displayName: GPT OSS 20B
    provider: models.bytefuture.ai
    model: groq/gpt-oss-20b
    claudeModelStrategy: custom-model-option
    enabled: true
  - id: kimi-k2-5
    displayName: Kimi K2.5
    provider: models.bytefuture.ai
    model: kimi/kimi-k2.5
    claudeModelStrategy: custom-model-option
    enabled: false
"#;
        let models = parse_models(yaml).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-oss-20b");
        assert_eq!(models[0].display_name, "GPT OSS 20B");
        assert_eq!(models[0].model, "groq/gpt-oss-20b");
        assert_eq!(models[0].claude_model_strategy, "custom-model-option");
        assert!(models[0].enabled);
        assert!(!models[1].enabled);
    }

    #[test]
    fn parses_benchmark_config_with_nested_sections() {
        let yaml = r#"
benchmark:
  runsPerTaskModel: 3
  timeoutSeconds: 1800
  outputDir: benchmark/runs
  reportDir: benchmark/reports
  claude:
    baseUrl: https://bec.bytefuture.ai/v1
    outputFormat: json
    projectSettingsFile: .claude/settings.json
    disableExperimentalBetas: true
  tokenStation:
    enabled: true
    mode: backend-dump
    correlation: execution-time-window
    dumpPath: benchmark/reports/token-station-usage.json
    matchWindowPaddingSeconds: 60
  judge:
    enabled: true
    provider: models.bytefuture.ai
    model: anthropic/claude-opus-4-6
    minimumScore: 4
  article:
    title: "Comparing Claude Code Tasks Across Models on ByteFuture"
    outputFile: benchmark/reports/article.md
"#;
        let cfg = parse_benchmark_config(yaml).unwrap();
        assert_eq!(cfg.runs_per_task_model, 3);
        assert_eq!(cfg.timeout_seconds, 1800);
        assert_eq!(cfg.claude.base_url, "https://bec.bytefuture.ai/v1");
        assert!(cfg.claude.disable_experimental_betas);
        assert_eq!(cfg.token_station.match_window_padding_seconds, Some(60));
        assert_eq!(cfg.judge.minimum_score, 4.0);
        assert_eq!(cfg.article.output_file, "benchmark/reports/article.md");
    }

    #[test]
    fn match_window_padding_seconds_defaults_to_none_when_absent() {
        let yaml = r#"
benchmark:
  runsPerTaskModel: 1
  timeoutSeconds: 60
  outputDir: runs
  reportDir: reports
  claude:
    baseUrl: x
    outputFormat: json
    projectSettingsFile: .claude/settings.json
    disableExperimentalBetas: false
  tokenStation:
    enabled: false
    mode: backend-dump
    correlation: execution-time-window
    dumpPath: d.json
  judge:
    enabled: false
    provider: p
    model: m
    minimumScore: 0
  article:
    title: t
    outputFile: a.md
"#;
        let cfg = parse_benchmark_config(yaml).unwrap();
        assert_eq!(cfg.token_station.match_window_padding_seconds, None);
    }

    #[test]
    fn project_paths_are_rooted_under_benchmark() {
        let p = project_paths("/root");
        assert_eq!(p.config_dir, PathBuf::from("/root/benchmark/config"));
        assert_eq!(p.tasks_dir, PathBuf::from("/root/benchmark/tasks"));
        assert_eq!(p.benchmark_dir, PathBuf::from("/root/benchmark"));
    }

    #[test]
    fn resolve_project_path_joins_relative_but_keeps_absolute() {
        assert_eq!(
            resolve_project_path("/root", "benchmark/runs"),
            PathBuf::from("/root/benchmark/runs")
        );
        assert_eq!(
            resolve_project_path("/root", "/abs/dump.json"),
            PathBuf::from("/abs/dump.json")
        );
    }
}
