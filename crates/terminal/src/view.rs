// Based on iced_term 0.8 (https://github.com/Harzu/iced_term), MIT License,
// Copyright (c) Ilya Shvyryalkin. Modified for codemux. See NOTICE.
use crate::backend::{
    Backend, Command, LinkAction, MouseButton, RenderableContent,
};
use crate::bindings::{BindingAction, BindingsLayout, InputKind};
use crate::terminal::{Event, Terminal};
use crate::theme::TerminalStyle;
use alacritty_terminal::index::Point as TerminalGridPoint;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::{cell, TermMode};
use alacritty_terminal::vte::ansi::{self as ansi, NamedColor};
use iced::alignment::Vertical;
use iced::font::{Style as FontStyle, Weight as FontWeight};
use iced::mouse::{Cursor, ScrollDelta};
use iced::widget::canvas::{Path, Text};
use iced::widget::container;
use iced::{Color, Element, Length, Point, Rectangle, Size, Theme};
use iced_core::clipboard::Kind as ClipboardKind;
use iced_core::keyboard::{Key, Modifiers};
use iced_core::mouse::{self, Click};
use iced_core::text::{Alignment, LineHeight, Shaping};
use iced_core::widget::operation::{self, Focusable};
use iced_graphics::core::widget::{tree, Tree};
use iced_graphics::core::Widget;
use iced_graphics::geometry::Stroke;

pub struct TerminalView<'a> {
    term: &'a Terminal,
}

impl<'a> TerminalView<'a> {
    pub fn show(term: &'a Terminal) -> Element<'a, Event> {
        container(Self { term })
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| term.theme.container_style())
            .into()
    }

    pub fn focus<Message: 'static>(
        id: iced::widget::Id,
    ) -> iced::Task<Message> {
        iced::widget::operation::focus(id)
    }

    fn is_cursor_in_layout(
        &self,
        cursor: Cursor,
        layout: iced_graphics::core::Layout<'_>,
    ) -> bool {
        if let Some(cursor_position) = cursor.position() {
            let layout_position = layout.position();
            let layout_size = layout.bounds();
            let is_triggered = cursor_position.x >= layout_position.x
                && cursor_position.y >= layout_position.y
                && cursor_position.x < (layout_position.x + layout_size.width)
                && cursor_position.y < (layout_position.y + layout_size.height);

            return is_triggered;
        }

        false
    }

    fn is_cursor_hovered_hyperlink(&self, state: &TerminalViewState) -> bool {
        let content = self.term.backend.renderable_content();
        if let Some(hyperlink_range) = &content.hovered_hyperlink {
            return hyperlink_range.contains(&state.mouse_position_on_grid);
        }

        false
    }

    fn handle_resize(
        &mut self,
        state: &mut TerminalViewState,
        layout: iced_graphics::core::Layout<'_>,
        shell: &mut iced_graphics::core::Shell<'_, Event>,
    ) {
        let layout_size = layout.bounds().size();
        if state.needs_resize(self.term.id, layout_size) {
            let cmd = Command::Resize(
                Some(layout_size),
                Some(self.term.font.measure),
            );
            shell.publish(Event::BackendCall(self.term.id, cmd));
        }
    }

    fn handle_focus(
        &self,
        event: &iced_core::Event,
        state: &mut TerminalViewState,
        is_cursor_in_layout: bool,
    ) {
        use iced::Event::Mouse;
        use iced_core::mouse::{Button::Left, Event::ButtonPressed};

        if let Mouse(ButtonPressed(Left)) = event {
            state.focus = is_cursor_in_layout;
        }
    }

    /// Ends a drag whose button was released outside the terminal (for
    /// example over the sidebar). Mouse events outside the terminal are not
    /// routed to it, so without this the drag stays on, and the next time
    /// the cursor moves over the terminal it stretches an old selection.
    fn end_drag_outside(
        state: &mut TerminalViewState,
        terminal_mode: &TermMode,
        event: &iced_core::Event,
        is_cursor_in_layout: bool,
        commands: &mut Vec<Command>,
    ) {
        use iced::Event::Mouse;
        use iced_core::mouse::Button::Left;
        use iced_core::mouse::Event::{ButtonPressed, ButtonReleased};

        if is_cursor_in_layout || !state.is_dragged {
            return;
        }

        match event {
            Mouse(ButtonReleased(Left)) => {
                state.is_dragged = false;
                if terminal_mode.intersects(TermMode::MOUSE_MODE) {
                    commands.push(Command::MouseReport(
                        MouseButton::LeftButton,
                        state.keyboard_modifiers,
                        state.mouse_position_on_grid,
                        false,
                    ));
                }
            },
            Mouse(ButtonPressed(_)) => state.is_dragged = false,
            _ => {},
        }
    }

    fn handle_mouse_event(
        &self,
        state: &mut TerminalViewState,
        layout_position: Point,
        cursor_position: Point,
        event: &iced::mouse::Event,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        let terminal_content = self.term.backend.renderable_content();
        let terminal_mode = terminal_content.terminal_mode;

        match event {
            iced_core::mouse::Event::ButtonPressed(
                iced_core::mouse::Button::Left,
            ) => {
                if !state.is_focused() {
                    return Vec::default();
                }

                Self::handle_left_button_pressed(
                    state,
                    &terminal_mode,
                    cursor_position,
                    layout_position,
                    &mut commands,
                );
            },
            iced_core::mouse::Event::CursorMoved { position } => {
                if !state.is_focused() {
                    return Vec::default();
                }

                Self::handle_cursor_moved(
                    state,
                    self.term.backend.renderable_content(),
                    position,
                    layout_position,
                    &mut commands,
                );
            },
            iced_core::mouse::Event::ButtonReleased(
                iced_core::mouse::Button::Left,
            ) => {
                if !state.is_focused() {
                    return Vec::default();
                }

                Self::handle_button_released(
                    state,
                    &terminal_mode,
                    &self.term.bindings,
                    &mut commands,
                );
            },
            iced::mouse::Event::WheelScrolled { delta } => {
                Self::handle_wheel_scrolled(
                    state,
                    *delta,
                    &self.term.font.measure,
                    self.term.scale_factor,
                    &mut commands,
                );
            },
            _ => {},
        }

        commands
    }

    fn handle_left_button_pressed(
        state: &mut TerminalViewState,
        terminal_mode: &TermMode,
        cursor_position: Point,
        layout_position: Point,
        commands: &mut Vec<Command>,
    ) {
        let cmd = if terminal_mode.intersects(TermMode::MOUSE_MODE) {
            Command::MouseReport(
                MouseButton::LeftButton,
                state.keyboard_modifiers,
                state.mouse_position_on_grid,
                true,
            )
        } else {
            let current_click = Click::new(
                cursor_position,
                mouse::Button::Left,
                state.last_click,
            );
            let selection_type = match current_click.kind() {
                mouse::click::Kind::Single => SelectionType::Simple,
                mouse::click::Kind::Double => SelectionType::Semantic,
                mouse::click::Kind::Triple => SelectionType::Lines,
            };
            state.last_click = Some(current_click);
            Command::SelectStart(
                selection_type,
                (
                    cursor_position.x - layout_position.x,
                    cursor_position.y - layout_position.y,
                ),
            )
        };
        commands.push(cmd);
        state.is_dragged = true;
    }

    fn handle_cursor_moved(
        state: &mut TerminalViewState,
        terminal_content: &RenderableContent,
        position: &Point,
        layout_position: Point,
        commands: &mut Vec<Command>,
    ) {
        let cursor_x = position.x - layout_position.x;
        let cursor_y = position.y - layout_position.y;
        state.mouse_position_on_grid = Backend::selection_point(
            cursor_x,
            cursor_y,
            &terminal_content.terminal_size,
            terminal_content.display_offset,
        );

        // Handle command or selection update based on terminal mode and modifiers
        if state.is_dragged {
            let terminal_mode = terminal_content.terminal_mode;
            let cmd = if terminal_mode.intersects(TermMode::MOUSE_MOTION) {
                Command::MouseReport(
                    MouseButton::LeftMove,
                    state.keyboard_modifiers,
                    state.mouse_position_on_grid,
                    true,
                )
            } else {
                Command::SelectUpdate((cursor_x, cursor_y))
            };
            commands.push(cmd);
        }

        // Handle link hover if applicable
        if state.keyboard_modifiers == Modifiers::COMMAND {
            commands.push(Command::ProcessLink(
                LinkAction::Hover,
                state.mouse_position_on_grid,
            ));
        }
    }

    fn handle_button_released(
        state: &mut TerminalViewState,
        terminal_mode: &TermMode,
        bindings: &BindingsLayout, // Use the actual type of your bindings here
        commands: &mut Vec<Command>,
    ) {
        state.is_dragged = false;

        if terminal_mode.intersects(TermMode::MOUSE_MODE) {
            commands.push(Command::MouseReport(
                MouseButton::LeftButton,
                state.keyboard_modifiers,
                state.mouse_position_on_grid,
                false,
            ));
        }

        if bindings.get_action(
            InputKind::Mouse(iced_core::mouse::Button::Left),
            state.keyboard_modifiers,
            *terminal_mode,
        ) == BindingAction::LinkOpen
        {
            commands.push(Command::ProcessLink(
                LinkAction::Open,
                state.mouse_position_on_grid,
            ));
        }
    }

    fn handle_wheel_scrolled(
        state: &mut TerminalViewState,
        delta: ScrollDelta,
        font_measure: &Size<f32>,
        scale_factor: f32,
        commands: &mut Vec<Command>,
    ) {
        match delta {
            ScrollDelta::Lines { y, .. } => {
                let lines = y.signum() * y.abs().round();
                commands.push(Command::Scroll(
                    lines as i32,
                    state.mouse_position_on_grid,
                    state.keyboard_modifiers,
                ));
            },
            ScrollDelta::Pixels { y, .. } => {
                // Same sign as Lines: positive y = toward history.
                // Deltas are physical pixels; line height is logical.
                state.scroll_pixels += y / scale_factor.max(0.1);
                let line_height = font_measure.height;
                let lines = (state.scroll_pixels / line_height).trunc();
                state.scroll_pixels %= line_height;
                if lines != 0.0 {
                    commands.push(Command::Scroll(
                        lines as i32,
                        state.mouse_position_on_grid,
                        state.keyboard_modifiers,
                    ));
                }
            },
        }
    }

    fn handle_keyboard_event(
        &self,
        state: &mut TerminalViewState,
        clipboard: &mut dyn iced_graphics::core::Clipboard,
        event: &iced::keyboard::Event,
    ) -> Option<Command> {
        let mut binding_action = BindingAction::Ignore;
        let last_content = self.term.backend.renderable_content();
        match event {
            iced::keyboard::Event::ModifiersChanged(m) => {
                state.keyboard_modifiers = *m;
                let action = if state.keyboard_modifiers == Modifiers::COMMAND {
                    LinkAction::Hover
                } else {
                    LinkAction::Clear
                };
                return Some(Command::ProcessLink(
                    action,
                    state.mouse_position_on_grid,
                ));
            },
            iced::keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            } => match &key {
                // Use the physical character key for bindings even when text is None (e.g., Ctrl/Cmd combos)
                Key::Character(k) => {
                    let lower = k.to_ascii_lowercase();
                    binding_action = self.term.bindings.get_action(
                        InputKind::Char(lower),
                        state.keyboard_modifiers,
                        last_content.terminal_mode,
                    );

                    // If no binding matched, only write printable text (when provided)
                    if binding_action == BindingAction::Ignore {
                        if let Some(c) = text {
                            return Some(Command::Write(c.as_bytes().to_vec()));
                        }
                    }
                },
                Key::Named(code) => {
                    binding_action = self.term.bindings.get_action(
                        InputKind::KeyCode(*code),
                        *modifiers,
                        last_content.terminal_mode,
                    );
                },
                _ => {},
            },
            _ => {},
        }

        match binding_action {
            BindingAction::Char(c) => {
                let mut buf = [0, 0, 0, 0];
                let str = c.encode_utf8(&mut buf);
                return Some(Command::Write(str.as_bytes().to_vec()));
            },
            BindingAction::Esc(seq) => {
                return Some(Command::Write(seq.as_bytes().to_vec()));
            },
            BindingAction::Paste => {
                if let Some(data) = clipboard.read(ClipboardKind::Standard) {
                    let input: Vec<u8> = data.bytes().collect();
                    return Some(Command::Write(input));
                }
            },
            BindingAction::Copy => {
                // Only copy our own selection. With no selection, keep the
                // clipboard: Claude Code may have filled it itself (OSC 52).
                let text = self.term.backend.selectable_content();
                if !text.is_empty() {
                    clipboard.write(ClipboardKind::Standard, text);
                }
            },
            _ => {},
        };

        None
    }
}

impl Widget<Event, Theme, iced::Renderer> for TerminalView<'_> {
    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<TerminalViewState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(TerminalViewState::new())
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &iced_core::layout::Limits,
    ) -> iced_core::layout::Node {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        iced::advanced::layout::Node::new(size)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: iced_core::Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn operation::Operation,
    ) {
        let state = tree.state.downcast_mut::<TerminalViewState>();
        let wid = self.term.widget_id();
        operation.focusable(Some(wid), layout.bounds(), state);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        _cursor: Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<TerminalViewState>();
        let content = self.term.backend.renderable_content();
        let term_size = content.terminal_size;
        let cell_width = term_size.cell_width as f32;
        let cell_height = term_size.cell_height as f32;
        let font_size = self.term.font.size;
        let font_scale_factor = self.term.font.scale_factor;
        let layout_offset_x = layout.position().x;
        let layout_offset_y = layout.position().y;

        let geom = self.term.cache.draw(renderer, viewport.size(), |frame| {
            // Read the live grid under its lock, only when a redraw is needed.
            self.term.backend.with_term(|term| {
                let grid = term.grid();
                let cursor_point = grid.cursor.point;
                // Precompute constants used in the inner loop
                let display_offset = grid.display_offset() as f32;
                let cell_size = Size::new(cell_width, cell_height);
                let half_w = cell_width * 0.5;
                let half_h = cell_height * 0.5;
                // We use the background pallete color as a default
                // because the widget global background color must be the same
                let default_bg = self
                    .term
                    .theme
                    .get_color(ansi::Color::Named(NamedColor::Background));

                let mut last_line: Option<i32> = None;
                let mut bg_batch_rect = BackgroundRect::default();

                for indexed in grid.display_iter() {
                    // Compute per-cell geometry cheaply
                    let line = indexed.point.line.0;
                    let col = indexed.point.column.0 as f32;

                    // Resolve position point for this cell
                    let x = layout_offset_x + (col * cell_width);
                    let y = layout_offset_y
                        + (((line as f32) + display_offset) * cell_height);
                    let cell_center_y = y + half_h;
                    let cell_center_x = x + half_w;

                    // Resolve colors for this cell
                    let mut fg = self.term.theme.get_color(indexed.fg);
                    let mut bg = self.term.theme.get_color(indexed.bg);

                    // If the new line was detected,
                    // need to flush pending background rect and init the new one
                    if last_line != Some(line) {
                        if bg_batch_rect.can_flush() {
                            let line = last_line.unwrap_or(line);
                            frame.fill(
                                &bg_batch_rect.build(line),
                                bg_batch_rect.color,
                            );
                        }

                        last_line = Some(line);
                        bg_batch_rect = BackgroundRect::default()
                            .with_cell_height(cell_height)
                            .with_display_offset(display_offset)
                            .with_layout_offset_y(layout_offset_y);
                    }

                    // Handle dim, inverse, and selected text
                    if indexed
                        .cell
                        .flags
                        .intersects(cell::Flags::DIM | cell::Flags::DIM_BOLD)
                    {
                        fg.a *= 0.7;
                    }
                    if indexed.cell.flags.contains(cell::Flags::INVERSE)
                        || content
                            .selectable_range
                            .is_some_and(|r| r.contains(indexed.point))
                    {
                        std::mem::swap(&mut fg, &mut bg);
                    }

                    // Batch draw backgrounds: skip default background (container already paints it)
                    if bg != default_bg {
                        if bg_batch_rect.can_extend(bg, x) {
                            // Same color and contiguous: extend current run
                            bg_batch_rect.extend(cell_width);
                        } else {
                            // New colored run (or non-contiguous): flush previous run if any
                            if bg_batch_rect.can_flush() {
                                frame.fill(
                                    &bg_batch_rect.build(line),
                                    bg_batch_rect.color,
                                );
                            }

                            // Start a new run but do not draw yet; wait for potential extensions
                            bg_batch_rect = BackgroundRect::default()
                                .with_cell_height(cell_height)
                                .with_display_offset(display_offset)
                                .with_layout_offset_y(layout_offset_y)
                                .activate()
                                .with_color(bg)
                                .with_start_x(x)
                                .with_width(cell_width);
                        }
                    } else if bg_batch_rect.can_flush() {
                        // Background returns to default, flush current background rect and init the new one
                        frame.fill(
                            &bg_batch_rect.build(line),
                            bg_batch_rect.color,
                        );

                        bg_batch_rect = BackgroundRect::default()
                            .with_cell_height(cell_height)
                            .with_display_offset(display_offset)
                            .with_layout_offset_y(layout_offset_y);
                    }

                    // Draw hovered hyperlink underline (rare; keep per-cell for correctness)
                    if content.hovered_hyperlink.as_ref().is_some_and(|range| {
                        range.contains(&indexed.point)
                            && range.contains(&state.mouse_position_on_grid)
                    }) || indexed.cell.flags.contains(cell::Flags::UNDERLINE)
                    {
                        let underline_height = y + cell_size.height;
                        let underline = Path::line(
                            Point::new(x, underline_height),
                            Point::new(x + cell_size.width, underline_height),
                        );
                        frame.stroke(
                            &underline,
                            Stroke::default()
                                .with_width(font_size * 0.15)
                                .with_color(fg),
                        );
                    }

                    // Handle cursor rendering
                    if cursor_point == indexed.point
                        && content.terminal_mode.contains(TermMode::SHOW_CURSOR)
                    {
                        let cursor_color =
                            self.term.theme.get_color(content.cursor.fg);
                        let cursor_rect =
                            Path::rectangle(Point::new(x, y), cell_size);
                        frame.fill(&cursor_rect, cursor_color);
                    }

                    // Draw text
                    if indexed.c != ' ' && indexed.c != '\t' {
                        if cursor_point == indexed.point
                            && content
                                .terminal_mode
                                .contains(TermMode::APP_CURSOR)
                        {
                            fg = bg;
                        }
                        // Resolve font style (bold/italic) from cell flags
                        let mut font = self.term.font.font_type;
                        if indexed.cell.flags.intersects(
                            cell::Flags::BOLD | cell::Flags::DIM_BOLD,
                        ) {
                            font.weight = FontWeight::Bold;
                        }
                        if indexed.cell.flags.contains(cell::Flags::ITALIC) {
                            font.style = FontStyle::Italic;
                        }
                        if let Some(radius) =
                            drawn_dot_radius(indexed.cell.c, cell_size)
                        {
                            frame.fill(
                                &Path::circle(
                                    Point::new(cell_center_x, cell_center_y),
                                    radius,
                                ),
                                fg,
                            );
                            continue;
                        }
                        let text = Text {
                            content: indexed.cell.c.to_string(),
                            position: Point::new(cell_center_x, cell_center_y),
                            font,
                            size: iced_core::Pixels(font_size),
                            color: fg,
                            align_x: Alignment::Center,
                            align_y: Vertical::Center,
                            shaping: Shaping::Advanced,
                            line_height: LineHeight::Relative(
                                font_scale_factor,
                            ),
                            ..Default::default()
                        };
                        frame.fill_text(text);
                    }
                }

                // Flush any remaining background run at the end
                if bg_batch_rect.can_flush() {
                    frame.fill(
                        &bg_batch_rect.build(last_line.unwrap_or(0)),
                        bg_batch_rect.color,
                    );
                }
            });
        });

        use iced::advanced::graphics::geometry::Renderer as _;
        renderer.draw_geometry(geom);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced_core::Event,
        layout: iced_graphics::core::Layout<'_>,
        cursor: Cursor,
        _renderer: &iced::Renderer,
        clipboard: &mut dyn iced_graphics::core::Clipboard,
        shell: &mut iced_graphics::core::Shell<'_, Event>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<TerminalViewState>();
        self.handle_resize(state, layout, shell);

        let is_cursor_in_layout = self.is_cursor_in_layout(cursor, layout);
        self.handle_focus(event, state, is_cursor_in_layout);

        let mut drag_end = Vec::new();
        Self::end_drag_outside(
            state,
            &self.term.backend.renderable_content().terminal_mode,
            event,
            is_cursor_in_layout,
            &mut drag_end,
        );

        let commands = match event {
            iced::Event::Mouse(_) if !drag_end.is_empty() => drag_end,
            iced::Event::Mouse(mouse_event) if is_cursor_in_layout => self
                .handle_mouse_event(
                    state,
                    layout.position(),
                    cursor.position().unwrap(),
                    mouse_event,
                ),
            iced::Event::Keyboard(keyboard_event) => {
                if !state.is_focused() {
                    return;
                }

                self.handle_keyboard_event(state, clipboard, keyboard_event)
                    .into_iter()
                    .collect()
            },
            _ => Vec::new(),
        };

        if !commands.is_empty() {
            shell.capture_event();
        }

        for cmd in commands {
            shell.publish(Event::BackendCall(self.term.id, cmd));
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> iced_core::mouse::Interaction {
        let state = tree.state.downcast_ref::<TerminalViewState>();
        let mut cursor_mode = iced_core::mouse::Interaction::Idle;
        let terminal_mode =
            self.term.backend.renderable_content().terminal_mode;
        if self.is_cursor_in_layout(cursor, layout)
            && !terminal_mode.contains(TermMode::SGR_MOUSE)
        {
            cursor_mode = iced_core::mouse::Interaction::Text;
        }

        if self.is_cursor_hovered_hyperlink(state) {
            cursor_mode = iced_core::mouse::Interaction::Pointer;
        }

        cursor_mode
    }
}

impl<'a> From<TerminalView<'a>> for Element<'a, Event, Theme, iced::Renderer> {
    fn from(widget: TerminalView<'a>) -> Self {
        Self::new(widget)
    }
}

#[derive(Debug, Clone)]
struct TerminalViewState {
    focus: bool,
    is_dragged: bool,
    last_click: Option<mouse::Click>,
    scroll_pixels: f32,
    keyboard_modifiers: Modifiers,
    size: Size<f32>,
    /// Terminal the size was sent to. iced keeps this state when another
    /// terminal takes the same place in the tree (switching tabs), and that
    /// terminal still needs its own size.
    sized_term: Option<u64>,
    mouse_position_on_grid: TerminalGridPoint,
}

impl TerminalViewState {
    fn new() -> Self {
        Self {
            focus: false,
            is_dragged: false,
            last_click: None,
            scroll_pixels: 0.0,
            keyboard_modifiers: Modifiers::empty(),
            size: Size::from([0.0, 0.0]),
            sized_term: None,
            mouse_position_on_grid: TerminalGridPoint::default(),
        }
    }

    /// True when terminal `id` has not been told about `size` yet.
    fn needs_resize(&mut self, id: u64, size: Size<f32>) -> bool {
        if self.size == size && self.sized_term == Some(id) {
            return false;
        }
        self.size = size;
        self.sized_term = Some(id);
        true
    }
}

impl Default for TerminalViewState {
    fn default() -> Self {
        Self::new()
    }
}

impl operation::Focusable for TerminalViewState {
    fn is_focused(&self) -> bool {
        self.focus
    }

    fn focus(&mut self) {
        self.focus = true;
    }

    fn unfocus(&mut self) {
        self.focus = false;
    }
}

#[derive(Default)]
struct BackgroundRect {
    display_offset: f32,
    cell_height: f32,
    layout_offset_y: f32,
    is_active: bool,
    color: Color,
    start_x: f32,
    width: f32,
}

impl BackgroundRect {
    fn with_display_offset(mut self, value: f32) -> Self {
        self.display_offset = value;
        self
    }

    fn with_cell_height(mut self, value: f32) -> Self {
        self.cell_height = value;
        self
    }

    fn with_layout_offset_y(mut self, value: f32) -> Self {
        self.layout_offset_y = value;
        self
    }

    fn with_width(mut self, value: f32) -> Self {
        self.width = value;
        self
    }

    fn with_start_x(mut self, value: f32) -> Self {
        self.start_x = value;
        self
    }

    fn with_color(mut self, value: Color) -> Self {
        self.color = value;
        self
    }

    fn activate(mut self) -> Self {
        self.is_active = true;
        self
    }

    fn build(&self, line: i32) -> Path {
        let flush_y = self.layout_offset_y
            + ((line as f32 + self.display_offset) * self.cell_height);
        Path::rectangle(
            Point::new(self.start_x, flush_y),
            Size::new(self.width, self.cell_height),
        )
    }

    fn can_flush(&self) -> bool {
        self.is_active && self.width > 0.0
    }

    fn can_extend(&self, bg: Color, x: f32) -> bool {
        self.is_active
            && bg == self.color
            && (self.start_x + self.width - x).abs() < f32::EPSILON
    }

    fn extend(&mut self, value: f32) {
        self.width += value;
    }
}

/// Claude Code marks its replies with ⏺. Fonts have no text form of it, so
/// macOS falls back to the color emoji (a blue button). Draw it as a plain
/// dot in the text color instead, like Ghostty.
fn drawn_dot_radius(c: char, cell: Size<f32>) -> Option<f32> {
    (c == '\u{23FA}').then(|| cell.width.min(cell.height) * 0.4)
}

#[cfg(test)]
mod tests {
    /// A new tab reuses the widget state of the old one. It must still get
    /// its size, or its PTY keeps the default 50 lines and the bottom rows
    /// are drawn off screen.
    #[test]
    fn reply_marker_is_drawn_as_plain_dot() {
        let cell = Size::new(10.0, 20.0);
        assert_eq!(drawn_dot_radius('\u{23FA}', cell), Some(4.0));
        assert_eq!(drawn_dot_radius('a', cell), None);
        assert_eq!(drawn_dot_radius('\u{25CF}', cell), None);
    }

    #[test]
    fn switching_terminal_with_same_size_still_resizes() {
        let mut state = super::TerminalViewState::new();
        let size = iced::Size::new(800.0, 400.0);
        assert!(state.needs_resize(1, size));
        assert!(!state.needs_resize(1, size));
        assert!(state.needs_resize(2, size));
        assert!(!state.needs_resize(2, size));
    }

    /// Copy must not wipe the clipboard when there is no selection.
    /// With Claude Code, the mouse goes to Claude, which copies by itself
    /// (OSC 52). An empty ⌘C used to overwrite that with "".
    mod copy_tests {
        use super::super::*;
        use crate::settings::{BackendSettings, Settings};
        use iced::keyboard::{key, Key, Location};

        struct TestClipboard(Option<String>);
        impl iced_graphics::core::Clipboard for TestClipboard {
            fn read(&self, _: ClipboardKind) -> Option<String> {
                self.0.clone()
            }
            fn write(&mut self, _: ClipboardKind, contents: String) {
                self.0 = Some(contents);
            }
        }

        #[test]
        fn copy_without_selection_keeps_clipboard() {
            let term = Terminal::new(
                0,
                Settings {
                    backend: BackendSettings {
                        program: "/bin/sh".into(),
                        args: vec!["-c".into(), "sleep 30".into()],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
            let view = TerminalView { term: &term };
            let mut state = TerminalViewState::new();
            state.keyboard_modifiers = Modifiers::COMMAND;
            let mut clipboard = TestClipboard(Some("from claude".into()));
            let event = iced::keyboard::Event::KeyPressed {
                key: Key::Character("c".into()),
                modified_key: Key::Character("c".into()),
                physical_key: key::Physical::Code(key::Code::KeyC),
                location: Location::Standard,
                modifiers: Modifiers::COMMAND,
                text: None,
                repeat: false,
            };
            view.handle_keyboard_event(&mut state, &mut clipboard, &event);
            assert_eq!(clipboard.0.as_deref(), Some("from claude"));
        }
    }

    use super::*;

    mod handle_left_button_pressed_tests {
        use super::*;
        use alacritty_terminal::index::{Column, Line};

        #[test]
        fn handles_mouse_mode_with_left_click() {
            let mut state = TerminalViewState::new();
            let terminal_mode = TermMode::MOUSE_MODE;
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();
            let _modifiers = Modifiers::empty();

            TerminalView::handle_left_button_pressed(
                &mut state,
                &terminal_mode,
                cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::MouseReport(
                    MouseButton::LeftButton,
                    _modifiers,
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(0),
                    },
                    true,
                )
            ));
            assert!(state.is_dragged);
        }

        #[test]
        fn starts_simple_selection_with_left_click() {
            let terminal_mode = TermMode::SGR_MOUSE;
            let cursor_position = Point { x: 200.0, y: 150.0 };
            let layout_position = Point { x: 50.0, y: 50.0 };

            let cases = vec![
                SelectionType::Simple,
                SelectionType::Semantic,
                SelectionType::Lines,
            ];

            for _selection_type in cases {
                let mut state = TerminalViewState::new();
                state.keyboard_modifiers = Modifiers::SHIFT;
                let mut commands = Vec::new();

                TerminalView::handle_left_button_pressed(
                    &mut state,
                    &terminal_mode,
                    cursor_position,
                    layout_position,
                    &mut commands,
                );

                assert_eq!(commands.len(), 1);
                assert!(matches!(
                    commands[0],
                    Command::SelectStart(_selection_type, (150.0, 100.0))
                ),);
                assert!(state.is_dragged);
            }
        }
    }

    mod handle_cursor_moved_tests {
        use alacritty_terminal::index::{Column, Line};

        use super::*;

        #[test]
        fn updates_mouse_position_on_grid() {
            let mut state = TerminalViewState::new();
            let terminal_content = RenderableContent::default();
            let mut commands = Vec::new();
            let cases = vec![
                (
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 1.0, y: 1.0 },
                    TerminalGridPoint {
                        line: Line(1),
                        column: Column(1),
                    },
                ),
                (
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 2.0, y: 2.0 },
                    TerminalGridPoint {
                        line: Line(2),
                        column: Column(2),
                    },
                ),
                (
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 30.0, y: 2.0 },
                    TerminalGridPoint {
                        line: Line(2),
                        column: Column(30),
                    },
                ),
                (
                    Point { x: 10.0, y: 0.0 },
                    Point { x: 30.0, y: 2.0 },
                    TerminalGridPoint {
                        line: Line(2),
                        column: Column(20),
                    },
                ),
                (
                    Point { x: 10.0, y: 10.0 },
                    Point { x: 30.0, y: 2.0 },
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(20),
                    },
                ),
            ];

            for (layout_position, cursor_position, expected) in cases {
                TerminalView::handle_cursor_moved(
                    &mut state,
                    &terminal_content,
                    &cursor_position,
                    layout_position,
                    &mut commands,
                );

                assert_eq!(state.mouse_position_on_grid, expected);
            }
        }

        #[test]
        fn release_outside_terminal_ends_drag() {
            let mut state = TerminalViewState::new();
            state.is_dragged = true;
            let release = iced::Event::Mouse(
                iced_core::mouse::Event::ButtonReleased(
                    iced_core::mouse::Button::Left,
                ),
            );
            let mut commands = Vec::new();

            TerminalView::end_drag_outside(
                &mut state,
                &TermMode::empty(),
                &release,
                false,
                &mut commands,
            );
            assert!(!state.is_dragged);
            assert!(commands.is_empty());

            // Moving back over the terminal must not stretch a selection.
            TerminalView::handle_cursor_moved(
                &mut state,
                &RenderableContent::default(),
                &Point { x: 100.0, y: 150.0 },
                Point { x: 5.0, y: 5.0 },
                &mut commands,
            );
            assert!(commands.is_empty());
        }

        #[test]
        fn generates_drag_update_command_when_dragged() {
            let mut state = TerminalViewState::new();
            state.is_dragged = true; // Simulate an ongoing drag operation
            let terminal_content = RenderableContent::default();
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();

            TerminalView::handle_cursor_moved(
                &mut state,
                &terminal_content,
                &cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::SelectUpdate((95.0, 145.0))
            ));
        }

        #[test]
        fn generates_drag_update_command_when_dragged_in_mouse_motion_mode() {
            let mut state = TerminalViewState::new();
            state.is_dragged = true; // Simulate an ongoing drag operation
            let mut terminal_content = RenderableContent::default();
            terminal_content.terminal_mode = TermMode::MOUSE_MOTION;
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();
            let _modifiers = Modifiers::empty();

            TerminalView::handle_cursor_moved(
                &mut state,
                &terminal_content,
                &cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::MouseReport(
                    MouseButton::LeftMove,
                    _modifiers,
                    TerminalGridPoint {
                        line: Line(49),
                        column: Column(79),
                    },
                    true,
                )
            ));
        }

        #[test]
        fn generates_drag_update_command_when_dragged_in_srg_mode_with_key_mods(
        ) {
            let mut state = TerminalViewState::new();
            state.keyboard_modifiers = Modifiers::SHIFT;
            state.is_dragged = true; // Simulate an ongoing drag operation
            let mut terminal_content = RenderableContent::default();
            terminal_content.terminal_mode = TermMode::SGR_MOUSE;
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();

            TerminalView::handle_cursor_moved(
                &mut state,
                &terminal_content,
                &cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::SelectUpdate((95.0, 145.0))
            ));
        }

        #[test]
        fn generates_drag_update_and_link_open() {
            let mut state = TerminalViewState::new();
            state.keyboard_modifiers = Modifiers::COMMAND;
            state.is_dragged = true; // Simulate an ongoing drag operation
            let mut terminal_content = RenderableContent::default();
            terminal_content.terminal_mode = TermMode::SGR_MOUSE;
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();

            TerminalView::handle_cursor_moved(
                &mut state,
                &terminal_content,
                &cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 2);
            assert!(matches!(
                commands[0],
                Command::SelectUpdate((95.0, 145.0))
            ));
            assert!(matches!(
                commands[1],
                Command::ProcessLink(
                    LinkAction::Hover,
                    TerminalGridPoint {
                        line: Line(49),
                        column: Column(79),
                    },
                )
            ));
        }
    }

    mod handle_button_released_tests {
        use super::*;
        use alacritty_terminal::index::{Column, Line};

        #[test]
        fn mouse_mode_activated() {
            let mut state = TerminalViewState::new();
            let terminal_mode = TermMode::MOUSE_MODE;
            let bindings = BindingsLayout::new();
            let mut commands = Vec::new();
            let _modifiers = Modifiers::empty();

            TerminalView::handle_button_released(
                &mut state,
                &terminal_mode,
                &bindings,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::MouseReport(
                    MouseButton::LeftButton,
                    _modifiers,
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(0)
                    },
                    false
                )
            ));
        }

        #[test]
        fn link_open_on_button_release() {
            let mut state = TerminalViewState::new();
            state.keyboard_modifiers = Modifiers::COMMAND;
            let terminal_mode = TermMode::MOUSE_MODE;
            let bindings = BindingsLayout::new();
            let mut commands = Vec::new();
            let _modifiers = Modifiers::empty();

            TerminalView::handle_button_released(
                &mut state,
                &terminal_mode,
                &bindings,
                &mut commands,
            );

            assert_eq!(commands.len(), 2);
            assert!(matches!(
                commands[0],
                Command::MouseReport(
                    MouseButton::LeftButton,
                    _modifiers,
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(0)
                    },
                    false
                )
            ));
            assert!(matches!(
                commands[1],
                Command::ProcessLink(
                    LinkAction::Open,
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(0)
                    }
                ),
            ));
        }

        #[test]
        fn link_open_on_button_release_in_non_mouse_mode() {
            let mut state = TerminalViewState::new();
            state.keyboard_modifiers = Modifiers::COMMAND;
            state.mouse_position_on_grid = TerminalGridPoint {
                line: Line(4),
                column: Column(10),
            };
            let terminal_mode = TermMode::empty(); // Assume SGR_MOUSE mode doesn't affect link opening
            let bindings = BindingsLayout::new();
            let mut commands = Vec::new();

            TerminalView::handle_button_released(
                &mut state,
                &terminal_mode,
                &bindings,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::ProcessLink(
                    LinkAction::Open,
                    TerminalGridPoint {
                        line: Line(4),
                        column: Column(10)
                    }
                ),
            ));
        }
    }

    mod handle_wheel_scrolled_tests {
        use super::*;
        use crate::font::TermFont;
        use crate::settings::FontSettings;

        /// Trackpad deltas arrive in physical pixels. On a 2x (Retina) screen
        /// the same finger move gives twice the pixels, but must scroll the
        /// same number of lines as on a 1x screen.
        #[test]
        fn pixel_scroll_respects_scale_factor() {
            let font = TermFont::new(FontSettings::default());
            let line = font.measure.height;
            let lines_for = |scale: f32, physical: f32| {
                let mut state = TerminalViewState::new();
                let mut commands = Vec::new();
                TerminalView::handle_wheel_scrolled(
                    &mut state,
                    ScrollDelta::Pixels {
                        x: 0.0,
                        y: physical,
                    },
                    &font.measure,
                    scale,
                    &mut commands,
                );
                commands
                    .iter()
                    .map(|c| match c {
                        Command::Scroll(n, _, _) => *n,
                        _ => 0,
                    })
                    .sum::<i32>()
            };
            // Two logical lines of finger movement.
            assert_eq!(lines_for(1.0, line * 2.0 + 1.0), 2);
            assert_eq!(lines_for(2.0, (line * 2.0 + 1.0) * 2.0), 2);
        }

        #[test]
        fn scroll_with_lines_downward() {
            let mut state = TerminalViewState::new();
            let font = TermFont::new(FontSettings::default());
            let mut commands = Vec::new();

            TerminalView::handle_wheel_scrolled(
                &mut state,
                ScrollDelta::Lines { y: 3.0, x: 0.0 }, // Scroll down 3 lines
                &font.measure,
                1.0,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::Scroll(3, _, _)));
        }

        #[test]
        fn scroll_with_lines_upward() {
            let mut state = TerminalViewState::new();
            let font = TermFont::new(FontSettings::default());
            let mut commands = Vec::new();

            TerminalView::handle_wheel_scrolled(
                &mut state,
                ScrollDelta::Lines { y: -2.0, x: 0.0 },
                &font.measure,
                1.0,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::Scroll(-2, _, _)));
        }

        #[test]
        fn scroll_with_pixels_accumulating_downward() {
            let mut state = TerminalViewState::new();
            let font = TermFont::new(FontSettings::default());
            let mut commands = Vec::new();

            TerminalView::handle_wheel_scrolled(
                &mut state,
                ScrollDelta::Pixels { y: 45.0, x: 0.0 },
                &font.measure,
                1.0,
                &mut commands,
            );

            // Same sign as ScrollDelta::Lines: positive y scrolls toward history.
            // macOS already applies "natural scrolling" to the delta.
            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::Scroll(2, _, _)));
            assert_eq!(state.scroll_pixels, 8.600002);
        }

        #[test]
        fn scroll_with_pixels_accumulating_upward() {
            let mut state = TerminalViewState::new();
            let font = TermFont::new(FontSettings::default());
            let mut commands = Vec::new();

            TerminalView::handle_wheel_scrolled(
                &mut state,
                ScrollDelta::Pixels { y: -60.0, x: 0.0 },
                &font.measure,
                1.0,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::Scroll(-3, _, _)));
            assert_eq!(state.scroll_pixels, -5.4000034);
        }
    }
}
