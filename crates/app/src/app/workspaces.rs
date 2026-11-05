//! Workspaces: named groups of sessions in the sidebar tree.

use super::*;
use super::ellipsis::ellipsis;
use super::sidebar::close_style;
use iced::widget::{column, hover, space};

/// One choice in the "Workspace" picker. `id` None is the default workspace.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct WorkspaceChoice {
    pub(super) id: Option<u64>,
    pub(super) name: String,
}

impl std::fmt::Display for WorkspaceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// A workspace row in the sidebar tree: an arrow, the name, and the number
/// of sessions. Click to collapse or expand. User workspaces also get a
/// delete button; the default workspace cannot be removed.
pub(super) fn workspace_header(id: Option<u64>, name: &str, count: usize, open_tabs: usize, open: bool) -> Element<'static, Message> {
    let arrow = if open { "▾" } else { "▸" };
    // The tabs themselves are in the OPEN block; this tells which
    // workspace they belong to, even when it is collapsed.
    let badge = if open_tabs > 0 { format!("  ● {open_tabs} open") } else { String::new() };
    // A long name ends in "…", so the count stays in view on one line.
    let label = row![ellipsis(format!("{arrow} {name}")).size(15).hug(), text(format!("({count}){badge}")).size(15).wrapping(text::Wrapping::None)]
        .spacing(8);
    let toggle = button(label)
        .on_press(Message::ToggleWorkspace(id))
        .width(Length::Fill)
        .style(button::text);
    let line: Element<'static, Message> = match id {
        // The ✕ shows only while the mouse is on the header: it is rarely
        // needed, and it sits next to a row that is clicked often. The
        // base keeps a clear spot of the same size for it, so a long name
        // never runs under it.
        Some(id) => {
            let delete = |on: bool| {
                let label = text("✕").size(13);
                let label = if on { label } else { label.color(iced::Color::TRANSPARENT) };
                let b = button(label).padding([4, 10]).style(close_style);
                if on { b.on_press(Message::DeleteWorkspaceAsk(id)) } else { b }
            };
            let base = row![toggle, delete(false)].spacing(4).align_y(iced::Alignment::Center);
            let top = row![space::horizontal(), tooltip(
                delete(true),
                container(text("Delete workspace").size(12)).padding(6).style(container::rounded_box),
                tooltip::Position::Left,
            )]
            .align_y(iced::Alignment::Center);
            hover(base, top)
        }
        None => toggle.into(),
    };
    container(line)
        .padding(iced::Padding { top: 10.0, ..iced::Padding::ZERO })
        .into()
}

/// The yes/no question shown under a workspace before it is deleted.
pub(super) fn delete_workspace_confirm(id: u64, name: &str) -> Element<'static, Message> {
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

impl App {
    pub(super) fn workspaces_changed(&mut self) {
        self.workspaces_version += 1;
        self.save_state();
    }

    pub(super) fn workspace_of(&self, sid: &str) -> Option<u64> {
        self.workspaces.iter().find(|w| w.sessions.iter().any(|s| s == sid)).map(|w| w.id)
    }

    /// The workspace of what the user is looking at: the selected tab or
    /// session. None is the default workspace.
    pub(super) fn current_workspace(&self) -> Option<u64> {
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
    pub(super) fn adopt_tab_sessions(&mut self) {
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

    pub(super) fn workspace_choices(&self) -> Vec<WorkspaceChoice> {
        std::iter::once(WorkspaceChoice { id: None, name: DEFAULT_WORKSPACE.into() })
            .chain(self.workspaces.iter().map(|w| WorkspaceChoice { id: Some(w.id), name: w.name.clone() }))
            .collect()
    }
}
