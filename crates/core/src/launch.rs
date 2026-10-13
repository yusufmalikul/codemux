//! Builds the command and environment for a new PTY, per OS (spec 11.1).

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeMode {
    /// `claude --resume <id>`
    Resume(String),
    /// `claude --session-id <uuid>`
    New(String),
}

/// Session IDs are UUIDs. Refuse anything else, so an ID can never inject shell code.
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn base_env() -> HashMap<String, String> {
    HashMap::from([
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("COLORTERM".to_string(), "truecolor".to_string()),
    ])
}

#[cfg(unix)]
fn user_shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| {
        if cfg!(target_os = "macos") { "/bin/zsh".into() } else { "/bin/bash".into() }
    })
}

/// A plain interactive shell, like a new tab in a normal terminal.
pub fn shell(cwd: PathBuf) -> LaunchSpec {
    #[cfg(unix)]
    let (program, args) = (user_shell(), vec!["-l".to_string()]);
    #[cfg(windows)]
    let (program, args) = ("powershell.exe".to_string(), vec!["-NoLogo".to_string()]);
    LaunchSpec { program, args, env: base_env(), cwd }
}

/// Run Claude Code through the user's login shell, so PATH matches a normal terminal.
pub fn claude(cwd: PathBuf, mode: &ClaudeMode) -> Option<LaunchSpec> {
    let (flag, id) = match mode {
        ClaudeMode::Resume(id) => ("--resume", id),
        ClaudeMode::New(id) => ("--session-id", id),
    };
    if !is_safe_id(id) {
        return None;
    }
    #[cfg(unix)]
    let (program, args) = (
        user_shell(),
        vec!["-l".to_string(), "-c".to_string(), format!("exec claude {flag} {id}")],
    );
    // cmd.exe finds both claude.exe and an npm claude.cmd through PATH.
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe".to_string(),
        vec!["/C".to_string(), "claude".to_string(), flag.to_string(), id.clone()],
    );
    Some(LaunchSpec { program, args, env: base_env(), cwd })
}

/// Remove variables inherited from a parent Claude Code process.
/// If they leak into a child `claude`, it stops saving its transcript.
/// Call once at startup, before any thread is spawned.
pub fn clean_inherited_env() {
    let keys: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| k.starts_with("CLAUDE_CODE_") || k == "CLAUDECODE")
        .collect();
    for k in keys {
        std::env::remove_var(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_ids() {
        assert!(is_safe_id("362edec9-a4ee-4c27-8918-37423684510b"));
        assert!(!is_safe_id("x; rm -rf ~"));
        assert!(!is_safe_id("$(id)"));
        assert!(!is_safe_id(""));
        assert!(claude(PathBuf::from("/"), &ClaudeMode::Resume("a b".into())).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn resume_runs_through_login_shell() {
        let s = claude(PathBuf::from("/p"), &ClaudeMode::Resume("abc-1".into())).unwrap();
        assert_eq!(s.args, vec!["-l", "-c", "exec claude --resume abc-1"]);
        assert_eq!(s.env["TERM"], "xterm-256color");
        assert_eq!(s.cwd, PathBuf::from("/p"));
    }
}
