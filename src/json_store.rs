//! Shared JSON persistence for `~/.config/lian-li-gtk/*.json`.
//!
//! `save` writes to a sibling temp file and renames it over the target —
//! rename within the same directory is atomic, so a crash or power loss
//! mid-write leaves either the old file or the fully-written new one, never
//! a truncated/corrupted one. Every other `fs::write`-based save here used
//! to truncate the file in place first, which could lose the whole thing
//! (device names, RGB prefs, saved Profiles, ...) if the process died at
//! the wrong moment.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

fn config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".config")
    });
    base.join("lian-li-gtk")
}

pub fn load<T: DeserializeOwned + Default>(filename: &str) -> T {
    fs::read_to_string(config_dir().join(filename))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save<T: Serialize + ?Sized>(filename: &str, value: &T) {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let Ok(json) = serde_json::to_string_pretty(value) else { return };
    let tmp_path = dir.join(format!("{filename}.tmp"));
    if fs::write(&tmp_path, json).is_ok() {
        let _ = fs::rename(&tmp_path, dir.join(filename));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn save_then_load_round_trips_and_leaves_no_tmp_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("json_store_test_{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        let data: HashMap<String, i32> = HashMap::from([("a".to_string(), 1)]);
        save("t.json", &data);
        let loaded: HashMap<String, i32> = load("t.json");
        assert_eq!(loaded, data);

        let store_dir = dir.join("lian-li-gtk");
        assert!(store_dir.join("t.json").is_file());
        assert!(!store_dir.join("t.json.tmp").exists());

        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
