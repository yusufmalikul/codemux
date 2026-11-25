//! Builds the command and environment for a new PTY, per OS (spec 11.1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Environment variable that picks Claude Code's config folder (the account).
pub const CONFIG_ENV: &str = "CLAUDE_CONFIG_DIR";

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

fn account_env(config_dir: Option<&Path>) -> HashMap<String, String> {
    let mut env = base_env();
    if let Some(dir) = config_dir {
        env.insert(CONFIG_ENV.to_string(), dir.to_string_lossy().into_owned());
    }
    env
}

/// Sign in to an account: `claude auth login` with its config folder.
pub fn login(config_dir: &Path, cwd: PathBuf) -> LaunchSpec {
    #[cfg(unix)]
    let (program, args) = (user_shell(), vec!["-l".to_string(), "-c".to_string(), "claude auth login".to_string()]);
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe".to_string(),
        vec!["/C".to_string(), "claude".to_string(), "auth".to_string(), "login".to_string()],
    );
    LaunchSpec { program, args, env: account_env(Some(config_dir)), cwd }
}

/// A plain interactive shell, like a new tab in a normal terminal.
pub fn shell(cwd: PathBuf) -> LaunchSpec {
    #[cfg(unix)]
    let (program, args) = (user_shell(), vec!["-l".to_string()]);
    #[cfg(windows)]
    let (program, args) = ("powershell.exe".to_string(), vec!["-NoLogo".to_string()]);
    LaunchSpec { program, args, env: base_env(), cwd }
}

/// Run Claude Code through the user's interactive login shell, so PATH and
/// aliases match a normal terminal (spec 11.1, 11.7).
/// `config_dir` picks the account: Some sets `CLAUDE_CONFIG_DIR`, None keeps
/// the default. The default must stay unset: Claude keys its saved login by
/// this value, so setting it to `~/.claude` finds no login.
pub fn claude(cwd: PathBuf, mode: &ClaudeMode, config_dir: Option<&Path>) -> Option<LaunchSpec> {
    let (flag, id) = match mode {
        ClaudeMode::Resume(id) => ("--resume", id),
        ClaudeMode::New(id) => ("--session-id", id),
    };
    if !is_safe_id(id) {
        return None;
    }
    #[cfg(unix)]
    let (program, args) = {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let link_dir = crate::paths::app_data_dir().and_then(|d| claude_link_dir(&d, &path));
        let script = claude_command(&account_command(config_dir), flag, id, link_dir.as_deref());
        (user_shell(), vec!["-l".to_string(), "-i".to_string(), "-c".to_string(), script])
    };
    // cmd.exe finds both claude.exe and an npm claude.cmd through PATH.
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe".to_string(),
        vec!["/C".to_string(), "claude".to_string(), flag.to_string(), id.clone()],
    );
    Some(LaunchSpec { program, args, env: account_env(config_dir), cwd })
}

/// The shell command that starts Claude for an account: the account folder's
/// name without the dot. `~/.claude` (the default) runs `claude`,
/// `~/.claude-pro` runs `claude-pro`. So a shell alias with that name, for
/// example one that sets a long-lived `CLAUDE_CODE_OAUTH_TOKEN`, is used.
/// A name that is not a plain word falls back to `claude`.
pub fn account_command(config_dir: Option<&Path>) -> String {
    config_dir
        .and_then(|d| d.file_name()?.to_str())
        .map(|n| n.trim_start_matches('.'))
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or("claude")
        .to_string()
}

/// Shell script that starts Claude with the account's command `cmd`.
///
/// If the shell knows `cmd` (an alias, function or program), it runs, so the
/// alias sets the account's token. If not, the real `claude` program runs
/// (`command` skips a `claude` alias, whose token is for the default account).
/// No `exec`: it would skip alias expansion. zsh replaces itself with the last
/// command anyway.
///
/// `link_dir` holds a hard link named `claude` (see `claude_link_dir`). It goes
/// first in PATH, so an alias that runs `claude` runs the link.
#[cfg(unix)]
fn claude_command(cmd: &str, flag: &str, id: &str, link_dir: Option<&Path>) -> String {
    let path = link_dir.map_or(String::new(), |d| format!("PATH={}:\"$PATH\"; ", shell_quote(&d.to_string_lossy())));
    format!("{path}if type {cmd} >/dev/null 2>&1; then {cmd} {flag} {id}; else command claude {flag} {id}; fi")
}

/// The native install is a symlink to `.../claude/versions/<version>`, and macOS
/// names the process after the real file, so Activity Monitor shows only the version.
/// To fix that, make a hard link named `claude` in `<data>/bin/<version>/` and
/// return that folder. A hard link is a second name for the same file, so it
/// takes no extra disk space. Each version gets its own folder, so an update
/// gets a fresh link. None when `claude` is not a native install (npm) or the
/// link fails; then `claude` runs as found in PATH.
#[cfg(unix)]
fn claude_link_dir(data_dir: &Path, path: &std::ffi::OsStr) -> Option<PathBuf> {
    let found = std::env::split_paths(path).map(|d| d.join("claude")).find(|p| p.is_file())?;
    let real = std::fs::canonicalize(found).ok()?;
    let version = real.file_name()?;
    if real.parent()?.file_name()? != "versions" || real.parent()?.parent()?.file_name()? != "claude" {
        return None;
    }
    let dir = data_dir.join("bin").join(version);
    let link = dir.join("claude");
    use std::os::unix::fs::MetadataExt;
    let same = |a: &Path, b: &Path| matches!((std::fs::metadata(a), std::fs::metadata(b)), (Ok(x), Ok(y)) if x.ino() == y.ino() && x.dev() == y.dev());
    if !same(&link, &real) {
        std::fs::create_dir_all(&dir).ok()?;
        let _ = std::fs::remove_file(&link);
        std::fs::hard_link(&real, &link).ok()?;
    }
    Some(dir)
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

/// Apps opened from Finder or Spotlight get a bare PATH (`/usr/bin:/bin:...`),
/// and `zsh -l` does not read `~/.zshrc`, so `claude` is not found.
/// Copy PATH from the user's interactive login shell, like a normal terminal.
/// Call once at startup, before any thread is spawned.
#[cfg(unix)]
pub fn load_shell_path() {
    if let Some(path) = read_shell_path(&user_shell(), None) {
        std::env::set_var("PATH", path);
    }
}

/// Run `shell -l -i` and print PATH between markers, because rc files may print text too.
#[cfg(unix)]
fn read_shell_path(shell: &str, home: Option<&std::path::Path>) -> Option<String> {
    use std::process::{Command, Stdio};
    const MARK: &str = "__CODEMUX_PATH__";
    let mut cmd = Command::new(shell);
    cmd.args(["-l", "-i", "-c", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if let Some(h) = home {
        cmd.env("HOME", h).env("ZDOTDIR", h);
    }
    let out = String::from_utf8(cmd.output().ok()?.stdout).ok()?;
    let path = out.split(MARK).nth(1)?;
    (!path.is_empty()).then(|| path.to_string())
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
        assert!(claude(PathBuf::from("/"), &ClaudeMode::Resume("a b".into()), None).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn resume_runs_through_interactive_login_shell() {
        let s = claude(PathBuf::from("/p"), &ClaudeMode::Resume("abc-1".into()), None).unwrap();
        assert_eq!(&s.args[..3], &["-l", "-i", "-c"]);
        assert!(s.args[3].ends_with("then claude --resume abc-1; else command claude --resume abc-1; fi"), "{}", s.args[3]);
        assert_eq!(s.env["TERM"], "xterm-256color");
        assert_eq!(s.cwd, PathBuf::from("/p"));
        assert!(!s.env.contains_key(CONFIG_ENV), "default account must not set the config folder");
    }

    #[test]
    fn other_account_sets_config_folder() {
        let s = claude(PathBuf::from("/p"), &ClaudeMode::Resume("abc-1".into()), Some(Path::new("/home/me/.claude-pro"))).unwrap();
        assert_eq!(s.env[CONFIG_ENV], "/home/me/.claude-pro");
        assert!(s.args.last().unwrap().contains("then claude-pro --resume abc-1;"));
        assert_eq!(login(Path::new("/x"), PathBuf::from("/p")).env[CONFIG_ENV], "/x");
    }

    #[test]
    fn account_command_is_the_folder_name() {
        assert_eq!(account_command(None), "claude");
        assert_eq!(account_command(Some(Path::new("/h/.claude"))), "claude");
        assert_eq!(account_command(Some(Path::new("/h/.claude-pro"))), "claude-pro");
        assert_eq!(account_command(Some(Path::new("/h/.claude-my_work2"))), "claude-my_work2");
        assert_eq!(account_command(Some(Path::new("/h/it's me"))), "claude");
        assert_eq!(account_command(Some(Path::new("/h/$(id)"))), "claude");
    }

    #[cfg(unix)]
    fn fake_claude(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, "#!/bin/sh\necho \"$0 $* token=$TOKEN config=$CLAUDE_CONFIG_DIR\"\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A native install laid out like `<bin>/claude -> .../claude/versions/9.9.9`.
    #[cfg(unix)]
    fn native_install(root: &Path) -> (PathBuf, PathBuf) {
        let versions = root.join("share/claude/versions");
        std::fs::create_dir_all(&versions).unwrap();
        let real = versions.join("9.9.9");
        fake_claude(&real);
        let bin = root.join("local bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&real, bin.join("claude")).unwrap();
        (bin, real)
    }

    #[cfg(unix)]
    #[test]
    fn links_native_install_as_claude() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        let (bin, real) = native_install(tmp.path());
        let data = tmp.path().join("App Support/it's");
        let path = std::env::join_paths([bin, "/usr/bin".into()]).unwrap();
        let dir = claude_link_dir(&data, &path).unwrap();
        assert_eq!(dir, data.join("bin/9.9.9"));
        let ino = |p: &Path| std::fs::metadata(p).unwrap().ino();
        assert_eq!(ino(&dir.join("claude")), ino(&real));
        // Second run reuses the existing link.
        assert_eq!(claude_link_dir(&data, &path).unwrap(), dir);
        assert_eq!(ino(&dir.join("claude")), ino(&real));
    }

    /// An npm install is not under `claude/versions/`, so no link is made.
    #[cfg(unix)]
    #[test]
    fn npm_install_gets_no_link() {
        let tmp = tempfile::tempdir().unwrap();
        fake_claude(&tmp.path().join("claude"));
        let data = tmp.path().join("data");
        assert!(claude_link_dir(&data, tmp.path().as_os_str()).is_none());
        assert!(!data.exists());
    }

    /// Runs the real script in an interactive zsh whose `.zshrc` defines the
    /// aliases, like `alias claude='CLAUDE_CODE_OAUTH_TOKEN=... claude'`.
    /// Each account must run its own alias, through the hard link.
    #[cfg(target_os = "macos")]
    #[test]
    fn each_account_runs_its_own_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let (bin, _) = native_install(tmp.path());
        std::fs::write(
            tmp.path().join(".zshrc"),
            "echo rc-noise\nalias claude='TOKEN=max claude'\n\
             alias claude-pro='CLAUDE_CONFIG_DIR=/h/.claude-pro TOKEN=pro command claude'\n",
        )
        .unwrap();
        let data = tmp.path().join("data");
        let path = std::env::join_paths([bin.clone(), "/usr/bin".into(), "/bin".into()]).unwrap();
        let link_dir = claude_link_dir(&data, &path).unwrap();
        let run = |config: Option<&str>| {
            let script = claude_command(&account_command(config.map(Path::new)), "--resume", "abc-1", Some(&link_dir));
            let mut cmd = std::process::Command::new("/bin/zsh");
            cmd.args(["-l", "-i", "-c", &script])
                .env("HOME", tmp.path())
                .env("ZDOTDIR", tmp.path())
                .env("PATH", &path)
                .env_remove(CONFIG_ENV)
                .stdin(std::process::Stdio::null());
            if let Some(c) = config {
                cmd.env(CONFIG_ENV, c);
            }
            let out = String::from_utf8(cmd.output().unwrap().stdout).unwrap();
            out.lines().last().unwrap_or_default().to_string()
        };
        let link = link_dir.join("claude");
        let link = link.display();
        assert_eq!(run(None), format!("{link} --resume abc-1 token=max config="));
        assert_eq!(run(Some("/h/.claude-pro")), format!("{link} --resume abc-1 token=pro config=/h/.claude-pro"));
        // No alias for this account: the real program, not the default account's alias.
        assert_eq!(run(Some("/h/.claude-work")), format!("{link} --resume abc-1 token= config=/h/.claude-work"));
    }

    /// Like Finder: bare PATH, and the user adds `~/.local/bin` in `~/.zshrc`.
    #[cfg(target_os = "macos")]
    #[test]
    fn reads_path_from_interactive_shell_rc() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".zshrc"), "echo hello\nexport PATH=\"/my/bin:$PATH\"\n").unwrap();
        let path = read_shell_path("/bin/zsh", Some(tmp.path())).unwrap();
        assert!(path.starts_with("/my/bin:"), "{path}");
    }
}
