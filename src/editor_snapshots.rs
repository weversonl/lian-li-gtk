//! Disk persistence for `RgbEditorSnapshot` (last-used RGB Editor controls per device).

use crate::context::RgbEditorSnapshot;
use std::collections::HashMap;

const FILE: &str = "editor_snapshots.json";

pub fn load() -> HashMap<String, RgbEditorSnapshot> {
    crate::json_store::load(FILE)
}

pub fn save(snapshots: &HashMap<String, RgbEditorSnapshot>) {
    crate::json_store::save(FILE, snapshots);
}
