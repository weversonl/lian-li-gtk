//! "Identify" — the L-Connect feature where selecting a device and hitting
//! a button blinks it 3x in yellow so you can tell physically identical
//! fans/cables apart. Only implemented for wireless devices: they're the
//! only ones this protocol lets a client push raw per-LED frames to
//! (`SetRgbFrames`).
//!
//! Restoring afterward, in priority order:
//! 1. If `Ctx::last_effect` has a record for this device (something this
//!    app applied this session, via RGB Editor or Global Effects), reapply
//!    *that* (see `reapply_last_effect`, also reused by `wireless_pairing`
//!    after a rebind — a device doesn't remember its own effect across an
//!    unbind/bind cycle, so without this it just comes back dark/idle).
//! 2. Otherwise (nothing recorded this session — e.g. right after opening
//!    the app, before touching this device), fall back to capturing the
//!    current per-zone colors before blinking and pushing them back with
//!    `SetRgbDirect`.
//!
//! Every per-zone/per-request send here is spaced out by
//! `IDENTIFY_SEND_DELAY_MS` — sending several `SetRgbDirect`/`GetZoneColors`
//! calls back-to-back with no gap is exactly what caused "Apply to All
//! Devices" to silently drop whichever command landed last (see
//! `global_effects::ipc_send_delay`); the daemon acknowledges a command as
//! soon as it's queued on the RF dongle, not once the device confirms
//! receipt, so a same-device follow-up sent too soon can still collide.
//!
//! Wired devices have no equivalent "read current per-LED color" query in
//! this protocol, so there's nothing reliable to restore to — Identify is
//! wireless-only for now.

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

/// Resends whatever `Ctx::last_effect` has recorded for `device_id`, if
/// anything. Returns `true` only if a send actually went through — it used
/// to unconditionally return `true` for a recorded `Frames` effect even
/// when every single send attempt failed, which silently left the device
/// however the Identify blink itself ended (its last frame, typically
/// dark) instead of ever really restoring anything. That's the most likely
/// explanation for "Identify sometimes leaves the device off/stuck" after
/// reopening the app: right after being closed for a while, a wireless
/// device's RF link can be colder/less responsive than it is mid-session
/// (same idle-watchdog behavior documented elsewhere in this app), so the
/// first send failing was actually a fairly likely — not exceptional —
/// case, not just a rebind-only edge case.
///
/// Retries up to `MAX_ATTEMPTS` times per send, with a growing gap between
/// each, since a cold link may need more than one extra nudge, not just
/// one, before it reliably takes a command again.
const MAX_ATTEMPTS: u32 = 3;

pub async fn reapply_last_effect(ctx: &Rc<Ctx>, device_id: &str) -> bool {
    // Let the link settle a moment past the bind-state confirmation before
    // pushing anything — the confirmation itself is the earliest sign the
    // device is bound, not proof it's ready for a data send.
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

    // Captured unconditionally now, even when there's a recorded effect to
    // reapply instead — `reapply_last_effect` retries hard, but a wireless
    // link that's genuinely gone cold can still fail every attempt, and
    // this is the only fallback for that case. Skipping the capture
    // whenever a recorded effect existed used to mean a total reapply
    // failure left the device wherever the Identify blink itself ended
    // (its last frame, typically dark) with nothing left to fall back to.
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
