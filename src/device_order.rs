//! Persisted device_id ordering, used by the sidebar and Global Effects.

const FILE: &str = "device_order.json";

pub fn load() -> Vec<String> {
    crate::json_store::load(FILE)
}

pub fn save(order: &[String]) {
    crate::json_store::save(FILE, order);
}
