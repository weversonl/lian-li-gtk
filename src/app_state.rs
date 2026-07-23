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
    /// Unused — reserved for detecting newly-connected devices.
    #[allow(dead_code)]
    known_device_ids: HashSet<String>,
}

pub type SharedState = Rc<RefCell<AppState>>;

#[allow(dead_code)]
pub fn diff_new_devices(state: &SharedState, previous: &HashSet<String>) -> Vec<String> {
    state
        .borrow()
        .devices
        .iter()
        .map(|d| d.device_id.clone())
        .filter(|id| !previous.contains(id))
        .collect()
}

pub fn new_shared_state() -> SharedState {
    Rc::new(RefCell::new(AppState::default()))
}

/// Polls device list + telemetry every second; `on_update` fires after each
/// successful poll. Call once at startup.
pub fn start_polling(
    client: IpcClient,
    state: SharedState,
    on_update: impl Fn() + 'static,
) {
    glib::spawn_future_local(async move {
        if let Ok(presets) = client.call::<Vec<RgbPreset>>(IpcRequest::ListRgbPresets).await {
            state.borrow_mut().presets = presets;
        }

        loop {
            let devices = client.call::<Vec<DeviceInfo>>(IpcRequest::ListDevices).await;
            let telemetry = client.call::<TelemetrySnapshot>(IpcRequest::GetTelemetry).await;

            let mut changed = false;
            if let Ok(devices) = devices {
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
