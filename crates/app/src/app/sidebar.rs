//! The sidebar: search, the "+" menu and the session tree.

use super::*;
use super::accounts::account_badge;
use super::ellipsis::ellipsis;
use codemux_core::textmatch;
use super::workspaces::{delete_workspace_confirm, workspace_header};
use iced::widget::{column, stack};

/// Soft gray for small text that only adds detail.
const MUTED: iced::Color = iced::Color::from_rgb8(0x80, 0x80, 0x88);

fn section(label: &'static str) -> Element<'static, Message> {
    // Space above so each group stands apart from the rows of the group before it.
    container(text(label).size(14)).padding(iced::Padding { top: 16.0, right: 4.0, bottom: 4.0, left: 4.0 }).into()
}

/// What the sidebar shows for one open tab. Hashable, so the lazy tree
/// rebuilds only when this text changes.
#[derive(Clone, Hash)]
pub(super) struct TabRow {
    /// Account name, shown only for accounts other than the default.
    account: String,
    id: u64,
    session: Option<String>,
    workspace: Option<u64>,
    /// Workspace name, shown only when the user has made workspaces.
    workspace_name: String,
    title: String,
    folder: String,
    status: &'static str,
    selected: bool,
    /// While searching: why the tab matched, when its name does not show it.
    why: Option<String>,
    /// The pinned terminal this tab runs, if any.
    terminal_pin: Option<u64>,
    /// Claude started waiting while the user looked elsewhere, and they
    /// have not opened the tab since (the tabs counted on the Dock).
    unread: bool,
}

fn tab_row(t: &TabRow, query: &str) -> Element<'static, Message> {
    let mut label = row![
        text("●").size(12).color(status_color(t.status, t.unread)),
        row_label(t.title.clone(), true, query),
        account_badge(&t.account),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    if t.unread {
        label = label.push(unread_tag());
    }
    if !t.workspace_name.is_empty() {
        label = label.push(workspace_tag(&t.workspace_name));
    }
    let selected = t.selected;
    let tab = button(with_why(label.into(), t.why.clone(), query)).on_press(Message::SelectTab(t.id)).width(Length::Fill).style(move |theme, status| open_tab_style(theme, status, selected));
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

/// Small gray name of the tab's workspace. The title before it fills the
/// row, so this sits at the right end.
fn workspace_tag(name: &str) -> Element<'static, Message> {
    text(name.to_string()).size(12).color(MUTED).wrapping(text::Wrapping::None).into()
}

/// Amber: Claude waits for the user.
const WAITING: iced::Color = iced::Color::from_rgb8(0xf0, 0xb4, 0x29);

/// Small amber "unread" tag: Claude finished while the user looked
/// elsewhere, and they have not opened the tab since.
fn unread_tag() -> Element<'static, Message> {
    container(text("unread").size(11).color(iced::Color::BLACK).wrapping(text::Wrapping::None))
        .padding([1, 6])
        .style(|_: &iced::Theme| container::Style { background: Some(WAITING.into()), border: iced::border::rounded(8), ..container::Style::default() })
        .into()
}

/// Dot color for a tab: bright amber when Claude waits for input and the
/// user has not seen it yet, soft amber when it waits and they have, green
/// while it works, gray for a plain shell.
fn status_color(status: &str, unread: bool) -> iced::Color {
    match status {
        "waiting" if unread => WAITING,
        "waiting" => WAITING.scale_alpha(0.5),
        "working" => iced::Color::from_rgb8(0x4c, 0xc3, 0x6a),
        _ => MUTED,
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
/// Always one line: a long name ends in "…". The search text is marked.
fn row_label(title: String, bold: bool, query: &str) -> Element<'static, Message> {
    ellipsis(title).size(TEXT_SIZE).font(if bold { BOLD } else { iced::Font::DEFAULT }).mark(query).into()
}

/// A row's label, with a second small gray line under it while searching
/// that shows why the row matched (the matching part of the conversation,
/// or the folder or branch), with the search text marked.
fn with_why(label: Element<'static, Message>, why: Option<String>, query: &str) -> Element<'static, Message> {
    match why {
        Some(why) => column![label, ellipsis(why).size(SMALL_TEXT_SIZE).color(MUTED).mark(query)].spacing(2).into(),
        None => label,
    }
}

fn indent(e: Element<'static, Message>) -> Element<'static, Message> {
    container(e).padding(iced::Padding { left: TREE_INDENT, ..iced::Padding::ZERO }).into()
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

pub(super) fn relative_time(t: DateTime<Utc>) -> String {
    let local = t.with_timezone(&Local);
    let mins = (Local::now() - local).num_minutes();
    match time_group(t) {
        "TODAY" if mins < 60 => format!("{}m", mins.max(0)),
        "TODAY" => local.format("%H:%M").to_string(),
        "YESTERDAY" => "yesterday".into(),
        _ => local.format("%d %b").to_string(),
    }
}

/// Plain like a text button, but red while hovered or pressed, so it is clear
/// that clicking closes the tab.
pub(super) fn close_style(theme: &iced::Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => button::danger(theme, status),
        _ => button::text(theme, status),
    }
}

impl App {
    /// The session count lives in the search box, so it takes no row of its own.
    pub(super) fn search_placeholder(&self) -> String {
        match self.index.count() {
            1 => "Search 1 session…".into(),
            n => format!("Search {n} sessions…"),
        }
    }

    /// The search box, with a clear button at its right end once it has text.
    fn search_box(&self) -> Element<'_, Message> {
        // Room on the right, so long text never runs under the button.
        let padding = iced::Padding { top: 5.0, right: 28.0, bottom: 5.0, left: 5.0 };
        let input = text_input(&self.search_placeholder(), &self.search)
            .id(iced::widget::Id::new(SEARCH_INPUT))
            .on_input(Message::Search)
            .padding(padding)
            .size(15);
        let mut search = stack![input];
        if !self.search.is_empty() {
            let clear = button(text("✕").size(12)).padding([2, 8]).on_press(Message::SearchClear).style(button::text);
            search = search.push(container(clear).align_right(Length::Fill).center_y(Length::Fill));
        }
        search.into()
    }

    pub(super) fn sidebar(&self) -> Element<'_, Message> {
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
                    workspace_name: self.tab_workspace_name(tab),
                    title,
                    folder,
                    status,
                    selected: self.selected == Selected::Tab(tab.id),
                    why: self.tab_why(tab),
                    terminal_pin: tab.terminal_pin,
                    unread: tab.unseen,
                }
            })
            .collect();
        let pinned = self.pinned_list(&tabs);
        let open = self.open_list(&tabs);
        let selected_session = match &self.selected {
            Selected::Session(s) => Some(s.clone()),
            _ => None,
        };
        let minute = Local::now().timestamp() / 60;
        let deps = (
            self.index_version,
            self.workspaces_version,
            self.search.clone(),
            self.content_hits_version,
            selected_session,
            self.previously_open.clone(),
            tabs,
            minute,
        );
        list = list.push(lazy(deps, move |(_, _, _, _, _, _, tabs, _)| self.history_list(tabs)));

        let mac = cfg!(target_os = "macos");
        let key = |k: &str| if mac { format!("⌘{k}") } else { format!("Ctrl+Shift+{k}") };
        let mut top = column![row![
            self.search_box(),
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
        if self.content_searching {
            top = top.push(text("Searching conversations…").size(12).color(MUTED));
        }
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

        let scrolling = self.sidebar_scrolled.is_some();
        let mut sidebar = column![top].spacing(8);
        if let Some(pinned) = pinned {
            sidebar = sidebar.push(pinned);
        }
        if let Some(open) = open {
            sidebar = sidebar.push(open);
        }
        sidebar.push(
            // A thin scrollbar with a small gap, so the rows (and their close
            // buttons) keep most of the sidebar width.
            scrollable(list)
                .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).spacing(2)))
                .on_scroll(|_| Message::SidebarScrolled)
                .style(move |theme, status| {
                    let mut style = scrollable::default(theme, status);
                    // Show the scrollbar only while scrolling, or when the mouse is on it.
                    let on_bar = matches!(
                        status,
                        scrollable::Status::Hovered { is_vertical_scrollbar_hovered: true, .. }
                            | scrollable::Status::Dragged { is_vertical_scrollbar_dragged: true, .. }
                    );
                    if !scrolling && !on_bar {
                        style.vertical_rail.background = None;
                        style.vertical_rail.scroller.background = iced::Color::TRANSPARENT.into();
                    }
                    style
                })
                .height(Length::Fill),
        )
        .into()
    }

    /// Every open tab that is not pinned (pinned ones are in PINNED), in
    /// tab order, from all workspaces. It sits above the tree and does not
    /// scroll with it, so switching between tabs of different workspaces
    /// never needs a scroll. Nothing is shown when no such tab is open or
    /// none matches the search.
    fn open_list(&self, tabs: &[TabRow]) -> Option<Element<'static, Message>> {
        let q = self.search.to_lowercase();
        let rows: Vec<&TabRow> = tabs.iter().filter(|t| !self.is_pinned(t.session.as_deref(), t.terminal_pin) && self.tab_matches(t, &q)).collect();
        if rows.is_empty() {
            return None;
        }
        let mut list = column![section("OPEN")].spacing(2).width(Length::Fill);
        for t in rows {
            list = list.push(tab_row(t, &q));
        }
        // Many tabs must not push the tree off screen: past this height the
        // block scrolls on its own.
        let list = scrollable(list)
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).spacing(2)));
        Some(container(list).max_height(OPEN_LIST_MAX_HEIGHT).into())
    }

    /// Pinned sessions, in pin order, above the OPEN block. Like OPEN it
    /// stays put while the tree scrolls. A pinned session open in a tab
    /// shows here as its tab, not in OPEN. Nothing is shown when no pinned
    /// session is listed or none matches the search.
    fn pinned_list(&self, tabs: &[TabRow]) -> Option<Element<'static, Message>> {
        let q = self.search.to_lowercase();
        let mut rows = Vec::new();
        for pin in &self.pinned {
            let open = tabs.iter().find(|t| match pin {
                Pin::Session(sid) => t.session.as_deref() == Some(sid.as_str()),
                Pin::Terminal(p) => t.terminal_pin == Some(p.id),
            });
            match (open, pin) {
                (Some(t), _) => {
                    if self.tab_matches(t, &q) {
                        rows.push(tab_row(t, &q));
                    }
                }
                (None, Pin::Session(sid)) => {
                    if let Some(m) = self.index.listed(sid).filter(|m| self.session_matches(m, &q)) {
                        rows.push(self.session_row(m));
                    }
                }
                (None, Pin::Terminal(p)) => {
                    if let Some(row) = self.pinned_terminal_row(p, &q) {
                        rows.push(row);
                    }
                }
            }
        }
        if rows.is_empty() {
            return None;
        }
        let mut list = column![section("PINNED")].spacing(2).width(Length::Fill);
        for r in rows {
            list = list.push(r);
        }
        let list = scrollable(list)
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).spacing(2)));
        Some(container(list).max_height(PINNED_LIST_MAX_HEIGHT).into())
    }

    /// A pinned terminal whose tab is closed: click to open it, ✕ to unpin.
    /// None when it does not match the search (by name or folder).
    fn pinned_terminal_row(&self, p: &PinnedTerminal, q: &str) -> Option<Element<'static, Message>> {
        let title = p.name.clone().unwrap_or_else(|| terminal_title(&p.cwd));
        let folder = p.cwd.display().to_string();
        let title_hit = title.to_lowercase().contains(q);
        if !q.is_empty() && !title_hit && !folder.to_lowercase().contains(q) {
            return None;
        }
        let why = (!q.is_empty() && !title_hit).then_some(folder);
        let label = row![row_label(title, false, q)];
        let open = button(with_why(label.into(), why, q)).on_press(Message::OpenPinnedTerminal(p.id)).width(Length::Fill).style(button::text);
        let unpin = tooltip(
            button(text("✕").size(15)).padding([10, 12]).on_press(Message::UnpinTerminal(p.id)).style(close_style),
            container(text("Unpin").size(12)).padding(6).style(container::rounded_box),
            tooltip::Position::Left,
        );
        Some(row![open, unpin].spacing(4).align_y(iced::Alignment::Center).into())
    }

    /// The search filter for a tab: its session's title, folder, branch and
    /// conversation, or for a plain terminal its title and folder.
    fn tab_matches(&self, t: &TabRow, q: &str) -> bool {
        match t.session.as_deref().and_then(|s| self.index.listed(s)) {
            Some(m) => self.session_matches(m, q),
            None => q.is_empty() || [&t.title, &t.folder].iter().any(|s| s.to_lowercase().contains(q)),
        }
    }

    fn session_matches(&self, m: &SessionMeta, q: &str) -> bool {
        q.is_empty()
            || self.content_hits.contains_key(&m.id)
            || [Some(self.session_title(m)), Some(m.title()), m.cwd.as_ref().map(|c| c.display().to_string()), m.git_branch.clone(), m.last_prompt.clone()]
                .into_iter()
                .flatten()
                .any(|s| s.to_lowercase().contains(q))
    }

    /// Why a session matched the search, to show under its name: the part of
    /// the conversation that has the search text, or else the folder or
    /// branch that has it. None when not searching, or when the name itself
    /// has the text, since the marked name already shows it.
    fn match_why(&self, m: &SessionMeta) -> Option<String> {
        let q = textmatch::normalize_query(&self.search);
        if q.is_empty() || !textmatch::match_ranges(&self.session_title(m), &q).is_empty() {
            return None;
        }
        // The conversation search runs in the background; until it is done,
        // the last prompt (already in memory) can show a match at once.
        self.content_hits
            .get(&m.id)
            .cloned()
            .or_else(|| m.last_prompt.as_deref().and_then(|p| textmatch::snippet(p, &q)))
            .or_else(|| m.cwd.as_ref().map(|c| c.display().to_string()).filter(|c| c.to_lowercase().contains(&q)))
            .or_else(|| m.git_branch.clone().filter(|b| b.to_lowercase().contains(&q)))
    }

    /// match_why for an open tab. A plain terminal can match by its folder.
    fn tab_why(&self, tab: &Tab) -> Option<String> {
        if let Some(m) = tab.session_id.as_deref().and_then(|s| self.index.listed(s)) {
            return self.match_why(m);
        }
        let q = textmatch::normalize_query(&self.search);
        let (title, _) = self.tab_title(tab);
        let folder = tab.cwd.display().to_string();
        (!q.is_empty() && !title.to_lowercase().contains(&q) && folder.to_lowercase().contains(&q)).then_some(folder)
    }

    /// The tab's workspace name for its row in the OPEN block. Empty when
    /// the user has no workspaces, since then every tab is in Default.
    fn tab_workspace_name(&self, tab: &Tab) -> String {
        if self.workspaces.is_empty() {
            return String::new();
        }
        tab.workspace
            .and_then(|id| self.workspaces.iter().find(|w| w.id == id))
            .map_or(DEFAULT_WORKSPACE.to_string(), |w| w.name.clone())
    }

    /// The sidebar tree: each workspace with its sessions. Open tabs and
    /// pinned sessions are in the blocks above, so they are left out here.
    /// The workspace header still counts it and shows how many are open.
    pub(super) fn history_list(&self, tabs: &[TabRow]) -> Element<'static, Message> {
        // Pinned sessions are in the PINNED block above, so the tree leaves
        // them out like open ones. They keep their workspace for when they
        // are unpinned.
        let active: HashSet<&str> =
            tabs.iter().filter_map(|t| t.session.as_deref()).chain(self.pinned_sessions()).collect();
        let mut list = Column::new().spacing(2).width(Length::Fill);
        let q = self.search.to_lowercase();
        let matches = |m: &SessionMeta| self.session_matches(m, &q);
        let tab_matches = |t: &TabRow| self.tab_matches(t, &q);

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

    pub(super) fn session_row(&self, m: &SessionMeta) -> Element<'static, Message> {
        let selected = self.selected == Selected::Session(m.id.clone());
        let label = row![row_label(self.session_title(m), false, &self.search), account_badge(&self.account_tag(m.account))]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        button(with_why(label.into(), self.match_why(m), &self.search))
            .on_press(Message::SelectSession(m.id.clone()))
            .width(Length::Fill)
            .style(if selected { button::primary } else { button::text })
            .into()
    }
}
