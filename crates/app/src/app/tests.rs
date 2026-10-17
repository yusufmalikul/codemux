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

/// An extra account links the Default account's settings and skills. Its
/// own copies are kept and shown on the Accounts page, and a launch
/// repairs a missing link.
#[cfg(unix)]
#[test]
fn accounts_share_the_default_config() {
    let (dir, mut app) = fixture();
    std::fs::write(dir.path().join("settings.json"), "{}").unwrap();
    std::fs::create_dir_all(dir.path().join(".claude-work")).unwrap();
    std::fs::write(dir.path().join(".claude-work/settings.json"), "own").unwrap();

    let pro = add_pro(&mut app, dir.path());
    let pro_dir = dir.path().join(".claude-pro");
    assert_eq!(std::fs::read_link(pro_dir.join("settings.json")).unwrap(), dir.path().join("settings.json"));
    assert_eq!(std::fs::read_link(pro_dir.join("skills")).unwrap(), dir.path().join("skills"));
    assert!(!pro_dir.join("projects").exists(), "sessions must stay per account");

    app.add_account("Work").unwrap().unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join(".claude-work/settings.json")).unwrap(), "own");
    assert!(found_after_frames(&mut app, vec![Message::ShowAccounts], "Own copy, not shared with Default: settings.json"));

    std::fs::remove_file(pro_dir.join("skills")).unwrap();
    assert_eq!(app.launch_config(pro), Some(pro_dir.clone()));
    assert!(pro_dir.join("skills").is_symlink(), "launch did not repair the link");
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
    let pid = fake_claude_tab(&mut app, dir.path(), "trap 'sleep 0.5; exit 0' HUP", "while :; do sleep 0.05; done");

    let _ = app.update(Message::SwitchAccount("s1".into(), pro));
    assert!(app.tabs.is_empty(), "tab not closed: {:?}", app.error);
    assert!(app.pending_switch.is_some(), "moved before the process exited");
    finish_switch(&mut app);
    assert!(app.error.is_none(), "{:?}", app.error);
    assert!(!app.scanner.is_running(pid), "moved while the process still ran");
    let pro_dir = dir.path().join(".claude-pro");
    assert!(s1_in(&pro_dir).exists());
    // Resumed with the new account.
    assert_eq!(app.resumed.len(), 1, "not resumed");
    assert_eq!(app.resumed[0].env[launch::CONFIG_ENV], pro_dir.display().to_string());
    assert!(app.resumed[0].args.last().unwrap().contains("--resume s1"));
}

/// A tab that runs `<trap>; <idle loop>` as if it were Claude, idle,
/// with session s1. Returns once its trap is set.
fn fake_claude_tab(app: &mut App, cwd: &std::path::Path, trap: &str, idle: &str) -> u32 {
    let spec = LaunchSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), format!("{trap}; touch ready; {idle}")],
        env: Default::default(),
        cwd: cwd.to_path_buf(),
    };
    let _ = app.spawn(TabKind::Claude, spec, Some("s1".into()), 0);
    let tab = app.tabs.last_mut().unwrap();
    tab.busy = Some(false);
    let pid = tab.term.child_pid().expect("no process");
    let start = Instant::now();
    while !cwd.join("ready").exists() && start.elapsed() < SWITCH_TIMEOUT {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.scanner.is_running(pid), "process not seen");
    pid
}

/// Run the switch checks, as the app's timer does, until the switch ends.
/// Polls faster than the app, so the test does not wait longer than needed.
fn finish_switch(app: &mut App) {
    let start = Instant::now();
    while app.pending_switch.is_some() && start.elapsed() < SWITCH_TIMEOUT + SWITCH_POLL * 2 {
        std::thread::sleep(Duration::from_millis(10));
        let _ = app.update(Message::SwitchCheck);
    }
}

/// Claude keeps redrawing its screen while idle. After its tab closes,
/// nobody reads that output any more; the stop must still reach it.
#[cfg(unix)]
#[test]
fn switch_account_stops_a_tab_that_keeps_printing() {
    let (dir, mut app) = fixture();
    let pro = add_pro(&mut app, dir.path());
    // Like Claude: redraws while idle, and writes a lot when it gets SIGHUP.
    let trap = "trap 'yes goodbye | head -c 400000; exit 0' HUP";
    let pid = fake_claude_tab(&mut app, dir.path(), trap, "while :; do echo redraw; sleep 0.001; done");
    // Let the output pile up, like a tab nobody looks at.
    std::thread::sleep(Duration::from_millis(300));

    let _ = app.update(Message::SwitchAccount("s1".into(), pro));
    finish_switch(&mut app);
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

/// Two Claudes in the same folder: one runs by hand in a terminal tab,
/// one in its own tab. Closing the second tab writes its session file
/// last. The terminal tab must keep its own session, not take that one.
#[cfg(unix)]
#[test]
fn terminal_tab_keeps_its_session_when_another_tab_closes() {
    let (dir, mut app) = fixture();
    let work = std::fs::canonicalize(dir.path().join("work")).unwrap();
    let proj = dir.path().join("projects").join(paths::project_folder_name(&work));
    std::fs::create_dir_all(&proj).unwrap();
    std::os::unix::fs::symlink("/bin/sleep", work.join("claude")).unwrap();

    // The terminal tab: a shell that runs `claude` by hand.
    let shell = LaunchSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "./claude 30; true".into()],
        env: Default::default(),
        cwd: work.clone(),
    };
    let _ = app.spawn(TabKind::Shell, shell, None, 0);
    let start = Instant::now();
    let pid = loop {
        app.scanner.refresh();
        if let Some(c) = app.scanner.find_claude_under(app.tabs[0].term.child_pid().unwrap()) {
            break c.pid;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "claude not started");
        std::thread::sleep(Duration::from_millis(20));
    };
    // Like Claude Code: it says which session it writes to.
    std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
    let started_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    std::fs::write(
        dir.path().join(format!("sessions/{pid}.json")),
        format!(r#"{{"pid":{pid},"sessionId":"mine","startedAt":{started_ms}}}"#),
    )
    .unwrap();
    std::fs::write(proj.join("mine.jsonl"), "{}\n").unwrap();

    // The other tab, in the same folder.
    let other = LaunchSpec { program: "/bin/sleep".into(), args: vec!["30".into()], env: Default::default(), cwd: work.clone() };
    let _ = app.spawn(TabKind::Claude, other, Some("other".into()), 0);
    let other_tab = app.tabs[1].id;
    std::fs::write(proj.join("other.jsonl"), "{}\n").unwrap();
    let _ = app.update(Message::Tick);
    assert_eq!(app.tabs[0].session_id.as_deref(), Some("mine"));

    // Close the other tab. Its Claude writes its file one last time.
    let _ = app.update(Message::CloseTab(other_tab));
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    std::fs::File::options().write(true).open(proj.join("other.jsonl")).unwrap().set_modified(later).unwrap();
    let _ = app.update(Message::Tick);
    assert_eq!(app.tabs[0].session_id.as_deref(), Some("mine"), "terminal tab took the closed tab's session");
}
