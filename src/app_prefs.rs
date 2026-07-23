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
    /// Device auto-selected on startup. `None` = first in `device_order`.
    #[serde(default)]
    pub default_device_id: Option<String>,
}

fn default_global_effect_mode() -> RgbMode {
    RgbMode::Rainbow
}

fn default_true() -> bool {
    true
}

fn default_gradient_colors() -> [[u8; 3]; 8] {
    [
        [255, 0, 0],
        [255, 0, 150],
        [0, 0, 139],
        [0, 191, 255],
        [0, 200, 0],
        [255, 255, 0],
        [255, 140, 0],
        [255, 0, 0],
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
            default_device_id: None,
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
