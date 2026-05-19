//! Shell execution tool. Port of `nanobot.agent.tools.shell`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::timeout;

use config::get_media_dir;
use security::contains_internal_url;

use super::base::{Tool, ToolExecError};
use super::sandbox::wrap_command;

const MAX_TIMEOUT_SECS: u64 = 600;
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_OUTPUT_CHARS: usize = 10_000;

static DEFAULT_DENY_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"\brm\s+-[rf]{1,2}\b",
        r"\bdel\s+/[fq]\b",
        r"\brmdir\s+/s\b",
        r"(?:^|[;&|]\s*)format\b",
        r"\b(mkfs|diskpart)\b",
        r"\bdd\s+if=",
        r">\s*/dev/sd",
        r"\b(shutdown|reboot|poweroff)\b",
        r":\(\)\s*\{.*\};\s*:",
        r">>?\s*\S*(?:history\.jsonl|\.dream_cursor)",
        r"\btee\b[^|;&<>]*(?:history\.jsonl|\.dream_cursor)",
        r"\b(?:cp|mv)\b(?:\s+[^\s|;&<>]+)+\s+\S*(?:history\.jsonl|\.dream_cursor)",
        r"\bdd\b[^|;&<>]*\bof=\S*(?:history\.jsonl|\.dream_cursor)",
        r"\bsed\s+-i[^|;&<>]*(?:history\.jsonl|\.dream_cursor)",
    ]
    .iter()
    .filter_map(|p| Regex::new(p).ok())
    .collect()
});

static WIN_PATH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"[A-Za-z]:\\[^\s"'|><;]*"#).unwrap());
static POSIX_PATH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?:^|[\s|>'"])(/[^\s"'>;|<]+)"#).unwrap());
static HOME_PATH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?:^|[\s|>'"])(~[^\s"'>;|<]*)"#).unwrap());

/// Shell execution tool.
pub struct ExecTool {
    pub timeout_secs: u64,
    pub working_dir: Option<PathBuf>,
    pub deny_patterns: Vec<Regex>,
    pub allow_patterns: Vec<Regex>,
    pub restrict_to_workspace: bool,
    pub sandbox: String,
    pub path_append: String,
    pub allowed_env_keys: Vec<String>,
}

impl ExecTool {
    pub fn new() -> Self {
        Self {
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            working_dir: None,
            deny_patterns: DEFAULT_DENY_PATTERNS.clone(),
            allow_patterns: Vec::new(),
            restrict_to_workspace: false,
            sandbox: String::new(),
            path_append: String::new(),
            allowed_env_keys: Vec::new(),
        }
    }

    pub fn with_working_dir(mut self, ws: PathBuf) -> Self {
        self.working_dir = Some(ws);
        self
    }

    pub fn with_timeout_secs(mut self, timeout: u64) -> Self {
        self.timeout_secs = timeout;
        self
    }

    pub fn with_path_append(mut self, path: impl Into<String>) -> Self {
        self.path_append = path.into();
        self
    }

    pub fn with_allowed_env_keys(mut self, keys: Vec<String>) -> Self {
        self.allowed_env_keys = keys;
        self
    }

    pub fn with_restrict_to_workspace(mut self, r: bool) -> Self {
        self.restrict_to_workspace = r;
        self
    }

    pub fn with_sandbox(mut self, sandbox: impl Into<String>) -> Self {
        self.sandbox = sandbox.into();
        self
    }

    fn guard_command(&self, command: &str, cwd: &Path) -> Option<String> {
        let cmd = command.trim();
        let lower = cmd.to_ascii_lowercase();
        for pattern in &self.deny_patterns {
            if pattern.is_match(&lower) {
                return Some(
                    "Error: Command blocked by safety guard (dangerous pattern detected)"
                        .into(),
                );
            }
        }
        if !self.allow_patterns.is_empty()
            && !self.allow_patterns.iter().any(|p| p.is_match(&lower))
        {
            return Some("Error: Command blocked by safety guard (not in allowlist)".into());
        }
        if contains_internal_url(cmd) {
            return Some(
                "Error: Command blocked by safety guard (internal/private URL detected)".into(),
            );
        }
        if self.restrict_to_workspace {
            if cmd.contains("..\\") || cmd.contains("../") {
                return Some(
                    "Error: Command blocked by safety guard (path traversal detected)".into(),
                );
            }
            let cwd_path = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
            let media_path = std::fs::canonicalize(&get_media_dir(None))
                .unwrap_or_else(|_| get_media_dir(None));
            for raw in extract_absolute_paths(cmd) {
                let expanded = shellexpand(raw.trim());
                let p = std::fs::canonicalize(&expanded).unwrap_or(expanded);
                if !p.is_absolute() {
                    continue;
                }
                if p == cwd_path || p.starts_with(&cwd_path) {
                    continue;
                }
                if p == media_path || p.starts_with(&media_path) {
                    continue;
                }
                return Some(
                    "Error: Command blocked by safety guard (path outside working dir)".into(),
                );
            }
        }
        None
    }

    fn build_env(&self) -> HashMap<String, String> {
        if cfg!(windows) {
            let mut env = HashMap::new();
            let sr = std::env::var("SYSTEMROOT").unwrap_or_else(|_| "C:\\Windows".into());
            env.insert("SYSTEMROOT".into(), sr.clone());
            env.insert(
                "COMSPEC".into(),
                std::env::var("COMSPEC").unwrap_or_else(|_| format!("{sr}\\system32\\cmd.exe")),
            );
            for key in [
                "USERPROFILE",
                "HOMEDRIVE",
                "HOMEPATH",
                "TEMP",
                "TMP",
                "PATHEXT",
                "PATH",
                "APPDATA",
                "LOCALAPPDATA",
                "ProgramData",
                "ProgramFiles",
                "ProgramFiles(x86)",
                "ProgramW6432",
            ] {
                if let Ok(v) = std::env::var(key) {
                    env.insert(key.into(), v);
                }
            }
            for k in &self.allowed_env_keys {
                if let Ok(v) = std::env::var(k) {
                    env.insert(k.clone(), v);
                }
            }
            env
        } else {
            let mut env = HashMap::new();
            env.insert(
                "HOME".into(),
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()),
            );
            env.insert(
                "LANG".into(),
                std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()),
            );
            env.insert(
                "TERM".into(),
                std::env::var("TERM").unwrap_or_else(|_| "dumb".into()),
            );
            for k in &self.allowed_env_keys {
                if let Ok(v) = std::env::var(k) {
                    env.insert(k.clone(), v);
                }
            }
            env
        }
    }
}

impl Default for ExecTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }
    fn description(&self) -> &str {
        "Execute a shell command and return its output. Prefer read_file/write_file/edit_file over cat/echo/sed, and grep/glob over shell find/grep. Output is truncated at 10 000 chars; timeout defaults to 60s."
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "command":{"type":"string","description":"The shell command to execute"},
                "working_dir":{"type":"string","description":"Optional working directory"},
                "timeout":{"type":"integer","minimum":1,"maximum":600,"description":"Timeout in seconds (default 60)"},
            },
            "required":["command"],
        })
    }
    fn exclusive(&self) -> bool {
        true
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let Some(command) = params.get("command").and_then(|v| v.as_str()) else {
            return Ok(Value::String("Error: command required".into()));
        };
        let working_dir = params
            .get("working_dir")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);
        let requested_timeout = params.get("timeout").and_then(|v| v.as_u64());

        let cwd = working_dir
            .clone()
            .or_else(|| self.working_dir.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        if self.restrict_to_workspace {
            if let Some(ws) = &self.working_dir {
                let requested =
                    std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
                let root = std::fs::canonicalize(ws).unwrap_or_else(|_| ws.clone());
                if requested != root && !requested.starts_with(&root) {
                    return Ok(Value::String(
                        "Error: working_dir is outside the configured workspace".into(),
                    ));
                }
            }
        }
        if let Some(err) = self.guard_command(command, &cwd) {
            return Ok(Value::String(err));
        }

        let (mut command_str, effective_cwd) = (command.to_string(), cwd);
        let mut effective_cwd = effective_cwd;
        if !self.sandbox.is_empty() {
            if cfg!(windows) {
                warn!("Sandbox '{}' not supported on Windows; running unsandboxed", self.sandbox);
            } else {
                let workspace = self.working_dir.clone().unwrap_or(effective_cwd.clone());
                match wrap_command(&self.sandbox, &command_str, &workspace, &effective_cwd) {
                    Ok(wrapped) => {
                        command_str = wrapped;
                        effective_cwd =
                            std::fs::canonicalize(&workspace).unwrap_or(workspace);
                    }
                    Err(e) => return Ok(Value::String(format!("Error: {e}"))),
                }
            }
        }

        let effective_timeout = requested_timeout
            .unwrap_or(self.timeout_secs)
            .min(MAX_TIMEOUT_SECS);
        let mut env = self.build_env();
        if !self.path_append.is_empty() {
            if cfg!(windows) {
                let p = env.entry("PATH".into()).or_default();
                if !p.is_empty() {
                    p.push(';');
                }
                p.push_str(&self.path_append);
            } else {
                command_str = format!("export PATH=\"$PATH:{}\"; {}", self.path_append, command_str);
            }
        }

        let mut cmd = if cfg!(windows) {
            let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
            let mut c = Command::new(comspec);
            c.arg("/c").arg(&command_str);
            c
        } else {
            let mut c = Command::new("/bin/bash");
            c.arg("-l").arg("-c").arg(&command_str);
            c
        };
        cmd.current_dir(&effective_cwd)
            .env_clear()
            .envs(&env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let fut = cmd.output();
        let output = match timeout(Duration::from_secs(effective_timeout), fut).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Ok(Value::String(format!("Error executing command: {e}")));
            }
            Err(_) => {
                return Ok(Value::String(format!(
                    "Error: Command timed out after {effective_timeout} seconds"
                )));
            }
        };

        let mut parts = Vec::new();
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            parts.push(stdout.to_string());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            parts.push(format!("STDERR:\n{stderr}"));
        }
        let code = output.status.code().unwrap_or(-1);
        parts.push(format!("\nExit code: {code}"));
        let result = if parts.is_empty() {
            "(no output)".to_string()
        } else {
            parts.join("\n")
        };
        let result = truncate_middle(&result, MAX_OUTPUT_CHARS);
        Ok(Value::String(result))
    }
}

fn truncate_middle(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let head: String = s.chars().take(half).collect();
    let tail: String = s.chars().rev().take(half).collect();
    let tail: String = tail.chars().rev().collect();
    let diff = s.len() - max;
    format!("{head}\n\n... ({diff} chars truncated) ...\n\n{tail}")
}

fn extract_absolute_paths(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in WIN_PATH_RE.find_iter(cmd) {
        out.push(m.as_str().to_string());
    }
    for caps in POSIX_PATH_RE.captures_iter(cmd) {
        if let Some(m) = caps.get(1) {
            out.push(m.as_str().to_string());
        }
    }
    for caps in HOME_PATH_RE.captures_iter(cmd) {
        if let Some(m) = caps.get(1) {
            out.push(m.as_str().to_string());
        }
    }
    out
}

fn shellexpand(raw: &str) -> PathBuf {
    if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guard_blocks_rm_rf() {
        let tool = ExecTool::new();
        let res = tool
            .execute(json!({"command":"rm -rf /"}))
            .await
            .unwrap();
        assert!(res.as_str().unwrap().contains("dangerous pattern"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn echo_roundtrip() {
        let tool = ExecTool::new();
        let res = tool
            .execute(json!({"command":"echo hello","timeout":5}))
            .await
            .unwrap();
        let s = res.as_str().unwrap();
        assert!(s.contains("hello"));
        assert!(s.contains("Exit code: 0"));
    }
}
