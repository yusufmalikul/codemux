//! App-owned state (spec 10). Saved atomically: write a temp file, then rename.

use serde::{Deserialize, Serialize};
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
    /// Privacy: hide all session rows in the sidebar.
    #[serde(default)]
    pub hide_sessions: bool,
    /// Sidebar width in logical pixels. None means the default width.
    #[serde(default)]
    pub sidebar_width: Option<f32>,
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
        let s = AppState { version: 0, open_sessions: vec!["a".into(), "b".into()], hide_sessions: true, sidebar_width: Some(420.0) };
        s.save(&p).unwrap();
        let loaded = AppState::load(&p);
        assert_eq!(loaded.open_sessions, s.open_sessions);
        assert!(loaded.hide_sessions);
        assert_eq!(loaded.sidebar_width, Some(420.0));
        assert_eq!(loaded.version, STATE_VERSION);
        assert!(!p.with_extension("tmp").exists());
    }

    #[test]
    fn broken_file_gives_default() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, "{broken").unwrap();
        assert_eq!(AppState::load(&p), AppState::default());
    }
}
