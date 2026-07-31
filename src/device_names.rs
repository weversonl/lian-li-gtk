//! Local device nicknames, client-side only. device_id -> nickname.

use std::collections::HashMap;

const FILE: &str = "device_names.json";

pub fn load() -> HashMap<String, String> {
    crate::json_store::load(FILE)
}

pub fn save(names: &HashMap<String, String>) {
    crate::json_store::save(FILE, names);
}
