//! Small local, page-level (not per-device) UI preferences — currently just
//! whether the Global Effects "Orientation" default is enabled. Same spirit
//! as `device_names`/`device_rgb_prefs`/`device_order`: plain JSON under the
//! user's config dir, never touches the daemon/hardware.

use lianli_shared::rgb::RgbMode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// UI language. Applied at startup (see `src/i18n.rs`) — switching it in
/// Preferences takes effect on the next launch, not live, since every page
/// in this app builds its widgets once from Rust string literals rather
/// than binding to a reactive text source; rebuilding the entire UI tree
/// in place would be far more failure-prone than asking for a restart.
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
    /// Whether the Fan Curve page's "View" toggle should default to Graph
    /// instead of List — remembers whichever the user picked last, since
    /// forcing it back to List every time the page reopened ignored an
    /// explicit choice.
    #[serde(default)]
    pub fan_curve_graph_view: bool,
    #[serde(default)]
    pub lang: Lang,
    /// Global Effects' Gradient Wave stops — see `Ctx::gradient_colors`.
    /// Defaults to the user's own reference gradient (OpenRGB's "Unicorn
    /// Vomit" preset: red, pink, dark blue, light blue, green, yellow,
    /// orange, red again to close the loop) instead of 8 identical white
    /// swatches, so the very first time this page opens there's already
    /// something worth seeing instead of a blank white strip.
    #[serde(default = "default_gradient_colors")]
    pub gradient_colors: [[u8; 3]; 8],
    /// Last effect mode picked on the Global Effects page — without this it
    /// always reopened on Rainbow regardless of what was actually applied
    /// last (e.g. picking Gradient Wave, leaving the page, and coming back
    /// silently forgot it and showed Rainbow selected instead).
    #[serde(default = "default_global_effect_mode")]
    pub global_effect_mode: RgbMode,
    /// Which device's detail should be auto-selected on startup (Dashboard
    /// sidebar) — see `Ctx::default_device_id`. `None` means "whatever
    /// device ends up first in `device_order`", not "no selection".
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
