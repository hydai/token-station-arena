use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A model under test, as defined in `benchmark/config/models.yml`.
///
/// Field names are camelCase on the wire (YAML and JSON) to stay byte-compatible
/// with the original configuration and run artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub model: String,
    pub claude_model_strategy: String,
    pub enabled: bool,
}

/// Global run behavior from `benchmark/config/benchmark.yml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkConfig {
    pub runs_per_task_model: u32,
    pub timeout_seconds: u64,
    pub output_dir: String,
    pub report_dir: String,
    pub claude: ClaudeConfig,
    pub token_station: TokenStationConfig,
    pub judge: JudgeConfig,
    pub article: ArticleConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeConfig {
    pub base_url: String,
    pub output_format: String,
    pub project_settings_file: String,
    pub disable_experimental_betas: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenStationConfig {
    pub enabled: bool,
    pub mode: String,
    pub correlation: String,
    pub dump_path: String,
    #[serde(default)]
    pub match_window_padding_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeConfig {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub minimum_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleConfig {
    pub title: String,
    pub output_file: String,
}

/// A single deterministic check command from `task.yml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckConfig {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuccessConfig {
    pub required_checks: Vec<String>,
}

/// Judge policy attached to a task: rubric, unrelated-change handling, and the
/// set of paths the model is allowed to touch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgePolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrelated_change_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_change_paths: Option<Vec<String>>,
}

/// A benchmark task definition from `benchmark/tasks/<id>/task.yml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskConfig {
    pub id: String,
    pub title: String,
    pub description: String,
    pub fixture_path: String,
    pub prompt_file: String,
    #[serde(default)]
    pub setup: Vec<String>,
    pub checks: Vec<CheckConfig>,
    pub success: SuccessConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_files: Option<Vec<String>>,
}

/// The result of running a subprocess: captured output, timing, exit status,
/// and whether it was killed by the timeout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub timed_out: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A task loaded from disk together with its resolved paths and prompt text.
#[derive(Debug, Clone)]
pub struct LoadedTask {
    pub config: TaskConfig,
    pub task_dir: PathBuf,
    pub fixture_dir: PathBuf,
    pub prompt_path: PathBuf,
    pub prompt: String,
}

/// The outcome of one deterministic check command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub name: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub passed: bool,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
}

/// Overall completion classification for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionStatus {
    Passed,
    Partial,
    Failed,
    Timeout,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    pub status: CompletionStatus,
    pub reason: String,
}

/// A single observation from the LLM judge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeFinding {
    pub severity: String,
    pub category: String,
    pub message: String,
}

/// The structured verdict from the LLM judge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeResult {
    pub enabled: bool,
    pub model_id: String,
    pub score: Option<f64>,
    pub passed: bool,
    pub correctness: Option<f64>,
    pub maintainability: Option<f64>,
    pub scope_control: Option<f64>,
    pub has_unrelated_changes: bool,
    pub findings: Vec<JudgeFinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_output_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Token usage for a run, imported from a Token Station backend dump.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub source: String,
    pub correlation: String,
    pub dump_file: String,
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_creation_input: Option<i64>,
    pub cache_read_input: Option<i64>,
    pub total: Option<i64>,
    pub estimated_cost_usd: Option<f64>,
}
