//! Shared handles every page needs: the IPC client, shared state, a place to
//! surface toasts, and the navigation stack to push new pages onto.

use crate::app_prefs::{AppPrefs, Lang};
use crate::app_state::SharedState;
use crate::device_rgb_prefs::DeviceRgbPrefs;
use crate::ipc_client::IpcClient;
use crate::pages::rgb_editor::MiddleKind;
use lianli_shared::ipc::DeviceInfo;
use lianli_shared::rgb::{RgbDirection, RgbEffect, RgbMode, RgbScope};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Global Effects page controls beyond mode/gradient — see
/// `Ctx::global_effect_controls`/`set_global_effect_controls`.
pub struct GlobalEffectControls {
    pub color: [u8; 3],
    pub meteor_tail: [u8; 3],
    pub speed_percent: f64,
    pub fps: f64,
    pub brightness_percent: f64,
    pub meteor_pause_secs: f64,
    pub sync_devices: bool,
}

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
    #[serde(default)]
    pub segments_enabled: bool,
    #[serde(default = "default_true")]
    pub edge_by_strip: bool,
    #[serde(default = "default_one")]
    pub edge_count: u32,
    #[serde(default = "default_white")]
    pub edge_color: [u8; 3],
    #[serde(default = "default_true")]
    pub edge_start: bool,
    #[serde(default = "default_true")]
    pub edge_end: bool,
    #[serde(default = "default_hundred")]
    pub edge_brightness_percent: f64,
    #[serde(default = "default_solid_kind")]
    pub edge_kind: MiddleKind,
    #[serde(default = "default_rainbow_mode")]
    pub edge_mode: RgbMode,
    #[serde(default)]
    pub edge_colors: [[u8; 3]; 8],
    #[serde(default)]
    pub middle_kind: MiddleKind,
    #[serde(default = "default_white")]
    pub middle_color: [u8; 3],
    #[serde(default = "default_rainbow_mode")]
    pub middle_mode: RgbMode,
    #[serde(default)]
    pub middle_colors: [[u8; 3]; 8],
    #[serde(default = "default_hundred")]
    pub middle_brightness_percent: f64,
    #[serde(default = "default_fps")]
    pub fps: f64,
    #[serde(default = "default_meteor_pause_secs")]
    pub meteor_pause_secs: f64,
    /// Whether this Segments effect was pushed to every zone via "Aplicar a
    /// Todos" — only one snapshot is kept per device, so this tells
    /// `apply_segments_from_snapshot` to restore every zone, not just `zone`.
    #[serde(default)]
    pub segments_applied_to_all_zones: bool,
}

fn default_true() -> bool {
    true
}

fn default_one() -> u32 {
    1
}

fn default_white() -> [u8; 3] {
    [255, 255, 255]
}

fn default_hundred() -> f64 {
    100.0
}

fn default_rainbow_mode() -> RgbMode {
    RgbMode::Rainbow
}

fn default_solid_kind() -> MiddleKind {
    MiddleKind::Solid
}

fn default_fps() -> f64 {
    30.0
}

fn default_meteor_pause_secs() -> f64 {
    5.0
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
    /// Generation counter per `"device_id:zone"`, bumped on every RGB Editor
    /// Apply. Lets a client-driven per-zone animation loop (repeated
    /// `SetRgbDirect` calls — see `pages::rgb_editor::apply_segments`) know
    /// to stop once its zone has been re-applied from under it.
    pub segment_anim_gen: Rc<RefCell<HashMap<String, u64>>>,
}

#[allow(dead_code)]
pub type SharedCtx = Rc<Ctx>;

const TOAST_TIMEOUT_SECS: u32 = 5;

impl Ctx {
    pub fn toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        toast.set_timeout(TOAST_TIMEOUT_SECS);
        self.toast_overlay.add_toast(toast);
    }

    pub fn toast_with_button(&self, message: &str, button_label: &str, on_click: impl Fn() + 'static) {
        let toast = adw::Toast::new(message);
        toast.set_timeout(TOAST_TIMEOUT_SECS);
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

    pub fn set_rgb_prefs(
        &self,
        device_id: &str,
        direction: RgbDirection,
        strip_count: usize,
        invert_direction: bool,
        meteor_circular: bool,
    ) {
        let mut prefs = self.rgb_prefs.borrow_mut();
        // Preserves `ring_offset_deg` — a calibration value set separately
        // (see `set_ring_offset_deg`), not one of this function's callers'
        // concern, so overwriting the whole struct here would silently
        // reset it back to 0 every time direction/strip_count/etc change.
        let ring_offset_deg = prefs.get(device_id).map(|p| p.ring_offset_deg).unwrap_or(0.0);
        prefs.insert(
            device_id.to_string(),
            DeviceRgbPrefs { direction, strip_count, invert_direction, meteor_circular, ring_offset_deg },
        );
        crate::device_rgb_prefs::save(&prefs);
    }

    /// Physical mount rotation calibration for a device's fan ring(s) — see
    /// `DeviceRgbPrefs::ring_offset_deg`. Kept separate from `set_rgb_prefs`
    /// since it's an advanced, rarely-touched setting.
    pub fn set_ring_offset_deg(&self, device_id: &str, ring_offset_deg: f64) {
        let mut prefs = self.rgb_prefs.borrow_mut();
        let mut entry = prefs.get(device_id).copied().unwrap_or_default();
        entry.ring_offset_deg = ring_offset_deg;
        prefs.insert(device_id.to_string(), entry);
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

    /// Everything else on the Global Effects page besides mode/gradient
    /// (which already have their own getters/setters above) — saved
    /// together on Apply so re-opening the page picks up where the user
    /// left off instead of resetting to hardcoded defaults.
    pub fn global_effect_controls(&self) -> GlobalEffectControls {
        let prefs = self.app_prefs.borrow();
        GlobalEffectControls {
            color: prefs.global_effect_color,
            meteor_tail: prefs.global_effect_meteor_tail,
            speed_percent: prefs.global_effect_speed_percent,
            fps: prefs.global_effect_fps,
            brightness_percent: prefs.global_effect_brightness_percent,
            meteor_pause_secs: prefs.global_effect_meteor_pause_secs,
            sync_devices: prefs.global_effect_sync_devices,
        }
    }

    pub fn set_global_effect_controls(&self, controls: GlobalEffectControls) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.global_effect_color = controls.color;
        prefs.global_effect_meteor_tail = controls.meteor_tail;
        prefs.global_effect_speed_percent = controls.speed_percent;
        prefs.global_effect_fps = controls.fps;
        prefs.global_effect_brightness_percent = controls.brightness_percent;
        prefs.global_effect_meteor_pause_secs = controls.meteor_pause_secs;
        prefs.global_effect_sync_devices = controls.sync_devices;
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

    pub fn set_window_size(&self, width: i32, height: i32) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.window_width = width;
        prefs.window_height = height;
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

    /// Merges into the recorded static effect for a device, keyed by
    /// `(zone, scope)` — a dual-ring fan (e.g. SL Infinity) applies Inner
    /// and Outer as two separate calls to this fn for the *same* zone, and
    /// each needs its own entry so `identify::reapply_last_effect` resends
    /// both instead of the later call clobbering the earlier one. A new
    /// `RgbScope::All` entry for a zone supersedes (and clears) any
    /// Inner/Outer entries for that zone, and vice versa, since `All`
    /// overwrites the whole zone on the device.
    pub fn record_static_effect(&self, device_id: &str, zone_effects: Vec<(u8, RgbEffect)>) {
        let mut map = self.last_effect.borrow_mut();
        let mut merged: Vec<(u8, RgbEffect)> = match map.get(device_id) {
            Some(LastEffect::Static(existing)) => existing.clone(),
            _ => Vec::new(),
        };
        for (zone, effect) in zone_effects {
            if effect.scope == RgbScope::All {
                merged.retain(|(z, _)| *z != zone);
            } else {
                merged.retain(|(z, e)| !(*z == zone && (e.scope == effect.scope || e.scope == RgbScope::All)));
            }
            merged.push((zone, effect));
        }
        map.insert(device_id.to_string(), LastEffect::Static(merged));
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

    /// Invalidates any running per-zone animation loop for `key`
    /// (`"device_id:zone"`) and returns the new generation to start a fresh
    /// one under, if needed.
    pub fn bump_segment_anim(&self, key: &str) -> u64 {
        let mut map = self.segment_anim_gen.borrow_mut();
        let gen = map.entry(key.to_string()).or_insert(0);
        *gen += 1;
        *gen
    }

    pub fn segment_anim_current(&self, key: &str) -> u64 {
        self.segment_anim_gen.borrow().get(key).copied().unwrap_or(0)
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

    /// Bulk overwrite used when applying a Profile — see `crate::profiles`.
    pub fn replace_rgb_prefs(&self, prefs: HashMap<String, DeviceRgbPrefs>) {
        crate::device_rgb_prefs::save(&prefs);
        *self.rgb_prefs.borrow_mut() = prefs;
    }

    pub fn replace_editor_snapshots(&self, snapshots: HashMap<String, RgbEditorSnapshot>) {
        crate::editor_snapshots::save(&snapshots);
        *self.editor_snapshots.borrow_mut() = snapshots;
    }

    pub fn replace_last_effect(&self, map: HashMap<String, LastEffect>) {
        crate::last_effect::save(&map);
        *self.last_effect.borrow_mut() = map;
    }

    /// Applies a Profile's hardware-configuration `AppPrefs` subset in one
    /// disk write — see `crate::profiles::ProfileAppPrefs`.
    pub fn apply_profile_app_prefs(&self, p: &crate::profiles::ProfileAppPrefs) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.global_direction_enabled = p.global_direction_enabled;
        prefs.gradient_colors = p.gradient_colors;
        prefs.global_effect_mode = p.global_effect_mode;
        prefs.global_effect_color = p.global_effect_color;
        prefs.global_effect_meteor_tail = p.global_effect_meteor_tail;
        prefs.global_effect_speed_percent = p.global_effect_speed_percent;
        prefs.global_effect_fps = p.global_effect_fps;
        prefs.global_effect_brightness_percent = p.global_effect_brightness_percent;
        prefs.global_effect_meteor_pause_secs = p.global_effect_meteor_pause_secs;
        prefs.global_effect_sync_devices = p.global_effect_sync_devices;
        crate::app_prefs::save(&prefs);
    }

    /// Reads the `ProfileAppPrefs` subset of the current `AppPrefs`, for
    /// capturing a new Profile.
    pub fn profile_app_prefs(&self) -> crate::profiles::ProfileAppPrefs {
        let prefs = self.app_prefs.borrow();
        crate::profiles::ProfileAppPrefs {
            global_direction_enabled: prefs.global_direction_enabled,
            gradient_colors: prefs.gradient_colors,
            global_effect_mode: prefs.global_effect_mode,
            global_effect_color: prefs.global_effect_color,
            global_effect_meteor_tail: prefs.global_effect_meteor_tail,
            global_effect_speed_percent: prefs.global_effect_speed_percent,
            global_effect_fps: prefs.global_effect_fps,
            global_effect_brightness_percent: prefs.global_effect_brightness_percent,
            global_effect_meteor_pause_secs: prefs.global_effect_meteor_pause_secs,
            global_effect_sync_devices: prefs.global_effect_sync_devices,
        }
    }

    pub fn profiles(&self) -> Vec<crate::profiles::Profile> {
        crate::profiles::load()
    }

    /// Replaces any existing profile with the same name.
    pub fn save_profile(&self, profile: crate::profiles::Profile) {
        let mut profiles = crate::profiles::load();
        profiles.retain(|p| p.name != profile.name);
        profiles.push(profile);
        crate::profiles::save(&profiles);
    }

    pub fn delete_profile(&self, name: &str) {
        let mut profiles = crate::profiles::load();
        profiles.retain(|p| p.name != name);
        crate::profiles::save(&profiles);
    }
}
