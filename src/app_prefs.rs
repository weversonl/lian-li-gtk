//! Small page-level (not per-device) UI preferences, persisted as JSON.

use lianli_shared::rgb::RgbMode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// UI language. Takes effect on next launch, not live — see `src/i18n.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "pt_br")]
    PtBr,
}

impl Default for Lang {
    fn default() -> Self {
        Lang::En
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPrefs {
    #[serde(default = "default_true")]
    pub global_direction_enabled: bool,
    #[serde(default)]
    pub fan_curve_graph_view: bool,
    #[serde(default)]
    pub lang: Lang,
    /// Global Effects' Gradient Wave stops — see `Ctx::gradient_colors`.
    #[serde(default = "default_gradient_colors")]
    pub gradient_colors: [[u8; 3]; 8],
    /// Last effect mode picked on the Global Effects page.
    #[serde(default = "default_global_effect_mode")]
    pub global_effect_mode: RgbMode,
    /// Global Effects' single-color swatch (Static/Breathing/Meteor head).
    #[serde(default = "default_white")]
    pub global_effect_color: [u8; 3],
    /// Global Effects' Meteor tail/background color.
    #[serde(default)]
    pub global_effect_meteor_tail: [u8; 3],
    #[serde(default = "default_thirty")]
    pub global_effect_speed_percent: f64,
    #[serde(default = "default_thirty")]
    pub global_effect_fps: f64,
    #[serde(default = "default_twenty_five")]
    pub global_effect_brightness_percent: f64,
    #[serde(default = "default_meteor_pause_secs")]
    pub global_effect_meteor_pause_secs: f64,
    /// "Sincronizar Efeito" — chains devices in order, one full turn each,
    /// instead of every device looping the effect independently at once.
    #[serde(default)]
    pub global_effect_sync_devices: bool,
    /// Device auto-selected on startup. `None` = first in `device_order`.
    #[serde(default)]
    pub default_device_id: Option<String>,
    /// Last non-maximized main window size, saved on close.
    #[serde(default = "default_window_width")]
    pub window_width: i32,
    #[serde(default = "default_window_height")]
    pub window_height: i32,
}

fn default_global_effect_mode() -> RgbMode {
    RgbMode::Rainbow
}

fn default_true() -> bool {
    true
}

fn default_white() -> [u8; 3] {
    [255, 255, 255]
}

fn default_thirty() -> f64 {
    30.0
}

fn default_twenty_five() -> f64 {
    25.0
}

fn default_window_width() -> i32 {
    980
}

fn default_window_height() -> i32 {
    640
}

fn default_meteor_pause_secs() -> f64 {
    5.0
}

pub fn default_gradient_colors() -> [[u8; 3]; 8] {
    [
        [0xE6, 0x00, 0x00],
        [0xE6, 0x00, 0xCF],
        [0x00, 0x00, 0xE6],
        [0x00, 0xA1, 0xE6],
        [0x00, 0xE6, 0x49],
        [0xD3, 0xE6, 0x00],
        [0xE6, 0xA1, 0x00],
        [0xE6, 0x00, 0x00],
    ]
}

impl Default for AppPrefs {
    fn default() -> Self {
        Self {
            global_direction_enabled: true,
            fan_curve_graph_view: false,
            lang: Lang::default(),
            gradient_colors: default_gradient_colors(),
            global_effect_mode: default_global_effect_mode(),
            global_effect_color: default_white(),
            global_effect_meteor_tail: [0, 0, 0],
            global_effect_speed_percent: default_thirty(),
            global_effect_fps: default_thirty(),
            global_effect_brightness_percent: default_twenty_five(),
            global_effect_meteor_pause_secs: default_meteor_pause_secs(),
            global_effect_sync_devices: false,
            default_device_id: None,
            window_width: default_window_width(),
            window_height: default_window_height(),
        }
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("lian-li-gtk").join("app_prefs.json")
}

pub fn load() -> AppPrefs {
    let path = config_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(prefs: &AppPrefs) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(prefs) {
        let _ = fs::write(&path, json);
    }
}
