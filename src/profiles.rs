//! Named full-app snapshots ("Perfis") — see `Ctx::profiles`/`apply_profile`.
//!
//! A profile can't just be a daemon `AppConfig`: wireless animated effects
//! and RGB Editor "Segments" are deliberately excluded from `AppConfig` (see
//! `rgb_persist::clear_stale_wireless_animations`) since `SetConfig` would
//! otherwise interrupt them with an RF push. So a profile bundles the daemon
//! config alongside the same client-side state `identify::reapply_last_effect`
//! already knows how to replay.

use crate::context::{LastEffect, RgbEditorSnapshot};
use crate::device_rgb_prefs::DeviceRgbPrefs;
use lianli_shared::config::AppConfig;
use lianli_shared::rgb::RgbMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The subset of `AppPrefs` that represents hardware configuration rather
/// than pure UI preference (`lang`, `fan_curve_graph_view`, `default_device_id`
/// are deliberately excluded).
#[derive(Clone, Serialize, Deserialize)]
pub struct ProfileAppPrefs {
    pub global_direction_enabled: bool,
    pub gradient_colors: [[u8; 3]; 8],
    pub global_effect_mode: RgbMode,
    pub global_effect_color: [u8; 3],
    pub global_effect_meteor_tail: [u8; 3],
    pub global_effect_speed_percent: f64,
    pub global_effect_fps: f64,
    pub global_effect_brightness_percent: f64,
    pub global_effect_meteor_pause_secs: f64,
    pub global_effect_sync_devices: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub app_config: AppConfig,
    pub last_effect: HashMap<String, LastEffect>,
    pub editor_snapshots: HashMap<String, RgbEditorSnapshot>,
    pub rgb_prefs: HashMap<String, DeviceRgbPrefs>,
    pub device_order: Vec<String>,
    pub app_prefs: ProfileAppPrefs,
}

const FILE: &str = "profiles.json";

pub fn load() -> Vec<Profile> {
    crate::json_store::load(FILE)
}

pub fn save(profiles: &[Profile]) {
    crate::json_store::save(FILE, profiles);
}
