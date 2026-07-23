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

/// A snapshot of every control on the RGB Editor page for one device —
/// everything needed to redraw the page exactly as the user left it.
/// Persisted to disk (see `crate::editor_snapshots`) — without this,
/// reopening the editor for a device (even across app restarts) always
/// reset to Static/100% brightness/first zone, discarding whatever the user
/// had actually picked, since nothing remembered it between visits.
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

/// What was last successfully applied to a wireless device — enough to
/// reapply it verbatim after "Identify" or a rebind, without needing to
/// know *why* it was applied (RGB Editor vs. Global Effects) or regenerate
/// it from scratch. `Static` keeps one `(zone, effect)` pair per zone that
/// was actually set, since Global Effects applies the same effect to every
/// zone on a device while the RGB Editor targets just one.
#[derive(Clone)]
pub enum LastEffect {
    /// Frame buffer + interval, plus the `RgbMode` they were rendered from
    /// (Rainbow/Rainbow Morph/Breathing) — the frames alone don't say which
    /// mode produced them, but the device detail page's "Current Effect"
    /// stat needs a name to show.
    Frames(Vec<Vec<[u8; 3]>>, u16, RgbMode),
    Static(Vec<(u8, RgbEffect)>),
}

impl LastEffect {
    /// The mode this effect displays as, for the device detail page's
    /// "Current Effect" stat.
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
    /// Local-only cosmetic nicknames, keyed by device_id. Never touches the
    /// daemon/hardware — see `crate::device_names`.
    pub custom_names: Rc<RefCell<HashMap<String, String>>>,
    /// Local-only per-device Rainbow direction/strip-count, keyed by
    /// device_id — see `crate::device_rgb_prefs`.
    pub rgb_prefs: Rc<RefCell<HashMap<String, DeviceRgbPrefs>>>,
    /// Local-only device_id ordering for Global Effects' "Included Devices"
    /// list — see `crate::device_order`.
    pub device_order: Rc<RefCell<Vec<String>>>,
    /// Small page-level UI toggles — see `crate::app_prefs`.
    pub app_prefs: Rc<RefCell<AppPrefs>>,
    /// In-memory (not persisted — this is runtime state, not a preference)
    /// record of the last effect successfully applied to each wireless
    /// device, from *any* page (RGB Editor or Global Effects). Two things
    /// read this: "Identify" resumes it after blinking instead of freezing
    /// a static snapshot (`GetZoneColors`/`SetRgbDirect` can only capture
    /// one still frame, not an animation), and Bind reapplies it after a
    /// rebind — the device doesn't remember its own effect across an
    /// unbind/bind cycle, so without this it just comes back dark/idle.
    pub last_effect: Rc<RefCell<HashMap<String, LastEffect>>>,
    /// Disk-persisted cache of the RGB Editor's last-used controls per
    /// device — see `RgbEditorSnapshot`.
    pub editor_snapshots: Rc<RefCell<HashMap<String, RgbEditorSnapshot>>>,
}

#[allow(dead_code)]
pub type SharedCtx = Rc<Ctx>;

impl Ctx {
    pub fn toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }

    /// A toast with an action button (e.g. "Restart") instead of a plain
    /// message telling the user to go do something themselves.
    pub fn toast_with_button(&self, message: &str, button_label: &str, on_click: impl Fn() + 'static) {
        let toast = adw::Toast::new(message);
        toast.set_button_label(Some(button_label));
        toast.connect_button_clicked(move |_| on_click());
        self.toast_overlay.add_toast(toast);
    }

    pub fn push(&self, page: &adw::NavigationPage) {
        self.nav.push(page);
    }

    /// Pushes `build()`'s page, unless a page tagged `tag` is already
    /// somewhere in the nav stack — then it just jumps back to that existing
    /// instance instead. Without this, repeatedly opening a global page like
    /// Wireless Pairing or Global Effects (both reachable from a sidebar
    /// icon at any time, not tied to a specific spot in the flow) stacked up
    /// one nav entry per click, so the back button had to be pressed once
    /// per click to actually get anywhere.
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

    /// `None` means nothing has been explicitly saved for this device yet —
    /// distinct from `rgb_prefs_for`'s default, which callers that don't
    /// have a separate "global default" fallback (like Global Effects does)
    /// can't tell apart from an explicit save of the default values.
    pub fn rgb_prefs_for_opt(&self, device_id: &str) -> Option<DeviceRgbPrefs> {
        self.rgb_prefs.borrow().get(device_id).copied()
    }

    pub fn set_rgb_prefs(&self, device_id: &str, direction: RgbDirection, strip_count: usize) {
        let mut prefs = self.rgb_prefs.borrow_mut();
        prefs.insert(device_id.to_string(), DeviceRgbPrefs { direction, strip_count });
        crate::device_rgb_prefs::save(&prefs);
    }

    /// Sorts `devices` by the saved order (devices never seen before keep
    /// their relative daemon-reported order and land at the end).
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

    /// Global Effects' Custom Gradient Wave stops — page-level, not
    /// per-device (that page applies to every device at once, so there's no
    /// single device to key an `RgbEditorSnapshot` off of). Without this it
    /// reset to the same 8 white swatches every time the page was reopened,
    /// forcing the user to repick all 8 colors every session.
    pub fn gradient_colors(&self) -> [[u8; 3]; 8] {
        self.app_prefs.borrow().gradient_colors
    }

    pub fn set_gradient_colors(&self, colors: [[u8; 3]; 8]) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.gradient_colors = colors;
        crate::app_prefs::save(&prefs);
    }

    /// Last effect mode picked on the Global Effects page — see
    /// `AppPrefs::global_effect_mode`'s doc comment.
    pub fn global_effect_mode(&self) -> RgbMode {
        self.app_prefs.borrow().global_effect_mode
    }

    pub fn set_global_effect_mode(&self, mode: RgbMode) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.global_effect_mode = mode;
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

    /// Takes effect on the next launch, not immediately — see `Lang`'s doc
    /// comment. Callers are expected to toast a "restart to apply" message
    /// themselves (Preferences does), since this alone doesn't rebuild any
    /// already-open page.
    pub fn set_lang(&self, lang: Lang) {
        let mut prefs = self.app_prefs.borrow_mut();
        prefs.lang = lang;
        crate::app_prefs::save(&prefs);
    }

    /// Looks up `key` in the current language — see `crate::i18n`.
    pub fn t(&self, key: &str) -> &'static str {
        crate::i18n::t(self.lang(), key)
    }

    pub fn record_frames(&self, device_id: &str, frames: Vec<Vec<[u8; 3]>>, interval_ms: u16, mode: RgbMode) {
        self.last_effect.borrow_mut().insert(device_id.to_string(), LastEffect::Frames(frames, interval_ms, mode));
    }

    /// Replaces (not merges) the recorded static effect for a device — call
    /// once with every `(zone, effect)` pair that was actually sent for
    /// this device in one "apply" action, not per-zone.
    pub fn record_static_effect(&self, device_id: &str, zone_effects: Vec<(u8, RgbEffect)>) {
        self.last_effect.borrow_mut().insert(device_id.to_string(), LastEffect::Static(zone_effects));
    }

    pub fn last_effect_for(&self, device_id: &str) -> Option<LastEffect> {
        self.last_effect.borrow().get(device_id).cloned()
    }

    /// Same-MAC device_id changes prefix across a bind/unbind cycle
    /// (`wireless:<mac>` <-> `wireless-unbound:<mac>`) — call this after a
    /// successful Bind so the freshly-rebound `wireless:<mac>` id picks up
    /// whatever was recorded under the old `wireless-unbound:<mac>` one (or
    /// vice versa after Unbind, for symmetry, even though there's nothing
    /// left to send an unbound device).
    pub fn editor_snapshot_for(&self, device_id: &str) -> Option<RgbEditorSnapshot> {
        self.editor_snapshots.borrow().get(device_id).cloned()
    }

    pub fn save_editor_snapshot(&self, device_id: &str, snapshot: RgbEditorSnapshot) {
        let mut snapshots = self.editor_snapshots.borrow_mut();
        snapshots.insert(device_id.to_string(), snapshot);
        crate::editor_snapshots::save(&snapshots);
    }

    pub fn migrate_last_effect(&self, old_device_id: &str, new_device_id: &str) {
        if old_device_id == new_device_id {
            return;
        }
        let mut map = self.last_effect.borrow_mut();
        if let Some(effect) = map.remove(old_device_id) {
            map.insert(new_device_id.to_string(), effect);
        }
    }
}
