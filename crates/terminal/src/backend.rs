// Based on iced_term 0.8 (https://github.com/Harzu/iced_term), MIT License,
// Copyright (c) Ilya Shvyryalkin. Modified for codemux. See NOTICE.
use crate::actions::Action;
use crate::settings::BackendSettings;
use alacritty_terminal::event::{
    Event, EventListener, Notify, OnResize, WindowSize,
};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{
    self, cell::Cell, test::TermSize, viewport_to_point, Term, TermMode,
};
use alacritty_terminal::tty;
use iced::keyboard::Modifiers;
use iced_core::Size;
use std::borrow::Cow;
use std::cmp::min;
use std::io::Result;
use std::ops::{Index, RangeInclusive};
use std::sync::Arc;
use tokio::sync::mpsc;

const URL_REGEX: &str = r#"(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file://|git://|ssh:|ftp://)[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>"\s{-}\^⟨⟩`]+"#;

#[derive(Debug, Clone)]
pub enum Command {
    Write(Vec<u8>),
    /// Wheel scroll: lines (positive = up), mouse cell, modifiers.
    Scroll(i32, Point, Modifiers),
    Resize(Option<Size<f32>>, Option<Size<f32>>),
    SelectStart(SelectionType, (f32, f32)),
    SelectUpdate((f32, f32)),
    ProcessLink(LinkAction, Point),
    MouseReport(MouseButton, Modifiers, Point, bool),
    ProcessAlacrittyEvent(Event),
}

#[derive(Debug, Clone)]
pub enum MouseMode {
    Sgr,
    Normal(bool),
}

impl From<TermMode> for MouseMode {
    fn from(term_mode: TermMode) -> Self {
        if term_mode.contains(TermMode::SGR_MOUSE) {
            MouseMode::Sgr
        } else if term_mode.contains(TermMode::UTF8_MOUSE) {
            MouseMode::Normal(true)
        } else {
            MouseMode::Normal(false)
        }
    }
}

#[derive(Debug, Clone)]
pub enum MouseButton {
    LeftButton = 0,
    MiddleButton = 1,
    RightButton = 2,
    LeftMove = 32,
    MiddleMove = 33,
    RightMove = 34,
    NoneMove = 35,
    ScrollUp = 64,
    ScrollDown = 65,
    Other = 99,
}

#[derive(Debug, Clone)]
pub enum LinkAction {
    Clear,
    Hover,
    Open,
}

#[derive(Clone, Copy, Debug)]
pub struct TerminalSize {
    pub cell_width: u16,
    pub cell_height: u16,
    num_cols: u16,
    num_lines: u16,
    layout_width: f32,
    layout_height: f32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cell_width: 1,
            cell_height: 1,
            num_cols: 80,
            num_lines: 50,
            layout_width: 80.0,
            layout_height: 50.0,
        }
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn columns(&self) -> usize {
        self.num_cols as usize
    }

    fn last_column(&self) -> Column {
        Column(self.num_cols as usize - 1)
    }

    fn bottommost_line(&self) -> Line {
        Line(self.num_lines as i32 - 1)
    }

    fn screen_lines(&self) -> usize {
        self.num_lines as usize
    }
}

impl From<TerminalSize> for WindowSize {
    fn from(size: TerminalSize) -> Self {
        Self {
            num_lines: size.num_lines,
            num_cols: size.num_cols,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        }
    }
}

pub struct Backend {
    term: Arc<FairMutex<Term<EventProxy>>>,
    size: TerminalSize,
    notifier: Notifier,
    last_content: RenderableContent,
    pub(crate) url_regex: RegexSearch,
    child_pid: Option<u32>,
    /// URI of a hovered OSC 8 hyperlink (its shown text is not the URL).
    hovered_uri: Option<String>,
}

impl Backend {
    pub fn new(
        id: u64,
        pty_event_proxy_sender: mpsc::Sender<Event>,
        settings: BackendSettings,
    ) -> Result<Self> {
        let pty_config = tty::Options {
            shell: Some(tty::Shell::new(settings.program, settings.args)),
            working_directory: settings.working_directory,
            env: settings.env,
            ..tty::Options::default()
        };

        let config = term::Config::default();
        let terminal_size = TerminalSize::default();
        let pty = tty::new(&pty_config, terminal_size.into(), id)?;
        #[cfg(unix)]
        let child_pid = Some(pty.child().id());
        #[cfg(windows)]
        let child_pid = pty.child_watcher().pid().map(|p| p.get());

        let event_proxy = EventProxy(pty_event_proxy_sender);

        let mut term = Term::new(config, &terminal_size, event_proxy.clone());

        let cursor = term.grid_mut().cursor_cell().clone();

        let initial_content = RenderableContent {
            display_offset: 0,
            cursor_point: term.grid().cursor.point,
            selectable_range: None,
            terminal_mode: *term.mode(),
            terminal_size,
            cursor: cursor.clone(),
            hovered_hyperlink: None,
        };

        let term = Arc::new(FairMutex::new(term));

        let pty_event_loop =
            EventLoop::new(term.clone(), event_proxy, pty, false, false)?;

        let notifier = Notifier(pty_event_loop.channel());

        let _ = pty_event_loop.spawn();

        Ok(Self {
            term: term.clone(),
            size: terminal_size,
            notifier,
            last_content: initial_content,
            url_regex: RegexSearch::new(URL_REGEX).expect("invalid url regexp"),
            child_pid,
            hovered_uri: None,
        })
    }

    /// PID of the process started in the PTY (usually the shell).
    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    pub fn handle(&mut self, cmd: Command) -> Action {
        let mut action = Action::default();
        let term = self.term.clone();
        let mut term = term.lock();
        match cmd {
            Command::ProcessAlacrittyEvent(event) => {
                match event {
                    Event::Exit => {
                        action = Action::Shutdown;
                    },
                    Event::ChildExit(code) => {
                        action = Action::ChildExit(code.code().unwrap_or(-1));
                    },
                    Event::Title(title) => {
                        action = Action::ChangeTitle(title);
                    },
                    Event::PtyWrite(pty) => {
                        self.notifier.notify(pty.into_bytes())
                    },
                    _ => {},
                };
            },
            Command::Write(input) => {
                self.write(input);
                term.scroll_display(Scroll::Bottom);
            },
            Command::Scroll(delta, point, modifiers) => {
                self.scroll(&mut term, delta, point, modifiers);
            },
            Command::Resize(layout_size, font_measure) => {
                self.resize(&mut term, layout_size, font_measure);
            },
            Command::SelectStart(selection_type, (x, y)) => {
                self.start_selection(&mut term, selection_type, x, y);
            },
            Command::SelectUpdate((x, y)) => {
                self.update_selection(&mut term, x, y);
            },
            Command::ProcessLink(link_action, point) => {
                self.process_link_action(&term, link_action, point);
            },
            Command::MouseReport(button, modifiers, point, pressed) => {
                self.process_mouse_report(button, modifiers, point, pressed);
            },
        };

        action
    }

    fn process_link_action(
        &mut self,
        terminal: &Term<EventProxy>,
        link_action: LinkAction,
        point: Point,
    ) {
        match link_action {
            LinkAction::Hover => {
                // OSC 8 first (Claude Code uses it), then plain URLs in text.
                match osc8_link_at(terminal, point) {
                    Some((uri, range)) => {
                        self.hovered_uri = Some(uri);
                        self.last_content.hovered_hyperlink = Some(range);
                    },
                    None => {
                        self.hovered_uri = None;
                        self.last_content.hovered_hyperlink = self
                            .regex_match_at(
                                terminal,
                                point,
                                &mut self.url_regex.clone(),
                            );
                    },
                }
            },
            LinkAction::Clear => {
                self.hovered_uri = None;
                self.last_content.hovered_hyperlink = None;
            },
            LinkAction::Open => {
                self.open_link();
            },
        };
    }

    /// URL of the link under the mouse, if any.
    pub(crate) fn hovered_link_url(&self) -> Option<String> {
        if let Some(uri) = &self.hovered_uri {
            return Some(uri.clone());
        }
        let range = self.last_content.hovered_hyperlink.as_ref()?;
        let (start, end) = (*range.start(), *range.end());
        let term = self.term.lock();
        let grid = term.grid();
        let mut url = String::from(grid.index(start).c);
        for indexed in grid.iter_from(start) {
            url.push(indexed.c);
            if indexed.point == end {
                break;
            }
        }
        Some(url)
    }

    fn open_link(&self) {
        if let Some(url) = self.hovered_link_url() {
            let _ = open::that(url);
        }
    }

    fn process_mouse_report(
        &self,
        button: MouseButton,
        modifiers: Modifiers,
        point: Point,
        pressed: bool,
    ) {
        let mods = Self::mouse_modifier_bits(modifiers);

        match MouseMode::from(self.last_content.terminal_mode) {
            MouseMode::Sgr => {
                self.sgr_mouse_report(point, button as u8 + mods, pressed)
            },
            MouseMode::Normal(is_utf8) => {
                if pressed {
                    self.normal_mouse_report(
                        point,
                        button as u8 + mods,
                        is_utf8,
                    )
                } else {
                    self.normal_mouse_report(point, 3 + mods, is_utf8)
                }
            },
        }
    }

    /// xterm modifier bits for mouse reports: Shift 4, Alt 8, Ctrl 16.
    fn mouse_modifier_bits(modifiers: Modifiers) -> u8 {
        let mut mods = 0;
        if modifiers.contains(Modifiers::SHIFT) {
            mods += 4;
        }
        if modifiers.contains(Modifiers::ALT) {
            mods += 8;
        }
        if modifiers.contains(Modifiers::CTRL) {
            mods += 16;
        }
        mods
    }

    fn sgr_mouse_report(&self, point: Point, button: u8, pressed: bool) {
        let c = if pressed { 'M' } else { 'm' };

        let msg = format!(
            "\x1b[<{};{};{}{}",
            button,
            point.column + 1,
            point.line + 1,
            c
        );

        self.notifier.notify(msg.as_bytes().to_vec());
    }

    fn normal_mouse_report(&self, point: Point, button: u8, is_utf8: bool) {
        let Point { line, column } = point;
        let max_point = if is_utf8 { 2015 } else { 223 };

        if line >= max_point || column >= max_point {
            return;
        }

        let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

        let mouse_pos_encode = |pos: usize| -> Vec<u8> {
            let pos = 32 + 1 + pos;
            let first = 0xC0 + pos / 64;
            let second = 0x80 + (pos & 63);
            vec![first as u8, second as u8]
        };

        if is_utf8 && column >= Column(95) {
            msg.append(&mut mouse_pos_encode(column.0));
        } else {
            msg.push(32 + 1 + column.0 as u8);
        }

        if is_utf8 && line >= 95 {
            msg.append(&mut mouse_pos_encode(line.0 as usize));
        } else {
            msg.push(32 + 1 + line.0 as u8);
        }

        self.notifier.notify(msg);
    }

    fn start_selection(
        &mut self,
        terminal: &mut Term<EventProxy>,
        selection_type: SelectionType,
        x: f32,
        y: f32,
    ) {
        let location = Self::selection_point(
            x,
            y,
            &self.size,
            terminal.grid().display_offset(),
        );
        terminal.selection = Some(Selection::new(
            selection_type,
            location,
            self.selection_side(x),
        ));
    }

    fn update_selection(
        &mut self,
        terminal: &mut Term<EventProxy>,
        x: f32,
        y: f32,
    ) {
        let display_offset = terminal.grid().display_offset();
        if let Some(ref mut selection) = terminal.selection {
            let location =
                Self::selection_point(x, y, &self.size, display_offset);
            selection.update(location, self.selection_side(x));
        }
    }

    pub fn selection_point(
        x: f32,
        y: f32,
        terminal_size: &TerminalSize,
        display_offset: usize,
    ) -> Point {
        let col = (x as usize) / (terminal_size.cell_width as usize);
        let col = min(Column(col), Column(terminal_size.num_cols as usize - 1));

        let line = (y as usize) / (terminal_size.cell_height as usize);
        let line = min(line, terminal_size.num_lines as usize - 1);

        viewport_to_point(display_offset, Point::new(line, col))
    }

    fn selection_side(&self, x: f32) -> Side {
        let cell_x = x as usize % self.size.cell_width as usize;
        let half_cell_width = (self.size.cell_width as f32 / 2.0) as usize;

        if cell_x > half_cell_width {
            Side::Right
        } else {
            Side::Left
        }
    }

    fn resize(
        &mut self,
        terminal: &mut Term<EventProxy>,
        layout_size: Option<Size<f32>>,
        font_measure: Option<Size<f32>>,
    ) {
        if let Some(size) = layout_size {
            self.size.layout_height = size.height;
            self.size.layout_width = size.width;
        };

        if let Some(size) = font_measure {
            self.size.cell_height = size.height as u16;
            self.size.cell_width = size.width as u16;
        }

        let lines = (self.size.layout_height / self.size.cell_height as f32)
            .floor() as u16;
        let cols = (self.size.layout_width / self.size.cell_width as f32)
            .floor() as u16;
        if lines > 0 && cols > 0 {
            self.size.num_lines = lines;
            self.size.num_cols = cols;
            self.notifier.on_resize(self.size.into());
            terminal.resize(TermSize::new(
                self.size.num_cols as usize,
                self.size.num_lines as usize,
            ));
        }
    }

    fn write<I: Into<Cow<'static, [u8]>>>(&self, input: I) {
        self.notifier.notify(input);
    }

    /// Wheel scroll (spec T3), in the same order of rules as Alacritty:
    /// 1. Mouse mode on: report buttons 64 (up) / 65 (down) at the mouse cell.
    /// 2. Alt screen + alternate scroll on: send arrow keys.
    /// 3. Otherwise: scroll our own scrollback.
    fn scroll(
        &mut self,
        terminal: &mut Term<EventProxy>,
        delta_value: i32,
        point: Point,
        modifiers: Modifiers,
    ) {
        if delta_value == 0 {
            return;
        }
        let mode = *terminal.mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            let button = if delta_value > 0 {
                MouseButton::ScrollUp
            } else {
                MouseButton::ScrollDown
            } as u8
                + Self::mouse_modifier_bits(modifiers);
            for _ in 0..delta_value.unsigned_abs() {
                match MouseMode::from(mode) {
                    MouseMode::Sgr => {
                        self.sgr_mouse_report(point, button, true)
                    },
                    MouseMode::Normal(is_utf8) => {
                        self.normal_mouse_report(point, button, is_utf8)
                    },
                }
            }
        } else if mode
            .contains(TermMode::ALTERNATE_SCROLL | TermMode::ALT_SCREEN)
        {
            let line_cmd = if delta_value > 0 { b'A' } else { b'B' };
            let mut content = vec![];
            for _ in 0..delta_value.abs() {
                content.extend_from_slice(&[0x1b, b'O', line_cmd]);
            }
            self.notifier.notify(content);
        } else {
            terminal
                .grid_mut()
                .scroll_display(Scroll::Delta(delta_value));
        }
    }

    pub fn selectable_content(&self) -> String {
        self.term.lock().selection_to_string().unwrap_or_default()
    }

    /// Run `f` with the live terminal state, under its lock.
    /// Drawing reads the grid here instead of keeping a full copy of it
    /// (with scrollback) on every PTY event.
    pub(crate) fn with_term<R>(
        &self,
        f: impl FnOnce(&Term<EventProxy>) -> R,
    ) -> R {
        f(&self.term.lock())
    }

    pub fn sync(&mut self) {
        let term = self.term.clone();
        let mut term = term.lock();
        self.internal_sync(&mut term);
    }

    fn internal_sync(&mut self, terminal: &mut Term<EventProxy>) {
        let selectable_range = match &terminal.selection {
            Some(s) => s.to_range(terminal),
            None => None,
        };

        let cursor = terminal.grid_mut().cursor_cell().clone();
        self.last_content.display_offset = terminal.grid().display_offset();
        self.last_content.cursor_point = terminal.grid().cursor.point;
        self.last_content.selectable_range = selectable_range;
        self.last_content.cursor = cursor.clone();
        self.last_content.terminal_mode = *terminal.mode();
        self.last_content.terminal_size = self.size;
    }

    pub fn renderable_content(&self) -> &RenderableContent {
        &self.last_content
    }

    /// Based on alacritty/src/display/hint.rs > regex_match_at
    /// Retrieve the match, if the specified point is inside the content matching the regex.
    fn regex_match_at(
        &self,
        terminal: &Term<EventProxy>,
        point: Point,
        regex: &mut RegexSearch,
    ) -> Option<Match> {
        let x = visible_regex_match_iter(terminal, regex)
            .find(|rm| rm.contains(&point));
        x
    }
}

/// The OSC 8 hyperlink at `point`, with the range of cells on that line that
/// share it (used to underline the whole link text).
fn osc8_link_at(
    term: &Term<EventProxy>,
    point: Point,
) -> Option<(String, RangeInclusive<Point>)> {
    let grid = term.grid();
    if point.line < grid.topmost_line() || point.line > grid.bottommost_line() {
        return None;
    }
    let link = grid[point].hyperlink()?;
    let row = &grid[point.line];
    let same =
        |col: usize| row[Column(col)].hyperlink().as_ref() == Some(&link);
    let mut start = point.column.0;
    while start > 0 && same(start - 1) {
        start -= 1;
    }
    let mut end = point.column.0;
    while end + 1 < grid.columns() && same(end + 1) {
        end += 1;
    }
    Some((
        link.uri().to_string(),
        Point::new(point.line, Column(start))
            ..=Point::new(point.line, Column(end)),
    ))
}

/// Copied from alacritty/src/display/hint.rs:
/// Iterate over all visible regex matches.
fn visible_regex_match_iter<'a>(
    term: &'a Term<EventProxy>,
    regex: &'a mut RegexSearch,
) -> impl Iterator<Item = Match> + 'a {
    let viewport_start = Line(-(term.grid().display_offset() as i32));
    let viewport_end = viewport_start + term.bottommost_line();
    let mut start =
        term.line_search_left(Point::new(viewport_start, Column(0)));
    let mut end = term.line_search_right(Point::new(viewport_end, Column(0)));
    start.line = start.line.max(viewport_start - 100);
    end.line = end.line.min(viewport_end + 100);

    RegexIter::new(start, end, Direction::Right, term, regex)
        .skip_while(move |rm| rm.end().line < viewport_start)
        .take_while(move |rm| rm.start().line <= viewport_end)
}

/// Small, cheap snapshot of the terminal state used for input handling.
/// The grid itself is not copied; drawing reads it via `Backend::with_term`.
pub struct RenderableContent {
    pub display_offset: usize,
    pub cursor_point: Point,
    pub hovered_hyperlink: Option<RangeInclusive<Point>>,
    pub selectable_range: Option<SelectionRange>,
    pub cursor: Cell,
    pub terminal_mode: TermMode,
    pub terminal_size: TerminalSize,
}

impl Default for RenderableContent {
    fn default() -> Self {
        Self {
            display_offset: 0,
            cursor_point: Point::default(),
            hovered_hyperlink: None,
            selectable_range: None,
            cursor: Cell::default(),
            terminal_mode: TermMode::empty(),
            terminal_size: TerminalSize::default(),
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

#[derive(Clone)]
pub struct EventProxy(mpsc::Sender<Event>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.blocking_send(event);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Start a real PTY running a tiny TUI that turns on the given DEC modes,
    /// then records every byte it receives into `out`.
    fn start_recorder(modes: &str, out: &std::path::Path) -> Backend {
        let (tx, mut rx) = mpsc::channel(100);
        // Drain events so the PTY thread never blocks.
        std::thread::spawn(move || while rx.blocking_recv().is_some() {});
        let script = format!(
            "stty raw -echo; printf '{modes}'; exec cat > '{}'",
            out.display()
        );
        let settings = BackendSettings {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
            ..Default::default()
        };
        let mut backend = Backend::new(0, tx, settings).unwrap();
        backend.handle(Command::Resize(
            Some(Size::new(800.0, 400.0)),
            Some(Size::new(10.0, 20.0)),
        ));
        backend
    }

    fn wait_for_mode(backend: &mut Backend, mode: TermMode) {
        let start = Instant::now();
        loop {
            backend.sync();
            if backend.renderable_content().terminal_mode.contains(mode) {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "mode {mode:?} never set"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn read_until(out: &std::path::Path, len: usize) -> Vec<u8> {
        let start = Instant::now();
        loop {
            let got = std::fs::read(out).unwrap_or_default();
            if got.len() >= len || start.elapsed() > Duration::from_secs(3) {
                return got;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Copy reads the selection from the live terminal (no grid copy kept).
    #[test]
    fn selection_copies_text_from_live_grid() {
        let (tx, mut rx) = mpsc::channel(100);
        std::thread::spawn(move || while rx.blocking_recv().is_some() {});
        let settings = BackendSettings {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf 'hello world'; sleep 30".into()],
            ..Default::default()
        };
        let mut backend = Backend::new(0, tx, settings).unwrap();
        backend.handle(Command::Resize(
            Some(Size::new(800.0, 400.0)),
            Some(Size::new(10.0, 20.0)),
        ));
        let start = Instant::now();
        loop {
            backend.handle(Command::SelectStart(
                SelectionType::Simple,
                (0.0, 5.0),
            ));
            backend.handle(Command::SelectUpdate((109.0, 5.0)));
            if backend.selectable_content() == "hello world" {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "got {:?}",
                backend.selectable_content()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn start_printer(text: &str) -> Backend {
        let (tx, mut rx) = mpsc::channel(100);
        std::thread::spawn(move || while rx.blocking_recv().is_some() {});
        let settings = BackendSettings {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), format!("printf '{text}'; sleep 30")],
            ..Default::default()
        };
        let mut backend = Backend::new(0, tx, settings).unwrap();
        backend.handle(Command::Resize(
            Some(Size::new(800.0, 400.0)),
            Some(Size::new(10.0, 20.0)),
        ));
        backend
    }

    /// Hover at `col` on the first line until a link shows up (output is async).
    fn hover_url(backend: &mut Backend, col: usize) -> Option<String> {
        let start = Instant::now();
        loop {
            backend.handle(Command::ProcessLink(
                LinkAction::Hover,
                Point::new(Line(0), Column(col)),
            ));
            let url = backend.hovered_link_url();
            if url.is_some() || start.elapsed() > Duration::from_secs(2) {
                return url;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Claude Code prints links as OSC 8 hyperlinks: the shown text is not
    /// the URL. ⌘+click must open the real URL.
    #[test]
    fn osc8_hyperlink_is_detected_on_hover() {
        let mut backend = start_printer(
            r"\033]8;;https://example.com/a\033\\click here\033]8;;\033\\ after",
        );
        assert_eq!(
            hover_url(&mut backend, 2).as_deref(),
            Some("https://example.com/a")
        );
        // Outside the link text: no link.
        backend.handle(Command::ProcessLink(
            LinkAction::Hover,
            Point::new(Line(0), Column(13)),
        ));
        assert_eq!(backend.hovered_link_url(), None);
    }

    #[test]
    fn plain_url_is_detected_on_hover() {
        let mut backend = start_printer("see https://x.io/p ok");
        assert_eq!(
            hover_url(&mut backend, 6).as_deref(),
            Some("https://x.io/p")
        );
    }

    /// Not a pass/fail test: prints the cost of one sync with a full scrollback.
    /// Run: cargo test --release -p codemux-terminal sync_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn sync_cost() {
        let (tx, mut rx) = mpsc::channel(100);
        std::thread::spawn(move || while rx.blocking_recv().is_some() {});
        let settings = BackendSettings {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "seq 1 20000; sleep 30".into()],
            ..Default::default()
        };
        let mut backend = Backend::new(0, tx, settings).unwrap();
        backend.handle(Command::Resize(
            Some(Size::new(2000.0, 1200.0)),
            Some(Size::new(10.0, 20.0)),
        ));
        std::thread::sleep(Duration::from_secs(2));
        let n = 300;
        let start = Instant::now();
        for _ in 0..n {
            backend.sync();
        }
        eprintln!(
            "SYNC_COST {:.3} ms each",
            start.elapsed().as_secs_f64() * 1000.0 / n as f64
        );
    }

    /// T3: with SGR mouse mode on (like Claude Code), the wheel must be reported
    /// as buttons 64 (up) / 65 (down) at the mouse cell, not turned into arrow keys.
    #[test]
    fn t3_wheel_is_reported_in_sgr_mouse_mode() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("input.bin");
        // 1000 = mouse, 1006 = SGR, 1049 = alt screen, 1007 = alternate scroll.
        let mut backend = start_recorder(
            r"\033[?1049h\033[?1007h\033[?1000h\033[?1006h",
            &out,
        );
        wait_for_mode(
            &mut backend,
            TermMode::SGR_MOUSE | TermMode::MOUSE_REPORT_CLICK,
        );

        let at = Point::new(Line(24), Column(8));
        backend.handle(Command::Scroll(2, at, Modifiers::empty()));
        backend.handle(Command::Scroll(-1, at, Modifiers::empty()));

        let expected = b"\x1b[<64;9;25M\x1b[<64;9;25M\x1b[<65;9;25M";
        assert_eq!(
            String::from_utf8_lossy(&read_until(&out, expected.len())),
            String::from_utf8_lossy(expected)
        );
    }

    /// Mouse mode off, alt screen + alternate scroll on: arrow keys are correct.
    #[test]
    fn t3_wheel_becomes_arrows_only_without_mouse_mode() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("input.bin");
        let mut backend = start_recorder(r"\033[?1049h\033[?1007h", &out);
        wait_for_mode(
            &mut backend,
            TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL,
        );

        let at = Point::new(Line(0), Column(0));
        backend.handle(Command::Scroll(1, at, Modifiers::empty()));
        backend.handle(Command::Scroll(-1, at, Modifiers::empty()));

        let expected = b"\x1bOA\x1bOB";
        assert_eq!(read_until(&out, expected.len()), expected);
    }

    /// Legacy (non-SGR) mouse encoding: ESC [ M <32+btn> <32+1+col> <32+1+line>.
    #[test]
    fn t3_wheel_is_reported_in_legacy_mouse_mode() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("input.bin");
        let mut backend = start_recorder(r"\033[?1000h", &out);
        wait_for_mode(&mut backend, TermMode::MOUSE_REPORT_CLICK);

        backend.handle(Command::Scroll(
            1,
            Point::new(Line(2), Column(3)),
            Modifiers::empty(),
        ));

        let expected = vec![0x1b, b'[', b'M', 32 + 64, 32 + 1 + 3, 32 + 1 + 2];
        assert_eq!(read_until(&out, expected.len()), expected);
    }
}
