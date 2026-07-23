//! Disk persistence for `LastEffect` (the last effect actually applied to
//! each wireless device, from either the RGB Editor or Global Effects) —
//! same spirit as `editor_snapshots`: purely client-side, never touches the
//! daemon/hardware on its own. Without this, "Identify" and rebind-restore
//! only had an in-session (in-memory) record to reapply after blinking a
//! device — restarting the app between applying an animation and hitting
//! Identify silently lost it, so Identify fell back to capturing/restoring
//! a single still frame instead of the animation, which for a device mid-
//! animation could easily read back as the device just turning off.

use crate::context::LastEffect;
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
    base.join("lian-li-gtk").join("last_effect.json")
}

pub fn load() -> HashMap<String, LastEffect> {
    let path = config_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(effects: &HashMap<String, LastEffect>) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(effects) {
        let _ = fs::write(&path, json);
    }
}
