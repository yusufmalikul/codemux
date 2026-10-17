//! The main area: the terminal of the selected tab, or the detail view of a
//! closed session.

use super::*;
use super::sidebar::{close_style, relative_time};
use iced::widget::column;

fn empty_state<'a>() -> Element<'a, Message> {
    container(text("Pick a session on the left, or start a New session / New terminal.").size(14))
        .center(Length::Fill)
        .into()
}

impl App {
    pub(super) fn main_area(&self) -> Element<'_, Message> {
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
            // Red banner with a ✕ at the end, so the user can clear it.
            let banner = row![
                text(e.clone()).size(12).width(Length::Fill),
                button(text("✕").size(12)).padding([0, 6]).on_press(Message::DismissError).style(close_style),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);
            col = col.push(container(banner).padding(8).width(Length::Fill).style(container::danger));
        }
        // Own row under the body, so it never covers terminal text.
        let credit = container(text("Created by yusufmalikul").size(13)).width(Length::Fill).align_right(Length::Fill).padding([2, 8]);
        let body = container(body).width(Length::Fill).height(Length::Fill);
        container(col.push(body).push(credit)).width(Length::Fill).height(Length::Fill).into()
    }

    /// A session's title with a "Rename" button, or the name box while it is
    /// being renamed.
    pub(super) fn title_or_rename(&self, sid: &str, title: String, size: u32, button_size: u32) -> Element<'_, Message> {
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
