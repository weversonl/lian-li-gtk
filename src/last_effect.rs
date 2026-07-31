//! Disk persistence for `LastEffect` — last effect applied per wireless
//! device, used to restore after Identify or a rebind.

use crate::context::LastEffect;
use std::collections::HashMap;

const FILE: &str = "last_effect.json";

pub fn load() -> HashMap<String, LastEffect> {
    crate::json_store::load(FILE)
}

pub fn save(effects: &HashMap<String, LastEffect>) {
    crate::json_store::save(FILE, effects);
}
