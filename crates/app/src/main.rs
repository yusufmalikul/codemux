//! Codemux: a session manager for Claude Code.
//! Sidebar of sessions on the left, a live terminal on the right.

// Windows: a release build is a GUI app, so no console window opens next to it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use chrono::{DateTime, Local, Utc};
use codemux_core::detect::{self, ClaudeProc, ProcScanner};
use codemux_core::launch::{self, ClaudeMode, LaunchSpec};
use codemux_core::paths;
use codemux_core::sessions::{Index, Role, Root, SessionMeta};
use codemux_core::store::{Account, AppState, Workspace};
use codemux_terminal as term;
use iced::widget::{button, column, container, lazy, mouse_area, pick_list, row, rule, scrollable, text, text_input, tooltip, Column};
use iced::{keyboard, time, Element, Length, Subscription, Task};
use std::collections::{BTreeMap, HashSet};
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

fn main() -> iced::Result {
    launch::clean_inherited_env();
    #[cfg(unix)]
    launch::load_shell_path();
    iced::application(App::new, App::update, App::view)
        .title("Codemux")
        .font(include_bytes!("../assets/fonts/FiraSans-Bold.ttf").as_slice())
        .window(window_settings())
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

fn window_settings() -> iced::window::Settings {
    iced::window::Settings {
        size: iced::Size::new(1280.0, 800.0),
        // macOS takes the Dock icon from set_dock_icon. Windows and X11 show this one.
        icon: if cfg!(target_os = "macos") { None } else { window_icon() },
        // Wayland finds the icon through the .desktop file with this name
        // (scripts/install-linux.sh).
        #[cfg(target_os = "linux")]
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: "codemux".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn window_icon() -> Option<iced::window::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/icon-256.png")));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    iced::window::icon::from_rgba(buf, info.width, info.height).ok()
}

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

struct App {
    tabs: Vec<Tab>,
    selected: Selected,
    next_id: u64,
    index: Index,
    /// Bumped when the index changes, so the lazy sidebar knows to rebuild.
    index_version: u64,
    /// Config folder of the default account (`~/.claude`).
    default_config: Option<PathBuf>,
    /// Extra accounts (saved in state.json).
    accounts: Vec<Account>,
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
    /// The "+" menu (new session, terminal, workspace) is open.
    add_menu: bool,
    /// Names the user gave to sessions, by session ID (saved in state.json).
    session_names: BTreeMap<String, String>,
    /// Session being renamed, and the text typed so far.
    renaming: Option<(String, String)>,
}

/// One choice in the "Account" picker. `id` 0 is the default account.
#[derive(Debug, Clone, PartialEq)]
struct AccountChoice {
    id: u64,
    name: String,
}

impl std::fmt::Display for AccountChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
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
    /// Open or close the "+" menu in the sidebar.
    ToggleAddMenu,
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
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let state_path = paths::app_data_dir().map(|d| d.join("state.json"));
        let scale = iced::window::latest()
            .and_then(iced::window::scale_factor)
            .map(Message::ScaleFactor);
        (Self::with_paths(paths::claude_dir(), state_path), scale)
    }

    /// `default_config` is the default account's Claude config folder.
    fn with_paths(default_config: Option<PathBuf>, state_path: Option<PathBuf>) -> Self {
        let state = state_path.as_deref().map(AppState::load).unwrap_or_default();
        let previously_open = state.open_sessions;
        let backup_dir = state_path.as_deref().and_then(|p| p.parent()).map(|d| d.join("moved-sessions"));
        if let Some(dir) = &backup_dir {
            codemux_core::accounts::prune_backups(dir, MOVE_BACKUP_AGE);
        }
        let mut app = Self {
            tabs: Vec::new(),
            selected: Selected::None,
            next_id: 0,
            index: Index::default(),
            index_version: 0,
            default_config,
            accounts: state.accounts,
            new_account: None,
            pending_switch: None,
            backup_dir,
            home: paths::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            #[cfg(test)]
            resumed: Vec::new(),
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
            add_menu: false,
            session_names: state.session_names,
            renaming: None,
        };
        app.refresh_index();
        app
    }

    fn refresh_index(&mut self) {
        if self.index.refresh(&self.roots()) {
            self.index_version += 1;
        }
    }

    /// Config folder of an account. 0 is the default account.
    fn config_of(&self, account: u64) -> Option<PathBuf> {
        match account {
            0 => self.default_config.clone(),
            id => self.accounts.iter().find(|a| a.id == id).map(|a| a.config_dir.clone()),
        }
    }

    /// `CLAUDE_CONFIG_DIR` for an account. The default account leaves it
    /// unset (see launch::claude).
    fn launch_config(&self, account: u64) -> Option<PathBuf> {
        (account != 0).then(|| self.config_of(account)).flatten()
    }

    fn roots(&self) -> Vec<Root> {
        std::iter::once(0)
            .chain(self.accounts.iter().map(|a| a.id))
            .filter_map(|id| self.config_of(id).map(|c| Root { account: id, projects_dir: c.join("projects") }))
            .collect()
    }

    fn account_name(&self, account: u64) -> String {
        self.accounts.iter().find(|a| a.id == account).map(|a| a.name.clone()).unwrap_or_else(|| DEFAULT_ACCOUNT.into())
    }

    fn account_choices(&self) -> Vec<AccountChoice> {
        std::iter::once(AccountChoice { id: 0, name: DEFAULT_ACCOUNT.into() })
            .chain(self.accounts.iter().map(|a| AccountChoice { id: a.id, name: a.name.clone() }))
            .collect()
    }

    /// The account a tab runs with: its session file's account, or the one
    /// it was started with.
    fn tab_account(&self, tab: &Tab) -> u64 {
        tab.session_id.as_deref().and_then(|s| self.index.get(s)).map_or(tab.account, |m| m.account)
    }

    /// Account of what the user is looking at. A new session uses it.
    fn current_account(&self) -> u64 {
        let id = match &self.selected {
            Selected::Tab(id) => self.tabs.iter().find(|t| t.id == *id).map_or(0, |t| self.tab_account(t)),
            Selected::Session(sid) => self.index.get(sid).map_or(0, |m| m.account),
            _ => 0,
        };
        if id == 0 || self.accounts.iter().any(|a| a.id == id) { id } else { 0 }
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

    /// Expand the workspace that holds a tab, so the selected tab is never
    /// hidden inside a collapsed workspace.
    fn reveal_tab(&mut self, id: u64) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        let collapsed = match tab.workspace {
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
        let projects_dirs: Vec<PathBuf> = self.roots().into_iter().map(|r| r.projects_dir).collect();
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
            if let Some(sid) = detect::match_session(&projects_dirs, &cwd, proc_.started, &others) {
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
            Message::WindowOpened => {
                set_dock_icon();
            }
            Message::Tick => {
                self.refresh_index();
                let before: Vec<_> = self.tabs.iter().map(|t| t.session_id.clone()).collect();
                self.detect();
                self.adopt_tab_sessions();
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
            Message::CloseTab(id) => return self.close_tab(id),
            Message::CloseSelected => {
                if let Selected::Tab(id) = self.selected {
                    return self.close_tab(id);
                }
            }
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

    fn workspaces_changed(&mut self) {
        self.workspaces_version += 1;
        self.save_state();
    }

    fn workspace_of(&self, sid: &str) -> Option<u64> {
        self.workspaces.iter().find(|w| w.sessions.iter().any(|s| s == sid)).map(|w| w.id)
    }

    /// The workspace of what the user is looking at: the selected tab or
    /// session. None is the default workspace.
    fn current_workspace(&self) -> Option<u64> {
        match &self.selected {
            Selected::Tab(id) => self.tabs.iter().find(|t| t.id == *id).and_then(|t| t.workspace),
            Selected::Session(sid) => self.workspace_of(sid),
            Selected::None | Selected::Accounts => None,
        }
    }

    /// Keep tabs and their sessions in the same workspace. A session in no
    /// workspace joins its tab's workspace (a new session, or Claude found in
    /// a terminal). A session already in a workspace pulls its tab there
    /// (for example after /resume inside Claude).
    fn adopt_tab_sessions(&mut self) {
        let mut changed = false;
        for i in 0..self.tabs.len() {
            let Some(sid) = self.tabs[i].session_id.clone() else { continue };
            match (self.workspace_of(&sid), self.tabs[i].workspace) {
                (Some(ws), tab_ws) if Some(ws) != tab_ws => {
                    self.tabs[i].workspace = Some(ws);
                    changed = true;
                }
                (None, Some(tab_ws)) => match self.workspaces.iter_mut().find(|w| w.id == tab_ws) {
                    Some(w) => {
                        w.sessions.push(sid);
                        changed = true;
                    }
                    None => self.tabs[i].workspace = None,
                },
                _ => {}
            }
        }
        if changed {
            self.workspaces_changed();
        }
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

    /// Add the typed account, then open a tab to log in to it.
    fn create_account(&mut self) -> Task<Message> {
        let name = self.new_account.take().unwrap_or_default();
        match self.add_account(&name) {
            Ok(Some(id)) => self.login(id),
            Ok(None) => Task::none(),
            Err(e) => {
                self.error = Some(e);
                Task::none()
            }
        }
    }

    /// Add an account with its folder `<home>/.claude-<name>`. A folder that
    /// already exists (for example made by hand) is used as it is, login
    /// included. Ok(None) for an empty name.
    fn add_account(&mut self, name: &str) -> Result<Option<u64>, String> {
        let name = name.trim().to_string();
        let Some(dir) = paths::account_dir_for(&self.home, &name) else { return Ok(None) };
        let same_dir = |d: &PathBuf| self.config_of(0).as_ref() == Some(d) || self.accounts.iter().any(|a| &a.config_dir == d);
        if same_dir(&dir) || self.accounts.iter().any(|a| a.name.eq_ignore_ascii_case(&name)) || name.eq_ignore_ascii_case(DEFAULT_ACCOUNT) {
            return Err(format!("An account named \"{name}\" already exists."));
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
        let id = self.accounts.iter().map(|a| a.id).max().unwrap_or(0) + 1;
        self.accounts.push(Account { id, name, config_dir: dir });
        self.error = None;
        self.accounts_changed();
        Ok(Some(id))
    }

    fn login(&mut self, id: u64) -> Task<Message> {
        let Some(dir) = self.config_of(id).filter(|_| id != 0) else { return Task::none() };
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        self.spawn(TabKind::Shell, launch::login(&dir, home), None, id)
    }

    /// Forget an account. Its folder, login and sessions stay on disk.
    fn remove_account(&mut self, id: u64) {
        let in_use = self.tabs.iter().any(|t| self.tab_account(t) == id) || self.pending_switch.as_ref().is_some_and(|p| p.to == id);
        if in_use {
            self.error = Some("A tab still runs with this account. Close it first.".into());
            return;
        }
        self.accounts.retain(|a| a.id != id);
        self.error = None;
        self.accounts_changed();
    }

    fn accounts_changed(&mut self) {
        self.refresh_index();
        self.workspaces_changed();
    }

    /// Continue a session with another account (spec 11.7). A running
    /// session is stopped first, and only while Claude waits for input.
    fn switch_account(&mut self, sid: String, to: u64) -> Task<Message> {
        let Some(from) = self.index.get(&sid).map(|m| m.account) else {
            self.error = Some("The session has no file yet. Send a first message, then switch.".into());
            return Task::none();
        };
        if from == to || self.config_of(to).is_none() || self.pending_switch.is_some() {
            return Task::none();
        }
        let Some(tab) = self.tabs.iter().find(|t| t.session_id.as_deref() == Some(sid.as_str())) else {
            if self.running_claude(&sid, from).is_some() {
                self.error = Some("Claude still runs this session outside Codemux. Exit it there first, then switch.".into());
                return Task::none();
            }
            self.pending_switch = Some(PendingSwitch { sid, to, pid: None, started: Instant::now(), resume: false });
            return self.switch_check();
        };
        if tab.kind == TabKind::Shell {
            self.error = Some("Claude runs inside a terminal tab. Exit Claude there first, then switch.".into());
            return Task::none();
        }
        if self.tab_status(tab) == "working" {
            self.error = Some("Claude is still working. Wait until it finishes, then switch.".into());
            return Task::none();
        }
        let (tab_id, pid) = (tab.id, tab.term.child_pid());
        self.pending_switch = Some(PendingSwitch { sid, to, pid, started: Instant::now(), resume: true });
        // Closing the tab stops Claude. The files move once it has exited.
        let _ = self.close_tab(tab_id);
        self.switch_check()
    }

    /// A running Claude process that has this session open: the tab's own,
    /// or one Claude itself lists in `<config>/sessions/`.
    fn running_claude(&mut self, sid: &str, account: u64) -> Option<u32> {
        let claimed = self.config_of(account).map(|c| codemux_core::accounts::claimed_by(&c, sid)).unwrap_or_default();
        claimed.into_iter().find(|pid| self.scanner.is_running(*pid))
    }

    fn switch_check(&mut self) -> Task<Message> {
        let Some(p) = &self.pending_switch else { return Task::none() };
        let (sid, tab_pid, started) = (p.sid.clone(), p.pid, p.started);
        let from = self.index.get(&sid).map_or(0, |m| m.account);
        let tab_running = tab_pid.is_some_and(|pid| self.scanner.is_running(pid));
        if tab_running || self.running_claude(&sid, from).is_some() {
            if started.elapsed() > SWITCH_TIMEOUT {
                self.pending_switch = None;
                self.error = Some("Claude still runs this session, so it was not moved.".into());
            }
            return Task::none();
        }
        let p = self.pending_switch.take().expect("checked above");
        let moved = (|| {
            let m = self.index.get(&p.sid).ok_or("The session file is gone.")?;
            let from = self.config_of(m.account).ok_or("Unknown account.")?;
            let to = self.config_of(p.to).ok_or("Unknown account.")?;
            let backups = self.backup_dir.clone().ok_or("No folder for backups.")?;
            codemux_core::accounts::move_session(&from, &to, &m.path, &backups).map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        })();
        self.refresh_index();
        match moved {
            Ok(()) => {
                self.error = None;
                if p.resume {
                    return self.resume(p.sid);
                }
                self.selected = Selected::Session(p.sid);
            }
            Err(e) => self.error = Some(format!("Failed to move the session: {e}")),
        }
        Task::none()
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
            terms
                .chain([time::every(TICK).map(|_| Message::Tick), iced::event::listen_with(shortcut)])
                // Follow the mouse only while dragging the divider.
                .chain(self.resizing.then(|| iced::event::listen_with(resize_event)))
                .chain(self.pending_switch.is_some().then(|| time::every(SWITCH_POLL).map(|_| Message::SwitchCheck))),
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

    /// The session count lives in the search box, so it takes no row of its own.
    fn search_placeholder(&self) -> String {
        match self.index.count() {
            1 => "Search 1 session…".into(),
            n => format!("Search {n} sessions…"),
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
                let status = self.tab_status(tab);
                TabRow {
                    account: self.account_tag(self.tab_account(tab)),
                    id: tab.id,
                    session: tab.session_id.clone(),
                    workspace: tab.workspace,
                    title,
                    folder,
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

        let mac = cfg!(target_os = "macos");
        let key = |k: &str| if mac { format!("⌘{k}") } else { format!("Ctrl+Shift+{k}") };
        let mut top = column![row![
            text_input(&self.search_placeholder(), &self.search).on_input(Message::Search).size(15),
            tooltip(
                button(text("+").size(16)).padding([4, 12]).on_press(Message::ToggleAddMenu),
                container(text("New session, terminal or workspace").size(12)).padding(6).style(container::rounded_box),
                tooltip::Position::Bottom,
            ),
            tooltip(
                button(text("‹").size(16)).padding([4, 10]).on_press(Message::ToggleHideSidebar).style(button::secondary),
                container(text(format!("Hide sidebar ({})", key("B"))).size(12)).padding(6).style(container::rounded_box),
                tooltip::Position::Bottom,
            ),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center)]
        .spacing(6);
        if self.add_menu {
            let item = |label: &str, shortcut: String, msg: Message| {
                button(row![text(label.to_string()).size(14).width(Length::Fill), text(shortcut).size(12)])
                    .on_press(msg)
                    .width(Length::Fill)
                    .style(button::text)
            };
            top = top.push(
                container(
                    column![
                        item("New session", key("N"), Message::NewSession),
                        item("New terminal", key("T"), Message::NewTerminal),
                        item("New workspace", String::new(), Message::NewWorkspaceStart),
                        item("Accounts", String::new(), Message::ShowAccounts),
                    ]
                    .spacing(2),
                )
                .padding(4)
                .style(container::rounded_box),
            );
        }
        if let Some(name) = &self.new_workspace {
            top = top.push(
                row![
                    text_input("Workspace name…", name)
                        .id(iced::widget::Id::new(WORKSPACE_INPUT))
                        .on_input(Message::NewWorkspaceName)
                        .on_submit(Message::NewWorkspaceCreate)
                        .size(15),
                    button(text("Add").size(14)).on_press(Message::NewWorkspaceCreate),
                    button(text("Cancel").size(14)).on_press(Message::NewWorkspaceCancel).style(button::secondary),
                ]
                .spacing(6),
            );
        }

        column![
            top,
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
                || [Some(self.session_title(m)), Some(m.title()), m.cwd.as_ref().map(|c| c.display().to_string()), m.git_branch.clone(), m.last_prompt.clone()]
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

        // Tabs per workspace.
        let ws_tabs = |id: Option<u64>| -> Vec<&TabRow> {
            tabs.iter().filter(|t| t.workspace == id).filter(|t| tab_matches(t)).collect()
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

    /// Account name for a sidebar row. Empty for the default account, so a
    /// user with one account sees no change.
    fn account_tag(&self, account: u64) -> String {
        if account == 0 { String::new() } else { self.account_name(account) }
    }

    fn session_row(&self, m: &SessionMeta) -> Element<'static, Message> {
        let selected = self.selected == Selected::Session(m.id.clone());
        let label = row![row_label(self.session_title(m), false), account_tag(&self.account_tag(m.account))]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        button(label)
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
                    let heading: Element<'_, Message> = match &tab.session_id {
                        Some(sid) => self.title_or_rename(sid, title, 15, 12),
                        None => text(title).size(15).into(),
                    };
                    let mut header = row![column![heading, text(info.join(" · ")).size(11)].width(Length::Fill)]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                        .padding(8);
                    if let Some(sid) = tab.session_id.as_ref().filter(|_| !self.accounts.is_empty() && tab.kind == TabKind::Claude) {
                        header = header.push(self.account_picker(sid, self.tab_account(tab), 12));
                    }
                    let header = header.push(button(text("■ Close").size(12)).on_press(Message::CloseTab(tab.id)).style(button::danger));
                    column![header, rule::horizontal(1), term::TerminalView::show(&tab.term).map(Message::Terminal)]
                        .into()
                }
                None => empty_state(),
            },
            Selected::Session(sid) => match self.index.get(sid) {
                Some(m) => {
                    let mut col = column![
                        self.title_or_rename(&m.id, self.session_title(m), 22, SMALL_TEXT_SIZE),
                        text(m.cwd.as_ref().map(|c| c.display().to_string()).unwrap_or_default()).size(TEXT_SIZE),
                    ]
                    .spacing(8);
                    if let Some(b) = &m.git_branch {
                        col = col.push(text(format!("Branch: {b}")).size(TEXT_SIZE));
                    }
                    col = col.push(text(format!("Last active: {}", relative_time(m.sort_time()))).size(TEXT_SIZE));
                    col = col.push(text(format!("ID: {}", m.id)).size(SMALL_TEXT_SIZE));
                    let choices = self.workspace_choices();
                    let current = self.workspace_of(&m.id);
                    let selected = choices.iter().find(|c| c.id == current).cloned();
                    let sid = m.id.clone();
                    col = col.push(
                        row![
                            text("Workspace:").size(TEXT_SIZE),
                            pick_list(choices, selected, move |c| Message::MoveSession(sid.clone(), c.id)).text_size(TEXT_SIZE),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    );
                    if !self.accounts.is_empty() {
                        col = col.push(self.account_picker(&m.id, m.account, TEXT_SIZE));
                    }
                    if let Some(p) = self.pending_switch.as_ref().filter(|p| p.sid == m.id) {
                        col = col.push(text(format!("Moving to {}…", self.account_name(p.to))).size(TEXT_SIZE));
                    }
                    let actions: Element<'_, Message> = if self.confirm_delete.as_deref() == Some(m.id.as_str()) {
                        row![
                            text("Delete this session file? This cannot be undone.").size(TEXT_SIZE),
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
                    if !m.recent_messages.is_empty() {
                        let mut prompts = column![text("Recent conversation").size(18)].spacing(8);
                        // Oldest first, newest at the bottom, like Claude Code.
                        for msg in &m.recent_messages {
                            let (who, style): (&str, fn(&iced::Theme) -> container::Style) = match msg.role {
                                Role::User => ("You", container::rounded_box),
                                Role::Assistant => ("Claude", container::bordered_box),
                            };
                            prompts = prompts.push(
                                container(column![text(who).size(SMALL_TEXT_SIZE), text(msg.text.clone()).size(TEXT_SIZE)].spacing(4))
                                    .padding(8)
                                    .width(Length::Fill)
                                    .style(style),
                            );
                        }
                        col = col.push(rule::horizontal(1)).push(prompts);
                    } else if let Some(p) = &m.last_prompt {
                        col = col.push(text(format!("Last prompt: {p}")).size(TEXT_SIZE));
                    }
                    container(scrollable(col.padding(24)).height(Length::Fill)).into()
                }
                None => empty_state(),
            },
            Selected::Accounts => self.accounts_view(),
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

impl App {
    /// "Account: [picker]". Picking another account moves the session there.
    fn account_picker(&self, sid: &str, current: u64, size: u32) -> Element<'_, Message> {
        let choices = self.account_choices();
        let selected = choices.iter().find(|c| c.id == current).cloned();
        let sid = sid.to_string();
        row![
            text("Account:").size(size),
            pick_list(choices, selected, move |c| Message::SwitchAccount(sid.clone(), c.id)).text_size(size),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
    }

    fn accounts_view(&self) -> Element<'_, Message> {
        let folder = |p: Option<PathBuf>| text(p.map(|p| p.display().to_string()).unwrap_or_default()).size(SMALL_TEXT_SIZE);
        let mut col = column![
            text("Accounts").size(22),
            text("Each account is its own Claude Code config folder, with its own login and sessions. \
                  To continue a session with another account, pick the account in the session's view.")
                .size(TEXT_SIZE),
            container(column![text(DEFAULT_ACCOUNT).size(TEXT_SIZE), folder(self.config_of(0))].spacing(2))
                .padding(8)
                .width(Length::Fill)
                .style(container::bordered_box),
        ]
        .spacing(12);
        for a in &self.accounts {
            col = col.push(
                container(
                    row![
                        column![text(a.name.clone()).size(TEXT_SIZE), folder(Some(a.config_dir.clone()))].spacing(2).width(Length::Fill),
                        button(text("Log in").size(SMALL_TEXT_SIZE)).on_press(Message::AccountLogin(a.id)).style(button::secondary),
                        button(text("Remove").size(SMALL_TEXT_SIZE)).on_press(Message::RemoveAccount(a.id)).style(button::danger),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                )
                .padding(8)
                .width(Length::Fill)
                .style(container::bordered_box),
            );
        }
        col = col.push(match &self.new_account {
            Some(name) => row![
                text_input("Account name, for example Pro…", name)
                    .id(iced::widget::Id::new(ACCOUNT_INPUT))
                    .on_input(Message::NewAccountName)
                    .on_submit(Message::NewAccountCreate)
                    .size(TEXT_SIZE as f32),
                button(text("Add and log in")).on_press(Message::NewAccountCreate),
                button(text("Cancel")).on_press(Message::NewAccountCancel).style(button::secondary),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into(),
            None => Element::from(button(text("+ Add account")).on_press(Message::NewAccountStart)),
        });
        col = col.push(
            text("Remove only forgets the account in Codemux. Its folder, login and sessions stay on disk.").size(SMALL_TEXT_SIZE),
        );
        container(scrollable(col.padding(24)).height(Length::Fill)).into()
    }

    /// A session's title with a "Rename" button, or the name box while it is
    /// being renamed.
    fn title_or_rename(&self, sid: &str, title: String, size: u32, button_size: u32) -> Element<'_, Message> {
        if let Some((_, typed)) = self.renaming.as_ref().filter(|(s, _)| s == sid) {
            return row![
                text_input("Session name… (empty = Claude's title)", typed)
                    .id(iced::widget::Id::new(RENAME_INPUT))
                    .on_input(Message::RenameInput)
                    .on_submit(Message::RenameSave)
                    .size(size as f32),
                button(text("Save").size(button_size)).on_press(Message::RenameSave),
                button(text("Cancel").size(button_size)).on_press(Message::RenameCancel).style(button::secondary),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .into();
        }
        row![
            text(title).size(size),
            button(text("Rename").size(button_size)).padding([2, 8]).on_press(Message::RenameStart(sid.to_string())).style(button::secondary),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
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
    /// Account name, shown only for accounts other than the default.
    account: String,
    id: u64,
    session: Option<String>,
    workspace: Option<u64>,
    title: String,
    folder: String,
    status: &'static str,
    selected: bool,
}

fn tab_row(t: &TabRow) -> Element<'static, Message> {
    let label = row![
        text("●").size(12).color(status_color(t.status)),
        row_label(t.title.clone(), true),
        account_tag(&t.account),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    let selected = t.selected;
    let tab = button(label).on_press(Message::SelectTab(t.id)).width(Length::Fill).style(move |theme, status| open_tab_style(theme, status, selected));
    // The accent shows through the left padding as a thin bar on the selected tab.
    let tab = container(tab).padding(iced::Padding { left: 3.0, ..iced::Padding::ZERO }).style(move |_: &iced::Theme| container::Style {
        background: selected.then(|| ACCENT.into()),
        border: iced::border::rounded(4),
        ..container::Style::default()
    });
    row![
        tab,
        button(text("✕").size(15)).padding([10, 12]).on_press(Message::CloseTab(t.id)).style(close_style),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .into()
}

/// Dot color for a tab: amber when Claude waits for input (it needs you),
/// green while it works, gray for a plain shell.
fn status_color(status: &str) -> iced::Color {
    match status {
        "waiting" => iced::Color::from_rgb8(0xf0, 0xb4, 0x29),
        "working" => iced::Color::from_rgb8(0x4c, 0xc3, 0x6a),
        _ => iced::Color::from_rgb8(0x80, 0x80, 0x88),
    }
}

/// Open tabs get a soft background, so they stand out from history rows
/// without being louder than the terminal. The selected one gets a soft
/// accent tint (plus the bar beside it).
fn open_tab_style(theme: &iced::Theme, status: button::Status, selected: bool) -> button::Style {
    let base = button::text(theme, status);
    let alpha = match (selected, status) {
        (true, _) => 0.45,
        (false, button::Status::Hovered | button::Status::Pressed) => 0.12,
        (false, _) => 0.06,
    };
    let color = if selected { ACCENT.scale_alpha(alpha) } else { iced::Color::WHITE.scale_alpha(alpha) };
    button::Style { background: Some(color.into()), border: iced::border::rounded(4), ..base }
}

/// Sidebar label: only the session name, bold for a session open in a tab.
/// Details (folder, branch, time) show in the main area when selected.
fn row_label(title: String, bold: bool) -> Element<'static, Message> {
    text(title).size(TEXT_SIZE).font(if bold { BOLD } else { iced::Font::DEFAULT }).into()
}

/// Small account name at the end of a sidebar row. Nothing when empty.
fn account_tag(name: &str) -> Element<'static, Message> {
    if name.is_empty() {
        return row![].into();
    }
    container(text(name.to_string()).size(12))
        .padding([1, 6])
        .style(|theme: &iced::Theme| container::Style {
            border: iced::border::rounded(4).color(theme.extended_palette().background.strong.color).width(1),
            ..container::Style::default()
        })
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

    #[test]
    fn window_icon_decodes() {
        assert!(window_icon().is_some());
    }

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
        let app = App::with_paths(Some(dir.path().to_path_buf()), Some(dir.path().join("state.json")));
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
        assert!(found_after_frames(&mut app, msgs, "Fix webhook retry"));
    }

    /// Opening a tab adds rows above the history list. The history must stay.
    #[test]
    fn sidebar_keeps_sessions_after_opening_a_terminal() {
        let (_dir, mut app) = fixture();
        let msgs = vec![Message::Tick, Message::NewTerminal, Message::Tick];
        assert!(found_after_frames(&mut app, msgs, "Fix webhook retry"));
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
        assert!(!found_after_frames(&mut app, msgs, "Fix webhook retry"));
        assert!(!file.exists());
        assert!(app.index.get("s1").is_none());
        assert_eq!(app.selected, Selected::None);
    }

    #[test]
    fn delete_is_refused_while_session_runs_in_a_tab() {
        let (dir, mut app) = fixture();
        let _ = app.update(Message::NewTerminal);
        app.tabs[0].session_id = Some("s1".into());
        app.adopt_tab_sessions();
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
        assert!(!found_after_frames(&mut app, msgs, "Fix webhook retry"), "history still visible");
        assert!(!found_after_frames(&mut app, vec![], "ACTIVE"), "active tabs still visible");
        assert!(!found_after_frames(&mut app, vec![], "+ New session"), "sidebar buttons still visible");
        assert!(found_after_frames(&mut app, vec![], "›"), "no button to show the sidebar again");

        let mut again = App::with_paths(Some(dir.path().to_path_buf()), Some(dir.path().join("state.json")));
        assert!(!found_after_frames(&mut again, vec![], "Fix webhook retry"), "not remembered");
        assert!(found_after_frames(&mut again, vec![Message::ToggleHideSidebar], "Fix webhook retry"));
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
        let again = App::with_paths(Some(dir.path().to_path_buf()), Some(dir.path().join("state.json")));
        assert_eq!(again.sidebar_width, 450.0);
        assert!(found_after_frames(&mut app, vec![], "Fix webhook retry"));
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
        assert!(found_after_frames(&mut app, vec![], "Fix webhook retry"));

        assert!(!found_after_frames(&mut app, vec![Message::ToggleWorkspace(Some(1))], "Fix webhook retry"), "not collapsed");
        assert!(found_after_frames(&mut app, vec![], "▸ Backend  (1)"));
        // Search opens collapsed workspaces so matches are not hidden.
        assert!(found_after_frames(&mut app, vec![Message::Search("webhook".into())], "Fix webhook retry"));
        let _ = app.update(Message::Search(String::new()));

        let mut again = App::with_paths(Some(dir.path().to_path_buf()), Some(dir.path().join("state.json")));
        assert!(found_after_frames(&mut again, vec![], "▸ Backend  (1)"), "not remembered");
        assert!(!found_after_frames(&mut again, vec![], "Fix webhook retry"));

        let msgs = vec![Message::DeleteWorkspace(1)];
        assert!(found_after_frames(&mut again, msgs, "Fix webhook retry"));
        assert!(found_after_frames(&mut again, vec![], "▾ Default  (1)"));
        assert!(!found_after_frames(&mut again, vec![Message::ToggleWorkspace(None)], "Fix webhook retry"), "default not collapsed");
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
        app.adopt_tab_sessions();
        // The title also shows above the terminal, so show nothing there:
        // then the title can only come from the sidebar row.
        app.selected = Selected::None;
        let (title, _) = app.tab_title(&app.tabs[0]);
        let row = title.as_str();

        assert!(!found_after_frames(&mut app, vec![], "ACTIVE"), "separate ACTIVE list still shown");
        assert!(found_after_frames(&mut app, vec![], "▾ Backend  (1)  ● 1 open"), "open badge missing");
        assert!(found_after_frames(&mut app, vec![], row), "tab row not in its workspace");
        assert!(found_after_frames(&mut app, vec![], "▾ Default  (0)"));

        let msgs = vec![Message::ToggleWorkspace(Some(1))];
        assert!(!found_after_frames(&mut app, msgs, row), "not collapsed");
        assert!(found_after_frames(&mut app, vec![], "▸ Backend  (1)  ● 1 open"));

        let tab = app.tabs[0].id;
        assert!(found_after_frames(&mut app, vec![Message::SelectTab(tab)], "▾ Backend  (1)  ● 1 open"), "not opened on select");
    }

    /// A new terminal opens in the workspace the user is looking at, and the
    /// Claude session later found in it joins that workspace too.
    #[test]
    fn new_terminal_opens_in_the_current_workspace() {
        let (_dir, mut app) = fixture();
        for m in [
            Message::NewWorkspaceStart,
            Message::NewWorkspaceName("Backend".into()),
            Message::NewWorkspaceCreate,
            Message::MoveSession("s1".into(), Some(1)),
            Message::SelectSession("s1".into()),
            Message::NewTerminal,
        ] {
            let _ = app.update(m);
        }
        let shown = std::mem::replace(&mut app.selected, Selected::None);
        let (row, _) = app.tab_title(&app.tabs[0]);
        assert!(found_after_frames(&mut app, vec![], "▾ Backend  (2)  ● 1 open"), "terminal not in Backend");
        assert!(found_after_frames(&mut app, vec![], &row), "tab row hidden");
        assert!(found_after_frames(&mut app, vec![], "▾ Default  (0)"));
        app.selected = shown;

        // A second terminal opened from that tab stays in Backend too.
        let _ = app.update(Message::NewTerminal);
        assert!(found_after_frames(&mut app, vec![], "▾ Backend  (3)  ● 2 open"));

        // Claude found in the tab: its session is saved into Backend.
        app.tabs[1].session_id = Some("s2".into());
        app.adopt_tab_sessions();
        assert_eq!(app.workspaces[0].sessions, vec!["s1".to_string(), "s2".to_string()]);

        // Moving the session moves its tab with it.
        let _ = app.update(Message::MoveSession("s2".into(), None));
        assert!(found_after_frames(&mut app, vec![], "▾ Backend  (2)  ● 1 open"));
        assert!(found_after_frames(&mut app, vec![], "▾ Default  (1)  ● 1 open"));
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
        let event = |e| Message::Terminal(term::Event::BackendCall(id, term::BackendCommand::ProcessAlacrittyEvent(e)));

        let idle = vec![event(term::AlacrittyEvent::Title("✳ Fix webhook".into())), event(term::AlacrittyEvent::Wakeup)];
        found_after_frames(&mut app, idle, "");
        assert_eq!(app.tab_status(&app.tabs[0]), "waiting", "idle redraw shown as working");

        let busy = vec![event(term::AlacrittyEvent::Title("◐ Fix webhook".into()))];
        app.tabs[0].last_output = Instant::now() - BUSY_WINDOW * 5;
        found_after_frames(&mut app, busy, "");
        assert_eq!(app.tab_status(&app.tabs[0]), "working", "spinner title not shown as working");
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

    /// New session, terminal and workspace live in the "+" menu. Picking
    /// one closes the menu.
    #[test]
    fn add_menu_opens_and_closes() {
        let (_dir, mut app) = fixture();
        assert!(!found_after_frames(&mut app, vec![], "New terminal"), "menu shown before \"+\"");
        assert!(found_after_frames(&mut app, vec![Message::ToggleAddMenu], "New terminal"), "menu not shown");
        assert!(found_after_frames(&mut app, vec![], "New workspace"));
        assert!(!found_after_frames(&mut app, vec![Message::NewTerminal], "New workspace"), "menu stays open");
        assert_eq!(app.tabs.len(), 1);
    }

    /// A renamed session shows its new name in the sidebar, is found by
    /// search, and keeps the name after restart. An empty name goes back to
    /// Claude's title. The session file is never changed.
    #[test]
    fn rename_session_shows_new_name_and_is_remembered() {
        let (dir, mut app) = fixture();
        let file = dir.path().join("projects/-tmp-proj/s1.jsonl");
        let before = std::fs::read(&file).unwrap();
        let msgs = vec![
            Message::SelectSession("s1".into()),
            Message::RenameStart("s1".into()),
            Message::RenameInput("  Webhook work ".into()),
            Message::RenameSave,
        ];
        assert!(found_after_frames(&mut app, msgs, "Webhook work"), "new name not shown");
        assert!(!found_after_frames(&mut app, vec![], "Fix webhook retry"), "old title still shown");
        assert!(found_after_frames(&mut app, vec![Message::Search("webhook work".into())], "Webhook work"), "not found by search");
        let _ = app.update(Message::Search(String::new()));
        assert_eq!(std::fs::read(&file).unwrap(), before, "session file changed");

        let mut again = App::with_paths(Some(dir.path().to_path_buf()), Some(dir.path().join("state.json")));
        assert!(found_after_frames(&mut again, vec![], "Webhook work"), "name not remembered");

        // A tab running the session shows the name too.
        let _ = again.update(Message::NewTerminal);
        again.tabs[0].session_id = Some("s1".into());
        assert_eq!(again.tab_title(&again.tabs[0]).0, "Webhook work");

        let msgs = vec![Message::RenameStart("s1".into()), Message::RenameInput("   ".into()), Message::RenameSave];
        assert!(found_after_frames(&mut again, msgs, "Fix webhook retry"), "empty name did not reset");
        assert!(again.session_names.is_empty());
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
        assert!(found_after_frames(&mut app, vec![], "Fix webhook retry"), "session not visible");
        assert!(found_after_frames(&mut app, vec![], "TODAY"), "time group not visible");
        assert_eq!(app.search_placeholder(), "Search 1 session…", "session count not shown");
    }

    /// A session whose folder was deleted cannot be resumed, so it must
    /// leave the sidebar (and the count), even though its file still exists.
    #[test]
    fn session_disappears_when_its_folder_is_deleted() {
        let (dir, mut app) = fixture();
        assert!(found_after_frames(&mut app, vec![], "Fix webhook retry"));
        std::fs::remove_dir_all(dir.path().join("work")).unwrap();
        assert!(!found_after_frames(&mut app, vec![Message::Tick], "Fix webhook retry"), "row still shown");
        assert_eq!(app.search_placeholder(), "Search 0 sessions…", "count not updated");

        // Restore the folder: the session comes back.
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        assert!(found_after_frames(&mut app, vec![Message::Tick], "Fix webhook retry"));
    }

    fn add_pro(app: &mut App, dir: &std::path::Path) -> u64 {
        app.home = dir.to_path_buf();
        app.add_account("Pro").unwrap().unwrap()
    }

    fn s1_in(config: &std::path::Path) -> PathBuf {
        config.join("projects/-tmp-proj/s1.jsonl")
    }

    /// The account list: an added account gets its own folder, shows in the
    /// list and is remembered. Names must be unique. Remove only forgets it.
    #[test]
    fn accounts_can_be_added_and_removed() {
        let (dir, mut app) = fixture();
        let id = add_pro(&mut app, dir.path());
        assert!(dir.path().join(".claude-pro").is_dir());
        assert!(found_after_frames(&mut app, vec![Message::ShowAccounts], "Pro"), "account not listed");
        assert!(app.add_account(" pro ").is_err(), "same name added twice");
        assert!(app.add_account("Default").is_err());
        assert_eq!(app.add_account("   "), Ok(None));

        let mut again = App::with_paths(Some(dir.path().to_path_buf()), Some(dir.path().join("state.json")));
        assert_eq!(again.accounts.len(), 1, "account not remembered");
        let _ = again.update(Message::RemoveAccount(id));
        assert!(again.accounts.is_empty());
        assert!(dir.path().join(".claude-pro").is_dir(), "remove must not delete the folder");
    }

    /// Default → Pro → Default for a session that is not running. The file
    /// moves each time, the sidebar shows the account, and the newest
    /// conversation always comes along.
    #[test]
    fn switch_account_moves_a_stopped_session_and_back() {
        let (dir, mut app) = fixture();
        let pro = add_pro(&mut app, dir.path());
        let pro_dir = dir.path().join(".claude-pro");
        assert!(!found_after_frames(&mut app, vec![], "Pro"), "tag shown for the default account");

        let msgs = vec![Message::SelectSession("s1".into()), Message::SwitchAccount("s1".into(), pro)];
        assert!(found_after_frames(&mut app, msgs, "Pro"), "account tag missing in the sidebar");
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(!s1_in(dir.path()).exists(), "still in the default account");
        assert!(s1_in(&pro_dir).exists(), "not in the Pro account");
        assert_eq!(app.index.get("s1").unwrap().account, pro);
        assert_eq!(app.index.count(), 1, "session listed twice");

        // Work done with Pro, then back to Default.
        use std::io::Write;
        let line = format!("{{\"type\":\"user\",\"cwd\":\"{}\",\"message\":{{\"content\":\"done with pro\"}}}}\n", dir.path().join("work").display());
        std::fs::OpenOptions::new().append(true).open(s1_in(&pro_dir)).unwrap().write_all(line.as_bytes()).unwrap();
        let _ = app.update(Message::Tick);
        let _ = app.update(Message::SwitchAccount("s1".into(), 0));
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(!s1_in(&pro_dir).exists());
        assert!(std::fs::read_to_string(s1_in(dir.path())).unwrap().contains("done with pro"), "newest conversation lost");
        assert_eq!(app.index.get("s1").unwrap().account, 0);
        assert!(found_after_frames(&mut app, vec![], "done with pro"));
    }

    /// While Claude works, the switch is refused and nothing moves.
    #[test]
    fn switch_account_waits_while_claude_works() {
        let (dir, mut app) = fixture();
        let pro = add_pro(&mut app, dir.path());
        let _ = app.update(Message::NewTerminal);
        app.tabs[0].kind = TabKind::Claude;
        app.tabs[0].session_id = Some("s1".into());
        app.tabs[0].busy = Some(true);
        let _ = app.update(Message::SwitchAccount("s1".into(), pro));
        assert!(app.error.as_deref().unwrap_or_default().contains("still working"));
        assert_eq!(app.tabs.len(), 1, "tab closed while Claude works");
        assert!(s1_in(dir.path()).exists());
    }

    /// An idle running session: the tab is closed, the app waits for the real
    /// process to exit, and only then moves the files.
    #[test]
    fn switch_account_stops_an_idle_tab_before_moving() {
        let (dir, mut app) = fixture();
        let pro = add_pro(&mut app, dir.path());
        // Like Claude: takes a moment to exit after SIGHUP.
        let pid = fake_claude_tab(&mut app, dir.path(), "trap 'sleep 0.5; exit 0' HUP; while :; do sleep 0.05; done");

        let _ = app.update(Message::SwitchAccount("s1".into(), pro));
        assert!(app.tabs.is_empty(), "tab not closed: {:?}", app.error);
        assert!(app.pending_switch.is_some(), "moved before the process exited");
        let start = Instant::now();
        while app.pending_switch.is_some() && start.elapsed() < SWITCH_TIMEOUT {
            std::thread::sleep(SWITCH_POLL);
            let _ = app.update(Message::SwitchCheck);
        }
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(!app.scanner.is_running(pid), "moved while the process still ran");
        let pro_dir = dir.path().join(".claude-pro");
        assert!(s1_in(&pro_dir).exists());
        // Resumed with the new account.
        assert_eq!(app.resumed.len(), 1, "not resumed");
        assert_eq!(app.resumed[0].env[launch::CONFIG_ENV], pro_dir.display().to_string());
        assert!(app.resumed[0].args.last().unwrap().contains("--resume s1"));
    }

    /// A tab that runs `script` as if it were Claude, idle, with session s1.
    fn fake_claude_tab(app: &mut App, cwd: &std::path::Path, script: &str) -> u32 {
        let spec = LaunchSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: Default::default(),
            cwd: cwd.to_path_buf(),
        };
        let _ = app.spawn(TabKind::Claude, spec, Some("s1".into()), 0);
        let tab = app.tabs.last_mut().unwrap();
        tab.busy = Some(false);
        let pid = tab.term.child_pid().expect("no process");
        std::thread::sleep(Duration::from_millis(300));
        assert!(app.scanner.is_running(pid), "process not seen");
        pid
    }

    /// Claude keeps redrawing its screen while idle. After its tab closes,
    /// nobody reads that output any more; the stop must still reach it.
    #[cfg(unix)]
    #[test]
    fn switch_account_stops_a_tab_that_keeps_printing() {
        let (dir, mut app) = fixture();
        let pro = add_pro(&mut app, dir.path());
        // Like Claude: redraws while idle, and writes a lot when it gets SIGHUP.
        let script = "trap 'yes goodbye | head -c 400000; exit 0' HUP; while :; do echo redraw; sleep 0.001; done";
        let pid = fake_claude_tab(&mut app, dir.path(), script);
        // Let the output pile up, like a tab nobody looks at.
        std::thread::sleep(Duration::from_secs(1));

        let _ = app.update(Message::SwitchAccount("s1".into(), pro));
        let start = Instant::now();
        while app.pending_switch.is_some() && start.elapsed() < SWITCH_TIMEOUT + SWITCH_POLL * 2 {
            std::thread::sleep(SWITCH_POLL);
            let _ = app.update(Message::SwitchCheck);
        }
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(!app.scanner.is_running(pid), "process still runs");
        assert!(s1_in(&dir.path().join(".claude-pro")).exists());
    }

    /// Claude started outside the app (or a tab already closed) can still
    /// run the session. Its files must not move under it.
    #[test]
    fn switch_account_refuses_while_claude_runs_elsewhere() {
        let (dir, mut app) = fixture();
        let pro = add_pro(&mut app, dir.path());
        let mut other = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
        std::fs::write(dir.path().join(format!("sessions/{}.json", other.id())), format!(r#"{{"pid":{},"sessionId":"s1"}}"#, other.id())).unwrap();

        let _ = app.update(Message::SwitchAccount("s1".into(), pro));
        assert!(app.error.as_deref().unwrap_or_default().contains("still runs"), "{:?}", app.error);
        assert!(s1_in(dir.path()).exists(), "moved while Claude runs");
        assert!(app.pending_switch.is_none());

        // Claude exits (its file stays, as after a crash): the move works.
        other.kill().unwrap();
        other.wait().unwrap();
        let _ = app.update(Message::SwitchAccount("s1".into(), pro));
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(s1_in(&dir.path().join(".claude-pro")).exists());
    }

    /// Delete works for a session of another account too.
    #[test]
    fn delete_works_in_another_account() {
        let (dir, mut app) = fixture();
        let pro = add_pro(&mut app, dir.path());
        let _ = app.update(Message::SwitchAccount("s1".into(), pro));
        let _ = app.update(Message::DeleteConfirm("s1".into()));
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(!s1_in(&dir.path().join(".claude-pro")).exists());
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
