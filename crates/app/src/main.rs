//! Codemux: a session manager for Claude Code.
//! Sidebar of sessions on the left, a live terminal on the right.

use chrono::{DateTime, Local, Utc};
use codemux_core::detect::{self, ClaudeProc, ProcScanner};
use codemux_core::launch::{self, ClaudeMode, LaunchSpec};
use codemux_core::paths;
use codemux_core::sessions::{Index, SessionMeta};
use codemux_core::store::{AppState, Workspace};
use codemux_terminal as term;
use iced::widget::{button, column, container, lazy, mouse_area, pick_list, row, rule, scrollable, text, text_input, Column};
use iced::{keyboard, time, Element, Length, Subscription, Task};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(1500);
/// Output in the last 2 seconds means Claude is working (spec 7, fallback).
const BUSY_WINDOW: Duration = Duration::from_secs(2);
const HISTORY_LIMIT: usize = 300;
const SIDEBAR_DEFAULT: f32 = 300.0;
const SIDEBAR_MIN: f32 = 180.0;
const SIDEBAR_MAX: f32 = 700.0;
/// Main color for buttons and the selected row.
const ACCENT: iced::Color = iced::Color::from_rgb8(0x35, 0x3d, 0xb0);
const DEFAULT_WORKSPACE: &str = "Default";
/// Left indent of rows inside a workspace, so the sidebar reads as a tree.
const TREE_INDENT: f32 = 16.0;
const WORKSPACE_INPUT: &str = "new-workspace-name";

fn main() -> iced::Result {
    launch::clean_inherited_env();
    #[cfg(unix)]
    launch::load_shell_path();
    iced::application(App::new, App::update, App::view)
        .title("Codemux")
        .window_size((1280.0, 800.0))
        .subscription(App::subscription)
        .theme(App::theme)
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
    /// "New session" or resume: the app started `claude` and knows the ID.
    Claude,
    /// "New terminal": a plain shell. Claude is detected when the user runs it.
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
    /// Busy or idle, read from the title Claude sets. None until Claude sets one.
    busy: Option<bool>,
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
    /// Session waiting for "Delete permanently" confirmation.
    confirm_delete: Option<String>,
    /// Workspace whose delete is waiting for a yes or no.
    confirm_delete_workspace: Option<u64>,
    /// Privacy: the sidebar is collapsed (saved in state.json).
    hide_sidebar: bool,
    sidebar_width: f32,
    /// True while the user drags the sidebar divider.
    resizing: bool,
    /// User-made workspaces (saved in state.json). Sessions in none of them
    /// are in the default workspace.
    workspaces: Vec<Workspace>,
    default_collapsed: bool,
    /// Bumped when workspaces change, so the lazy sidebar knows to rebuild.
    workspaces_version: u64,
    /// Name typed for a new workspace. Some while the name box is shown.
    new_workspace: Option<String>,
}

/// One choice in the "Workspace" picker. `id` None is the default workspace.
#[derive(Debug, Clone, PartialEq)]
struct WorkspaceChoice {
    id: Option<u64>,
    name: String,
}

impl std::fmt::Display for WorkspaceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

#[derive(Debug, Clone)]
enum Message {
    Terminal(term::Event),
    Tick,
    ToggleHideSidebar,
    ResizeStart,
    /// Cursor x while dragging the divider.
    ResizeTo(f32),
    ResizeEnd,
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
    NewWorkspaceStart,
    NewWorkspaceName(String),
    NewWorkspaceCreate,
    NewWorkspaceCancel,
    /// Collapse or expand a workspace. None is the default workspace.
    ToggleWorkspace(Option<u64>),
    /// Ask before removing a workspace.
    DeleteWorkspaceAsk(u64),
    DeleteWorkspaceCancel,
    /// Remove a workspace. Its sessions go back to the default workspace.
    DeleteWorkspace(u64),
    /// Move a session to a workspace. None is the default workspace.
    MoveSession(String, Option<u64>),
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
        let state = state_path.as_deref().map(AppState::load).unwrap_or_default();
        let previously_open = state.open_sessions;
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
            confirm_delete_workspace: None,
            hide_sidebar: state.hide_sidebar,
            sidebar_width: state.sidebar_width.unwrap_or(SIDEBAR_DEFAULT).clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            resizing: false,
            workspaces: state.workspaces,
            default_collapsed: state.default_collapsed,
            workspaces_version: 0,
            new_workspace: None,
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
        let _ = AppState { version: 0, open_sessions, hide_sidebar: self.hide_sidebar,
            sidebar_width: Some(self.sidebar_width),
            workspaces: self.workspaces.clone(),
            default_collapsed: self.default_collapsed,
        }
        .save(path);
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
                    busy: None,
                });
                self.selected = Selected::Tab(id);
                self.reveal_tab(id);
                self.error = None;
                self.save_state();
                focus
            }
            Err(e) => {
                self.error = Some(format!("Failed to open terminal: {e}"));
                Task::none()
            }
        }
    }

    /// Show a tab: copy its latest screen, then give it keyboard focus.
    fn focus_tab(&mut self, id: u64) -> Task<Message> {
        self.reveal_tab(id);
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else { return Task::none() };
        let wakeup = term::BackendCommand::ProcessAlacrittyEvent(term::AlacrittyEvent::Wakeup);
        tab.term.handle(term::Command::ProxyToBackend(wakeup));
        term::TerminalView::focus(tab.term.widget_id().clone())
    }

    /// Expand the workspace that holds a tab, so the selected tab is never
    /// hidden inside a collapsed workspace.
    fn reveal_tab(&mut self, id: u64) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        let ws = tab.session_id.as_deref().and_then(|s| self.workspace_of(s));
        let collapsed = match ws {
            Some(ws) => self.workspaces.iter_mut().find(|w| w.id == ws).map(|w| &mut w.collapsed),
            None => Some(&mut self.default_collapsed),
        };
        if let Some(c) = collapsed.filter(|c| **c) {
            *c = false;
            self.workspaces_changed();
        }
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
            self.error = Some(format!("Folder no longer exists: {}", cwd.display()));
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
                match &cmd {
                    term::BackendCommand::ProcessAlacrittyEvent(term::AlacrittyEvent::Title(t)) => {
                        self.tabs[pos].busy = claude_title_busy(t);
                    }
                    term::BackendCommand::ProcessAlacrittyEvent(term::AlacrittyEvent::ResetTitle) => {
                        self.tabs[pos].busy = None;
                    }
                    _ => {}
                }
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
                    rfd::AsyncFileDialog::new().set_title("Choose project folder").pick_folder(),
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
            Message::ToggleHideSidebar => {
                self.hide_sidebar = !self.hide_sidebar;
                self.save_state();
            }
            Message::ResizeStart => self.resizing = true,
            Message::ResizeTo(x) if self.resizing => self.sidebar_width = x.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            Message::ResizeTo(_) => {}
            Message::ResizeEnd => {
                self.resizing = false;
                self.save_state();
            }
            Message::DeleteAsk(sid) => self.confirm_delete = Some(sid),
            Message::DeleteCancel => self.confirm_delete = None,
            Message::DeleteConfirm(sid) => self.delete_session(&sid),
            Message::NewWorkspaceStart => {
                self.new_workspace = Some(String::new());
                return iced::widget::operation::focus(iced::widget::Id::new(WORKSPACE_INPUT));
            }
            Message::NewWorkspaceName(name) => self.new_workspace = Some(name),
            Message::NewWorkspaceCancel => self.new_workspace = None,
            Message::NewWorkspaceCreate => {
                let name = self.new_workspace.take().unwrap_or_default().trim().to_string();
                if !name.is_empty() {
                    let id = self.workspaces.iter().map(|w| w.id).max().unwrap_or(0) + 1;
                    self.workspaces.push(Workspace { id, name, ..Default::default() });
                    self.workspaces_changed();
                }
            }
            Message::ToggleWorkspace(None) => {
                self.default_collapsed = !self.default_collapsed;
                self.workspaces_changed();
            }
            Message::ToggleWorkspace(Some(id)) => {
                if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == id) {
                    w.collapsed = !w.collapsed;
                    self.workspaces_changed();
                }
            }
            Message::DeleteWorkspaceAsk(id) => {
                self.confirm_delete_workspace = Some(id);
                // The tree is cached; a version bump makes it show the question.
                self.workspaces_version += 1;
            }
            Message::DeleteWorkspaceCancel => {
                self.confirm_delete_workspace = None;
                self.workspaces_version += 1;
            }
            Message::DeleteWorkspace(id) => {
                self.confirm_delete_workspace = None;
                self.workspaces.retain(|w| w.id != id);
                self.workspaces_changed();
            }
            Message::MoveSession(sid, to) => {
                for w in &mut self.workspaces {
                    w.sessions.retain(|s| s != &sid);
                }
                if let Some(w) = to.and_then(|id| self.workspaces.iter_mut().find(|w| w.id == id)) {
                    w.sessions.push(sid);
                }
                self.workspaces_changed();
            }
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

    fn workspaces_changed(&mut self) {
        self.workspaces_version += 1;
        self.save_state();
    }

    fn workspace_of(&self, sid: &str) -> Option<u64> {
        self.workspaces.iter().find(|w| w.sessions.iter().any(|s| s == sid)).map(|w| w.id)
    }

    fn workspace_choices(&self) -> Vec<WorkspaceChoice> {
        std::iter::once(WorkspaceChoice { id: None, name: DEFAULT_WORKSPACE.into() })
            .chain(self.workspaces.iter().map(|w| WorkspaceChoice { id: Some(w.id), name: w.name.clone() }))
            .collect()
    }

    /// Permanently delete a session file. Refused while it runs in a tab.
    fn delete_session(&mut self, sid: &str) {
        self.confirm_delete = None;
        if self.tab_for_session(sid).is_some() {
            self.error = Some("This session is still running in a tab. Close the tab first.".into());
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
                for w in &mut self.workspaces {
                    w.sessions.retain(|s| s != sid);
                }
                self.workspaces_version += 1;
                if self.selected == Selected::Session(sid.to_string()) {
                    self.selected = Selected::None;
                }
                self.refresh_index();
                self.save_state();
            }
            Err(e) => self.error = Some(format!("Failed to delete session: {e}")),
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
            terms.chain([time::every(TICK).map(|_| Message::Tick), iced::event::listen_with(shortcut)]).chain(
                // Follow the mouse only while dragging the divider.
                self.resizing.then(|| iced::event::listen_with(resize_event)),
            ),
        )
    }

    // ---------- view ----------

    fn view(&self) -> Element<'_, Message> {
        if self.hide_sidebar {
            let show = container(button(text("›").size(16)).on_press(Message::ToggleHideSidebar).style(button::text))
                .height(Length::Fill)
                .padding(4);
            return row![show, rule::vertical(1), self.main_area()].into();
        }
        let sidebar = container(self.sidebar()).width(self.sidebar_width).height(Length::Fill).padding(8);
        // A wider grab area than the 1px line, so the divider is easy to hit.
        let divider = mouse_area(container(rule::vertical(1)).width(5).center_x(5))
            .on_press(Message::ResizeStart)
            .interaction(iced::mouse::Interaction::ResizingHorizontally);
        row![sidebar, divider, self.main_area()].into()
    }

    fn theme(&self) -> iced::Theme {
        let palette = iced::theme::Palette { primary: ACCENT, ..iced::theme::Palette::DARK };
        iced::Theme::custom("codemux", palette)
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
        } else if tab.busy.unwrap_or_else(|| tab.last_output.elapsed() < BUSY_WINDOW) {
            ("●", "working")
        } else {
            ("◐", "waiting")
        }
    }

    fn sidebar(&self) -> Element<'_, Message> {
        // Explicit Fill: `lazy` reports Shrink, so the column would not grow by itself.
        let mut list = Column::new().spacing(2).width(Length::Fill);

        // The tree is large. Rebuild it only when its inputs change, not on
        // every PTY event. Open tabs are part of the inputs, but only their
        // shown text, which changes rarely.
        let tabs: Vec<TabRow> = self
            .tabs
            .iter()
            .map(|tab| {
                let (title, folder) = self.tab_title(tab);
                let (icon, status) = self.tab_status(tab);
                TabRow {
                    id: tab.id,
                    session: tab.session_id.clone(),
                    title,
                    folder,
                    icon,
                    status,
                    selected: self.selected == Selected::Tab(tab.id),
                }
            })
            .collect();
        let selected_session = match &self.selected {
            Selected::Session(s) => Some(s.clone()),
            _ => None,
        };
        let minute = Local::now().timestamp() / 60;
        let deps =
            (self.index_version, self.workspaces_version, self.search.clone(), selected_session, self.previously_open.clone(), tabs, minute);
        list = list.push(lazy(deps, move |(_, _, _, _, _, tabs, _)| self.history_list(tabs)));

        let workspace_bar: Element<'_, Message> = match &self.new_workspace {
            Some(name) => row![
                text_input("Workspace name…", name)
                    .id(iced::widget::Id::new(WORKSPACE_INPUT))
                    .on_input(Message::NewWorkspaceName)
                    .on_submit(Message::NewWorkspaceCreate)
                    .size(15),
                button(text("Add").size(14)).on_press(Message::NewWorkspaceCreate),
                button(text("Cancel").size(14)).on_press(Message::NewWorkspaceCancel).style(button::secondary),
            ]
            .spacing(6)
            .into(),
            None => button(text("+ New workspace").size(15))
                .on_press(Message::NewWorkspaceStart)
                .width(Length::Fill)
                .style(button::secondary)
                .into(),
        };

        let shortcut_hint = if cfg!(target_os = "macos") { "⌘N session · ⌘T terminal · ⌘W close · ⌘B hide" } else { "Ctrl+Shift+N / T / W / B" };
        column![
            row![
                text_input("Search sessions…", &self.search).on_input(Message::Search).size(15),
                button(text("‹ Hide").size(14)).on_press(Message::ToggleHideSidebar).style(button::secondary),
            ]
            .spacing(6),
            row![
                button(text("+ New session").size(15)).on_press(Message::NewSession).width(Length::Fill),
                button(text(">_ New terminal").size(15)).on_press(Message::NewTerminal).width(Length::Fill),
            ]
            .spacing(6),
            workspace_bar,
            row![text(shortcut_hint).size(12).width(Length::Fill), text(format!("Sessions: {}", self.index.count())).size(12)],
            // Keep a small gap between the rows (and their close buttons) and the scrollbar.
            scrollable(list)
                .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().spacing(6)))
                .height(Length::Fill),
        ]
        .spacing(8)
        .into()
    }

    /// The sidebar tree: each workspace with its open tabs first, then its
    /// other sessions. A session open in a tab is shown only as that tab.
    fn history_list(&self, tabs: &[TabRow]) -> Element<'static, Message> {
        let active: HashSet<&str> = tabs.iter().filter_map(|t| t.session.as_deref()).collect();
        let mut list = Column::new().spacing(2).width(Length::Fill);
        let q = self.search.to_lowercase();
        let matches = |m: &SessionMeta| {
            q.is_empty()
                || [Some(m.title()), m.cwd.as_ref().map(|c| c.display().to_string()), m.git_branch.clone(), m.last_prompt.clone()]
                    .into_iter()
                    .flatten()
                    .any(|s| s.to_lowercase().contains(&q))
        };
        let tab_matches = |t: &TabRow| match t.session.as_deref().and_then(|s| self.index.listed(s)) {
            Some(m) => matches(m),
            None => q.is_empty() || [&t.title, &t.folder].iter().any(|s| s.to_lowercase().contains(&q)),
        };

        // While searching, open every workspace so no match is hidden.
        let searching = !q.is_empty();

        // Tabs per workspace. Tabs with no session, or a session in no
        // workspace, belong to the default workspace.
        let ws_tabs = |id: Option<u64>| -> Vec<&TabRow> {
            tabs.iter().filter(|t| t.session.as_deref().and_then(|s| self.workspace_of(s)) == id).filter(|t| tab_matches(t)).collect()
        };

        for ws in &self.workspaces {
            let open_tabs = ws_tabs(Some(ws.id));
            let mut items: Vec<&SessionMeta> = ws
                .sessions
                .iter()
                .filter(|s| !active.contains(s.as_str()))
                .filter_map(|s| self.index.listed(s))
                .filter(|m| matches(m))
                .collect();
            if searching && items.is_empty() && open_tabs.is_empty() {
                continue;
            }
            items.sort_by_key(|m| std::cmp::Reverse(m.sort_time()));
            let open = searching || !ws.collapsed;
            list = list.push(workspace_header(Some(ws.id), &ws.name, items.len() + open_tabs.len(), open_tabs.len(), open));
            if self.confirm_delete_workspace == Some(ws.id) {
                list = list.push(indent(delete_workspace_confirm(ws.id, &ws.name)));
            }
            if open {
                for t in open_tabs {
                    list = list.push(indent(tab_row(t)));
                }
                for m in items {
                    list = list.push(indent(self.session_row(m)));
                }
            }
        }

        let in_workspace: HashSet<&str> =
            self.workspaces.iter().flat_map(|w| w.sessions.iter().map(String::as_str)).collect();
        let prev_set: HashSet<&str> = self.previously_open.iter().map(String::as_str).collect();
        let prev: Vec<&SessionMeta> = self
            .previously_open
            .iter()
            .filter(|s| !active.contains(s.as_str()) && !in_workspace.contains(s.as_str()))
            .filter_map(|s| self.index.listed(s))
            .filter(|m| matches(m))
            .collect();
        let rest: Vec<&SessionMeta> = self
            .index
            .sessions()
            .into_iter()
            .filter(|m| {
                let id = m.id.as_str();
                !active.contains(id) && !prev_set.contains(id) && !in_workspace.contains(id) && matches(m)
            })
            .take(HISTORY_LIMIT)
            .collect();
        let open_tabs = ws_tabs(None);
        let open = searching || !self.default_collapsed;
        let count = prev.len() + rest.len() + open_tabs.len();
        list = list.push(workspace_header(None, DEFAULT_WORKSPACE, count, open_tabs.len(), open));
        if !open {
            return list.into();
        }

        for t in open_tabs {
            list = list.push(indent(tab_row(t)));
        }

        if !prev.is_empty() {
            list = list.push(indent(section("PREVIOUSLY OPEN")));
            for m in prev {
                list = list.push(indent(self.session_row(m)));
            }
        }

        let mut group = "";
        for m in rest {
            let g = time_group(m.sort_time());
            if g != group {
                group = g;
                list = list.push(indent(section(g)));
            }
            list = list.push(indent(self.session_row(m)));
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
        button(row_label(format!("○ {}", m.title()), format!("   {}", sub.join(" · "))))
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
                        button(text("■ Close").size(12)).on_press(Message::CloseTab(tab.id)).style(button::danger),
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
                    col = col.push(text(format!("Last active: {}", relative_time(m.sort_time()))).size(12));
                    if let Some(p) = &m.last_prompt {
                        col = col.push(text(format!("Last prompt: {p}")).size(12));
                    }
                    col = col.push(text(format!("ID: {}", m.id)).size(11));
                    let choices = self.workspace_choices();
                    let current = self.workspace_of(&m.id);
                    let selected = choices.iter().find(|c| c.id == current).cloned();
                    let sid = m.id.clone();
                    col = col.push(
                        row![
                            text("Workspace:").size(12),
                            pick_list(choices, selected, move |c| Message::MoveSession(sid.clone(), c.id)).text_size(13),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    );
                    let actions: Element<'_, Message> = if self.confirm_delete.as_deref() == Some(m.id.as_str()) {
                        row![
                            text("Delete this session file? This cannot be undone.").size(12),
                            button(text("Delete permanently")).on_press(Message::DeleteConfirm(m.id.clone())).style(button::danger),
                            button(text("Cancel")).on_press(Message::DeleteCancel).style(button::secondary),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                        .into()
                    } else {
                        row![
                            button(text("▶ Resume")).on_press(Message::Resume(m.id.clone())),
                            button(text("Delete")).on_press(Message::DeleteAsk(m.id.clone())).style(button::danger),
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
        // Own row under the body, so it never covers terminal text.
        let credit = container(text("Created by yusufmalikul").size(13)).width(Length::Fill).align_right(Length::Fill).padding([2, 8]);
        let body = container(body).width(Length::Fill).height(Length::Fill);
        container(col.push(body).push(credit)).width(Length::Fill).height(Length::Fill).into()
    }
}

fn empty_state<'a>() -> Element<'a, Message> {
    container(text("Pick a session on the left, or start a New session / New terminal.").size(14))
        .center(Length::Fill)
        .into()
}

fn section(label: &'static str) -> Element<'static, Message> {
    // Space above so each group stands apart from the rows of the group before it.
    container(text(label).size(14)).padding(iced::Padding { top: 16.0, right: 4.0, bottom: 4.0, left: 4.0 }).into()
}

/// A workspace row in the sidebar tree: an arrow, the name, and the number
/// of sessions. Click to collapse or expand. User workspaces also get a
/// delete button; the default workspace cannot be removed.
fn workspace_header(id: Option<u64>, name: &str, count: usize, open_tabs: usize, open: bool) -> Element<'static, Message> {
    let arrow = if open { "▾" } else { "▸" };
    // Shown even when collapsed, so open tabs are never out of sight.
    let badge = if open_tabs > 0 { format!("  ● {open_tabs} open") } else { String::new() };
    let toggle = button(text(format!("{arrow} {name}  ({count}){badge}")).size(15))
        .on_press(Message::ToggleWorkspace(id))
        .width(Length::Fill)
        .style(button::text);
    let mut line = row![toggle].spacing(4).align_y(iced::Alignment::Center);
    if let Some(id) = id {
        line = line.push(button(text("✕").size(13)).padding([4, 10]).on_press(Message::DeleteWorkspaceAsk(id)).style(close_style));
    }
    container(line)
        .padding(iced::Padding { top: 10.0, ..iced::Padding::ZERO })
        .into()
}

/// The yes/no question shown under a workspace before it is deleted.
fn delete_workspace_confirm(id: u64, name: &str) -> Element<'static, Message> {
    column![
        text(format!("Delete \"{name}\"? Its sessions move to Default.")).size(13),
        row![
            button(text("Delete").size(13)).on_press(Message::DeleteWorkspace(id)).style(button::danger),
            button(text("Cancel").size(13)).on_press(Message::DeleteWorkspaceCancel).style(button::secondary),
        ]
        .spacing(6),
    ]
    .spacing(6)
    .padding(iced::Padding { top: 4.0, right: 0.0, bottom: 8.0, left: 0.0 })
    .into()
}

/// What the sidebar shows for one open tab. Hashable, so the lazy tree
/// rebuilds only when this text changes.
#[derive(Clone, Hash)]
struct TabRow {
    id: u64,
    session: Option<String>,
    title: String,
    folder: String,
    icon: &'static str,
    status: &'static str,
    selected: bool,
}

fn tab_row(t: &TabRow) -> Element<'static, Message> {
    row![
        button(row_label(format!("{} {}", t.icon, t.title), format!("   {} · {}", t.folder, t.status)))
            .on_press(Message::SelectTab(t.id))
            .width(Length::Fill)
            // Green marks a running tab; the selected one stays blue so it still stands out.
            .style(if t.selected { button::primary } else { button::success }),
        button(text("✕").size(15)).padding([10, 12]).on_press(Message::CloseTab(t.id)).style(close_style),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .into()
}

/// Two-line sidebar label. The name is larger so it is easy to scan;
/// the details line is smaller so it does not compete with it.
fn row_label(title: String, details: String) -> Element<'static, Message> {
    column![
        text(title).size(16),
        text(details).size(12),
    ]
    .spacing(2)
    .into()
}

fn indent(e: Element<'static, Message>) -> Element<'static, Message> {
    container(e).padding(iced::Padding { left: TREE_INDENT, ..iced::Padding::ZERO }).into()
}

fn folder_label(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn time_group(t: DateTime<Utc>) -> &'static str {
    let today = Local::now().date_naive();
    let days = (today - t.with_timezone(&Local).date_naive()).num_days();
    match days {
        ..=0 => "TODAY",
        1 => "YESTERDAY",
        2..=7 => "LAST 7 DAYS",
        8..=30 => "LAST 30 DAYS",
        _ => "OLDER",
    }
}

fn relative_time(t: DateTime<Utc>) -> String {
    let local = t.with_timezone(&Local);
    let mins = (Local::now() - local).num_minutes();
    match time_group(t) {
        "TODAY" if mins < 60 => format!("{}m", mins.max(0)),
        "TODAY" => local.format("%H:%M").to_string(),
        "YESTERDAY" => "yesterday".into(),
        _ => local.format("%d %b").to_string(),
    }
}

fn resize_event(event: iced::Event, _status: iced::event::Status, _window: iced::window::Id) -> Option<Message> {
    match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => Some(Message::ResizeTo(position.x)),
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => Some(Message::ResizeEnd),
        _ => None,
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
            "b" => Some(Message::ToggleHideSidebar),
            d if d.len() == 1 && ('1'..='9').contains(&d.chars().next()?) => {
                Some(Message::SelectIndex(d.parse::<usize>().ok()? - 1))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Claude Code puts its state at the start of the terminal title:
/// "✳ <title>" while it waits, a spinner like "◐ <title>" while it works.
/// None for any other title, such as a plain shell prompt.
fn claude_title_busy(title: &str) -> Option<bool> {
    let mut chars = title.chars();
    let first = chars.next()?;
    if chars.next() != Some(' ') || first.is_alphanumeric() || first.is_ascii() {
        return None;
    }
    Some(first != '✳')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("projects").join("-tmp-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let cwd = work.display();
        let now = Utc::now().to_rfc3339();
        std::fs::write(
            proj.join("s1.jsonl"),
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"s1\",\"cwd\":\"{cwd}\",\"timestamp\":\"{now}\",\"message\":{{\"content\":\"hello\"}}}}\n\
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

    /// Hide collapses the whole sidebar (privacy), and stays hidden after restart.
    #[test]
    fn hide_sidebar_collapses_it_and_is_remembered() {
        let (dir, mut app) = fixture();
        let _ = app.update(Message::NewTerminal);
        let msgs = vec![Message::ToggleHideSidebar];
        assert!(!found_after_frames(&mut app, msgs, "○ Fix webhook retry"), "history still visible");
        assert!(!found_after_frames(&mut app, vec![], "ACTIVE"), "active tabs still visible");
        assert!(!found_after_frames(&mut app, vec![], "+ New session"), "sidebar buttons still visible");
        assert!(found_after_frames(&mut app, vec![], "›"), "no button to show the sidebar again");

        let mut again = App::with_paths(Some(dir.path().join("projects")), Some(dir.path().join("state.json")));
        assert!(!found_after_frames(&mut again, vec![], "○ Fix webhook retry"), "not remembered");
        assert!(found_after_frames(&mut again, vec![Message::ToggleHideSidebar], "○ Fix webhook retry"));
    }

    /// Dragging the divider sets the sidebar width, within limits, and it is remembered.
    #[test]
    fn sidebar_resize_is_clamped_and_remembered() {
        let (dir, mut app) = fixture();
        for m in [Message::ResizeStart, Message::ResizeTo(420.0), Message::ResizeEnd] {
            let _ = app.update(m);
        }
        assert_eq!(app.sidebar_width, 420.0);
        let _ = app.update(Message::ResizeTo(900.0));
        assert_eq!(app.sidebar_width, 420.0, "moves after release must be ignored");
        for m in [Message::ResizeStart, Message::ResizeTo(10.0), Message::ResizeEnd] {
            let _ = app.update(m);
        }
        assert_eq!(app.sidebar_width, SIDEBAR_MIN);
        for m in [Message::ResizeStart, Message::ResizeTo(450.0), Message::ResizeEnd] {
            let _ = app.update(m);
        }
        let again = App::with_paths(Some(dir.path().join("projects")), Some(dir.path().join("state.json")));
        assert_eq!(again.sidebar_width, 450.0);
        assert!(found_after_frames(&mut app, vec![], "○ Fix webhook retry"));
    }

    /// A new workspace shows in the sidebar. A session moved into it is
    /// listed under it, hidden when it is collapsed, and all of it is
    /// remembered after restart. Deleting it sends the session back to Default.
    #[test]
    fn workspaces_group_sessions_as_a_collapsible_tree() {
        let (dir, mut app) = fixture();
        assert!(found_after_frames(&mut app, vec![], "▾ Default  (1)"), "no default workspace");

        let msgs = vec![
            Message::NewWorkspaceStart,
            Message::NewWorkspaceName("  Backend ".into()),
            Message::NewWorkspaceCreate,
            Message::MoveSession("s1".into(), Some(1)),
        ];
        assert!(found_after_frames(&mut app, msgs, "▾ Backend  (1)"), "workspace not shown");
        assert!(found_after_frames(&mut app, vec![], "▾ Default  (0)"));
        assert!(found_after_frames(&mut app, vec![], "○ Fix webhook retry"));

        assert!(!found_after_frames(&mut app, vec![Message::ToggleWorkspace(Some(1))], "○ Fix webhook retry"), "not collapsed");
        assert!(found_after_frames(&mut app, vec![], "▸ Backend  (1)"));
        // Search opens collapsed workspaces so matches are not hidden.
        assert!(found_after_frames(&mut app, vec![Message::Search("webhook".into())], "○ Fix webhook retry"));
        let _ = app.update(Message::Search(String::new()));

        let mut again = App::with_paths(Some(dir.path().join("projects")), Some(dir.path().join("state.json")));
        assert!(found_after_frames(&mut again, vec![], "▸ Backend  (1)"), "not remembered");
        assert!(!found_after_frames(&mut again, vec![], "○ Fix webhook retry"));

        let msgs = vec![Message::DeleteWorkspace(1)];
        assert!(found_after_frames(&mut again, msgs, "○ Fix webhook retry"));
        assert!(found_after_frames(&mut again, vec![], "▾ Default  (1)"));
        assert!(!found_after_frames(&mut again, vec![Message::ToggleWorkspace(None)], "○ Fix webhook retry"), "default not collapsed");
    }

    /// A session open in a tab stays inside its workspace (there is no
    /// separate ACTIVE list). A collapsed workspace shows how many tabs are
    /// open in it, and selecting such a tab opens the workspace again.
    #[test]
    fn open_tab_stays_inside_its_workspace() {
        let (_dir, mut app) = fixture();
        for m in [
            Message::NewWorkspaceStart,
            Message::NewWorkspaceName("Backend".into()),
            Message::NewWorkspaceCreate,
            Message::MoveSession("s1".into(), Some(1)),
            Message::NewTerminal,
        ] {
            let _ = app.update(m);
        }
        app.tabs[0].session_id = Some("s1".into());
        let tab = app.tabs[0].id;

        assert!(!found_after_frames(&mut app, vec![], "ACTIVE"), "separate ACTIVE list still shown");
        assert!(found_after_frames(&mut app, vec![], "▾ Backend  (1)  ● 1 open"), "open badge missing");
        assert!(found_after_frames(&mut app, vec![], "▸ Fix webhook retry"), "tab row not in its workspace");
        assert!(found_after_frames(&mut app, vec![], "▾ Default  (0)"));

        let msgs = vec![Message::ToggleWorkspace(Some(1))];
        assert!(!found_after_frames(&mut app, msgs, "▸ Fix webhook retry"), "not collapsed");
        assert!(found_after_frames(&mut app, vec![], "▸ Backend  (1)  ● 1 open"));

        assert!(found_after_frames(&mut app, vec![Message::SelectTab(tab)], "▸ Fix webhook retry"), "not opened on select");
        assert!(found_after_frames(&mut app, vec![], "▾ Backend  (1)  ● 1 open"));
    }

    /// Claude redraws its status line now and then while idle. That output
    /// must not flip the row to "working". Claude's own title tells the truth:
    /// a spinner while it works, "✳" while it waits.
    #[test]
    fn idle_redraw_does_not_show_working() {
        let (_dir, mut app) = fixture();
        let _ = app.update(Message::NewTerminal);
        app.tabs[0].kind = TabKind::Claude;
        let id = app.tabs[0].id;
        let folder = folder_label(&app.tabs[0].cwd);
        let event = |e| Message::Terminal(term::Event::BackendCall(id, term::BackendCommand::ProcessAlacrittyEvent(e)));

        let idle = vec![event(term::AlacrittyEvent::Title("✳ Fix webhook".into())), event(term::AlacrittyEvent::Wakeup)];
        assert!(found_after_frames(&mut app, idle, &format!("   {folder} · waiting")), "idle redraw shown as working");

        let busy = vec![event(term::AlacrittyEvent::Title("◐ Fix webhook".into()))];
        app.tabs[0].last_output = Instant::now() - BUSY_WINDOW * 5;
        assert!(found_after_frames(&mut app, busy, &format!("   {folder} · working")), "spinner title not shown as working");
    }

    /// The ✕ on a workspace only asks first. Cancel keeps it; Delete removes it.
    #[test]
    fn delete_workspace_asks_for_confirmation() {
        let (_dir, mut app) = fixture();
        for m in [Message::NewWorkspaceStart, Message::NewWorkspaceName("Backend".into()), Message::NewWorkspaceCreate] {
            let _ = app.update(m);
        }
        let msgs = vec![Message::DeleteWorkspaceAsk(1)];
        assert!(found_after_frames(&mut app, msgs, "Delete \"Backend\"? Its sessions move to Default."), "no confirmation shown");
        assert_eq!(app.workspaces.len(), 1, "deleted without confirmation");

        let msgs = vec![Message::DeleteWorkspaceCancel];
        assert!(!found_after_frames(&mut app, msgs, "Delete \"Backend\"? Its sessions move to Default."));
        assert_eq!(app.workspaces.len(), 1);

        let _ = app.update(Message::DeleteWorkspaceAsk(1));
        let _ = app.update(Message::DeleteWorkspace(1));
        assert!(app.workspaces.is_empty());
        assert!(app.confirm_delete_workspace.is_none());
    }

    #[test]
    fn empty_workspace_name_is_ignored() {
        let (_dir, mut app) = fixture();
        for m in [Message::NewWorkspaceStart, Message::NewWorkspaceName("   ".into()), Message::NewWorkspaceCreate] {
            let _ = app.update(m);
        }
        assert!(app.workspaces.is_empty());
        assert!(app.new_workspace.is_none());
    }

    #[test]
    fn sidebar_shows_history_sessions() {
        let (_dir, mut app) = fixture();
        assert!(found_after_frames(&mut app, vec![], "○ Fix webhook retry"), "session not visible");
        assert!(found_after_frames(&mut app, vec![], "TODAY"), "time group not visible");
        assert!(found_after_frames(&mut app, vec![], "Sessions: 1"), "session count not visible");
    }

    /// A session whose folder was deleted cannot be resumed, so it must
    /// leave the sidebar (and the count), even though its file still exists.
    #[test]
    fn session_disappears_when_its_folder_is_deleted() {
        let (dir, mut app) = fixture();
        assert!(found_after_frames(&mut app, vec![], "○ Fix webhook retry"));
        std::fs::remove_dir_all(dir.path().join("work")).unwrap();
        assert!(!found_after_frames(&mut app, vec![Message::Tick], "○ Fix webhook retry"), "row still shown");
        assert!(found_after_frames(&mut app, vec![], "Sessions: 0"), "count not updated");

        // Restore the folder: the session comes back.
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        assert!(found_after_frames(&mut app, vec![Message::Tick], "○ Fix webhook retry"));
    }
}

/// Plain like a text button, but red while hovered or pressed, so it is clear
/// that clicking closes the tab.
fn close_style(theme: &iced::Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => button::danger(theme, status),
        _ => button::text(theme, status),
    }
}
