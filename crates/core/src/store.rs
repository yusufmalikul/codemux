//! App-owned state (spec 10). Saved atomically: write a temp file, then rename.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppState {
    pub version: u32,
    /// Session IDs that had a running terminal at the last save, in tab order.
    #[serde(default)]
    pub open_sessions: Vec<String>,
    /// Privacy: the sidebar is collapsed.
    #[serde(default, alias = "hide_sidebar")]
    pub hide_sidebar: bool,
    /// Sidebar width in logical pixels. None means the default width.
    #[serde(default)]
    pub sidebar_width: Option<f32>,
    /// User-made workspaces. Sessions in none of them belong to the default workspace.
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// The default workspace is collapsed in the sidebar.
    #[serde(default)]
    pub default_collapsed: bool,
    /// Names the user gave to sessions, by session ID. Only shown in the app;
    /// Claude's session files are never changed.
    #[serde(default)]
    pub session_names: BTreeMap<String, String>,
    /// Extra Claude accounts. The default account (`~/.claude`) is not listed.
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Pinned sessions and terminals, in the order they were pinned. Shown
    /// in the PINNED block at the top of the sidebar.
    #[serde(default)]
    pub pinned: Vec<Pin>,
}

/// One entry of the PINNED block. A session is saved as its plain ID (the
/// only kind older versions wrote), a terminal as an object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Pin {
    Session(String),
    Terminal(PinnedTerminal),
}

/// A pinned plain terminal. It stays pinned after its tab closes; opening
/// it starts a new shell in the same folder, with the same name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinnedTerminal {
    /// Unique among pinned terminals.
    pub id: u64,
    /// The name the user gave it. None shows the default "Terminal · folder".
    #[serde(default)]
    pub name: Option<String>,
    pub cwd: std::path::PathBuf,
}

/// A Claude account: its own Claude Code config folder (`CLAUDE_CONFIG_DIR`),
/// with its own login and sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Never 0: 0 is the default account.
    pub id: u64,
    pub name: String,
    pub config_dir: std::path::PathBuf,
}

/// A named group of sessions, shown as a folder in the sidebar tree.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: u64,
    pub name: String,
    /// Session IDs in this workspace. A session is in at most one workspace.
    #[serde(default)]
    pub sessions: Vec<String>,
    #[serde(default)]
    pub collapsed: bool,
}

impl AppState {
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<AppState>(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut s = self.clone();
        s.version = STATE_VERSION;
        write_atomic(path, &serde_json::to_vec_pretty(&s)?)
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("state.json");
        let s = AppState { version: 0, open_sessions: vec!["a".into(), "b".into()], hide_sidebar: true, sidebar_width: Some(420.0),
            workspaces: vec![Workspace { id: 1, name: "Work".into(), sessions: vec!["a".into()], collapsed: true }],
            default_collapsed: true, session_names: BTreeMap::from([("a".into(), "My name".into())]),
            accounts: vec![Account { id: 1, name: "Pro".into(), config_dir: "/home/me/.claude-pro".into() }],
            pinned: vec![Pin::Session("b".into()), Pin::Terminal(PinnedTerminal { id: 1, name: Some("Logs".into()), cwd: "/tmp".into() })] };
        s.save(&p).unwrap();
        let loaded = AppState::load(&p);
        assert_eq!(loaded.open_sessions, s.open_sessions);
        assert!(loaded.hide_sidebar);
        assert_eq!(loaded.sidebar_width, Some(420.0));
        assert_eq!(loaded.workspaces, s.workspaces);
        assert!(loaded.default_collapsed);
        assert_eq!(loaded.session_names, s.session_names);
        assert_eq!(loaded.accounts, s.accounts);
        assert_eq!(loaded.pinned, s.pinned);
        assert_eq!(loaded.version, STATE_VERSION);
        assert!(!p.with_extension("tmp").exists());
    }

    /// Files from before terminals could be pinned list only session IDs.
    #[test]
    fn old_pins_load_as_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, r#"{"version":1,"pinned":["a","b"]}"#).unwrap();
        assert_eq!(AppState::load(&p).pinned, [Pin::Session("a".into()), Pin::Session("b".into())]);
    }

    #[test]
    fn broken_file_gives_default() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, "{broken").unwrap();
        assert_eq!(AppState::load(&p), AppState::default());
    }
}
