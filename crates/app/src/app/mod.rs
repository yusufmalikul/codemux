//! The app: state, messages and the update loop. The views and the
//! account and workspace logic live in the submodules.

mod accounts;
mod detail;
mod sidebar;
mod notify;
mod workspaces;
#[cfg(test)]
mod tests;

use chrono::{DateTime, Local, Utc};
use codemux_core::detect::{self, ClaudeProc, ProcScanner};
use codemux_core::launch::{self, ClaudeMode, LaunchSpec};
use codemux_core::paths;
use codemux_core::sessions::{Index, Role, Root, SessionMeta};
use codemux_core::store::{Account, AppState, Workspace};
use codemux_terminal as term;
use iced::widget::{button, container, lazy, mouse_area, pick_list, row, rule, scrollable, text, text_input, tooltip, Column};
use iced::{keyboard, time, Element, Length, Subscription, Task};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(1500);

/// How long the sidebar scrollbar stays visible after the last scroll.
const SCROLLBAR_LINGER: Duration = Duration::from_millis(800);

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

/// Text size of sidebar rows. The session detail view uses it too, so both read the same.
const TEXT_SIZE: u32 = 16;

/// Smaller text for labels and secondary info.
const SMALL_TEXT_SIZE: u32 = 14;

const WORKSPACE_INPUT: &str = "new-workspace-name";

const ACCOUNT_INPUT: &str = "new-account-name";

const DEFAULT_ACCOUNT: &str = "Default";

/// How long to wait for Claude to exit before moving its session.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(10);

const SWITCH_POLL: Duration = Duration::from_millis(200);

/// Originals of moved sessions are kept this long, in case something went wrong.
const MOVE_BACKUP_AGE: Duration = Duration::from_secs(14 * 86_400);

const RENAME_INPUT: &str = "rename-session";

/// iced ships only Fira Sans Regular, so bold text fell back to another
/// font. The app loads Fira Sans Bold itself and asks for it by name.
const BOLD: iced::Font = iced::Font {
    family: iced::font::Family::Name("Fira Sans"),
    weight: iced::font::Weight::Bold,
    ..iced::Font::DEFAULT
};

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
    /// Account the tab started Claude with. Once the session file exists,
    /// its location (the index) is the truth.
    account: u64,
    /// Workspace the tab is shown in. None is the default workspace. Kept in
    /// step with the workspace of its session, once it has one.
    workspace: Option<u64>,
    last_output: Instant,
    /// Busy or idle, read from the title Claude sets. None until Claude sets one.
    busy: Option<bool>,
    /// Status at the last check, to notice when Claude starts waiting (notify.rs).
    last_status: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
enum Selected {
    None,
    Tab(u64),
    Session(String),
    /// The account list: add, sign in, remove.
    Accounts,
}

/// A session waiting for its Claude process to exit before it moves to
/// another account (spec 11.7).
struct PendingSwitch {
    sid: String,
    to: u64,
    /// Claude's process. None if the tab had no process.
    pid: Option<u32>,
    started: Instant,
    /// Open the session again with the new account once it has moved.
    resume: bool,
}

pub(crate) struct App {
    tabs: Vec<Tab>,
    selected: Selected,
    next_id: u64,
    index: Index,
    /// Bumped when the index changes, so the lazy sidebar knows to rebuild.
    index_version: u64,
    /// A background refresh of the index is running (see refresh_index_in_background).
    index_refreshing: bool,
    /// Bumped by every refresh done on this thread. A background result that
    /// started from an older index is thrown away.
    index_gen: u64,
    /// Config folder of the default account (`~/.claude`).
    default_config: Option<PathBuf>,
    /// Extra accounts (saved in state.json).
    accounts: Vec<Account>,
    /// Shared config entries each extra account keeps its own copy of
    /// (see codemux_core::accounts::share_config). Shown on the Accounts page.
    unshared: HashMap<u64, Vec<&'static str>>,
    /// Name typed for a new account. Some while the name box is shown.
    new_account: Option<String>,
    pending_switch: Option<PendingSwitch>,
    /// Where the originals of moved sessions are kept for a while.
    backup_dir: Option<PathBuf>,
    /// New accounts get a config folder here (`<home>/.claude-<name>`).
    home: PathBuf,
    /// Tests record resumes here instead of starting the real `claude`.
    #[cfg(test)]
    resumed: Vec<LaunchSpec>,
    /// Tests record OS notifications here instead of showing them.
    #[cfg(test)]
    notified: Vec<notify::Alert>,
    /// The window has keyboard focus. A tab on screen in a focused window
    /// needs no notification.
    window_focused: bool,
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
    /// Tab whose close waits for "Close anyway", because Claude still works in it.
    confirm_close_tab: Option<u64>,
    /// Workspace whose delete is waiting for a yes or no.
    confirm_delete_workspace: Option<u64>,
    /// Privacy: the sidebar is collapsed (saved in state.json).
    hide_sidebar: bool,
    sidebar_width: f32,
    /// True while the user drags the sidebar divider.
    resizing: bool,
    /// When the sidebar was last scrolled; the scrollbar shows only shortly after.
    sidebar_scrolled: Option<Instant>,
    /// User-made workspaces (saved in state.json). Sessions in none of them
    /// are in the default workspace.
    workspaces: Vec<Workspace>,
    default_collapsed: bool,
    /// Bumped when workspaces change, so the lazy sidebar knows to rebuild.
    workspaces_version: u64,
    /// Name typed for a new workspace. Some while the name box is shown.
    new_workspace: Option<String>,
    /// The "+" menu (new session, terminal, workspace) is open.
    add_menu: bool,
    /// Names the user gave to sessions, by session ID (saved in state.json).
    session_names: BTreeMap<String, String>,
    /// Session being renamed, and the text typed so far.
    renaming: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    Terminal(term::Event),
    Tick,
    /// A background refresh finished: the index it started from (by
    /// generation) and the new index, if anything changed.
    IndexRefreshed(u64, Option<Index>),
    /// The sidebar list was scrolled.
    SidebarScrolled,
    /// Check whether the sidebar scrollbar should hide again.
    SidebarScrollIdle,
    ToggleHideSidebar,
    /// Open or close the "+" menu in the sidebar.
    ToggleAddMenu,
    ResizeStart,
    /// Cursor x while dragging the divider.
    ResizeTo(f32),
    ResizeEnd,
    /// The window is up; macOS resets the Dock icon during launch, so set it now.
    WindowOpened,
    /// The window got or lost keyboard focus.
    WindowFocused(bool),
    NewSession,
    FolderPicked(Option<PathBuf>),
    NewTerminal,
    SelectTab(u64),
    SelectSession(String),
    Resume(String),
    /// Close a tab. Asks first while Claude works in it.
    CloseTab(u64),
    CloseSelected,
    /// Close a tab even though Claude still works in it.
    CloseTabConfirm(u64),
    CloseTabCancel,
    /// Cancel whatever inline input or question is open.
    Escape,
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
    /// Show the name box for a session.
    RenameStart(String),
    RenameInput(String),
    /// Save the typed name. An empty name goes back to Claude's title.
    RenameSave,
    RenameCancel,
    ShowAccounts,
    NewAccountStart,
    NewAccountName(String),
    NewAccountCreate,
    NewAccountCancel,
    /// Open a tab that runs `claude auth login` for an account.
    AccountLogin(u64),
    RemoveAccount(u64),
    /// Continue a session with another account: stop it, move its files, resume.
    SwitchAccount(String, u64),
    /// Check if the stopped Claude of a pending switch has exited.
    SwitchCheck,
    /// Hide the error banner.
    DismissError,
}

/// The work of a background refresh. Some(index) when anything changed.
fn refresh_job(mut index: Index, roots: &[Root]) -> Option<Index> {
    index.refresh(roots).then_some(index)
}

fn folder_label(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
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
        iced::Event::Window(iced::window::Event::Focused) => return Some(Message::WindowFocused(true)),
        iced::Event::Window(iced::window::Event::Unfocused) => return Some(Message::WindowFocused(false)),
        _ => {}
    }
    let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else { return None };
    // Escape needs no modifier. The terminal still gets it too, which is
    // fine: it only cancels inline inputs and questions in the app.
    if key.as_ref() == keyboard::Key::Named(keyboard::key::Named::Escape) {
        return Some(Message::Escape);
    }
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

impl App {
    pub(crate) fn new() -> (Self, Task<Message>) {
        let state_path = paths::app_data_dir().map(|d| d.join("state.json"));
        let scale = iced::window::latest()
            .and_then(iced::window::scale_factor)
            .map(Message::ScaleFactor);
        notify::prepare();
        let mut app = Self::with_paths(paths::claude_dir(), state_path);
        // Read the session files in the background, so the window opens at once.
        let load = app.refresh_index_in_background();
        (app, Task::batch([scale, load]))
    }

    /// `default_config` is the default account's Claude config folder.
    fn with_paths(default_config: Option<PathBuf>, state_path: Option<PathBuf>) -> Self {
        let state = state_path.as_deref().map(AppState::load).unwrap_or_default();
        let previously_open = state.open_sessions;
        let backup_dir = state_path.as_deref().and_then(|p| p.parent()).map(|d| d.join("moved-sessions"));
        if let Some(dir) = &backup_dir {
            codemux_core::accounts::prune_backups(dir, MOVE_BACKUP_AGE);
        }
        Self {
            tabs: Vec::new(),
            selected: Selected::None,
            next_id: 0,
            index: Index::default(),
            index_version: 0,
            index_refreshing: false,
            index_gen: 0,
            default_config,
            accounts: state.accounts,
            unshared: HashMap::new(),
            new_account: None,
            pending_switch: None,
            backup_dir,
            home: paths::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            #[cfg(test)]
            resumed: Vec::new(),
            #[cfg(test)]
            notified: Vec::new(),
            window_focused: true,
            scanner: ProcScanner::default(),
            state_path,
            previously_open,
            search: String::new(),
            error: None,
            scale_factor: 1.0,
            confirm_delete: None,
            confirm_close_tab: None,
            confirm_delete_workspace: None,
            hide_sidebar: state.hide_sidebar,
            sidebar_width: state.sidebar_width.unwrap_or(SIDEBAR_DEFAULT).clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            resizing: false,
            sidebar_scrolled: None,
            workspaces: state.workspaces,
            default_collapsed: state.default_collapsed,
            workspaces_version: 0,
            new_workspace: None,
            add_menu: false,
            session_names: state.session_names,
            renaming: None,
        }
    }

    /// Read the session files on this thread. For user actions that must
    /// show their result at once (delete, move). The timer uses
    /// refresh_index_in_background instead.
    fn refresh_index(&mut self) {
        self.index_gen += 1;
        if self.index.refresh(&self.roots()) {
            self.index_version += 1;
        }
    }

    /// Read the session files on another thread, so the window never waits
    /// for disk. Reading a copy of the index means the sidebar keeps showing
    /// the current one meanwhile. The result comes back as
    /// Message::IndexRefreshed. Nothing starts while a refresh still runs.
    fn refresh_index_in_background(&mut self) -> Task<Message> {
        if self.index_refreshing {
            return Task::none();
        }
        self.index_refreshing = true;
        let gen = self.index_gen;
        let (index, roots) = (self.index.clone(), self.roots());
        #[cfg(test)]
        {
            // Tests drive the app one message at a time, so do the work here.
            self.update(Message::IndexRefreshed(gen, refresh_job(index, &roots)))
        }
        #[cfg(not(test))]
        {
            let (tx, rx) = iced::futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let _ = tx.send(refresh_job(index, &roots));
            });
            Task::perform(rx, move |r| Message::IndexRefreshed(gen, r.ok().flatten()))
        }
    }

    fn save_state(&self) {
        let Some(path) = &self.state_path else { return };
        let open_sessions = self.tabs.iter().filter_map(|t| t.session_id.clone()).collect();
        let _ = AppState { version: 0, open_sessions, hide_sidebar: self.hide_sidebar,
            sidebar_width: Some(self.sidebar_width),
            workspaces: self.workspaces.clone(),
            default_collapsed: self.default_collapsed,
            session_names: self.session_names.clone(),
            accounts: self.accounts.clone(),
        }
        .save(path);
    }

    fn spawn(&mut self, kind: TabKind, spec: LaunchSpec, session_id: Option<String>, account: u64) -> Task<Message> {
        let id = self.next_id;
        self.next_id += 1;
        let settings = term::settings::Settings {
            backend: term::settings::BackendSettings {
                program: spec.program,
                args: spec.args,
                env: spec.env,
                working_directory: Some(spec.cwd.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        // A resumed session keeps its workspace. Anything else opens in the
        // workspace the user is looking at.
        let workspace = match session_id.as_deref().and_then(|s| self.workspace_of(s)) {
            Some(ws) => Some(ws),
            None => self.current_workspace(),
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
                    account,
                    workspace,
                    last_output: Instant::now(),
                    busy: None,
                    last_status: None,
                });
                self.selected = Selected::Tab(id);
                self.adopt_tab_sessions();
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

    fn tab_for_session(&self, sid: &str) -> Option<u64> {
        self.tabs.iter().find(|t| t.session_id.as_deref() == Some(sid)).map(|t| t.id)
    }

    fn resume(&mut self, sid: String) -> Task<Message> {
        if let Some(id) = self.tab_for_session(&sid) {
            self.selected = Selected::Tab(id);
            return self.focus_tab(id);
        }
        let Some((cwd, account)) = self.index.get(&sid).and_then(|m| Some((m.cwd.clone()?, m.account))) else {
            self.error = Some("Folder session tidak diketahui.".into());
            return Task::none();
        };
        if !cwd.is_dir() {
            self.error = Some(format!("Folder no longer exists: {}", cwd.display()));
            return Task::none();
        }
        match launch::claude(cwd, &ClaudeMode::Resume(sid.clone()), self.launch_config(account).as_deref()) {
            Some(spec) => {
                self.previously_open.retain(|s| s != &sid);
                #[cfg(test)]
                {
                    self.resumed.push(spec);
                    Task::none()
                }
                #[cfg(not(test))]
                self.spawn(TabKind::Claude, spec, Some(sid), account)
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
        let config_dirs: Vec<PathBuf> = std::iter::once(0).chain(self.accounts.iter().map(|a| a.id)).filter_map(|id| self.config_of(id)).collect();
        let projects_dirs: Vec<PathBuf> = config_dirs.iter().map(|c| c.join("projects")).collect();
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
            let Some(proc_) = &tab.claude else { continue };
            let cwd = proc_.cwd.clone().unwrap_or_else(|| tab.cwd.clone());
            let mut others = taken.clone();
            if let Some(own) = &tab.session_id {
                others.remove(own);
            }
            // Re-check every tick: /clear or /resume inside Claude switches the file.
            // Ask Claude first. Guess from file times only if it does not say.
            let sid = detect::session_of(&config_dirs, proc_)
                .or_else(|| detect::match_session(&projects_dirs, &cwd, proc_.started, &others));
            if let Some(sid) = sid {
                if tab.session_id.as_deref() != Some(&sid) {
                    if let Some(old) = tab.session_id.replace(sid.clone()) {
                        taken.remove(&old);
                    }
                    taken.insert(sid);
                }
            }
        }
    }

    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        // Every message can change a tab's status (a title, the timer, a
        // focus change), so check after each one.
        self.notify_waiting();
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        if matches!(message, Message::NewSession | Message::NewTerminal | Message::NewWorkspaceStart | Message::ShowAccounts) {
            self.add_menu = false;
        }
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
            Message::SidebarScrolled => {
                self.sidebar_scrolled = Some(Instant::now());
            }
            Message::SidebarScrollIdle => {
                if self.sidebar_scrolled.is_some_and(|t| t.elapsed() >= SCROLLBAR_LINGER) {
                    self.sidebar_scrolled = None;
                }
            }
            Message::WindowOpened => {
                crate::set_dock_icon();
            }
            Message::WindowFocused(focused) => self.window_focused = focused,
            Message::Tick => {
                let load = self.refresh_index_in_background();
                let before: Vec<_> = self.tabs.iter().map(|t| t.session_id.clone()).collect();
                self.detect();
                self.adopt_tab_sessions();
                let after: Vec<_> = self.tabs.iter().map(|t| t.session_id.clone()).collect();
                if before != after {
                    self.save_state();
                }
                return load;
            }
            Message::IndexRefreshed(gen, index) => {
                self.index_refreshing = false;
                // A refresh on this thread ran meanwhile: that index is newer.
                if gen != self.index_gen {
                    return Task::none();
                }
                if let Some(index) = index {
                    self.index = index;
                    self.index_version += 1;
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
                let account = self.current_account();
                if let Some(spec) = launch::claude(dir, &ClaudeMode::New(sid.clone()), self.launch_config(account).as_deref()) {
                    return self.spawn(TabKind::Claude, spec, Some(sid), account);
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
                return self.spawn(TabKind::Shell, launch::shell(cwd), None, 0);
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
            Message::CloseTab(id) => return self.request_close(id),
            Message::CloseSelected => {
                if let Selected::Tab(id) = self.selected {
                    return self.request_close(id);
                }
            }
            Message::CloseTabConfirm(id) => {
                self.confirm_close_tab = None;
                return self.close_tab(id);
            }
            Message::CloseTabCancel => self.confirm_close_tab = None,
            Message::Escape => self.cancel_inline(),
            Message::Search(s) => self.search = s,
            Message::ToggleAddMenu => self.add_menu = !self.add_menu,
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
            Message::DismissError => self.error = None,
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
                for tab in self.tabs.iter_mut().filter(|t| t.workspace == Some(id)) {
                    tab.workspace = None;
                }
                self.workspaces_changed();
            }
            Message::MoveSession(sid, to) => {
                for w in &mut self.workspaces {
                    w.sessions.retain(|s| s != &sid);
                }
                for tab in self.tabs.iter_mut().filter(|t| t.session_id.as_deref() == Some(sid.as_str())) {
                    tab.workspace = to;
                }
                if let Some(w) = to.and_then(|id| self.workspaces.iter_mut().find(|w| w.id == id)) {
                    w.sessions.push(sid);
                }
                self.workspaces_changed();
            }
            Message::RenameStart(sid) => {
                let current = self.session_names.get(&sid).cloned().unwrap_or_default();
                self.renaming = Some((sid, current));
                return iced::widget::operation::focus(iced::widget::Id::new(RENAME_INPUT));
            }
            Message::RenameInput(text) => {
                if let Some((_, t)) = &mut self.renaming {
                    *t = text;
                }
            }
            Message::RenameCancel => self.renaming = None,
            Message::RenameSave => {
                if let Some((sid, name)) = self.renaming.take() {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        self.session_names.remove(&sid);
                    } else {
                        self.session_names.insert(sid, name);
                    }
                    // Names show in the cached sidebar tree.
                    self.workspaces_changed();
                }
            }
            Message::ScaleFactor(scale) => {
                self.scale_factor = scale;
                for tab in &mut self.tabs {
                    tab.term.handle(term::Command::SetScaleFactor(scale));
                }
            }
            Message::ShowAccounts => {
                self.confirm_delete = None;
                self.selected = Selected::Accounts;
                for id in self.accounts.iter().map(|a| a.id).collect::<Vec<_>>() {
                    self.share_config(id);
                }
            }
            Message::NewAccountStart => {
                self.new_account = Some(String::new());
                return iced::widget::operation::focus(iced::widget::Id::new(ACCOUNT_INPUT));
            }
            Message::NewAccountName(name) => self.new_account = Some(name),
            Message::NewAccountCancel => self.new_account = None,
            Message::NewAccountCreate => return self.create_account(),
            Message::AccountLogin(id) => return self.login(id),
            Message::RemoveAccount(id) => self.remove_account(id),
            Message::SwitchAccount(sid, to) => return self.switch_account(sid, to),
            Message::SwitchCheck => return self.switch_check(),
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
            self.error = Some("This session is still running in a tab. Close the tab first.".into());
            return;
        }
        let found = self.index.get(sid).and_then(|m| Some((self.config_of(m.account)?.join("projects"), m.path.clone())));
        let Some((dir, path)) = found else {
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
                self.session_names.remove(sid);
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

    /// Close a tab, or ask first while Claude works in it. The question shows
    /// in the tab's header, so the tab is selected if it was not. A second
    /// close request on the shown question (for example ⌘W twice) closes.
    fn request_close(&mut self, id: u64) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return Task::none() };
        let asked = self.confirm_close_tab == Some(id) && self.selected == Selected::Tab(id);
        if self.tab_status(tab) == "working" && !asked {
            self.confirm_close_tab = Some(id);
            if self.selected != Selected::Tab(id) {
                self.selected = Selected::Tab(id);
                return self.focus_tab(id);
            }
            return Task::none();
        }
        self.confirm_close_tab = None;
        self.close_tab(id)
    }

    /// Escape: drop every inline input and open question.
    fn cancel_inline(&mut self) {
        self.renaming = None;
        self.new_workspace = None;
        self.new_account = None;
        self.add_menu = false;
        self.confirm_delete = None;
        self.confirm_close_tab = None;
        if self.confirm_delete_workspace.take().is_some() {
            self.workspaces_version += 1;
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

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let terms = self.tabs.iter().map(|t| t.term.subscription().map(Message::Terminal));
        Subscription::batch(
            terms
                .chain([time::every(TICK).map(|_| Message::Tick), iced::event::listen_with(shortcut)])
                // Follow the mouse only while dragging the divider.
                .chain(self.resizing.then(|| iced::event::listen_with(resize_event)))
                .chain(self.pending_switch.is_some().then(|| time::every(SWITCH_POLL).map(|_| Message::SwitchCheck)))
                // Hide the sidebar scrollbar once scrolling stops.
                .chain(self.sidebar_scrolled.is_some().then(|| time::every(Duration::from_millis(200)).map(|_| Message::SidebarScrollIdle))),
        )
    }

    pub(crate) fn view(&self) -> Element<'_, Message> {
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

    pub(crate) fn theme(&self) -> iced::Theme {
        let palette = iced::theme::Palette { primary: ACCENT, ..iced::theme::Palette::DARK };
        iced::Theme::custom("codemux", palette)
    }

    /// Title order from spec 8.3: the user's own name first, then Claude's title.
    fn session_title(&self, m: &SessionMeta) -> String {
        self.session_names.get(&m.id).cloned().unwrap_or_else(|| m.title())
    }

    fn tab_title(&self, tab: &Tab) -> (String, String) {
        let folder = folder_label(&tab.cwd);
        if let Some(name) = tab.session_id.as_ref().and_then(|s| self.session_names.get(s)) {
            return (name.clone(), folder);
        }
        match tab.session_id.as_deref().and_then(|s| self.index.get(s)) {
            Some(m) if m.ai_title.is_some() || m.agent_name.is_some() || m.first_prompt.is_some() => {
                (m.title(), folder)
            }
            _ if tab.kind == TabKind::Claude || tab.claude.is_some() => (format!("claude · {folder}"), folder),
            _ => (format!("Terminal · {folder}"), folder),
        }
    }

    fn tab_status(&self, tab: &Tab) -> &'static str {
        let running_claude = tab.kind == TabKind::Claude || tab.claude.is_some();
        if !running_claude {
            "shell"
        } else if tab.busy.unwrap_or_else(|| tab.last_output.elapsed() < BUSY_WINDOW) {
            "working"
        } else {
            "waiting"
        }
    }
}
