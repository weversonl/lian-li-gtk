//! Local, persisted ordering for the "Included Devices" list in Global
//! Effects — the Rainbow wave's cross-device order matters (see
//! `effects::rainbow_wave_frames`), so losing it on every restart meant
//! re-dragging ▲▼ back into place each session. Same spirit as
//! `device_names`/`device_rgb_prefs`: a plain ordered list of device_ids,
//! never sent to the daemon.

use std::fs;
use std::path::PathBuf;

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("lian-li-gtk").join("device_order.json")
}

pub fn load() -> Vec<String> {
    let path = config_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(order: &[String]) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(order) {
        let _ = fs::write(&path, json);
    }
}
