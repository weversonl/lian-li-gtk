//! Shared client-side state, kept in sync with the daemon via polling.
//!
//! GTK runs single-threaded on the main loop, so state is a plain
//! `Rc<RefCell<_>>` updated from `glib::spawn_future_local` tasks — no
//! cross-thread channel needed.

use crate::ipc_client::IpcClient;
use gtk::glib;
use lianli_shared::ipc::{DeviceInfo, IpcRequest, TelemetrySnapshot};
use lianli_shared::rgb::RgbPreset;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

#[derive(Default)]
pub struct AppState {
    pub devices: Vec<DeviceInfo>,
    pub telemetry: TelemetrySnapshot,
    pub presets: Vec<RgbPreset>,
}

pub type SharedState = Rc<RefCell<AppState>>;

pub fn new_shared_state() -> SharedState {
    Rc::new(RefCell::new(AppState::default()))
}

/// Polls device list + telemetry every second; `on_update` fires after each
/// successful poll. `on_reconnect` fires once per device_id that reappears
/// in `ListDevices` after having been missing from a previous poll (e.g. a
/// wireless device's RF link dropped and came back) — including on the
/// very first poll at startup, since a device present there could equally
/// be one that just regained power after a full PC reboot (its RGB reset
/// to nothing) or one that never lost power at all (already showing the
/// right thing, so reapplying is a harmless no-op). There's no way to tell
/// those two cases apart from here, and only the first one needs the
/// reapply, so it always fires. Call once at startup.
pub fn start_polling(
    client: IpcClient,
    state: SharedState,
    on_update: impl Fn() + 'static,
    on_reconnect: impl Fn(String) + 'static,
) {
    glib::spawn_future_local(async move {
        if let Ok(presets) = client.call::<Vec<RgbPreset>>(IpcRequest::ListRgbPresets).await {
            state.borrow_mut().presets = presets;
        }

        let mut known_device_ids: HashSet<String> = HashSet::new();

        loop {
            let devices = client.call::<Vec<DeviceInfo>>(IpcRequest::ListDevices).await;
            let telemetry = client.call::<TelemetrySnapshot>(IpcRequest::GetTelemetry).await;

            let mut changed = false;
            if let Ok(devices) = devices {
                let current_ids: HashSet<String> = devices.iter().map(|d| d.device_id.clone()).collect();
                for id in current_ids.difference(&known_device_ids) {
                    on_reconnect(id.clone());
                }
                known_device_ids = current_ids;
                state.borrow_mut().devices = devices;
                changed = true;
            }
            if let Ok(telemetry) = telemetry {
                state.borrow_mut().telemetry = telemetry;
                changed = true;
            }

            if changed {
                on_update();
            }

            glib::timeout_future(Duration::from_secs(1)).await;
        }
    });
}

#[allow(dead_code)]
pub async fn refresh_presets(client: &IpcClient, state: &SharedState) -> anyhow::Result<()> {
    let presets = client.call::<Vec<RgbPreset>>(IpcRequest::ListRgbPresets).await?;
    state.borrow_mut().presets = presets;
    Ok(())
}
