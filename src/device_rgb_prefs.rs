//! Per-device Rainbow prefs (wave direction + physical LED strip count), client-side only.

use lianli_shared::rgb::RgbDirection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

const FILE: &str = "device_rgb_prefs.json";

pub fn load() -> HashMap<String, DeviceRgbPrefs> {
    crate::json_store::load(FILE)
}

pub fn save(prefs: &HashMap<String, DeviceRgbPrefs>) {
    crate::json_store::save(FILE, prefs);
}
