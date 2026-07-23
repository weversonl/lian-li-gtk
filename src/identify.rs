//! "Identify": blinks a wireless device 3x in yellow, then restores its
//! previous effect (from `Ctx::last_effect`, or a captured color snapshot
//! if nothing was recorded). Wireless-only — wired devices have no
//! per-LED read/write query in this protocol.
//!
//! Sends are spaced out by `IDENTIFY_SEND_DELAY_MS`: the daemon acks a
//! command once it's queued on the RF dongle, not once the device confirms
//! receipt, so back-to-back sends to the same device can still collide.

use crate::context::{Ctx, LastEffect};
use crate::effects::{identify_frames, IDENTIFY_STEP_MS};
use gtk::glib;
use lianli_shared::ipc::IpcRequest;
use lianli_shared::rgb::RgbDeviceCapabilities;
use std::rc::Rc;
use std::time::Duration;

const IDENTIFY_SEND_DELAY_MS: u64 = 100;

async fn send_delay() {
    glib::timeout_future(Duration::from_millis(IDENTIFY_SEND_DELAY_MS)).await;
}

/// Resends whatever `Ctx::last_effect` has for `device_id`. Returns `true`
/// only on an actual successful send — a cold RF link (e.g. right after
/// reopening the app) can need a few retries before it takes a command.
const MAX_ATTEMPTS: u32 = 3;

pub async fn reapply_last_effect(ctx: &Rc<Ctx>, device_id: &str) -> bool {
    // Let the link settle before pushing anything.
    glib::timeout_future(Duration::from_millis(400)).await;

    match ctx.last_effect_for(device_id) {
        Some(LastEffect::Frames(frames, interval_ms, _mode)) => {
            let mut ok = false;
            for attempt in 0..MAX_ATTEMPTS {
                let request = IpcRequest::SetRgbFrames {
                    device_id: device_id.to_string(),
                    frames: frames.clone(),
                    interval_ms,
                };
                if ctx.client.call_unit(request).await.is_ok() {
                    ok = true;
                    break;
                }
                glib::timeout_future(Duration::from_millis(IDENTIFY_SEND_DELAY_MS * (attempt as u64 + 1))).await;
            }
            if !ok {
                ctx.toast(ctx.t("identify.failed_reapply"));
            }
            ok
        }
        Some(LastEffect::Static(zone_effects)) => {
            let mut all_ok = true;
            for (zone, effect) in zone_effects {
                let mut zone_ok = false;
                for attempt in 0..MAX_ATTEMPTS {
                    let request = IpcRequest::SetRgbEffect {
                        device_id: device_id.to_string(),
                        zone,
                        effect: effect.clone(),
                    };
                    if ctx.client.call_unit(request).await.is_ok() {
                        zone_ok = true;
                        break;
                    }
                    glib::timeout_future(Duration::from_millis(IDENTIFY_SEND_DELAY_MS * (attempt as u64 + 1))).await;
                }
                all_ok &= zone_ok;
                send_delay().await;
            }
            if !all_ok {
                ctx.toast(ctx.t("identify.failed_reapply"));
            }
            all_ok
        }
        None => false,
    }
}

pub async fn identify(ctx: &Rc<Ctx>, device_id: &str) {
    if !device_id.starts_with("wireless:") {
        ctx.toast(ctx.t("identify.no_wired"));
        return;
    }

    let caps = match ctx
        .client
        .call::<Vec<RgbDeviceCapabilities>>(IpcRequest::GetRgbCapabilities)
        .await
    {
        Ok(caps) => caps,
        Err(e) => {
            ctx.toast(&format!("{}: {e}", ctx.t("identify.failed_load_caps")));
            return;
        }
    };
    let Some(dev_caps) = caps.iter().find(|c| c.device_id == device_id) else {
        ctx.toast(ctx.t("identify.caps_not_found"));
        return;
    };

    let zone_count = dev_caps.zones.len().max(1);
    let led_count: usize = dev_caps.zones.iter().map(|z| z.led_count as usize).sum();
    if led_count == 0 {
        ctx.toast(ctx.t("identify.zero_leds"));
        return;
    }

    // Captured even when a recorded effect exists, as a fallback if
    // `reapply_last_effect` fails all its retries.
    let mut captured: Vec<(u8, Vec<[u8; 3]>)> = Vec::new();
    for zone in 0..zone_count as u8 {
        let request = IpcRequest::GetZoneColors { device_id: device_id.to_string(), zone };
        if let Ok(colors) = ctx.client.call::<Vec<[u8; 3]>>(request).await {
            captured.push((zone, colors));
        }
        send_delay().await;
    }

    let frames = identify_frames(led_count);
    let frame_count = frames.len();
    let request = IpcRequest::SetRgbFrames {
        device_id: device_id.to_string(),
        frames,
        interval_ms: IDENTIFY_STEP_MS,
    };
    if let Err(e) = ctx.client.call_unit(request).await {
        ctx.toast(&format!("{}: {e}", ctx.t("identify.failed")));
        return;
    }

    // Let it actually blink for one full pass before restoring — otherwise
    // the restore command would land mid-blink and cut it short.
    glib::timeout_future(Duration::from_millis(frame_count as u64 * IDENTIFY_STEP_MS as u64)).await;

    if reapply_last_effect(ctx, device_id).await {
        return;
    }

    for (zone, colors) in captured {
        let request = IpcRequest::SetRgbDirect { device_id: device_id.to_string(), zone, colors };
        let _ = ctx.client.call_unit(request).await;
        send_delay().await;
    }
}
