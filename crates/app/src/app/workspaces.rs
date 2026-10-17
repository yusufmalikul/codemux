//! Workspaces: named groups of sessions in the sidebar tree.

use super::*;
use super::sidebar::close_style;
use iced::widget::column;

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
    /// Expand the workspace that holds a tab, so the selected tab is never
    /// hidden inside a collapsed workspace.
    pub(super) fn reveal_tab(&mut self, id: u64) {
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
