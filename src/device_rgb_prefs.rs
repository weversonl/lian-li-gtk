//! Local, per-device Rainbow settings (wave direction + physical LED strip
//! count) that the user dials in once and shouldn't have to re-enter every
//! session — e.g. this cable really does have 8 physical strips and really
//! does look best running "Down". Purely a client-side preference, same
//! spirit as `device_names`: never sent to the daemon, never affects
//! `AppConfig` or hardware state, just what this app pre-fills the Rainbow
//! controls with for a given device_id.

use lianli_shared::rgb::RgbDirection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DeviceRgbPrefs {
    pub direction: RgbDirection,
    pub strip_count: usize,
}

impl Default for DeviceRgbPrefs {
    fn default() -> Self {
        Self { direction: RgbDirection::Up, strip_count: 1 }
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
