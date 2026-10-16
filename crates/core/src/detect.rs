//! Finds a `claude` process running inside a terminal tab, and the session
//! file it writes to. Used by "New terminal" tabs, where the user starts
//! `claude` by hand, so the app does not know the session ID in advance.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeProc {
    pub pid: u32,
    pub cwd: Option<PathBuf>,
    pub started: SystemTime,
}

pub struct ProcScanner {
    sys: System,
}

impl Default for ProcScanner {
    fn default() -> Self {
        Self { sys: System::new() }
    }
}

/// True if a process looks like Claude Code (native binary or npm install).
pub fn looks_like_claude(name: &str, exe: Option<&Path>, cmd: &[String]) -> bool {
    let base = |s: &str| {
        let s = s.rsplit(['/', '\\']).next().unwrap_or(s).to_ascii_lowercase();
        s.strip_suffix(".exe").map(str::to_string).unwrap_or(s)
    };
    if base(name) == "claude" {
        return true;
    }
    if cmd.first().is_some_and(|a| base(a) == "claude") {
        return true;
    }
    // Native installs live at .../claude/versions/<version>.
    if exe.is_some_and(|e| e.to_string_lossy().replace('\\', "/").contains("/claude/versions/")) {
        return true;
    }
    cmd.iter().any(|a| a.contains("@anthropic-ai/claude-code"))
}

impl ProcScanner {
    pub fn refresh(&mut self) {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cwd(UpdateKind::Always)
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );
    }

    /// True while a process runs. A finished process that was not reaped
    /// yet (a zombie) counts as stopped: it can no longer write files.
    pub fn is_running(&mut self, pid: u32) -> bool {
        let pid = Pid::from_u32(pid);
        self.sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::nothing());
        self.sys.process(pid).is_some_and(|p| !matches!(p.status(), sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead))
    }

    /// Current working directory of a process, if the OS lets us read it.
    pub fn cwd_of(&self, pid: u32) -> Option<PathBuf> {
        self.sys.process(Pid::from_u32(pid))?.cwd().map(Path::to_path_buf)
    }

    /// Breadth-first search below `root` for the first Claude Code process.
    pub fn find_claude_under(&self, root: u32) -> Option<ClaudeProc> {
        let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
        for (pid, p) in self.sys.processes() {
            if let Some(parent) = p.parent() {
                children.entry(parent).or_default().push(*pid);
            }
        }
        let mut queue: VecDeque<Pid> = children.get(&Pid::from_u32(root)).cloned().unwrap_or_default().into();
        while let Some(pid) = queue.pop_front() {
            let Some(p) = self.sys.process(pid) else { continue };
            let cmd: Vec<String> = p.cmd().iter().map(|s| s.to_string_lossy().into_owned()).collect();
            if looks_like_claude(&p.name().to_string_lossy(), p.exe(), &cmd) {
                return Some(ClaudeProc {
                    pid: pid.as_u32(),
                    cwd: p.cwd().map(Path::to_path_buf),
                    started: UNIX_EPOCH + Duration::from_secs(p.start_time()),
                });
            }
            if let Some(c) = children.get(&pid) {
                queue.extend(c.iter().copied());
            }
        }
        None
    }
}

/// The session a Claude process works on, as Claude Code itself says.
/// While it runs, Claude keeps `<config>/sessions/<pid>.json` up to date with
/// its session ID, also after /clear or /resume. The process may run with
/// any account, so every config folder is checked. A file left by an older
/// process with the same PID (after a crash) is ignored.
pub fn session_of(config_dirs: &[PathBuf], proc_: &ClaudeProc) -> Option<String> {
    let started_ms = proc_.started.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
    config_dirs.iter().find_map(|d| {
        let v: serde_json::Value = serde_json::from_slice(&fs::read(d.join(format!("sessions/{}.json", proc_.pid))).ok()?).ok()?;
        if v.get("pid")?.as_u64()? != u64::from(proc_.pid) {
            return None;
        }
        // The process start time has 1 second precision; Claude writes its
        // own start a little later.
        if v.get("startedAt").and_then(|s| s.as_u64()).is_some_and(|s| s + 2000 < started_ms) {
            return None;
        }
        Some(v.get("sessionId")?.as_str()?.to_string())
    })
}

/// Guess the session file that a Claude process (started at `started` in `cwd`)
/// is writing to: the most recently changed `.jsonl` in its project folder that
/// changed after the process started and is not already taken by another tab.
/// Only for Claude versions that do not write `sessions/<pid>.json` (see
/// [`session_of`]): a file that another Claude in the same folder writes
/// can be picked by mistake.
/// The process may run with any account, so every projects folder is checked.
pub fn match_session(projects_dirs: &[PathBuf], cwd: &Path, started: SystemTime, taken: &HashSet<String>) -> Option<String> {
    let folder = crate::paths::project_folder_name(cwd);
    // start_time has 1 second precision.
    let since = started.checked_sub(Duration::from_secs(2)).unwrap_or(started);
    projects_dirs
        .iter()
        .filter_map(|d| fs::read_dir(d.join(&folder)).ok())
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()?.to_str()? != "jsonl" {
                return None;
            }
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let mtime = e.metadata().ok()?.modified().ok()?;
            (mtime >= since && !taken.contains(&id)).then_some((mtime, id))
        })
        .max()
        .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_claude_processes() {
        assert!(looks_like_claude("claude", None, &[]));
        assert!(looks_like_claude("claude.exe", None, &[]));
        assert!(looks_like_claude("2.1.296", Some(Path::new("/Users/me/.local/share/claude/versions/2.1.296")), &[]));
        assert!(looks_like_claude("node", None, &["node".into(), "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js".into()]));
        assert!(!looks_like_claude("zsh", Some(Path::new("/bin/zsh")), &["-zsh".into()]));
        assert!(!looks_like_claude("Claude Helper", None, &[]));
    }

    #[test]
    fn matches_newest_untaken_session_after_start() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = Path::new("/work/app");
        let proj = dir.path().join(crate::paths::project_folder_name(cwd));
        fs::create_dir_all(&proj).unwrap();
        let now = SystemTime::now();
        let set = |name: &str, t: SystemTime| {
            let f = proj.join(format!("{name}.jsonl"));
            fs::write(&f, "{}\n").unwrap();
            fs::File::options().write(true).open(&f).unwrap().set_modified(t).unwrap();
        };
        set("old", now - Duration::from_secs(3600));
        set("a", now - Duration::from_secs(1));
        set("b", now);

        let start = now - Duration::from_secs(10);
        assert_eq!(match_session(&[dir.path().to_path_buf()], cwd, start, &HashSet::new()).as_deref(), Some("b"));
        let taken = HashSet::from(["b".to_string()]);
        assert_eq!(match_session(&[dir.path().to_path_buf()], cwd, start, &taken).as_deref(), Some("a"));
        // Nothing written yet after a later start.
        assert_eq!(match_session(&[dir.path().to_path_buf()], cwd, now + Duration::from_secs(60), &HashSet::new()), None);
    }

    #[test]
    fn session_of_reads_claudes_pid_file() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let dirs = [a.path().to_path_buf(), b.path().to_path_buf()];
        let started = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let proc_ = ClaudeProc { pid: 42, cwd: None, started };
        assert_eq!(session_of(&dirs, &proc_), None);

        // Written by a process that had the same PID before: ignored.
        fs::create_dir_all(b.path().join("sessions")).unwrap();
        let write = |sid: &str, started_ms: u64| {
            fs::write(b.path().join("sessions/42.json"), format!(r#"{{"pid":42,"sessionId":"{sid}","startedAt":{started_ms}}}"#)).unwrap();
        };
        write("old", 900_000_000);
        assert_eq!(session_of(&dirs, &proc_), None);

        // Written by this process, in the second account's folder.
        write("mine", 1_000_000_500);
        assert_eq!(session_of(&dirs, &proc_).as_deref(), Some("mine"));
    }

    #[test]
    fn is_running_follows_a_real_process() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let mut scanner = ProcScanner::default();
        assert!(scanner.is_running(child.id()));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!scanner.is_running(child.id()));
    }

    /// Real process tree: a shell that runs a program named `claude`.
    #[cfg(unix)]
    #[test]
    fn finds_claude_child_in_real_process_tree() {
        // A symlink named `claude` to a real binary, like the native install.
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/bin/sleep", dir.path().join("claude")).unwrap();

        let mut shell = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("cd {} && ./claude 30; true", dir.path().display()))
            .spawn()
            .unwrap();
        let mut scanner = ProcScanner::default();
        let mut found = None;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            scanner.refresh();
            found = scanner.find_claude_under(shell.id());
            if found.is_some() {
                break;
            }
        }
        let _ = shell.kill();
        let found = found.expect("claude child not found");
        let cwd = fs::canonicalize(found.cwd.unwrap()).unwrap();
        assert_eq!(cwd, fs::canonicalize(dir.path()).unwrap());
    }
}
