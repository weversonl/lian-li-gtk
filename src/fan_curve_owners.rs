//! Local-only bookkeeping: which fan hub "owns" each saved curve. The
//! daemon has no such concept — curves are just named entries in one
//! global `AppConfig.fan_curves` list, referenced by name from any hub's
//! `FanGroup`. This file exists purely so the GTK client can keep each
//! hub's curves visually and operationally separate, instead of letting
//! any hub pick any curve (which let editing one hub's curve silently
//! affect another hub sharing it by name).

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
    base.join("lian-li-gtk").join("fan_curve_owners.json")
}

pub fn load() -> HashMap<String, String> {
    let path = config_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(owners: &HashMap<String, String>) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(owners) {
        let _ = fs::write(&path, json);
    }
}
