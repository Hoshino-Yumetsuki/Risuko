use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use tauri_plugin_shell::ShellExt;
use tokio::time::timeout;

use crate::state::AppState;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 5 * 60_000;
const MAX_OUTPUT_BYTES: usize = 8 * 1024;

#[derive(Debug, Default, Clone)]
struct ScriptConfig {
    enabled: bool,
    command: String,
    args_template: String,
    timeout_ms: u64,
}

fn read_user_value<'a>(user: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    user.get(key)
}

fn read_script_config(state: &AppState) -> Result<ScriptConfig, String> {
    let cfg = state.config.lock().map_err(|e| e.to_string())?;
    let user = cfg.get_user_config();

    let enabled = read_user_value(user, "completion-script-enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let command = read_user_value(user, "completion-script-command")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let args_template = read_user_value(user, "completion-script-args")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let timeout_ms = read_user_value(user, "completion-script-timeout-ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1_000, MAX_TIMEOUT_MS);

    Ok(ScriptConfig {
        enabled,
        command,
        args_template,
        timeout_ms,
    })
}

/// Tokenize an args template by whitespace (no shell interpretation), then
/// substitute `{path}`, `{hash}`, `{status}` placeholders. Empty tokens after
/// substitution are preserved as empty arguments so positional scripts still
/// receive the expected number of args
fn build_args(template: &str, path: &str, hash: &str, status: &str) -> Vec<String> {
    template
        .split_whitespace()
        .map(|tok| {
            tok.replace("{path}", path)
                .replace("{hash}", hash)
                .replace("{status}", status)
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct ScriptRunResult {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    duration_ms: u128,
    timed_out: bool,
    message: Option<String>,
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_OUTPUT_BYTES {
        s.truncate(MAX_OUTPUT_BYTES);
        s.push_str("\n...[truncated]");
    }
    s
}

async fn execute(
    handle: &AppHandle,
    command: &str,
    args: Vec<String>,
    env_vars: Vec<(String, String)>,
    timeout_ms: u64,
) -> ScriptRunResult {
    let start = std::time::Instant::now();
    let shell = handle.shell();
    let mut cmd = shell.command(command).args(args);
    for (k, v) in env_vars {
        cmd = cmd.env(k, v);
    }

    let fut = cmd.output();
    match timeout(Duration::from_millis(timeout_ms), fut).await {
        Ok(Ok(output)) => {
            let stdout = truncate(String::from_utf8_lossy(&output.stdout).into_owned());
            let stderr = truncate(String::from_utf8_lossy(&output.stderr).into_owned());
            let exit_code = output.status.code();
            let success = output.status.success();
            ScriptRunResult {
                success,
                exit_code,
                stdout,
                stderr,
                duration_ms: start.elapsed().as_millis(),
                timed_out: false,
                message: None,
            }
        }
        Ok(Err(e)) => ScriptRunResult {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: start.elapsed().as_millis(),
            timed_out: false,
            message: Some(format!("spawn failed: {e}")),
        },
        Err(_) => ScriptRunResult {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: start.elapsed().as_millis(),
            timed_out: true,
            message: Some(format!("script timed out after {timeout_ms}ms")),
        },
    }
}

/// Per-task overrides for the completion script. Any field set takes
/// precedence over the saved global preference. `enabled = Some(false)`
/// fully disables the script for that task even when globally enabled
#[derive(Debug, Default, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionScriptOverrides {
    pub enabled: Option<bool>,
    pub command: Option<String>,
    pub args: Option<String>,
    pub timeout_ms: Option<u64>,
}

fn merge_with_overrides(
    mut cfg: ScriptConfig,
    overrides: Option<CompletionScriptOverrides>,
) -> ScriptConfig {
    let Some(o) = overrides else {
        return cfg;
    };
    if let Some(enabled) = o.enabled {
        cfg.enabled = enabled;
    }
    if let Some(command) = o.command {
        let trimmed = command.trim().to_string();
        if !trimmed.is_empty() {
            cfg.command = trimmed;
            // A per-task command implies opt-in even if global was disabled,
            // unless the override explicitly set enabled=false above
            if o.enabled.is_none() {
                cfg.enabled = true;
            }
        }
    }
    if let Some(args) = o.args {
        cfg.args_template = args;
    }
    if let Some(timeout_ms) = o.timeout_ms {
        cfg.timeout_ms = timeout_ms.clamp(1_000, MAX_TIMEOUT_MS);
    }
    cfg
}

/// Fire-and-forget invocation from the renderer when a download finishes
/// Reads the saved script config; no-op when disabled or unconfigured
#[tauri::command]
pub async fn run_completion_script(
    handle: AppHandle,
    state: State<'_, AppState>,
    path: String,
    hash: Option<String>,
    status: String,
    overrides: Option<CompletionScriptOverrides>,
) -> Result<(), String> {
    let base = read_script_config(&state)?;
    let cfg = merge_with_overrides(base, overrides);
    if !cfg.enabled || cfg.command.is_empty() {
        return Ok(());
    }

    let hash = hash.unwrap_or_default();
    let args = build_args(&cfg.args_template, &path, &hash, &status);
    let env = vec![
        ("RISUKO_PATH".into(), path.clone()),
        ("RISUKO_HASH".into(), hash.clone()),
        ("RISUKO_STATUS".into(), status.clone()),
    ];

    let handle_clone = handle.clone();
    let command = cfg.command.clone();
    let timeout_ms = cfg.timeout_ms;
    tauri::async_runtime::spawn(async move {
        let result = execute(&handle_clone, &command, args, env, timeout_ms).await;
        if result.success {
            log::info!(
                "[completion-script] ok exit={:?} dur={}ms path={}",
                result.exit_code,
                result.duration_ms,
                path
            );
        } else {
            log::warn!(
                "[completion-script] failed exit={:?} timed_out={} dur={}ms msg={:?} stderr={}",
                result.exit_code,
                result.timed_out,
                result.duration_ms,
                result.message,
                result.stderr
            );
        }
    });

    Ok(())
}

/// Synchronous test invocation used by the Preference UI to validate the
/// configured script without waiting for an actual download to complete
#[tauri::command]
pub async fn test_completion_script(
    handle: AppHandle,
    command: String,
    args: String,
    timeout_ms: Option<u64>,
) -> Result<ScriptRunResult, String> {
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("command is empty".into());
    }
    let timeout_ms = timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1_000, MAX_TIMEOUT_MS);
    let path = "/tmp/risuko-test-file".to_string();
    let hash = "0000000000000000000000000000000000000000".to_string();
    let status = "complete".to_string();
    let parsed_args = build_args(&args, &path, &hash, &status);
    let env = vec![
        ("RISUKO_PATH".into(), path),
        ("RISUKO_HASH".into(), hash),
        ("RISUKO_STATUS".into(), status),
    ];
    Ok(execute(&handle, &command, parsed_args, env, timeout_ms).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_args_substitutes_placeholders() {
        let args = build_args(
            "--path {path} --hash {hash} --status {status}",
            "/tmp/file.iso",
            "abc123",
            "complete",
        );
        assert_eq!(
            args,
            vec![
                "--path",
                "/tmp/file.iso",
                "--hash",
                "abc123",
                "--status",
                "complete",
            ]
        );
    }

    #[test]
    fn build_args_handles_empty_template() {
        assert!(build_args("", "p", "h", "s").is_empty());
    }

    fn cfg(enabled: bool, command: &str, args: &str, timeout_ms: u64) -> ScriptConfig {
        ScriptConfig {
            enabled,
            command: command.to_string(),
            args_template: args.to_string(),
            timeout_ms,
        }
    }

    #[test]
    fn override_command_implies_enabled_when_global_disabled() {
        let merged = merge_with_overrides(
            cfg(false, "/global", "", 30_000),
            Some(CompletionScriptOverrides {
                enabled: None,
                command: Some("/per-task".into()),
                args: None,
                timeout_ms: None,
            }),
        );
        assert!(merged.enabled);
        assert_eq!(merged.command, "/per-task");
    }

    #[test]
    fn override_can_explicitly_disable_per_task() {
        let merged = merge_with_overrides(
            cfg(true, "/global", "--g", 30_000),
            Some(CompletionScriptOverrides {
                enabled: Some(false),
                command: Some("/per-task".into()),
                args: None,
                timeout_ms: None,
            }),
        );
        assert!(!merged.enabled);
    }

    #[test]
    fn override_args_and_timeout_replace_global() {
        let merged = merge_with_overrides(
            cfg(true, "/global", "--global", 5_000),
            Some(CompletionScriptOverrides {
                enabled: None,
                command: None,
                args: Some("--task {path}".into()),
                timeout_ms: Some(10_000),
            }),
        );
        assert_eq!(merged.args_template, "--task {path}");
        assert_eq!(merged.timeout_ms, 10_000);
        assert_eq!(merged.command, "/global");
    }
}
