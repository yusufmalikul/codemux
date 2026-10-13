// Based on iced_term 0.8 (https://github.com/Harzu/iced_term), MIT License,
// Copyright (c) Ilya Shvyryalkin. Modified for codemux. See NOTICE.
use crate::actions::Action;
use crate::backend;
use crate::bindings::{Binding, BindingAction, BindingsLayout, InputKind};
use crate::font::TermFont;
use crate::settings::{FontSettings, Settings, ThemeSettings};
use crate::theme::{ColorPalette, Theme};
use crate::AlacrittyEvent;
use iced::futures::stream::BoxStream;
use iced::futures::{SinkExt, StreamExt};
use iced::widget::canvas::Cache;
use iced::Subscription;
use std::hash::{Hash, Hasher};
use std::io::Result;
use std::sync::Arc;
use tokio::sync::mpsc::{self, Receiver};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub enum Event {
    BackendCall(u64, backend::Command),
}

#[derive(Debug, Clone)]
pub enum Command {
    ChangeTheme(Box<ColorPalette>),
    ChangeFont(FontSettings),
    SetScaleFactor(f32),
    AddBindings(Vec<(Binding<InputKind>, BindingAction)>),
    ProxyToBackend(backend::Command),
}

pub struct Terminal {
    pub id: u64,
    widget_id: iced::widget::Id,
    pub(crate) font: TermFont,
    pub(crate) theme: Theme,
    pub(crate) cache: Cache,
    pub(crate) bindings: BindingsLayout,
    pub(crate) backend: backend::Backend,
    /// Window scale factor (2.0 on Retina). Trackpad deltas are in physical
    /// pixels, so they are divided by this.
    pub(crate) scale_factor: f32,
    backend_event_rx: Arc<Mutex<Receiver<AlacrittyEvent>>>,
}

impl Terminal {
    pub fn new(id: u64, settings: Settings) -> Result<Self> {
        let (backend_event_tx, backend_event_rx) = mpsc::channel(100);
        let theme = Theme::new(settings.theme);
        let font = TermFont::new(settings.font);

        let mut term = Self {
            id,
            widget_id: iced::widget::Id::unique(),
            font,
            theme,
            bindings: BindingsLayout::default(),
            cache: Cache::default(),
            scale_factor: 1.0,
            backend: backend::Backend::new(
                id,
                backend_event_tx,
                settings.backend,
            )?,
            backend_event_rx: Arc::new(Mutex::new(backend_event_rx)),
        };
        // Measure the font once. Doing it per event is costly (text shaping).
        term.sync_font();
        Ok(term)
    }

    /// PID of the process started in the PTY (usually the shell).
    pub fn child_pid(&self) -> Option<u32> {
        self.backend.child_pid()
    }

    pub fn widget_id(&self) -> &iced::widget::Id {
        &self.widget_id
    }

    pub fn subscription(&self) -> Subscription<Event> {
        let data = TerminalSubscriptionData {
            id: self.id,
            event_receiver: self.backend_event_rx.clone(),
        };

        Subscription::run_with(data, terminal_subscription_stream)
    }

    pub fn handle(&mut self, cmd: Command) -> Action {
        let mut action = Action::default();

        match cmd {
            Command::ChangeTheme(color_pallete) => {
                self.theme = Theme::new(ThemeSettings::new(color_pallete));
            },
            Command::ChangeFont(font_settings) => {
                self.font = TermFont::new(font_settings);
                self.sync_font();
            },
            Command::SetScaleFactor(scale) => {
                if scale > 0.0 {
                    self.scale_factor = scale;
                }
            },
            Command::AddBindings(bindings) => {
                self.bindings.add_bindings(bindings);
            },
            Command::ProxyToBackend(cmd) => {
                action = self.backend.handle(cmd);
            },
        };

        self.backend.sync();
        self.redraw();
        action
    }

    fn sync_font(&mut self) {
        self.font.sync();
        self.backend
            .handle(backend::Command::Resize(None, Some(self.font.measure)));
    }

    fn redraw(&mut self) {
        self.cache.clear();
    }
}

#[derive(Clone)]
struct TerminalSubscriptionData {
    id: u64,
    event_receiver: Arc<Mutex<Receiver<AlacrittyEvent>>>,
}

impl Hash for TerminalSubscriptionData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

fn terminal_subscription_stream(
    data: &TerminalSubscriptionData,
) -> BoxStream<'static, Event> {
    let id = data.id;
    let event_receiver = data.event_receiver.clone();
    iced::stream::channel(1000, async move |mut output| {
        let mut event_receiver = event_receiver.lock().await;
        while let Some(first) = event_receiver.recv().await {
            // Fast output produces many Wakeup events in a row. Each one would
            // copy the grid and redraw, so send at most one per batch.
            let mut batch = vec![first];
            while let Ok(next) = event_receiver.try_recv() {
                batch.push(next);
            }
            for event in coalesce_wakeups(batch) {
                let msg = Event::BackendCall(
                    id,
                    backend::Command::ProcessAlacrittyEvent(event),
                );
                if output.send(msg).await.is_err() {
                    return;
                }
            }
        }
        // Channel closed: the PTY is gone. Stay idle until the
        // subscription is dropped.
        std::future::pending::<()>().await;
    })
    .boxed()
}

/// Keep every event in order, but only the last of several Wakeup events.
fn coalesce_wakeups(batch: Vec<AlacrittyEvent>) -> Vec<AlacrittyEvent> {
    let last_wakeup = batch
        .iter()
        .rposition(|e| matches!(e, AlacrittyEvent::Wakeup));
    batch
        .into_iter()
        .enumerate()
        .filter(|(i, e)| {
            !matches!(e, AlacrittyEvent::Wakeup) || Some(*i) == last_wakeup
        })
        .map(|(_, e)| e)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_keeps_one_wakeup_and_all_other_events() {
        let out = coalesce_wakeups(vec![
            AlacrittyEvent::Wakeup,
            AlacrittyEvent::Title("a".into()),
            AlacrittyEvent::Wakeup,
            AlacrittyEvent::Bell,
            AlacrittyEvent::Wakeup,
        ]);
        let names: Vec<String> = out.iter().map(|e| format!("{e:?}")).collect();
        assert_eq!(names, vec!["Title(a)", "Bell", "Wakeup"]);
    }
}
