//! Moving a session between Claude accounts (spec 11.7).
//!
//! Each account is a Claude Code config folder (`CLAUDE_CONFIG_DIR`). A
//! session lives in exactly one of them. To continue it with another account,
//! its files move to that account's folder, then `claude --resume` runs there.
//!
//! The file format is not an official API, so this moves every known file
//! that carries the session ID, and nothing else:
//! - `projects/<project>/<id>.jsonl`: the conversation
//! - `projects/<project>/<id>/`: subagent transcripts and large tool output
//! - `file-history/<id>/`: file backups for `/rewind`

use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Suffix of a half-done copy. Never a real Claude file name.
const TMP_SUFFIX: &str = ".codemux-tmp";

/// Move a session from one config folder to another. Returns the new
/// `.jsonl` path.
///
/// The session must not be running. Steps, so a failure leaves the session
/// usable where it was:
/// 1. Copy every file to the target under a temporary name. On error, remove
///    the copies and stop: nothing has changed.
/// 2. Put the copies in place. A stale copy already in the target (left by an
///    older move) goes to the backup first.
/// 3. Move the originals to the backup folder, so the session is in one place.
pub fn move_session(from_config: &Path, to_config: &Path, jsonl: &Path, backup_root: &Path) -> io::Result<PathBuf> {
    let refuse = |why: &str| Err(Error::new(ErrorKind::InvalidInput, why.to_string()));
    let from_root = fs::canonicalize(from_config.join("projects"))?;
    let file = fs::canonicalize(jsonl)?;
    if file.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return refuse("not a session file");
    }
    let project_dir = file.parent().ok_or_else(|| Error::other("no parent"))?;
    if project_dir.parent() != Some(from_root.as_path()) {
        return refuse("not inside the account's projects folder");
    }
    let id = file.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
    if !crate::launch::is_safe_id(&id) {
        return refuse("bad session id");
    }
    if same_dir(from_config, to_config) {
        return refuse("the session is already in this account");
    }
    let project = project_dir.file_name().ok_or_else(|| Error::other("no project folder"))?;

    let rel: [PathBuf; 3] = [
        Path::new("projects").join(project).join(format!("{id}.jsonl")),
        Path::new("projects").join(project).join(&id),
        Path::new("file-history").join(&id),
    ];
    let items: Vec<&PathBuf> = rel.iter().filter(|r| from_config.join(r).exists()).collect();

    // 1. Copy.
    let mut copied = Vec::new();
    for r in &items {
        let tmp = with_suffix(&to_config.join(r), TMP_SUFFIX);
        let result = remove_any(&tmp).and_then(|_| copy_any(&from_config.join(r), &tmp));
        copied.push(tmp);
        if let Err(e) = result {
            for t in &copied {
                let _ = remove_any(t);
            }
            return Err(e);
        }
    }

    // 2. Put in place.
    let backup = backup_root.join(format!("{}-{id}", unix_secs()));
    for (r, tmp) in items.iter().zip(&copied) {
        let dest = to_config.join(r);
        if dest.exists() {
            move_any(&dest, &backup.join("replaced").join(r))?;
        }
        fs::rename(tmp, &dest)?;
    }

    // 3. Move the originals out.
    for r in &items {
        move_any(&from_config.join(r), &backup.join("moved").join(r))?;
    }
    Ok(to_config.join(&rel[0]))
}

/// Config folders shared by every account: each extra account gets a symlink
/// to the default account's copy, so settings, skills, agents, commands,
/// rules, output styles and plugins are the same everywhere. Logins,
/// `.claude.json` and sessions stay per account. A missing folder is first
/// made in the shared config, so an account never grows its own copy.
/// Plugins and skills that claude.ai syncs live under a per-account ID in
/// `plugins/synced/` and `skills/synced/`, so sharing these folders is safe.
pub const SHARED_DIRS: &[&str] = &["skills", "agents", "commands", "rules", "output-styles", "plugins"];

/// Config files shared like [`SHARED_DIRS`]. A file the shared config does not
/// have is skipped. `CLAUDE.md` is not here: Claude Code ignores a user
/// `CLAUDE.md` that is a symlink or a hard link (checked in v2.1.300).
pub const SHARED_FILES: &[&str] = &["settings.json", "keybindings.json"];

/// Link the shared config entries of `shared` into the account folder
/// `account` (spec 11.7). An entry the account already has (a real file or
/// folder, or a link to somewhere else) is never changed: it may be the
/// user's own setup. Returns the names that are not shared because of that,
/// or because the link failed.
pub fn share_config(shared: &Path, account: &Path) -> Vec<&'static str> {
    if !shared.is_dir() || same_dir(shared, account) {
        return Vec::new();
    }
    let dirs = SHARED_DIRS.iter().map(|n| (*n, true));
    let files = SHARED_FILES.iter().map(|n| (*n, false));
    let mut unshared = Vec::new();
    for (name, is_dir) in dirs.chain(files) {
        let (target, link) = (shared.join(name), account.join(name));
        match fs::symlink_metadata(&link) {
            Ok(m) if m.file_type().is_symlink() && fs::read_link(&link).is_ok_and(|t| t == target) => continue,
            Ok(_) => {
                unshared.push(name);
                continue;
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(_) => {
                unshared.push(name);
                continue;
            }
        }
        let ready = if is_dir { fs::create_dir_all(&target).is_ok() } else { target.is_file() };
        if !ready {
            continue;
        }
        if fs::create_dir_all(account).and_then(|_| symlink(&target, &link, is_dir)).is_err() {
            unshared.push(name);
        }
    }
    unshared
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path, _is_dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows may need Developer Mode for this; a failure leaves the entry
/// unshared and the account still works.
#[cfg(windows)]
fn symlink(target: &Path, link: &Path, is_dir: bool) -> io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// PIDs that Claude Code says are running this session. Claude writes
/// `<config>/sessions/<pid>.json` with the session ID while it runs. A file
/// can outlive a crash, so the caller must check that the PID still runs.
pub fn claimed_by(config: &Path, sid: &str) -> Vec<u32> {
    let Ok(entries) = fs::read_dir(config.join("sessions")) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let v: serde_json::Value = serde_json::from_slice(&fs::read(e.path()).ok()?).ok()?;
            (v.get("sessionId")?.as_str()? == sid).then(|| v.get("pid")?.as_u64()).flatten().map(|p| p as u32)
        })
        .collect()
}

/// Remove backups older than `max_age`. Each move makes one folder.
pub fn prune_backups(backup_root: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(backup_root) else { return };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > max_age);
        if old {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn unix_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn remove_any(p: &Path) -> io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Copy a file or a whole folder. Symlinks are skipped: Claude does not
/// make them, and following one could copy data from outside the session.
fn copy_any(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    let m = fs::symlink_metadata(from)?;
    if m.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            copy_any(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else if m.is_file() {
        fs::copy(from, to).map(|_| ())
    } else {
        Ok(())
    }
}

/// Rename, or copy and remove when the folders are on different disks.
fn move_any(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_any(from, to)?;
    remove_any(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "362edec9-a4ee-4c27-8918-37423684510b";

    struct Setup {
        _tmp: tempfile::TempDir,
        a: PathBuf,
        b: PathBuf,
        backups: PathBuf,
    }

    fn setup() -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("claude-a");
        let b = tmp.path().join("claude-b");
        let proj = a.join("projects/-work-app");
        fs::create_dir_all(proj.join(ID).join("subagents")).unwrap();
        fs::write(proj.join(format!("{ID}.jsonl")), "line1\nline2\n").unwrap();
        fs::write(proj.join(ID).join("subagents/agent-1.jsonl"), "sub\n").unwrap();
        fs::write(proj.join("other.jsonl"), "keep\n").unwrap();
        fs::create_dir_all(a.join("file-history").join(ID)).unwrap();
        fs::write(a.join("file-history").join(ID).join("abc@v1"), "old file\n").unwrap();
        fs::create_dir_all(b.join("projects")).unwrap();
        let backups = tmp.path().join("backups");
        Setup { a, b, backups, _tmp: tmp }
    }

    fn jsonl(config: &Path) -> PathBuf {
        config.join(format!("projects/-work-app/{ID}.jsonl"))
    }

    #[test]
    fn moves_all_session_files_and_leaves_others() {
        let s = setup();
        let new = move_session(&s.a, &s.b, &jsonl(&s.a), &s.backups).unwrap();
        assert_eq!(new, jsonl(&s.b));
        assert_eq!(fs::read_to_string(jsonl(&s.b)).unwrap(), "line1\nline2\n");
        assert_eq!(fs::read_to_string(s.b.join(format!("projects/-work-app/{ID}/subagents/agent-1.jsonl"))).unwrap(), "sub\n");
        assert_eq!(fs::read_to_string(s.b.join(format!("file-history/{ID}/abc@v1"))).unwrap(), "old file\n");

        assert!(!jsonl(&s.a).exists(), "session still in the old account");
        assert!(!s.a.join(format!("projects/-work-app/{ID}")).exists());
        assert!(!s.a.join(format!("file-history/{ID}")).exists());
        assert!(s.a.join("projects/-work-app/other.jsonl").exists(), "other session touched");
        assert!(!s.b.join("projects/-work-app/other.jsonl").exists(), "other session copied");

        // The originals are kept in the backup folder.
        let backup = fs::read_dir(&s.backups).unwrap().next().unwrap().unwrap().path();
        assert!(backup.join(format!("moved/projects/-work-app/{ID}.jsonl")).exists());
        // No temporary copies are left behind.
        let leftovers = fs::read_dir(s.b.join("projects/-work-app")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(TMP_SUFFIX)).count();
        assert_eq!(leftovers, 0);
    }

    /// A → B, work in B, then B → A: A gets B's newer conversation, and the
    /// stale copy is never used.
    #[test]
    fn round_trip_keeps_the_newest_conversation() {
        let s = setup();
        move_session(&s.a, &s.b, &jsonl(&s.a), &s.backups).unwrap();
        fs::OpenOptions::new().append(true).open(jsonl(&s.b)).and_then(|mut f| io::Write::write_all(&mut f, b"line3 from b\n")).unwrap();
        // A stale copy in A (for example made by hand) is replaced, not kept.
        fs::write(jsonl(&s.a), "stale\n").unwrap();

        move_session(&s.b, &s.a, &jsonl(&s.b), &s.backups).unwrap();
        assert_eq!(fs::read_to_string(jsonl(&s.a)).unwrap(), "line1\nline2\nline3 from b\n");
        assert!(!jsonl(&s.b).exists());
    }

    #[test]
    fn missing_optional_folders_are_fine() {
        let s = setup();
        fs::remove_dir_all(s.a.join("file-history")).unwrap();
        fs::remove_dir_all(s.a.join(format!("projects/-work-app/{ID}"))).unwrap();
        move_session(&s.a, &s.b, &jsonl(&s.a), &s.backups).unwrap();
        assert!(jsonl(&s.b).exists());
        assert!(!s.b.join("file-history").exists());
    }

    #[test]
    fn refuses_bad_paths_and_same_account() {
        let s = setup();
        assert!(move_session(&s.a, &s.a, &jsonl(&s.a), &s.backups).is_err());
        let not_session = s.a.join("projects/-work-app/notes.txt");
        fs::write(&not_session, "x").unwrap();
        assert!(move_session(&s.a, &s.b, &not_session, &s.backups).is_err());
        // A file of another account is refused.
        assert!(move_session(&s.b, &s.a, &jsonl(&s.a), &s.backups).is_err());
        assert!(jsonl(&s.a).exists());
    }

    /// If a copy fails, nothing changes: the session stays in the old account.
    #[cfg(unix)]
    #[test]
    fn failed_copy_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let s = setup();
        let target = s.b.join("projects/-work-app");
        fs::create_dir_all(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o500)).unwrap();
        let result = move_session(&s.a, &s.b, &jsonl(&s.a), &s.backups);
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert!(jsonl(&s.a).exists());
        assert!(s.a.join(format!("file-history/{ID}/abc@v1")).exists());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
    }

    /// Shared entries become links to the default config. A folder the default
    /// config lacks is made there first; a missing file is skipped.
    #[cfg(unix)]
    #[test]
    fn share_config_links_shared_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let (shared, account) = (tmp.path().join("claude"), tmp.path().join("claude-pro"));
        fs::create_dir_all(shared.join("skills/my-skill")).unwrap();
        fs::write(shared.join("settings.json"), "{}").unwrap();
        fs::write(shared.join("CLAUDE.md"), "rules").unwrap();

        assert!(share_config(&shared, &account).is_empty());
        for name in SHARED_DIRS.iter().chain(&["settings.json"]) {
            assert_eq!(fs::read_link(account.join(name)).unwrap(), shared.join(name), "{name} not linked");
        }
        assert!(shared.join("agents").is_dir(), "missing shared folder not made");
        assert!(account.join("skills/my-skill").is_dir());
        assert!(!account.join("keybindings.json").exists(), "link to a missing file");
        assert!(!account.join("CLAUDE.md").exists(), "Claude Code ignores a linked CLAUDE.md");

        // A write in the account lands in the shared config.
        fs::write(account.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(fs::read_to_string(shared.join("settings.json")).unwrap(), r#"{"theme":"dark"}"#);

        // Running again changes nothing and picks up a file added later.
        fs::write(shared.join("keybindings.json"), "{}").unwrap();
        assert!(share_config(&shared, &account).is_empty());
        assert_eq!(fs::read_link(account.join("keybindings.json")).unwrap(), shared.join("keybindings.json"));
    }

    /// The account's own entries, and links made by the user, are kept.
    #[cfg(unix)]
    #[test]
    fn share_config_keeps_existing_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let (shared, account) = (tmp.path().join("claude"), tmp.path().join("claude-pro"));
        fs::create_dir_all(shared.join("skills")).unwrap();
        fs::write(shared.join("settings.json"), "shared").unwrap();
        fs::create_dir_all(account.join("skills/own")).unwrap();
        fs::write(account.join("settings.json"), "own").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("dotfiles-agents"), account.join("agents")).unwrap();

        let unshared = share_config(&shared, &account);
        assert_eq!(unshared, vec!["skills", "agents", "settings.json"]);
        assert_eq!(fs::read_to_string(account.join("settings.json")).unwrap(), "own");
        assert!(account.join("skills/own").is_dir());
        assert_eq!(fs::read_link(account.join("agents")).unwrap(), tmp.path().join("dotfiles-agents"));
        assert_eq!(fs::read_link(account.join("commands")).unwrap(), shared.join("commands"));
    }

    #[test]
    fn share_config_never_links_a_folder_to_itself() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("skills")).unwrap();
        assert!(share_config(tmp.path(), tmp.path()).is_empty());
        assert!(tmp.path().join("skills").is_dir());
        assert!(!fs::symlink_metadata(tmp.path().join("skills")).unwrap().file_type().is_symlink());
        assert!(share_config(&tmp.path().join("missing"), &tmp.path().join("pro")).is_empty());
        assert!(!tmp.path().join("pro").exists(), "made an account folder without a shared config");
    }

    #[test]
    fn claimed_by_reads_claude_session_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sessions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("101.json"), format!(r#"{{"pid":101,"sessionId":"{ID}","cwd":"/x"}}"#)).unwrap();
        fs::write(dir.join("102.json"), r#"{"pid":102,"sessionId":"other"}"#).unwrap();
        fs::write(dir.join("broken.json"), "{").unwrap();
        assert_eq!(claimed_by(tmp.path(), ID), vec![101]);
        assert!(claimed_by(&tmp.path().join("missing"), ID).is_empty());
    }

    #[test]
    fn prune_removes_only_old_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("1-old");
        let new = tmp.path().join("2-new");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        let past = SystemTime::now() - Duration::from_secs(10 * 86_400);
        fs::File::open(&old).unwrap().set_modified(past).unwrap();
        prune_backups(tmp.path(), Duration::from_secs(7 * 86_400));
        assert!(!old.exists());
        assert!(new.exists());
    }
}
