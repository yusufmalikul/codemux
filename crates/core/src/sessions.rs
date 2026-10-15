//! Read-only parser and index for Claude Code session files
//! (`<projects>/<folder>/<session-id>.jsonl`).
//!
//! The format is not an official API. Unknown or broken lines are skipped.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const TITLE_MAX: usize = 80;
/// How many recent messages (prompts and replies) each session keeps for the
/// detail view.
pub const RECENT_MESSAGES: usize = 10;
/// Longest stored recent message, in characters. Pastes and replies can be huge.
const RECENT_MESSAGE_MAX: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// One message in the conversation: a typed prompt or Claude's reply text.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionMeta {
    pub id: String,
    pub path: PathBuf,
    /// The account whose config folder holds the file. 0 is the default.
    pub account: u64,
    pub cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub ai_title: Option<String>,
    pub agent_name: Option<String>,
    pub first_prompt: Option<String>,
    pub last_prompt: Option<String>,
    /// Last few typed prompts and Claude replies, oldest first (at most
    /// `RECENT_MESSAGES`).
    pub recent_messages: Vec<ChatMessage>,
    pub last_activity: Option<DateTime<Utc>>,
    /// File modified time. Used when no timestamp is found.
    pub modified: Option<SystemTime>,
}

impl SessionMeta {
    /// Title order from the spec (8.3), without the local name (the app adds that).
    pub fn title(&self) -> String {
        self.agent_name
            .clone()
            .or_else(|| self.ai_title.clone())
            .or_else(|| self.first_prompt.as_deref().map(truncate))
            .unwrap_or_else(|| "(untitled)".to_string())
    }

    pub fn folder_name(&self) -> String {
        self.cwd
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Best time to sort by: last timestamp in the file, else file mtime.
    pub fn sort_time(&self) -> DateTime<Utc> {
        self.last_activity
            .or_else(|| self.modified.map(DateTime::<Utc>::from))
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
    }
}

fn truncate(s: &str) -> String {
    // Only look at the start. A first prompt can be a very long paste.
    let head: String = s.chars().take(TITLE_MAX * 4).collect();
    let one_line = head.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > TITLE_MAX {
        let cut: String = one_line.chars().take(TITLE_MAX).collect();
        format!("{cut}…")
    } else {
        one_line
    }
}

/// Text of a user message, or None if it is not real typed text.
fn user_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let text = text.trim();
    // Skip command wrappers and system-injected text.
    if text.is_empty() || text.starts_with('<') {
        return None;
    }
    Some(text.to_string())
}

/// Reply text of an assistant line. Thinking and tool calls are left out.
fn assistant_text(message: &Value) -> Option<String> {
    let parts = message.get("content")?.as_array()?;
    let text = parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn cut_long(text: String) -> String {
    if text.chars().count() > RECENT_MESSAGE_MAX {
        let cut: String = text.chars().take(RECENT_MESSAGE_MAX).collect();
        format!("{cut}…")
    } else {
        text
    }
}

/// Add a message. Claude writes one reply as many lines (split by tool
/// calls), so reply text right after reply text joins the same message.
fn push_recent(list: &mut Vec<ChatMessage>, role: Role, text: String) {
    if role == Role::Assistant {
        if let Some(last) = list.last_mut().filter(|m| m.role == Role::Assistant) {
            if !last.text.ends_with('…') {
                last.text = cut_long(format!("{}\n\n{text}", last.text));
            }
            return;
        }
    }
    list.push(ChatMessage { role, text: cut_long(text) });
    if list.len() > RECENT_MESSAGES {
        list.remove(0);
    }
}

/// Apply one JSON line to the session meta.
fn apply_line(meta: &mut SessionMeta, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let str_field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);

    match v.get("type").and_then(Value::as_str) {
        Some("ai-title") => {
            if let Some(t) = str_field("aiTitle").filter(|t| !t.trim().is_empty()) {
                meta.ai_title = Some(t);
            }
        }
        Some("agent-name") => {
            if let Some(t) = str_field("agentName").filter(|t| !t.trim().is_empty()) {
                meta.agent_name = Some(t);
            }
        }
        Some("last-prompt") => {
            if let Some(t) = str_field("lastPrompt") {
                meta.last_prompt = Some(t);
            }
        }
        Some(kind @ ("user" | "assistant")) => {
            if let Some(cwd) = str_field("cwd") {
                meta.cwd = Some(PathBuf::from(cwd));
            }
            if let Some(b) = str_field("gitBranch").filter(|b| !b.is_empty()) {
                meta.git_branch = Some(b);
            }
            if let Some(ts) = str_field("timestamp").and_then(|t| t.parse::<DateTime<Utc>>().ok()) {
                meta.last_activity = Some(meta.last_activity.map_or(ts, |old| old.max(ts)));
            }
            let is_meta = v.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
            let is_side = v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false);
            if kind == "user" && !is_meta && !is_side {
                if let Some(t) = v.get("message").and_then(user_text) {
                    if meta.first_prompt.is_none() {
                        meta.first_prompt = Some(t.clone());
                    }
                    push_recent(&mut meta.recent_messages, Role::User, t);
                }
            }
            if kind == "assistant" && !is_side {
                if let Some(t) = v.get("message").and_then(assistant_text) {
                    push_recent(&mut meta.recent_messages, Role::Assistant, t);
                }
            }
        }
        _ => {}
    }
}

/// Parse a whole session file.
pub fn parse_file(path: &Path) -> Option<SessionMeta> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let text = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&text);
    let mut meta = SessionMeta { id, path: path.to_path_buf(), ..Default::default() };
    for line in text.lines() {
        apply_line(&mut meta, line);
    }
    meta.modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    Some(meta)
}

/// Delete a session file and its sub-folder (subagent transcripts).
/// This is the only write the app ever does in the projects folder, and only
/// when the user asks for it. Refuses anything that is not a top-level
/// `<project>/<id>.jsonl` inside `projects_dir`.
pub fn delete_session(projects_dir: &Path, path: &Path) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    let refuse = |why: &str| Err(Error::new(ErrorKind::InvalidInput, why.to_string()));

    let root = fs::canonicalize(projects_dir)?;
    let file = fs::canonicalize(path)?;
    if file.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return refuse("not a session file");
    }
    // Must be exactly <root>/<project>/<id>.jsonl
    let project = file.parent().ok_or_else(|| Error::other("no parent"))?;
    if project.parent() != Some(root.as_path()) {
        return refuse("not inside the Claude projects folder");
    }
    let Some(stem) = file.file_stem() else { return refuse("no session id") };

    fs::remove_file(&file)?;
    let sub = project.join(stem);
    if sub.is_dir() {
        fs::remove_dir_all(sub)?;
    }
    Ok(())
}

/// Incremental index. Reads only lines added since the last scan.
///
/// It reads the projects folder of every account. A session should be in
/// one account only, but a copy made by hand can put the same ID in two.
/// Then only the most recently changed file counts.
#[derive(Default)]
pub struct Index {
    entries: HashMap<PathBuf, Entry>,
    by_id: HashMap<String, PathBuf>,
}

/// One account's projects folder to scan.
#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub account: u64,
    pub projects_dir: PathBuf,
}

struct Entry {
    offset: u64,
    meta: SessionMeta,
    /// The session's folder still exists. A session whose folder was deleted
    /// cannot be resumed, so it is not listed.
    cwd_exists: bool,
}

impl Entry {
    fn listed(&self) -> bool {
        self.meta.cwd.is_some() && self.cwd_exists
    }
}

impl Index {
    /// Scan the project folders of every account. Returns true if anything changed.
    pub fn refresh(&mut self, roots: &[Root]) -> bool {
        let mut changed = false;
        let mut seen = std::collections::HashSet::new();

        for root in roots {
            let Ok(projects) = fs::read_dir(&root.projects_dir) else { continue };
            for project in projects.flatten() {
                let Ok(files) = fs::read_dir(project.path()) else { continue };
                // Only top-level files. Subagent (sidechain) files live in sub-folders.
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    seen.insert(path.clone());
                    if self.update_file(&path, root.account) {
                        changed = true;
                    }
                }
            }
        }

        let before = self.entries.len();
        self.entries.retain(|p, _| seen.contains(p));
        if changed || before != self.entries.len() {
            self.rebuild_by_id();
        }

        // Folders can be deleted at any time, so check them on every scan.
        // Many sessions share a folder: check each folder once.
        let mut dirs: HashMap<PathBuf, bool> = HashMap::new();
        for entry in self.entries.values_mut() {
            let exists = match &entry.meta.cwd {
                Some(cwd) => *dirs.entry(cwd.clone()).or_insert_with(|| cwd.is_dir()),
                None => false,
            };
            if exists != entry.cwd_exists {
                entry.cwd_exists = exists;
                changed = true;
            }
        }
        changed || before != self.entries.len()
    }

    /// Point each ID at its newest file, so a stale copy is never used.
    fn rebuild_by_id(&mut self) {
        self.by_id.clear();
        for (path, e) in &self.entries {
            let newer = match self.by_id.get(&e.meta.id).and_then(|p| self.entries.get(p)) {
                Some(old) => e.meta.modified > old.meta.modified,
                None => true,
            };
            if newer {
                self.by_id.insert(e.meta.id.clone(), path.clone());
            }
        }
    }

    /// True if this entry is the one its ID points to.
    fn is_current(&self, path: &Path, e: &Entry) -> bool {
        self.by_id.get(&e.meta.id).is_some_and(|p| p == path)
    }

    fn update_file(&mut self, path: &Path, account: u64) -> bool {
        use std::io::{Read, Seek, SeekFrom};

        let Ok(md) = fs::metadata(path) else { return false };
        let len = md.len();
        let fresh = |id: String| SessionMeta { id, path: path.to_path_buf(), account, ..Default::default() };
        let entry = self.entries.entry(path.to_path_buf()).or_insert_with(|| Entry {
            offset: 0,
            cwd_exists: false,
            meta: fresh(path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
        });
        if len == entry.offset {
            return false;
        }
        if len < entry.offset {
            // File was replaced or cut. Read it again from the start.
            entry.offset = 0;
            entry.meta = fresh(entry.meta.id.clone());
        }

        let Ok(mut f) = fs::File::open(path) else { return false };
        if f.seek(SeekFrom::Start(entry.offset)).is_err() {
            return false;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return false;
        }
        // Only consume complete lines. A half-written last line is read next time.
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return false };
        let chunk = String::from_utf8_lossy(&buf[..=end]);
        for line in chunk.lines() {
            apply_line(&mut entry.meta, line);
        }
        entry.offset += end as u64 + 1;
        entry.meta.modified = md.modified().ok();
        true
    }

    pub fn get(&self, id: &str) -> Option<&SessionMeta> {
        self.by_id.get(id).and_then(|p| self.entries.get(p)).map(|e| &e.meta)
    }

    /// Like `get`, but only for a session that `sessions()` lists.
    pub fn listed(&self, id: &str) -> Option<&SessionMeta> {
        self.by_id.get(id).and_then(|p| self.entries.get(p)).filter(|e| e.listed()).map(|e| &e.meta)
    }

    /// Number of sessions that `sessions()` returns.
    pub fn count(&self) -> usize {
        self.entries.iter().filter(|(p, e)| e.listed() && self.is_current(p, e)).count()
    }

    /// Sessions with at least one real message whose folder still exists,
    /// newest first.
    pub fn sessions(&self) -> Vec<&SessionMeta> {
        let mut list: Vec<_> =
            self.entries.iter().filter(|(p, e)| e.listed() && self.is_current(p, e)).map(|(_, e)| &e.meta).collect();
        list.sort_by(|a, b| b.sort_time().cmp(&a.sort_time()));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const FIXTURE: &str = r#"{"type":"user","sessionId":"s1","cwd":"/tmp/proj","gitBranch":"main","timestamp":"2026-10-13T08:00:00Z","isSidechain":false,"message":{"role":"user","content":"<command-name>/clear</command-name>"}}
{"type":"user","sessionId":"s1","cwd":"/tmp/proj","gitBranch":"main","timestamp":"2026-10-13T08:01:00Z","isSidechain":false,"message":{"role":"user","content":[{"type":"text","text":"fix the   webhook retry"}]}}
not json at all
{"type":"something-new","foo":1}
{"type":"assistant","sessionId":"s1","cwd":"/tmp/proj","gitBranch":"fix/webhook","timestamp":"2026-10-13T08:02:00Z","message":{}}
{"type":"last-prompt","lastPrompt":"and add a test","sessionId":"s1"}
"#;

    fn roots(projects_dir: &Path) -> Vec<Root> {
        vec![Root { account: 0, projects_dir: projects_dir.to_path_buf() }]
    }

    fn write_fixture(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("proj");
        fs::create_dir_all(&p).unwrap();
        let f = p.join("s1.jsonl");
        fs::write(&f, body).unwrap();
        f
    }

    #[test]
    fn parses_fields_and_skips_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let f = write_fixture(dir.path(), FIXTURE);
        let m = parse_file(&f).unwrap();
        assert_eq!(m.id, "s1");
        assert_eq!(m.cwd.as_deref(), Some(Path::new("/tmp/proj")));
        assert_eq!(m.git_branch.as_deref(), Some("fix/webhook"));
        assert_eq!(m.first_prompt.as_deref(), Some("fix the   webhook retry"));
        assert_eq!(m.last_prompt.as_deref(), Some("and add a test"));
        assert_eq!(m.title(), "fix the webhook retry");
        assert_eq!(m.folder_name(), "proj");
    }

    #[test]
    fn keeps_last_ten_messages_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut body = String::from(FIXTURE);
        for i in 1..=11 {
            body.push_str(&format!(
                r#"{{"type":"user","cwd":"/tmp/proj","isSidechain":false,"message":{{"role":"user","content":"prompt {i}"}}}}"#
            ));
            body.push('\n');
        }
        body.push_str(r#"{"type":"user","cwd":"/tmp/proj","isMeta":true,"message":{"role":"user","content":"meta text"}}"#);
        body.push('\n');
        let f = write_fixture(dir.path(), &body);
        let m = parse_file(&f).unwrap();
        let texts: Vec<&str> = m.recent_messages.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, (2..=11).map(|i| format!("prompt {i}")).collect::<Vec<_>>());
        assert_eq!(m.first_prompt.as_deref(), Some("fix the   webhook retry"));

        let short = parse_file(&write_fixture(dir.path(), FIXTURE)).unwrap();
        let texts: Vec<&str> = short.recent_messages.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["fix the   webhook retry"]);
    }

    /// Claude's reply text sits between the prompts. A reply split by tool
    /// calls is one message. Thinking, tool calls and tool results are left out.
    #[test]
    fn recent_messages_include_claude_replies() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"type":"user","cwd":"/tmp/proj","isSidechain":false,"message":{"role":"user","content":"push it"}}
{"type":"assistant","cwd":"/tmp/proj","isSidechain":false,"message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"}]}}
{"type":"assistant","cwd":"/tmp/proj","isSidechain":false,"message":{"role":"assistant","content":[{"type":"text","text":"Running tests first."}]}}
{"type":"assistant","cwd":"/tmp/proj","isSidechain":false,"message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}
{"type":"user","cwd":"/tmp/proj","isSidechain":false,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}
{"type":"assistant","cwd":"/tmp/proj","isSidechain":false,"message":{"role":"assistant","content":[{"type":"text","text":"Pushed."}]}}
{"type":"assistant","cwd":"/tmp/proj","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"subagent text"}]}}
{"type":"user","cwd":"/tmp/proj","isSidechain":false,"message":{"role":"user","content":"thanks"}}
"#;
        let m = parse_file(&write_fixture(dir.path(), body)).unwrap();
        let got: Vec<(Role, &str)> = m.recent_messages.iter().map(|c| (c.role, c.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (Role::User, "push it"),
                (Role::Assistant, "Running tests first.\n\nPushed."),
                (Role::User, "thanks"),
            ]
        );
    }

    #[test]
    fn title_of_huge_prompt_is_cut() {
        let m = SessionMeta { first_prompt: Some("word ".repeat(100_000)), ..Default::default() };
        let t = m.title();
        assert_eq!(t.chars().count(), TITLE_MAX + 1);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn title_order_agent_name_then_ai_title() {
        let mut m = SessionMeta { first_prompt: Some("p".into()), ..Default::default() };
        assert_eq!(m.title(), "p");
        m.ai_title = Some("AI".into());
        assert_eq!(m.title(), "AI");
        m.agent_name = Some("Named".into());
        assert_eq!(m.title(), "Named");
        assert_eq!(SessionMeta::default().title(), "(untitled)");
    }

    #[test]
    fn index_reads_appended_lines_and_waits_for_full_line() {
        let dir = tempfile::tempdir().unwrap();
        let f = write_fixture(dir.path(), FIXTURE);
        let mut idx = Index::default();
        assert!(idx.refresh(&roots(dir.path())));
        assert_eq!(idx.get("s1").unwrap().title(), "fix the webhook retry");
        assert!(!idx.refresh(&roots(dir.path())));

        // Half a line: not applied yet.
        let mut h = fs::OpenOptions::new().append(true).open(&f).unwrap();
        write!(h, r#"{{"type":"ai-title","aiTitle":"Fix webhook"#).unwrap();
        idx.refresh(&roots(dir.path()));
        assert_eq!(idx.get("s1").unwrap().ai_title, None);

        writeln!(h, r#" retry","sessionId":"s1"}}"#).unwrap();
        assert!(idx.refresh(&roots(dir.path())));
        assert_eq!(idx.get("s1").unwrap().title(), "Fix webhook retry");
    }

    #[test]
    fn delete_removes_file_and_subagent_folder() {
        let dir = tempfile::tempdir().unwrap();
        let f = write_fixture(dir.path(), FIXTURE);
        let sub = dir.path().join("proj").join("s1").join("subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("agent-x.jsonl"), "{}").unwrap();
        let other = dir.path().join("proj").join("s2.jsonl");
        fs::write(&other, FIXTURE).unwrap();

        delete_session(dir.path(), &f).unwrap();

        assert!(!f.exists());
        assert!(!dir.path().join("proj").join("s1").exists());
        assert!(other.exists(), "other sessions must stay");
    }

    #[test]
    fn delete_refuses_paths_outside_projects_dir() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let f = write_fixture(outside.path(), FIXTURE);
        assert!(delete_session(root.path(), &f).is_err());
        assert!(f.exists());

        // Not a session file.
        let txt = root.path().join("proj").join("notes.txt");
        fs::create_dir_all(txt.parent().unwrap()).unwrap();
        fs::write(&txt, "x").unwrap();
        assert!(delete_session(root.path(), &txt).is_err());
        assert!(txt.exists());

        // Traversal out of the root.
        let sneaky = root.path().join("proj").join("..").join("..").join(f.strip_prefix("/").unwrap());
        assert!(delete_session(root.path(), &sneaky).is_err());
        assert!(f.exists());
    }

    /// Sessions of every account are listed, each with its account. If the
    /// same ID is in two accounts, only the newer file counts.
    #[test]
    fn index_reads_all_accounts_and_prefers_newest_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let body = FIXTURE.replace("/tmp/proj", &work.display().to_string());
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        write_fixture(&a, &body);
        let copy = write_fixture(&b, &body.replace("fix the   webhook retry", "newer copy"));
        let roots = vec![Root { account: 0, projects_dir: a.clone() }, Root { account: 7, projects_dir: b.clone() }];

        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        fs::File::options().write(true).open(a.join("proj/s1.jsonl")).unwrap().set_modified(past).unwrap();
        let mut idx = Index::default();
        idx.refresh(&roots);
        assert_eq!(idx.count(), 1);
        assert_eq!(idx.sessions().len(), 1);
        let m = idx.get("s1").unwrap();
        assert_eq!((m.account, m.path.clone()), (7, copy.clone()));
        assert_eq!(m.first_prompt.as_deref(), Some("newer copy"));

        // The newer copy is moved away: the other one counts again.
        fs::remove_file(&copy).unwrap();
        idx.refresh(&roots);
        assert_eq!(idx.get("s1").unwrap().account, 0);
        assert_eq!(idx.count(), 1);
    }

    #[test]
    fn index_ignores_subagent_folders() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let body = FIXTURE.replace("/tmp/proj", &work.display().to_string());
        write_fixture(dir.path(), &body);
        let sub = dir.path().join("proj").join("s1").join("subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("agent-x.jsonl"), &body).unwrap();
        let mut idx = Index::default();
        idx.refresh(&roots(dir.path()));
        assert_eq!(idx.sessions().len(), 1);
    }
}
