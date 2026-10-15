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

/// Suggested config folder for a new account: `<home>/.claude-<name>`, with
/// the name in lower case and every other character turned into `-`.
pub fn account_dir_for(home: &Path, name: &str) -> Option<PathBuf> {
    let slug: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        return None;
    }
    Some(home.join(format!(".claude-{slug}")))
}

pub fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
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

    #[test]
    fn account_dir_uses_a_clean_name() {
        assert_eq!(account_dir_for(Path::new("/h"), "  My Pro!! ").unwrap(), Path::new("/h/.claude-my-pro"));
        assert!(account_dir_for(Path::new("/h"), " !! ").is_none());
    }
}
