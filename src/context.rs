//! Shared handles every page needs: the IPC client, shared state, a place to
//! surface toasts, and the navigation stack to push new pages onto.

use crate::app_prefs::{AppPrefs, Lang};
use crate::app_state::SharedState;
use crate::device_rgb_prefs::DeviceRgbPrefs;
use crate::ipc_client::IpcClient;
use lianli_shared::ipc::DeviceInfo;
use lianli_shared::rgb::{RgbDirection, RgbEffect, RgbMode, RgbScope};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Every control on the RGB Editor page for one device. Persisted to disk
/// (`crate::editor_snapshots`) so reopening the editor restores it.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RgbEditorSnapshot {
    pub mode: RgbMode,
    pub direction: RgbDirection,
    pub scope: RgbScope,
    pub colors: [[u8; 3]; 8],
    pub speed_percent: f64,
    pub brightness_percent: f64,
    pub zone: u8,
    pub strip_count: usize,
}

/// Last effect successfully applied to a wireless device, for reapplying
/// after "Identify" or a rebind. `Static` holds one `(zone, effect)` pair
/// per zone actually set.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum LastEffect {
    /// Frames + interval, plus the `RgbMode` they were rendered from (the
    /// frames alone don't say which mode produced them).
    Frames(Vec<Vec<[u8; 3]>>, u16, RgbMode),
    Static(Vec<(u8, RgbEffect)>),
}

impl LastEffect {
    pub fn mode(&self) -> RgbMode {
        match self {
            LastEffect::Frames(_, _, mode) => *mode,
            LastEffect::Static(zones) => zones.first().map(|(_, e)| e.mode).unwrap_or(RgbMode::Static),
        }
    }
}

pub struct Ctx {
    pub client: IpcClient,
    pub state: SharedState,
    pub toast_overlay: adw::ToastOverlay,
    pub nav: adw::NavigationView,
    /// Cosmetic nicknames, keyed by device_id — see `crate::device_names`.
    pub custom_names: Rc<RefCell<HashMap<String, String>>>,
    /// Per-device Rainbow direction/strip-count — see `crate::device_rgb_prefs`.
    pub rgb_prefs: Rc<RefCell<HashMap<String, DeviceRgbPrefs>>>,
    /// Device order for the sidebar and Global Effects — see `crate::device_order`.
    pub device_order: Rc<RefCell<Vec<String>>>,
    pub app_prefs: Rc<RefCell<AppPrefs>>,
    /// Last effect applied per wireless device — see `crate::last_effect`.
    pub last_effect: Rc<RefCell<HashMap<String, LastEffect>>>,
    /// RGB Editor's last-used controls per device — see `RgbEditorSnapshot`.
    pub editor_snapshots: Rc<RefCell<HashMap<String, RgbEditorSnapshot>>>,
}

#[allow(dead_code)]
pub type SharedCtx = Rc<Ctx>;

impl Ctx {
    pub fn toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }

    pub fn toast_with_button(&self, message: &str, button_label: &str, on_click: impl Fn() + 'static) {
        let toast = adw::Toast::new(message);
        toast.set_button_label(Some(button_label));
        toast.connect_button_clicked(move |_| on_click());
        self.toast_overlay.add_toast(toast);
    }

    pub fn push(&self, page: &adw::NavigationPage) {
        self.nav.push(page);
    }

    /// Pushes `build()`'s page, unless one tagged `tag` is already in the
    /// nav stack — then jumps back to it instead of stacking a duplicate.
    pub fn push_singleton(&self, tag: &str, build: impl FnOnce() -> adw::NavigationPage) {
        if self.nav.find_page(tag).is_some() {
            self.nav.pop_to_tag(tag);
        } else {
            self.nav.push(&build());
        }
    }

    /// The nickname if the user set one, otherwise the daemon-reported name.
    pub fn display_name(&self, device: &DeviceInfo) -> String {
        self.custom_names
            .borrow()
            .get(&device.device_id)
            .cloned()
            .unwrap_or_else(|| device.name.clone())
    }

    pub fn set_custom_name(&self, device_id: &str, name: Option<String>) {
        let mut names = self.custom_names.borrow_mut();
        match name {
            Some(n) if !n.trim().is_empty() => {
                names.insert(device_id.to_string(), n.trim().to_string());
            }
            _ => {
                names.remove(device_id);
            }
        }
        crate::device_names::save(&names);
    }

    pub fn rgb_prefs_for(&self, device_id: &str) -> DeviceRgbPrefs {
        self.rgb_prefs.borrow().get(device_id).copied().unwrap_or_default()
    }

    /// `None` if nothing's been explicitly saved yet, unlike `rgb_prefs_for`.
    pub fn rgb_prefs_for_opt(&self, device_id: &str) -> Option<DeviceRgbPrefs> {
        self.rgb_prefs.borrow().get(device_id).copied()
    }

    pub fn set_rgb_prefs(&self, device_id: &str, direction: RgbDirection, strip_count: usize) {
        let mut prefs = self.rgb_prefs.borrow_mut();
        prefs.insert(device_id.to_string(), DeviceRgbPrefs { direction, strip_count });
        crate::device_rgb_prefs::save(&prefs);
    }

    /// Sorts by saved order; unseen devices land at the end.
    pub fn sort_by_saved_order(&self, mut devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
        let order = self.device_order.borrow();
        devices.sort_by_key(|d| order.iter().position(|id| id == &d.device_id).unwrap_or(usize::MAX));
        devices
    }

    pub fn set_device_order(&self, ids: Vec<String>) {
        *self.device_order.borrow_mut() = ids.clone();
        crate::device_order::save(&ids);
    }

    pub fn global_direction_enabled(&self) -> bool {
        self.app_prefs.borrow().global_direction_enabled
    }

    pub fn set_global_direction_enabled(&self, enabled: bool) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.global_direction_enabled = enabled;
        crate::app_prefs::save(&prefs);
    }

    /// Global Effects' Gradient Wave stops — page-level, not per-device.
    pub fn gradient_colors(&self) -> [[u8; 3]; 8] {
        self.app_prefs.borrow().gradient_colors
    }

    pub fn set_gradient_colors(&self, colors: [[u8; 3]; 8]) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.gradient_colors = colors;
        crate::app_prefs::save(&prefs);
    }

    pub fn global_effect_mode(&self) -> RgbMode {
        self.app_prefs.borrow().global_effect_mode
    }

    pub fn set_global_effect_mode(&self, mode: RgbMode) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.global_effect_mode = mode;
        crate::app_prefs::save(&prefs);
    }

    pub fn default_device_id(&self) -> Option<String> {
        self.app_prefs.borrow().default_device_id.clone()
    }

    pub fn set_default_device_id(&self, device_id: Option<String>) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.default_device_id = device_id;
        crate::app_prefs::save(&prefs);
    }

    pub fn fan_curve_graph_view(&self) -> bool {
        self.app_prefs.borrow().fan_curve_graph_view
    }

    pub fn set_fan_curve_graph_view(&self, graph: bool) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.fan_curve_graph_view = graph;
        crate::app_prefs::save(&prefs);
    }

    pub fn lang(&self) -> Lang {
        self.app_prefs.borrow().lang
    }

    /// Takes effect on next launch, not immediately — see `Lang`'s doc comment.
    pub fn set_lang(&self, lang: Lang) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.lang = lang;
        crate::app_prefs::save(&prefs);
    }

    pub fn t(&self, key: &str) -> &'static str {
        crate::i18n::t(self.lang(), key)
    }

    pub fn record_frames(&self, device_id: &str, frames: Vec<Vec<[u8; 3]>>, interval_ms: u16, mode: RgbMode) {
        let mut map = self.last_effect.borrow_mut();
        map.insert(device_id.to_string(), LastEffect::Frames(frames, interval_ms, mode));
        crate::last_effect::save(&map);
    }

    /// Replaces (not merges) the recorded static effect for a device.
    pub fn record_static_effect(&self, device_id: &str, zone_effects: Vec<(u8, RgbEffect)>) {
        let mut map = self.last_effect.borrow_mut();
        map.insert(device_id.to_string(), LastEffect::Static(zone_effects));
        crate::last_effect::save(&map);
    }

    pub fn last_effect_for(&self, device_id: &str) -> Option<LastEffect> {
        self.last_effect.borrow().get(device_id).cloned()
    }

    pub fn editor_snapshot_for(&self, device_id: &str) -> Option<RgbEditorSnapshot> {
        self.editor_snapshots.borrow().get(device_id).cloned()
    }

    pub fn save_editor_snapshot(&self, device_id: &str, snapshot: RgbEditorSnapshot) {
        let mut snapshots = self.editor_snapshots.borrow_mut();
        snapshots.insert(device_id.to_string(), snapshot);
        crate::editor_snapshots::save(&snapshots);
    }

    /// Carries a recorded effect across a device_id change from bind/unbind
    /// (`wireless:<mac>` <-> `wireless-unbound:<mac>`).
    pub fn migrate_last_effect(&self, old_device_id: &str, new_device_id: &str) {
        if old_device_id == new_device_id {
            return;
        }
        let mut map = self.last_effect.borrow_mut();
        if let Some(effect) = map.remove(old_device_id) {
            map.insert(new_device_id.to_string(), effect);
            crate::last_effect::save(&map);
        }
    }
}
