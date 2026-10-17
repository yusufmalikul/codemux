// Based on iced_term 0.8 (https://github.com/Harzu/iced_term), MIT License,
// Copyright (c) Ilya Shvyryalkin. Modified for codemux. See NOTICE.
use std::{collections::HashMap, path::PathBuf, time::Duration};

use crate::ColorPalette;
use iced::Font;

#[cfg(target_os = "windows")]
const DEFAULT_SHELL: &str = "powershell.exe";

#[cfg(not(target_os = "windows"))]
const DEFAULT_SHELL: &str = "/bin/bash";

#[derive(Default, Clone)]
pub struct Settings {
    pub font: FontSettings,
    pub theme: ThemeSettings,
    pub backend: BackendSettings,
}

#[derive(Debug, Clone)]
pub struct BackendSettings {
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub working_directory: Option<PathBuf>,
    /// How long the program gets to exit after SIGHUP, when the terminal
    /// closes, before it is killed.
    pub stop_grace: Duration,
    /// Lines kept above the screen for scrolling back. Every line costs
    /// memory for each column (about 24 bytes per cell), so 10,000 lines at
    /// 200 columns would be ~50 MB per terminal. Claude Code redraws a lot,
    /// so its scrollback fills up fast. The full conversation is in the
    /// session file anyway.
    pub scrollback: usize,
}

impl Default for BackendSettings {
    fn default() -> Self {
        Self {
            program: DEFAULT_SHELL.to_string(),
            args: vec![],
            env: HashMap::new(),
            working_directory: None,
            stop_grace: Duration::from_secs(3),
            scrollback: 3000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FontSettings {
    pub size: f32,
    pub scale_factor: f32,
    pub font_type: Font,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            size: 14.0,
            scale_factor: 1.3,
            font_type: Font::MONOSPACE,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct ThemeSettings {
    pub color_pallete: Box<ColorPalette>,
}

impl ThemeSettings {
    pub fn new(color_pallete: Box<ColorPalette>) -> Self {
        Self { color_pallete }
    }
}
