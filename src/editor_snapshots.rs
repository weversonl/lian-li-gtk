//! Disk persistence for `RgbEditorSnapshot` (the RGB Editor's last-used
//! controls per device) — purely a client-side convenience, same spirit as
//! `device_rgb_prefs`: never sent to the daemon, never touches hardware
//! state. Without this, closing and reopening the app reset every device's
//! editor back to Static/100% brightness/zone 0, discarding whatever was
//! actually applied last, even though the in-session cache remembered it
//! fine across page navigation.

use crate::context::RgbEditorSnapshot;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("lian-li-gtk").join("editor_snapshots.json")
}

pub fn load() -> HashMap<String, RgbEditorSnapshot> {
    let path = config_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(snapshots: &HashMap<String, RgbEditorSnapshot>) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(snapshots) {
        let _ = fs::write(&path, json);
    }
}
