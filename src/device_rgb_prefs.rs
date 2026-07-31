//! Per-device Rainbow prefs (wave direction + physical LED strip count), client-side only.

use lianli_shared::rgb::RgbDirection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DeviceRgbPrefs {
    pub direction: RgbDirection,
    pub strip_count: usize,
    /// Flips the rendered wave direction on top of `direction` — some
    /// devices have their physical LED wiring order reversed.
    #[serde(default)]
    pub invert_direction: bool,
    /// Whether Meteor treats the strip as a closed ring (wraps seamlessly)
    /// vs. two real physical ends — see `meteor_frames`.
    #[serde(default)]
    pub meteor_circular: bool,
    /// Physical mount rotation of this device's fan ring(s), in degrees;
    /// 0 = local LED index 0 at 12 o'clock — see `effects::band_positions`.
    #[serde(default)]
    pub ring_offset_deg: f64,
}

impl Default for DeviceRgbPrefs {
    fn default() -> Self {
        Self {
            direction: RgbDirection::Up,
            strip_count: 1,
            invert_direction: false,
            meteor_circular: false,
            ring_offset_deg: 0.0,
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
    base.join("lian-li-gtk").join("device_rgb_prefs.json")
}

pub fn load() -> HashMap<String, DeviceRgbPrefs> {
    let path = config_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(prefs: &HashMap<String, DeviceRgbPrefs>) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(prefs) {
        let _ = fs::write(&path, json);
    }
}
