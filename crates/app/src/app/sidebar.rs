//! The sidebar: search, the "+" menu and the session tree.

use super::*;
use super::accounts::account_badge;
use super::workspaces::{delete_workspace_confirm, workspace_header};
use iced::widget::column;

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
    title: String,
    folder: String,
    status: &'static str,
    selected: bool,
}

fn tab_row(t: &TabRow) -> Element<'static, Message> {
    let label = row![
        text("●").size(12).color(status_color(t.status)),
        row_label(t.title.clone(), true),
        account_badge(&t.account),
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

        let scrolling = self.sidebar_scrolled.is_some();
        column![
            top,
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
        ]
        .spacing(8)
        .into()
    }

    /// The sidebar tree: each workspace with its open tabs first, then its
    /// other sessions. A session open in a tab is shown only as that tab.
    pub(super) fn history_list(&self, tabs: &[TabRow]) -> Element<'static, Message> {
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

    pub(super) fn session_row(&self, m: &SessionMeta) -> Element<'static, Message> {
        let selected = self.selected == Selected::Session(m.id.clone());
        let label = row![row_label(self.session_title(m), false), account_badge(&self.account_tag(m.account))]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        button(label)
            .on_press(Message::SelectSession(m.id.clone()))
            .width(Length::Fill)
            .style(if selected { button::primary } else { button::text })
            .into()
    }
}
