use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use crate::types::CommandResult;
use crate::util::iso8601;

/// Maximum number of bytes of stdout/stderr retained per stream. Output beyond
/// this is drained (to avoid blocking the child) but not stored.
const OUTPUT_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// How long a process gets to handle `SIGTERM` before it is `SIGKILL`ed.
const GRACE_AFTER_SIGTERM: Duration = Duration::from_secs(5);

/// Options shared by [`run_process`] and [`run_shell_command`].
#[derive(Clone, Default)]
pub struct RunOptions {
    pub cwd: PathBuf,
    /// Environment variables to set on top of the inherited process environment.
    pub env: Vec<(String, String)>,
    pub timeout: Option<Duration>,
    /// Substrings to mask out of captured stdout/stderr.
    pub secrets: Vec<String>,
}

/// Runs `command` with `args` directly (no shell).
pub async fn run_process(command: &str, args: &[String], options: &RunOptions) -> CommandResult {
    run_inner(command, args, options, false).await
}

/// Runs `command` through `sh -c` so shell syntax (pipes, `&&`, quoting) works.
pub async fn run_shell_command(command: &str, options: &RunOptions) -> CommandResult {
    run_inner(command, &[], options, true).await
}

/// Replaces every occurrence of each secret in `text` with `[REDACTED]`.
pub fn redact_text(text: &str, secrets: &[String]) -> String {
    let mut redacted = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            redacted = redacted.replace(secret.as_str(), "[REDACTED]");
        }
    }
    redacted
}

/// Masks the values of environment variables whose name looks secret-bearing
/// (contains key/token/secret/password, case-insensitive).
pub fn redact_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    const NEEDLES: [&str; 4] = ["key", "token", "secret", "password"];
    env.iter()
        .map(|(name, value)| {
            let lower = name.to_lowercase();
            let masked = !value.is_empty() && NEEDLES.iter().any(|needle| lower.contains(needle));
            let value = if masked {
                "[REDACTED]".to_string()
            } else {
                value.clone()
            };
            (name.clone(), value)
        })
        .collect()
}

/// Renders a command and its args as a shell-quoted, copy-pasteable string.
pub fn format_command(command: &str, args: &[String]) -> String {
    std::iter::once(command.to_string())
        .chain(args.iter().cloned())
        .map(|token| shell_quote(&token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    let is_safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:=@+-".contains(c));
    if is_safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

async fn run_inner(
    command: &str,
    args: &[String],
    options: &RunOptions,
    shell: bool,
) -> CommandResult {
    let started_at = iso8601(Utc::now());
    let start = Instant::now();
    let secrets = &options.secrets;

    let mut cmd = if shell {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    } else {
        let mut c = Command::new(command);
        c.args(args);
        c
    };
    cmd.current_dir(&options.cwd);
    for (key, value) in &options.env {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Put the child in its own process group (pgid = child pid) so a timeout
        // can signal the whole group, killing the shell and anything it forks.
        .process_group(0)
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return CommandResult {
                command: command.to_string(),
                args: Some(args.to_vec()),
                exit_code: None,
                stdout: redact_text("", secrets),
                stderr: redact_text(&format!("{error}\n"), secrets),
                started_at,
                finished_at: iso8601(Utc::now()),
                duration_ms: start.elapsed().as_millis() as u64,
                timed_out: false,
                error: Some(error.to_string()),
            };
        }
    };
    // Capture the pid now: after a cancelled `wait()` future, `child.id()` can
    // return None, which would silently skip the kill signal.
    let pid = child.id();

    // Drain both pipes concurrently so a chatty child never blocks on a full
    // pipe buffer while we wait for it to exit.
    let stdout_pipe = child.stdout.take().expect("stdout is piped");
    let stderr_pipe = child.stderr.take().expect("stderr is piped");
    let stdout_task = tokio::spawn(read_capped(stdout_pipe));
    let stderr_task = tokio::spawn(read_capped(stderr_pipe));

    let mut timed_out = false;
    let status = match options.timeout {
        Some(duration) => match tokio::time::timeout(duration, child.wait()).await {
            Ok(result) => result.ok(),
            Err(_) => {
                timed_out = true;
                terminate(&mut child, pid).await
            }
        },
        None => child.wait().await.ok(),
    };

    let (stdout_join, stderr_join) = tokio::join!(stdout_task, stderr_task);
    let (stdout_raw, stdout_bytes) = stdout_join.unwrap_or_default();
    let (stderr_raw, stderr_bytes) = stderr_join.unwrap_or_default();

    CommandResult {
        command: command.to_string(),
        args: Some(args.to_vec()),
        exit_code: status.and_then(|s| s.code()),
        stdout: redact_text(&append_truncation(stdout_raw, stdout_bytes), secrets),
        stderr: redact_text(&append_truncation(stderr_raw, stderr_bytes), secrets),
        started_at,
        finished_at: iso8601(Utc::now()),
        duration_ms: start.elapsed().as_millis() as u64,
        timed_out,
        error: None,
    }
}

/// On timeout, ask the process group politely with `SIGTERM`, then force
/// `SIGKILL` if it has not exited within the grace period. Signalling the group
/// (not just the leader pid) reaches children the shell may have forked.
async fn terminate(child: &mut Child, pid: Option<u32>) -> Option<ExitStatus> {
    if let Some(pid) = pid {
        let group = nix::unistd::Pid::from_raw(pid as i32);
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGTERM);
        if let Ok(result) = tokio::time::timeout(GRACE_AFTER_SIGTERM, child.wait()).await {
            return result.ok();
        }
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
    }
    let _ = child.start_kill();
    child.wait().await.ok()
}

/// Reads a stream to EOF, retaining at most [`OUTPUT_LIMIT_BYTES`] but draining
/// the rest. Returns the retained text and the total byte count seen.
async fn read_capped<R>(mut reader: R) -> (String, usize)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut retained: Vec<u8> = Vec::new();
    let mut total = 0usize;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                total += n;
                if total <= OUTPUT_LIMIT_BYTES {
                    retained.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    (String::from_utf8_lossy(&retained).into_owned(), total)
}

fn append_truncation(value: String, total_bytes: usize) -> String {
    if total_bytes <= OUTPUT_LIMIT_BYTES {
        value
    } else {
        format!("{value}\n[output truncated after {OUTPUT_LIMIT_BYTES} bytes]\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn opts() -> RunOptions {
        RunOptions {
            cwd: PathBuf::from("."),
            ..RunOptions::default()
        }
    }

    #[test]
    fn format_command_quotes_arguments_that_need_it() {
        let rendered = format_command("claude", &["-p".into(), "fix the bug".into()]);
        assert_eq!(rendered, "claude -p 'fix the bug'");
    }

    #[test]
    fn format_command_leaves_safe_tokens_unquoted() {
        let rendered = format_command("cargo", &["clippy".into(), "--all-targets".into()]);
        assert_eq!(rendered, "cargo clippy --all-targets");
    }

    #[test]
    fn redact_text_masks_every_secret_occurrence() {
        let red = redact_text("key=abc123 then abc123", &["abc123".to_string()]);
        assert_eq!(red, "key=[REDACTED] then [REDACTED]");
    }

    #[test]
    fn redact_env_masks_only_secret_like_keys() {
        let mut env = BTreeMap::new();
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-123".to_string());
        env.insert("ANTHROPIC_BASE_URL".to_string(), "https://x".to_string());
        let red = redact_env(&env);
        assert_eq!(red["ANTHROPIC_API_KEY"], "[REDACTED]");
        assert_eq!(red["ANTHROPIC_BASE_URL"], "https://x");
    }

    #[tokio::test]
    async fn shell_command_captures_stdout_and_zero_exit() {
        let result = run_shell_command("echo hello", &opts()).await;
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello"));
        assert!(!result.timed_out);
        assert!(!result.started_at.is_empty());
    }

    #[tokio::test]
    async fn shell_command_reports_nonzero_exit() {
        let result = run_shell_command("exit 3", &opts()).await;
        assert_eq!(result.exit_code, Some(3));
    }

    #[tokio::test]
    async fn timeout_kills_long_command_and_sets_timed_out() {
        let options = RunOptions {
            cwd: PathBuf::from("."),
            timeout: Some(Duration::from_millis(200)),
            ..RunOptions::default()
        };
        let result = run_shell_command("sleep 5", &options).await;
        assert!(result.timed_out, "expected the run to time out");
        assert!(
            result.duration_ms < 5000,
            "command should have been killed early, took {}ms",
            result.duration_ms
        );
    }

    #[tokio::test]
    async fn secrets_are_redacted_from_captured_output() {
        let options = RunOptions {
            cwd: PathBuf::from("."),
            secrets: vec!["SECRET123".to_string()],
            ..RunOptions::default()
        };
        let result = run_shell_command("echo SECRET123", &options).await;
        assert!(result.stdout.contains("[REDACTED]"));
        assert!(!result.stdout.contains("SECRET123"));
    }

    #[tokio::test]
    async fn missing_binary_yields_error_and_null_exit_code() {
        let result = run_process("definitely-not-a-real-binary-xyzzy", &[], &opts()).await;
        assert!(result.exit_code.is_none());
        assert!(result.error.is_some());
    }
}
