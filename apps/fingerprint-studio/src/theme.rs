// SPDX-License-Identifier: GPL-3.0-or-later
//! Appearance preferences contain no biometric/session data.
use std::{io, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    System,
}
impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::System => "System",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
            Self::System => Self::Light,
        }
    }
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Light => "sun",
            Self::Dark => "moon",
            Self::System => "monitor",
        }
    }
    pub fn palette(self, system_dark: bool) -> Palette {
        match self {
            Self::Dark => Palette::dark(),
            Self::System if system_dark => Palette::dark(),
            _ => Palette::light(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: u32,
    pub surface: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub on_accent: u32,
    pub hover: u32,
    pub selected: u32,
    pub success: u32,
    pub danger: u32,
    pub warning: u32,
    pub warning_background: u32,
}
impl Palette {
    pub const fn light() -> Self {
        Self {
            background: 0xffffff,
            surface: 0xf3f6fa,
            border: 0xd9e2ec,
            text: 0x25364a,
            muted: 0x5c6e82,
            accent: 0x245ac6,
            accent_hover: 0x194ba8,
            on_accent: 0xffffff,
            hover: 0xeaf0f8,
            selected: 0xffffff,
            success: 0x23705a,
            danger: 0xa13930,
            warning: 0x7b5a14,
            warning_background: 0xf9f3e4,
        }
    }
    pub const fn dark() -> Self {
        Self {
            background: 0x171a1f,
            surface: 0x20252b,
            border: 0x3c4550,
            text: 0xf1f3f5,
            muted: 0xadb9c8,
            accent: 0x90b4ff,
            accent_hover: 0xb0caff,
            on_accent: 0x11213c,
            hover: 0x2d3540,
            selected: 0x303a48,
            success: 0x8cd7b2,
            danger: 0xffaaa1,
            warning: 0xf5d086,
            warning_background: 0x30291c,
        }
    }
}

pub struct PreferenceStore {
    path: PathBuf,
}
impl PreferenceStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn for_current_user() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let root = if cfg!(target_os = "macos") {
            home?.join("Library/Application Support/Fingerprint Studio")
        } else if cfg!(target_os = "windows") {
            PathBuf::from(std::env::var_os("APPDATA")?).join("Fingerprint Studio")
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| home.map(|path| path.join(".config")))?
                .join("fingerprint-studio")
        };
        Some(Self::new(root.join("appearance")))
    }
    pub fn load(&self) -> ThemeMode {
        match std::fs::read_to_string(&self.path)
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("light") => ThemeMode::Light,
            Some("dark") => ThemeMode::Dark,
            _ => ThemeMode::System,
        }
    }
    pub fn save(&self, mode: ThemeMode) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let value = match mode {
            ThemeMode::Light => "light\n",
            ThemeMode::Dark => "dark\n",
            ThemeMode::System => "system\n",
        };
        std::fs::write(&self.path, value)
    }
}
