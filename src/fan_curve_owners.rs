//! Local-only bookkeeping: which fan hub "owns" each saved curve. The
//! daemon has no such concept — curves are just named entries in one
//! global `AppConfig.fan_curves` list, referenced by name from any hub's
//! `FanGroup`. This file exists purely so the GTK client can keep each
//! hub's curves visually and operationally separate, instead of letting
//! any hub pick any curve (which let editing one hub's curve silently
//! affect another hub sharing it by name).

use std::collections::HashMap;

const FILE: &str = "fan_curve_owners.json";

pub fn load() -> HashMap<String, String> {
    crate::json_store::load(FILE)
}

pub fn save(owners: &HashMap<String, String>) {
    crate::json_store::save(FILE, owners);
}
