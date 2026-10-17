//! Several Claude accounts (spec 11.7): the account list, shared config,
//! and moving a session from one account to another.

use super::*;
use iced::widget::column;

/// One choice in the "Account" picker. `id` 0 is the default account.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct AccountChoice {
    pub(super) id: u64,
    pub(super) name: String,
}

impl std::fmt::Display for AccountChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// Small account name at the end of a sidebar row. Nothing when empty.
pub(super) fn account_badge(name: &str) -> Element<'static, Message> {
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

impl App {
    /// Config folder of an account. 0 is the default account.
    pub(super) fn config_of(&self, account: u64) -> Option<PathBuf> {
        match account {
            0 => self.default_config.clone(),
            id => self.accounts.iter().find(|a| a.id == id).map(|a| a.config_dir.clone()),
        }
    }

    /// `CLAUDE_CONFIG_DIR` for an account. The default account leaves it
    /// unset (see launch::claude). Shared config links are made or repaired
    /// first, so a skill or setting added since the last launch is there.
    pub(super) fn launch_config(&mut self, account: u64) -> Option<PathBuf> {
        self.share_config(account);
        (account != 0).then(|| self.config_of(account)).flatten()
    }

    /// Link the default account's settings, skills and other shared config
    /// into an extra account's folder (spec 11.7).
    pub(super) fn share_config(&mut self, account: u64) {
        let (Some(shared), Some(dir)) = (self.config_of(0), self.config_of(account).filter(|_| account != 0)) else { return };
        let unshared = codemux_core::accounts::share_config(&shared, &dir);
        self.unshared.insert(account, unshared);
    }

    pub(super) fn roots(&self) -> Vec<Root> {
        std::iter::once(0)
            .chain(self.accounts.iter().map(|a| a.id))
            .filter_map(|id| self.config_of(id).map(|c| Root { account: id, projects_dir: c.join("projects") }))
            .collect()
    }

    pub(super) fn account_name(&self, account: u64) -> String {
        self.accounts.iter().find(|a| a.id == account).map(|a| a.name.clone()).unwrap_or_else(|| DEFAULT_ACCOUNT.into())
    }

    pub(super) fn account_choices(&self) -> Vec<AccountChoice> {
        std::iter::once(AccountChoice { id: 0, name: DEFAULT_ACCOUNT.into() })
            .chain(self.accounts.iter().map(|a| AccountChoice { id: a.id, name: a.name.clone() }))
            .collect()
    }

    /// The account a tab runs with: its session file's account, or the one
    /// it was started with.
    pub(super) fn tab_account(&self, tab: &Tab) -> u64 {
        tab.session_id.as_deref().and_then(|s| self.index.get(s)).map_or(tab.account, |m| m.account)
    }

    /// Account of what the user is looking at. A new session uses it.
    pub(super) fn current_account(&self) -> u64 {
        let id = match &self.selected {
            Selected::Tab(id) => self.tabs.iter().find(|t| t.id == *id).map_or(0, |t| self.tab_account(t)),
            Selected::Session(sid) => self.index.get(sid).map_or(0, |m| m.account),
            _ => 0,
        };
        if id == 0 || self.accounts.iter().any(|a| a.id == id) { id } else { 0 }
    }

    /// Add the typed account, then open a tab to log in to it.
    pub(super) fn create_account(&mut self) -> Task<Message> {
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
    pub(super) fn add_account(&mut self, name: &str) -> Result<Option<u64>, String> {
        let name = name.trim().to_string();
        let Some(dir) = paths::account_dir_for(&self.home, &name) else { return Ok(None) };
        let same_dir = |d: &PathBuf| self.config_of(0).as_ref() == Some(d) || self.accounts.iter().any(|a| &a.config_dir == d);
        if same_dir(&dir) || self.accounts.iter().any(|a| a.name.eq_ignore_ascii_case(&name)) || name.eq_ignore_ascii_case(DEFAULT_ACCOUNT) {
            return Err(format!("An account named \"{name}\" already exists."));
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
        let id = self.accounts.iter().map(|a| a.id).max().unwrap_or(0) + 1;
        self.accounts.push(Account { id, name, config_dir: dir });
        self.share_config(id);
        self.error = None;
        self.accounts_changed();
        Ok(Some(id))
    }

    pub(super) fn login(&mut self, id: u64) -> Task<Message> {
        self.share_config(id);
        let Some(dir) = self.config_of(id).filter(|_| id != 0) else { return Task::none() };
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        self.spawn(TabKind::Shell, launch::login(&dir, home), None, id)
    }

    /// Forget an account. Its folder, login and sessions stay on disk.
    pub(super) fn remove_account(&mut self, id: u64) {
        let in_use = self.tabs.iter().any(|t| self.tab_account(t) == id) || self.pending_switch.as_ref().is_some_and(|p| p.to == id);
        if in_use {
            self.error = Some("A tab still runs with this account. Close it first.".into());
            return;
        }
        self.accounts.retain(|a| a.id != id);
        self.error = None;
        self.accounts_changed();
    }

    pub(super) fn accounts_changed(&mut self) {
        self.refresh_index();
        self.workspaces_changed();
    }

    /// Continue a session with another account (spec 11.7). A running
    /// session is stopped first, and only while Claude waits for input.
    pub(super) fn switch_account(&mut self, sid: String, to: u64) -> Task<Message> {
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
    pub(super) fn running_claude(&mut self, sid: &str, account: u64) -> Option<u32> {
        let claimed = self.config_of(account).map(|c| codemux_core::accounts::claimed_by(&c, sid)).unwrap_or_default();
        claimed.into_iter().find(|pid| self.scanner.is_running(*pid))
    }

    pub(super) fn switch_check(&mut self) -> Task<Message> {
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

    /// Account name for a sidebar row. Empty for the default account, so a
    /// user with one account sees no change.
    pub(super) fn account_tag(&self, account: u64) -> String {
        if account == 0 { String::new() } else { self.account_name(account) }
    }

    /// "Account: [picker]". Picking another account moves the session there.
    pub(super) fn account_picker(&self, sid: &str, current: u64, size: u32) -> Element<'_, Message> {
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

    pub(super) fn accounts_view(&self) -> Element<'_, Message> {
        let folder = |p: Option<PathBuf>| text(p.map(|p| p.display().to_string()).unwrap_or_default()).size(SMALL_TEXT_SIZE);
        let mut col = column![
            text("Accounts").size(22),
            text("Each account is its own Claude Code config folder, with its own login and sessions. \
                  Settings, skills, agents, commands, rules, output styles and plugins are links to the \
                  Default account's, so they are the same everywhere. \
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
                        column![text(a.name.clone()).size(TEXT_SIZE), folder(Some(a.config_dir.clone()))]
                            .push(self.unshared.get(&a.id).filter(|u| !u.is_empty()).map(|u| {
                                text(format!("Own copy, not shared with Default: {}", u.join(", "))).size(SMALL_TEXT_SIZE)
                            }))
                            .spacing(2)
                            .width(Length::Fill),
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
}
