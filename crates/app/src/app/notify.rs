//! OS notifications: tell the user when Claude waits for input in a tab
//! they are not looking at (spec 19, question 3).

use super::*;

/// What a notification says.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Alert {
    pub title: String,
    pub body: String,
}

/// Once at startup. On macOS, notifications from a plain binary would show
/// as from Finder; name the installed app so they show as from Codemux.
/// Nothing happens when the app is not installed (`cargo run`).
pub(super) fn prepare() {
    #[cfg(target_os = "macos")]
    let _ = notify_rust::set_application("dev.codemux.app");
}

#[cfg(not(test))]
fn send(alert: Alert) {
    // The OS call can block for a moment (D-Bus on Linux); keep it off the UI thread.
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().appname("Codemux").summary(&alert.title).body(&alert.body).show();
    });
}

impl App {
    /// Compare each tab's status with the last check. A tab whose Claude went
    /// from working to waiting gets a notification, unless the user is
    /// looking at it right now: its tab is selected and the window has focus.
    pub(super) fn notify_waiting(&mut self) {
        for i in 0..self.tabs.len() {
            let status = self.tab_status(&self.tabs[i]);
            let was = self.tabs[i].last_status.replace(status);
            if was != Some("working") || status != "waiting" {
                continue;
            }
            let on_screen = self.window_focused && self.selected == Selected::Tab(self.tabs[i].id);
            if on_screen {
                continue;
            }
            let (title, folder) = self.tab_title(&self.tabs[i]);
            let alert = Alert { title, body: format!("Claude is waiting for your input · {folder}") };
            #[cfg(test)]
            self.notified.push(alert);
            #[cfg(not(test))]
            send(alert);
        }
    }
}
