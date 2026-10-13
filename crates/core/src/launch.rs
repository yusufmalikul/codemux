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
        vec!["-l".to_string(), "-c".to_string(), claude_command(flag, id, crate::paths::app_data_dir())],
    );
    // cmd.exe finds both claude.exe and an npm claude.cmd through PATH.
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe".to_string(),
        vec!["/C".to_string(), "claude".to_string(), flag.to_string(), id.clone()],
    );
    Some(LaunchSpec { program, args, env: base_env(), cwd })
}

/// Shell script that starts `claude`.
///
/// The native install is a symlink to `.../claude/versions/<version>`, and macOS
/// names the process after the real file, so Activity Monitor shows only the version.
/// To fix that, run it through a hard link named `claude` in `<data>/bin/<version>/`.
/// A hard link is a second name for the same file, so it takes no extra disk space.
/// Each version gets its own folder, so an update gets a fresh link.
/// If anything fails (or it is an npm install), fall back to plain `claude`.
#[cfg(unix)]
fn claude_command(flag: &str, id: &str, data_dir: Option<PathBuf>) -> String {
    let plain = format!("exec claude {flag} {id}");
    let Some(dir) = data_dir else { return plain };
    let bin = shell_quote(&dir.join("bin").to_string_lossy());
    format!(
        "r=$(realpath \"$(command -v claude)\" 2>/dev/null) && case \"$r\" in */claude/versions/*) \
         l={bin}/\"${{r##*/}}\"/claude; \
         {{ [ \"$l\" -ef \"$r\" ] || {{ mkdir -p \"${{l%/*}}\" && ln -f \"$r\" \"$l\"; }}; }} 2>/dev/null \
         && exec \"$l\" {flag} {id};; esac; {plain}"
    )
}

#[cfg(unix)]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
        assert_eq!(&s.args[..2], &["-l", "-c"]);
        assert!(s.args[2].ends_with("exec claude --resume abc-1"));
        assert_eq!(s.env["TERM"], "xterm-256color");
        assert_eq!(s.cwd, PathBuf::from("/p"));
    }

    /// Runs the real script against a fake native install laid out like
    /// `~/.local/bin/claude -> .../claude/versions/9.9.9`.
    #[cfg(unix)]
    #[test]
    fn runs_claude_through_hard_link_named_claude() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let versions = tmp.path().join("share/claude/versions");
        std::fs::create_dir_all(&versions).unwrap();
        let real = versions.join("9.9.9");
        std::fs::write(&real, "#!/bin/sh\necho \"$0 $*\"\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path_dir = tmp.path().join("local bin");
        std::fs::create_dir_all(&path_dir).unwrap();
        symlink(&real, path_dir.join("claude")).unwrap();
        let data = tmp.path().join("App Support/it's");

        let script = claude_command("--resume", "abc-1", Some(data.clone()));
        let run = || {
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", &script])
                .env("PATH", format!("{}:/usr/bin:/bin", path_dir.display()))
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let link = data.join("bin/9.9.9/claude");
        assert_eq!(run(), format!("{} --resume abc-1\n", link.display()));
        // Second run reuses the existing link.
        assert_eq!(run(), format!("{} --resume abc-1\n", link.display()));
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&link).unwrap().ino(), std::fs::metadata(&real).unwrap().ino());
    }

    /// An npm install is not under `claude/versions/`, so run it as before.
    #[cfg(unix)]
    #[test]
    fn npm_install_falls_back_to_plain_claude() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("claude"), "#!/bin/sh\necho \"$0 $*\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path().join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let script = claude_command("--resume", "abc-1", Some(tmp.path().join("data")));
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .env("PATH", format!("{}:/usr/bin:/bin", tmp.path().display()))
            .output()
            .unwrap();
        assert!(String::from_utf8(out.stdout).unwrap().ends_with("claude --resume abc-1\n"));
        assert!(!tmp.path().join("data").exists());
    }
}
