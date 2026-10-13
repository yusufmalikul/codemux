//! codemux: a session manager for Claude Code.
//! Sidebar of sessions on the left, a live terminal on the right.

use chrono::{DateTime, Local, Utc};
use codemux_core::detect::{self, ClaudeProc, ProcScanner};
use codemux_core::launch::{self, ClaudeMode, LaunchSpec};
use codemux_core::paths;
use codemux_core::sessions::{Index, SessionMeta};
use codemux_core::store::AppState;
use codemux_terminal as term;
use iced::widget::{button, column, container, lazy, row, rule, scrollable, text, text_input, Column};
use iced::{keyboard, time, Element, Length, Subscription, Task};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(1500);
/// Output in the last 2 seconds means Claude is working (spec 7, fallback).
const BUSY_WINDOW: Duration = Duration::from_secs(2);
const HISTORY_LIMIT: usize = 300;

fn main() -> iced::Result {
    launch::clean_inherited_env();
    iced::application(App::new, App::update, App::view)
        .title("codemux")
        .window_size((1280.0, 800.0))
        .subscription(App::subscription)
        .run()
}

/// The app runs as a plain binary, not a `.app` bundle, so macOS has no icon
/// file to read. Set the Dock icon at runtime instead.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(include_bytes!("../assets/icon.png"));
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(not(target_os = "macos"))]
fn set_dock_icon() {}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TabKind {
    /// "Session baru" or resume: the app started `claude` and knows the ID.
    Claude,
    /// "Terminal baru": a plain shell. Claude is detected when the user runs it.
    Shell,
}

struct Tab {
    id: u64,
    kind: TabKind,
    term: term::Terminal,
    /// Known (Claude tabs) or detected (Shell tabs).
    session_id: Option<String>,
    /// A `claude` process found inside a Shell tab.
    claude: Option<ClaudeProc>,
    cwd: PathBuf,
    last_output: Instant,
}

#[derive(Debug, Clone, PartialEq)]
enum Selected {
    None,
    Tab(u64),
    Session(String),
}

struct App {
    tabs: Vec<Tab>,
    selected: Selected,
    next_id: u64,
    index: Index,
    /// Bumped when the index changes, so the lazy sidebar knows to rebuild.
    index_version: u64,
    projects_dir: Option<PathBuf>,
    scanner: ProcScanner,
    state_path: Option<PathBuf>,
    /// Sessions that were open when the app last ran (spec 11.4).
    previously_open: Vec<String>,
    search: String,
    error: Option<String>,
    /// Window scale factor, passed to every terminal (trackpad scroll speed).
    scale_factor: f32,
    /// Session waiting for "Hapus permanen" confirmation.
    confirm_delete: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    Terminal(term::Event),
    Tick,
    /// The window is up; macOS resets the Dock icon during launch, so set it now.
    WindowOpened,
    NewSession,
    FolderPicked(Option<PathBuf>),
    NewTerminal,
    SelectTab(u64),
    SelectSession(String),
    Resume(String),
    CloseTab(u64),
    CloseSelected,
    Search(String),
    SelectIndex(usize),
    ScaleFactor(f32),
    DeleteAsk(String),
    DeleteConfirm(String),
    DeleteCancel,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let state_path = paths::app_data_dir().map(|d| d.join("state.json"));
        let scale = iced::window::latest()
            .and_then(iced::window::scale_factor)
            .map(Message::ScaleFactor);
        (Self::with_paths(paths::projects_dir(), state_path), scale)
    }

    fn with_paths(projects_dir: Option<PathBuf>, state_path: Option<PathBuf>) -> Self {
        let previously_open = state_path.as_deref().map(AppState::load).unwrap_or_default().open_sessions;
        let mut app = Self {
            tabs: Vec::new(),
            selected: Selected::None,
            next_id: 0,
            index: Index::default(),
            index_version: 0,
            projects_dir,
            scanner: ProcScanner::default(),
            state_path,
            previously_open,
            search: String::new(),
            error: None,
            scale_factor: 1.0,
            confirm_delete: None,
        };
        app.refresh_index();
        app
    }

    fn refresh_index(&mut self) {
        if let Some(dir) = &self.projects_dir {
            if self.index.refresh(dir) {
                self.index_version += 1;
            }
        }
    }

    fn save_state(&self) {
        let Some(path) = &self.state_path else { return };
        let open_sessions = self.tabs.iter().filter_map(|t| t.session_id.clone()).collect();
        let _ = AppState { version: 0, open_sessions }.save(path);
    }

    fn spawn(&mut self, kind: TabKind, spec: LaunchSpec, session_id: Option<String>) -> Task<Message> {
        let id = self.next_id;
        self.next_id += 1;
        let settings = term::settings::Settings {
            backend: term::settings::BackendSettings {
                program: spec.program,
                args: spec.args,
                env: spec.env,
                working_directory: Some(spec.cwd.clone()),
            },
            ..Default::default()
        };
        match term::Terminal::new(id, settings) {
            Ok(mut t) => {
                t.handle(term::Command::SetScaleFactor(self.scale_factor));
                let focus = term::TerminalView::focus(t.widget_id().clone());
                self.tabs.push(Tab {
                    id,
                    kind,
                    term: t,
                    session_id,
                    claude: None,
                    cwd: spec.cwd,
                    last_output: Instant::now(),
                });
                self.selected = Selected::Tab(id);
                self.error = None;
                self.save_state();
                focus
            }
            Err(e) => {
                self.error = Some(format!("Gagal membuka terminal: {e}"));
                Task::none()
            }
        }
    }

    /// Show a tab: copy its latest screen, then give it keyboard focus.
    fn focus_tab(&mut self, id: u64) -> Task<Message> {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else { return Task::none() };
        let wakeup = term::BackendCommand::ProcessAlacrittyEvent(term::AlacrittyEvent::Wakeup);
        tab.term.handle(term::Command::ProxyToBackend(wakeup));
        term::TerminalView::focus(tab.term.widget_id().clone())
    }

    fn tab_for_session(&self, sid: &str) -> Option<u64> {
        self.tabs.iter().find(|t| t.session_id.as_deref() == Some(sid)).map(|t| t.id)
    }

    fn resume(&mut self, sid: String) -> Task<Message> {
        if let Some(id) = self.tab_for_session(&sid) {
            self.selected = Selected::Tab(id);
            return self.focus_tab(id);
        }
        let Some(cwd) = self.index.get(&sid).and_then(|m| m.cwd.clone()) else {
            self.error = Some("Folder session tidak diketahui.".into());
            return Task::none();
        };
        if !cwd.is_dir() {
            self.error = Some(format!("Folder sudah tidak ada: {}", cwd.display()));
            return Task::none();
        }
        match launch::claude(cwd, &ClaudeMode::Resume(sid.clone())) {
            Some(spec) => {
                self.previously_open.retain(|s| s != &sid);
                self.spawn(TabKind::Claude, spec, Some(sid))
            }
            None => {
                self.error = Some("Session ID tidak valid.".into());
                Task::none()
            }
        }
    }

    /// Detect `claude` inside Shell tabs and follow the shell's folder.
    fn detect(&mut self) {
        // Reading every process is not free. Skip it when there is nothing to detect.
        if !self.tabs.iter().any(|t| t.kind == TabKind::Shell) {
            return;
        }
        self.scanner.refresh();
        let mut taken: HashSet<String> = self.tabs.iter().filter_map(|t| t.session_id.clone()).collect();
        for tab in self.tabs.iter_mut().filter(|t| t.kind == TabKind::Shell) {
            let Some(shell_pid) = tab.term.child_pid() else { continue };
            if let Some(cwd) = self.scanner.cwd_of(shell_pid) {
                tab.cwd = cwd;
            }
            let found = self.scanner.find_claude_under(shell_pid);
            if found.as_ref().map(|c| c.pid) != tab.claude.as_ref().map(|c| c.pid) {
                // Claude started, stopped, or was restarted in this tab.
                if let Some(old) = tab.session_id.take() {
                    taken.remove(&old);
                }
                tab.claude = found;
            }
            let (Some(proc_), Some(dir)) = (&tab.claude, &self.projects_dir) else { continue };
            let cwd = proc_.cwd.clone().unwrap_or_else(|| tab.cwd.clone());
            let mut others = taken.clone();
            if let Some(own) = &tab.session_id {
                others.remove(own);
            }
            // Re-check every tick: /clear or /resume inside Claude switches the file.
            if let Some(sid) = detect::match_session(dir, &cwd, proc_.started, &others) {
                if tab.session_id.as_deref() != Some(&sid) {
                    if let Some(old) = tab.session_id.replace(sid.clone()) {
                        taken.remove(&old);
                    }
                    taken.insert(sid);
                }
            }
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Terminal(term::Event::BackendCall(id, cmd)) => {
                let Some(pos) = self.tabs.iter().position(|t| t.id == id) else { return Task::none() };
                if let term::BackendCommand::ProcessAlacrittyEvent(term::AlacrittyEvent::Wakeup) = &cmd {
                    self.tabs[pos].last_output = Instant::now();
                    // A hidden tab does not need a fresh copy of its screen.
                    // It is synced when it is shown again (see focus_tab).
                    if self.selected != Selected::Tab(id) {
                        return Task::none();
                    }
                }
                if let term::actions::Action::Shutdown = self.tabs[pos].term.handle(term::Command::ProxyToBackend(cmd)) {
                    return self.close_tab(id);
                }
            }
            Message::WindowOpened => {
                set_dock_icon();
            }
            Message::Tick => {
                self.refresh_index();
                let before: Vec<_> = self.tabs.iter().map(|t| t.session_id.clone()).collect();
                self.detect();
                let after: Vec<_> = self.tabs.iter().map(|t| t.session_id.clone()).collect();
                if before != after {
                    self.save_state();
                }
            }
            Message::NewSession => {
                return Task::perform(
                    rfd::AsyncFileDialog::new().set_title("Pilih folder proyek").pick_folder(),
                    |f| Message::FolderPicked(f.map(|h| h.path().to_path_buf())),
                );
            }
            Message::FolderPicked(Some(dir)) => {
                let sid = launch::new_session_id();
                if let Some(spec) = launch::claude(dir, &ClaudeMode::New(sid.clone())) {
                    return self.spawn(TabKind::Claude, spec, Some(sid));
                }
            }
            Message::FolderPicked(None) => {}
            Message::NewTerminal => {
                let cwd = match &self.selected {
                    Selected::Tab(id) => self.tabs.iter().find(|t| t.id == *id).map(|t| t.cwd.clone()),
                    _ => None,
                }
                .or_else(|| std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("."));
                return self.spawn(TabKind::Shell, launch::shell(cwd), None);
            }
            Message::SelectTab(id) => {
                self.selected = Selected::Tab(id);
                return self.focus_tab(id);
            }
            Message::SelectSession(sid) => {
                if let Some(id) = self.tab_for_session(&sid) {
                    self.selected = Selected::Tab(id);
                    return self.focus_tab(id);
                }
                self.confirm_delete = None;
                self.selected = Selected::Session(sid);
            }
            Message::Resume(sid) => return self.resume(sid),
            Message::CloseTab(id) => return self.close_tab(id),
            Message::CloseSelected => {
                if let Selected::Tab(id) = self.selected {
                    return self.close_tab(id);
                }
            }
            Message::Search(s) => self.search = s,
            Message::DeleteAsk(sid) => self.confirm_delete = Some(sid),
            Message::DeleteCancel => self.confirm_delete = None,
            Message::DeleteConfirm(sid) => self.delete_session(&sid),
            Message::ScaleFactor(scale) => {
                self.scale_factor = scale;
                for tab in &mut self.tabs {
                    tab.term.handle(term::Command::SetScaleFactor(scale));
                }
            }
            Message::SelectIndex(i) => {
                if let Some(id) = self.tabs.get(i).map(|t| t.id) {
                    self.selected = Selected::Tab(id);
                    return self.focus_tab(id);
                }
            }
        }
        Task::none()
    }

    /// Permanently delete a session file. Refused while it runs in a tab.
    fn delete_session(&mut self, sid: &str) {
        self.confirm_delete = None;
        if self.tab_for_session(sid).is_some() {
            self.error = Some("Session masih berjalan di tab. Tutup tab-nya dulu.".into());
            return;
        }
        let (Some(dir), Some(path)) = (self.projects_dir.clone(), self.index.get(sid).map(|m| m.path.clone())) else {
            self.error = Some("Session tidak ditemukan.".into());
            return;
        };
        match codemux_core::sessions::delete_session(&dir, &path) {
            Ok(()) => {
                self.error = None;
                self.previously_open.retain(|s| s != sid);
                if self.selected == Selected::Session(sid.to_string()) {
                    self.selected = Selected::None;
                }
                self.refresh_index();
                self.save_state();
            }
            Err(e) => self.error = Some(format!("Gagal menghapus session: {e}")),
        }
    }

    fn close_tab(&mut self, id: u64) -> Task<Message> {
        let Some(pos) = self.tabs.iter().position(|t| t.id == id) else { return Task::none() };
        // Dropping the terminal shuts down its PTY.
        let tab = self.tabs.remove(pos);
        if self.selected == Selected::Tab(id) {
            self.selected = match (&tab.session_id, self.tabs.get(pos.saturating_sub(1))) {
                (Some(sid), _) => Selected::Session(sid.clone()),
                (None, Some(t)) => Selected::Tab(t.id),
                (None, None) => Selected::None,
            };
        }
        self.save_state();
        match &self.selected {
            Selected::Tab(id) => self.focus_tab(*id),
            _ => Task::none(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let terms = self.tabs.iter().map(|t| t.term.subscription().map(Message::Terminal));
        Subscription::batch(
            terms.chain([time::every(TICK).map(|_| Message::Tick), iced::event::listen_with(shortcut)]),
        )
    }

    // ---------- view ----------

    fn view(&self) -> Element<'_, Message> {
        let sidebar = container(self.sidebar()).width(300).height(Length::Fill).padding(8);
        row![sidebar, rule::vertical(1), self.main_area()].into()
    }

    fn tab_title(&self, tab: &Tab) -> (String, String) {
        let folder = folder_label(&tab.cwd);
        match tab.session_id.as_deref().and_then(|s| self.index.get(s)) {
            Some(m) if m.ai_title.is_some() || m.agent_name.is_some() || m.first_prompt.is_some() => {
                (m.title(), folder)
            }
            _ if tab.kind == TabKind::Claude || tab.claude.is_some() => (format!("claude · {folder}"), folder),
            _ => (format!("Terminal · {folder}"), folder),
        }
    }

    fn tab_status(&self, tab: &Tab) -> (&'static str, &'static str) {
        let running_claude = tab.kind == TabKind::Claude || tab.claude.is_some();
        if !running_claude {
            ("▸", "shell")
        } else if tab.last_output.elapsed() < BUSY_WINDOW {
            ("●", "bekerja")
        } else {
            ("◐", "menunggu")
        }
    }

    fn sidebar(&self) -> Element<'_, Message> {
        // Explicit Fill: `lazy` reports Shrink, so the column would not grow by itself.
        let mut list = Column::new().spacing(2).width(Length::Fill);

        if !self.tabs.is_empty() {
            list = list.push(section("AKTIF"));
            for tab in &self.tabs {
                let (title, folder) = self.tab_title(tab);
                let (icon, status) = self.tab_status(tab);
                let selected = self.selected == Selected::Tab(tab.id);
                let line = row![
                    button(column![text(format!("{icon} {title}")).size(13), text(format!("   {folder} · {status}")).size(11)])
                        .on_press(Message::SelectTab(tab.id))
                        .width(Length::Fill)
                        .style(if selected { button::primary } else { button::text }),
                    button(text("✕").size(11)).on_press(Message::CloseTab(tab.id)).style(button::text),
                ];
                list = list.push(line);
            }
        }

        // The history list is large. Rebuild it only when its inputs change,
        // not on every PTY event.
        let active: Vec<String> = self.tabs.iter().filter_map(|t| t.session_id.clone()).collect();
        let selected_session = match &self.selected {
            Selected::Session(s) => Some(s.clone()),
            _ => None,
        };
        let minute = Local::now().timestamp() / 60;
        let deps = (self.index_version, self.search.clone(), selected_session, self.previously_open.clone(), active, minute);
        list = list.push(lazy(deps, move |(_, _, _, _, active, _)| self.history_list(active)));

        let shortcut_hint = if cfg!(target_os = "macos") { "⌘N session · ⌘T terminal · ⌘W tutup" } else { "Ctrl+Shift+N / T / W" };
        column![
            text_input("Cari session…", &self.search).on_input(Message::Search).size(13),
            row![
                button(text("+ Session baru").size(13)).on_press(Message::NewSession).width(Length::Fill),
                button(text(">_ Terminal baru").size(13)).on_press(Message::NewTerminal).width(Length::Fill),
            ]
            .spacing(6),
            text(shortcut_hint).size(10),
            scrollable(list).height(Length::Fill),
        ]
        .spacing(8)
        .into()
    }

    fn history_list(&self, active: &[String]) -> Element<'static, Message> {
        let active: HashSet<&str> = active.iter().map(String::as_str).collect();
        let mut list = Column::new().spacing(2).width(Length::Fill);
        let q = self.search.to_lowercase();
        let matches = |m: &SessionMeta| {
            q.is_empty()
                || [Some(m.title()), m.cwd.as_ref().map(|c| c.display().to_string()), m.git_branch.clone(), m.last_prompt.clone()]
                    .into_iter()
                    .flatten()
                    .any(|s| s.to_lowercase().contains(&q))
        };

        let prev: Vec<&SessionMeta> = self
            .previously_open
            .iter()
            .filter(|s| !active.contains(s.as_str()))
            .filter_map(|s| self.index.get(s))
            .filter(|m| matches(m))
            .collect();
        if !prev.is_empty() {
            list = list.push(section("TERBUKA SEBELUMNYA"));
            for m in prev {
                list = list.push(self.session_row(m));
            }
        }

        let mut group = "";
        let prev_set: HashSet<&str> = self.previously_open.iter().map(String::as_str).collect();
        for m in self
            .index
            .sessions()
            .into_iter()
            .filter(|m| !active.contains(m.id.as_str()) && !prev_set.contains(m.id.as_str()) && matches(m))
            .take(HISTORY_LIMIT)
        {
            let g = time_group(m.sort_time());
            if g != group {
                group = g;
                list = list.push(section(g));
            }
            list = list.push(self.session_row(m));
        }

        list.into()
    }

    fn session_row(&self, m: &SessionMeta) -> Element<'static, Message> {
        let selected = self.selected == Selected::Session(m.id.clone());
        let mut sub = vec![m.folder_name()];
        if let Some(b) = &m.git_branch {
            sub.push(b.clone());
        }
        sub.push(relative_time(m.sort_time()));
        button(column![text(format!("○ {}", m.title())).size(13), text(format!("   {}", sub.join(" · "))).size(11)])
            .on_press(Message::SelectSession(m.id.clone()))
            .width(Length::Fill)
            .style(if selected { button::primary } else { button::text })
            .into()
    }

    fn main_area(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = match &self.selected {
            Selected::Tab(id) => match self.tabs.iter().find(|t| t.id == *id) {
                Some(tab) => {
                    let (title, _) = self.tab_title(tab);
                    let meta = tab.session_id.as_deref().and_then(|s| self.index.get(s));
                    let mut info = vec![tab.cwd.display().to_string()];
                    if let Some(b) = meta.and_then(|m| m.git_branch.clone()) {
                        info.push(b);
                    }
                    let header = row![
                        column![text(title).size(15), text(info.join(" · ")).size(11)].width(Length::Fill),
                        button(text("■ Tutup").size(12)).on_press(Message::CloseTab(tab.id)).style(button::danger),
                    ]
                    .padding(8);
                    column![header, rule::horizontal(1), term::TerminalView::show(&tab.term).map(Message::Terminal)]
                        .into()
                }
                None => empty_state(),
            },
            Selected::Session(sid) => match self.index.get(sid) {
                Some(m) => {
                    let mut col = column![
                        text(m.title()).size(20),
                        text(m.cwd.as_ref().map(|c| c.display().to_string()).unwrap_or_default()).size(12),
                    ]
                    .spacing(8);
                    if let Some(b) = &m.git_branch {
                        col = col.push(text(format!("Branch: {b}")).size(12));
                    }
                    col = col.push(text(format!("Terakhir: {}", relative_time(m.sort_time()))).size(12));
                    if let Some(p) = &m.last_prompt {
                        col = col.push(text(format!("Prompt terakhir: {p}")).size(12));
                    }
                    col = col.push(text(format!("ID: {}", m.id)).size(11));
                    let actions: Element<'_, Message> = if self.confirm_delete.as_deref() == Some(m.id.as_str()) {
                        row![
                            text("Hapus file session ini? Tidak bisa dibatalkan.").size(12),
                            button(text("Hapus permanen")).on_press(Message::DeleteConfirm(m.id.clone())).style(button::danger),
                            button(text("Batal")).on_press(Message::DeleteCancel).style(button::secondary),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                        .into()
                    } else {
                        row![
                            button(text("▶ Lanjutkan")).on_press(Message::Resume(m.id.clone())),
                            button(text("Hapus")).on_press(Message::DeleteAsk(m.id.clone())).style(button::danger),
                        ]
                        .spacing(8)
                        .into()
                    };
                    col = col.push(actions);
                    container(col).padding(24).into()
                }
                None => empty_state(),
            },
            Selected::None => empty_state(),
        };
        let mut col = Column::new();
        if let Some(e) = &self.error {
            col = col.push(container(text(e.clone()).size(12)).padding(8).style(container::danger));
        }
        container(col.push(body)).width(Length::Fill).height(Length::Fill).into()
    }
}

fn empty_state<'a>() -> Element<'a, Message> {
    container(text("Pilih session di kiri, atau buat Session baru / Terminal baru.").size(14))
        .center(Length::Fill)
        .into()
}

fn section(label: &'static str) -> Element<'static, Message> {
    container(text(label).size(10)).padding([6, 4]).into()
}

fn folder_label(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn time_group(t: DateTime<Utc>) -> &'static str {
    let today = Local::now().date_naive();
    let days = (today - t.with_timezone(&Local).date_naive()).num_days();
    match days {
        ..=0 => "HARI INI",
        1 => "KEMARIN",
        2..=7 => "7 HARI TERAKHIR",
        8..=30 => "30 HARI TERAKHIR",
        _ => "LEBIH LAMA",
    }
}

fn relative_time(t: DateTime<Utc>) -> String {
    let local = t.with_timezone(&Local);
    let mins = (Local::now() - local).num_minutes();
    match time_group(t) {
        "HARI INI" if mins < 60 => format!("{}m", mins.max(0)),
        "HARI INI" => local.format("%H:%M").to_string(),
        "KEMARIN" => "kemarin".into(),
        _ => local.format("%d %b").to_string(),
    }
}

/// App shortcuts. macOS uses ⌘, which the terminal does not need.
/// Windows/Linux use Ctrl+Shift, so plain Ctrl keys stay with the terminal (spec 6.3).
fn shortcut(event: iced::Event, _status: iced::event::Status, _window: iced::window::Id) -> Option<Message> {
    match event {
        iced::Event::Window(iced::window::Event::Rescaled(scale)) => return Some(Message::ScaleFactor(scale)),
        iced::Event::Window(iced::window::Event::Opened { .. }) => return Some(Message::WindowOpened),
        _ => {}
    }
    let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else { return None };
    let app_mod = if cfg!(target_os = "macos") {
        modifiers.logo() && !modifiers.control()
    } else {
        modifiers.control() && modifiers.shift()
    };
    if !app_mod {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character(c) => match c.to_lowercase().as_str() {
            "n" => Some(Message::NewSession),
            "t" => Some(Message::NewTerminal),
            "w" => Some(Message::CloseSelected),
            d if d.len() == 1 && ('1'..='9').contains(&d.chars().next()?) => {
                Some(Message::SelectIndex(d.parse::<usize>().ok()? - 1))
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("projects").join("-tmp-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let now = Utc::now().to_rfc3339();
        std::fs::write(
            proj.join("s1.jsonl"),
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"s1\",\"cwd\":\"/tmp/proj\",\"timestamp\":\"{now}\",\"message\":{{\"content\":\"hello\"}}}}\n\
                 {{\"type\":\"ai-title\",\"aiTitle\":\"Fix webhook retry\",\"sessionId\":\"s1\"}}\n"
            ),
        )
        .unwrap();
        let app = App::with_paths(Some(dir.path().join("projects")), Some(dir.path().join("state.json")));
        (dir, app)
    }

    /// Build the UI for several frames with a shared cache, like the real
    /// runtime does, sending one message between frames.
    fn found_after_frames(app: &mut App, messages: Vec<Message>, needle: &str) -> bool {
        use iced_test::core::renderer::Headless;
        use iced_test::core::{widget, Font, Pixels, Size};
        use iced_test::runtime::user_interface::{Cache, UserInterface};
        use iced_test::selector::Selector;

        let mut renderer = iced_test::futures::futures::executor::block_on(
            iced_test::renderer::Renderer::new(Font::with_name("Fira Sans"), Pixels(16.0), None),
        )
        .unwrap();
        let size = Size::new(1280.0, 800.0);
        let mut cache = Cache::default();
        for m in messages {
            let ui = UserInterface::build(app.view(), size, cache, &mut renderer);
            cache = ui.into_cache();
            let _ = app.update(m);
        }
        let mut ui = UserInterface::build(app.view(), size, cache, &mut renderer);
        let mut op = Selector::find(needle);
        ui.operate(&renderer, &mut widget::operation::black_box(&mut op));
        match widget::Operation::finish(&op) {
            // Must be really visible, not only present in the widget tree.
            widget::operation::Outcome::Some(Some(target)) => {
                target.visible_bounds().is_some_and(|b| b.width > 0.0 && b.height > 0.0)
            }
            _ => false,
        }
    }

    #[test]
    fn sidebar_keeps_sessions_after_more_frames() {
        let (_dir, mut app) = fixture();
        let msgs = vec![Message::Tick, Message::Search(String::new()), Message::Tick];
        assert!(found_after_frames(&mut app, msgs, "○ Fix webhook retry"));
    }

    /// Opening a tab adds rows above the history list. The history must stay.
    #[test]
    fn sidebar_keeps_sessions_after_opening_a_terminal() {
        let (_dir, mut app) = fixture();
        let msgs = vec![Message::Tick, Message::NewTerminal, Message::Tick];
        assert!(found_after_frames(&mut app, msgs, "○ Fix webhook retry"));
    }

    #[test]
    fn delete_session_removes_file_and_sidebar_row() {
        let (dir, mut app) = fixture();
        let file = dir.path().join("projects/-tmp-proj/s1.jsonl");
        let msgs = vec![
            Message::SelectSession("s1".into()),
            Message::DeleteAsk("s1".into()),
            Message::DeleteConfirm("s1".into()),
        ];
        assert!(!found_after_frames(&mut app, msgs, "○ Fix webhook retry"));
        assert!(!file.exists());
        assert!(app.index.get("s1").is_none());
        assert_eq!(app.selected, Selected::None);
    }

    #[test]
    fn delete_is_refused_while_session_runs_in_a_tab() {
        let (dir, mut app) = fixture();
        let _ = app.update(Message::NewTerminal);
        app.tabs[0].session_id = Some("s1".into());
        let _ = app.update(Message::DeleteConfirm("s1".into()));
        assert!(dir.path().join("projects/-tmp-proj/s1.jsonl").exists());
        assert!(app.error.is_some());
    }

    #[test]
    fn sidebar_shows_history_sessions() {
        let (_dir, mut app) = fixture();
        assert!(found_after_frames(&mut app, vec![], "○ Fix webhook retry"), "session not visible");
        assert!(found_after_frames(&mut app, vec![], "HARI INI"), "time group not visible");
    }
}
