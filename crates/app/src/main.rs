//! Codemux: a session manager for Claude Code.
//! Sidebar of sessions on the left, a live terminal on the right.

// Windows: a release build is a GUI app, so no console window opens next to it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;

use app::App;
use codemux_core::launch;

fn main() -> iced::Result {
    launch::clean_inherited_env();
    #[cfg(unix)]
    launch::load_shell_path();
    iced::application(App::new, App::update, App::view)
        .title("Codemux")
        .font(include_bytes!("../assets/fonts/FiraSans-Bold.ttf").as_slice())
        .window(window_settings())
        .subscription(App::subscription)
        .theme(App::theme)
        .run()
}

/// The app runs as a plain binary, not a `.app` bundle, so macOS has no icon
/// file to read. Set the Dock icon at runtime instead.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(include_bytes!("../assets/icon.png"));
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(not(target_os = "macos"))]
fn set_dock_icon() {}

fn window_settings() -> iced::window::Settings {
    iced::window::Settings {
        size: iced::Size::new(1280.0, 800.0),
        // macOS takes the Dock icon from set_dock_icon. Windows and X11 show this one.
        icon: if cfg!(target_os = "macos") { None } else { window_icon() },
        // Wayland finds the icon through the .desktop file with this name
        // (scripts/install-linux.sh).
        #[cfg(target_os = "linux")]
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: "codemux".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn window_icon() -> Option<iced::window::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/icon-256.png")));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    iced::window::icon::from_rgba(buf, info.width, info.height).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_decodes() {
        assert!(window_icon().is_some());
    }
}
