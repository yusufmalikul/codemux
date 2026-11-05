//! OS notifications: tell the user when Claude waits for input in a tab
//! they are not looking at (spec 19, question 3), and count the waiting
//! tabs on the Dock icon.

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
    /// from working to waiting gets a notification and counts as unseen,
    /// unless the user is looking at it right now: its tab is selected and
    /// the window has focus. Looking at it, or Claude working again, makes
    /// it seen.
    pub(super) fn notify_waiting(&mut self) {
        for i in 0..self.tabs.len() {
            let status = self.tab_status(&self.tabs[i]);
            let was = self.tabs[i].last_status.replace(status);
            let on_screen = self.window_focused && self.selected == Selected::Tab(self.tabs[i].id);
            if on_screen || status != "waiting" {
                self.tabs[i].unseen = false;
                continue;
            }
            if was != Some("working") {
                continue;
            }
            self.tabs[i].unseen = true;
            let (title, folder) = self.tab_title(&self.tabs[i]);
            let alert = Alert { title, body: format!("Claude is waiting for your input · {folder}") };
            #[cfg(test)]
            self.notified.push(alert);
            #[cfg(not(test))]
            send(alert);
        }
    }
}

/// Put `count` on the Dock icon, or clear it at 0. macOS only: other
/// systems have no common way to do this.
#[cfg(all(target_os = "macos", not(test)))]
fn set_dock_badge(count: usize) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    // AppKit must be called on the main thread, where iced runs `update`.
    let Some(mtm) = MainThreadMarker::new() else { return };
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(label.as_deref());
}

#[cfg(all(not(target_os = "macos"), not(test)))]
fn set_dock_badge(_count: usize) {}

impl App {
    /// Show on the Dock how many tabs have Claude waiting for input that the
    /// user has not looked at yet (see notify_waiting). The OS is called
    /// only when the number changes.
    pub(super) fn update_dock_badge(&mut self) {
        let count = self.tabs.iter().filter(|t| t.unseen).count();
        if count != self.dock_badge {
            self.dock_badge = count;
            #[cfg(not(test))]
            set_dock_badge(count);
        }
    }
}
