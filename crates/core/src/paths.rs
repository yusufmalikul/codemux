use std::path::{Path, PathBuf};

/// The Claude Code config folder: `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
pub fn claude_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    directories::BaseDirs::new().map(|b| b.home_dir().join(".claude"))
}

/// The folder that holds one sub-folder of `.jsonl` files per project.
pub fn projects_dir() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("projects"))
}

/// Claude Code names a project folder after its working directory,
/// with every character that is not a letter or digit replaced by `-`.
/// Example: `/Users/me/dev/app` -> `-Users-me-dev-app`.
pub fn project_folder_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Folder for app-owned data (state.json and friends).
pub fn app_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "codemux").map(|p| p.data_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name_replaces_non_alphanumeric() {
        assert_eq!(project_folder_name(Path::new("/Users/me/dev/my_app.v2")), "-Users-me-dev-my-app-v2");
    }
}
