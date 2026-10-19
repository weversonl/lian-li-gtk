//! "Identify": blinks a device 3x in yellow, then restores its previous
//! effect (from `Ctx::last_effect`/the RGB Editor snapshot, or — wireless
//! only — a captured color snapshot if nothing was recorded).
//!
//! Wireless devices blink via a real per-LED `SetRgbFrames` animation.
//! Wired devices have no per-LED read/write query in this protocol and no
//! raw frame buffer that's safe to guess the physical LED order of (see
//! `apply_segments`'s doc comment on ring vs. linear-strip layout), so the
//! blink there is "fake": alternating `SetRgbEffect` calls toggling
//! `disabled` on a yellow Static effect, through the same ring-aware
//! scope/zone addressing the RGB Editor uses.
//!
//! Sends are spaced out by `IDENTIFY_SEND_DELAY_MS`: the daemon acks a
//! command once it's queued on the RF dongle, not once the device confirms
//! receipt, so back-to-back sends to the same device can still collide.

use crate::context::{Ctx, LastEffect};
use crate::effects::{identify_frames, percent_to_brightness4, percent_to_speed4, IDENTIFY_STEP_MS};
use gtk::glib;
use lianli_shared::ipc::IpcRequest;
use lianli_shared::rgb::{RgbDeviceCapabilities, RgbDirection, RgbEffect, RgbMode, RgbScope};
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
    reapply_last_effect_inner(ctx, device_id, true).await
}

async fn reapply_last_effect_inner(ctx: &Rc<Ctx>, device_id: &str, notify_on_failure: bool) -> bool {
    // Let the link settle before pushing anything.
    glib::timeout_future(Duration::from_millis(400)).await;

    // A Segments preset (RGB Editor's "Dividir em Duas Zonas") never goes
    // through `Ctx::record_frames`/`record_static_effect` below — it's
    // scoped per zone via `SetRgbDirect`, not a whole-device send, and an
    // animated one only exists as a running client-side loop with nothing
    // to "resend". Re-deriving it from the saved editor snapshot and
    // restarting `apply_segments` is the only way to actually bring back
    // what the user last configured, instead of falling through to a
    // stale (or absent) `LastEffect` recording.
    if let Some(snapshot) = ctx.editor_snapshot_for(device_id) {
        if snapshot.segments_enabled {
            let is_wireless = device_id.starts_with("wireless:");
            crate::pages::rgb_editor::apply_segments_from_snapshot(ctx, device_id, is_wireless, &snapshot).await;
            return true;
        }
    }

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
            if !ok && notify_on_failure {
                ctx.toast(ctx.t("identify.failed_reapply"));
            }
            ok
        }
        Some(LastEffect::Static(zone_effects)) => {
            // The daemon names ENE6K77 wired ports differently across
            // endpoints (`hid:<serial>:portN` from ListDevices, which is
            // what `device_id` is here, vs `hid:<serial>:groupN` from
            // GetRgbCapabilities) — see `rgb_caps_id_matches` in
            // rgb_editor.rs. `SetRgbEffect` needs the capabilities one.
            let outbound_device_id = if device_id.starts_with("wireless:") {
                device_id.to_string()
            } else {
                match ctx.client.call::<Vec<RgbDeviceCapabilities>>(IpcRequest::GetRgbCapabilities).await {
                    Ok(caps) => caps
                        .into_iter()
                        .find(|c| crate::pages::rgb_editor::rgb_caps_id_matches(&c.device_id, device_id))
                        .map(|c| c.device_id)
                        .unwrap_or_else(|| device_id.to_string()),
                    Err(_) => device_id.to_string(),
                }
            };
            let mut all_ok = true;
            for (zone, effect) in zone_effects {
                let mut zone_ok = false;
                for attempt in 0..MAX_ATTEMPTS {
                    let request = IpcRequest::SetRgbEffect {
                        device_id: outbound_device_id.clone(),
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
            if !all_ok && notify_on_failure {
                ctx.toast(ctx.t("identify.failed_reapply"));
            }
            all_ok
        }
        None => false,
    }
}

/// Guards against the firmware silently resetting a device's lighting to
/// factory default without dropping from `ListDevices`. Devices are
/// re-sent sequentially, not concurrently — parallel sends to all 4
/// wireless devices saturated the shared RF dongle and caused real fan
/// stalls/RGB flicker.
///
/// Covers both `LastEffect::Frames` (per-zone animated Segments excluded —
/// it self-heals via its own `SetRgbDirect` loop and never records a
/// `Frames` entry; whole-device Segments applies via one `SetRgbFrames`
/// like any other animated effect, and still needs this) and
/// `LastEffect::Static` (plain color/Breathing applies, and the ring-aware
/// Segments apply for render_profile devices — see `rgb_editor.rs`'s
/// `record_static_effect` call there). Static needs its own reinforcement
/// for the same reason Frames does: `persist_rgb_effect` only writes to
/// the daemon's `AppConfig`, and that drives auto-resync for wired, not
/// wireless (see its doc comment) — SL-INF fan hubs have been observed
/// idle-blanking their own RGB firmware-side hours after the last apply,
/// with nothing in the daemon noticing or correcting it.
const HEARTBEAT_SECS: u64 = 20;

pub fn spawn_frame_heartbeat(ctx: &Rc<Ctx>) {
    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        loop {
            glib::timeout_future(Duration::from_secs(HEARTBEAT_SECS)).await;

            let device_ids: Vec<String> = ctx
                .state
                .borrow()
                .devices
                .iter()
                .map(|d| d.device_id.clone())
                .filter(|id| id.starts_with("wireless:"))
                .collect();

            for device_id in device_ids {
                match ctx.last_effect_for(&device_id) {
                    Some(LastEffect::Frames(frames, interval_ms, _)) => {
                        let request = IpcRequest::SetRgbFrames { device_id, frames, interval_ms };
                        let _ = ctx.client.call_unit(request).await;
                    }
                    Some(LastEffect::Static(zone_effects)) => {
                        for (zone, effect) in zone_effects {
                            let request = IpcRequest::SetRgbEffect { device_id: device_id.clone(), zone, effect };
                            let _ = ctx.client.call_unit(request).await;
                            send_delay().await;
                        }
                    }
                    None => {}
                }
            }
        }
    });
}

pub async fn identify(ctx: &Rc<Ctx>, device_id: &str) {
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
    // Wired ports/groups are named differently across endpoints — see
    // `rgb_caps_id_matches`'s doc comment.
    let Some(dev_caps) = caps.iter().find(|c| crate::pages::rgb_editor::rgb_caps_id_matches(&c.device_id, device_id))
    else {
        ctx.toast(ctx.t("identify.caps_not_found"));
        return;
    };

    if !device_id.starts_with("wireless:") {
        identify_wired(ctx, device_id, dev_caps).await;
        return;
    }

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

/// 25% yellow, matching `effects::identify_frames`' wireless blink.
const IDENTIFY_COLOR: [u8; 3] = [(255.0 * 0.25) as u8, (255.0 * 0.25) as u8, 0];

const IDENTIFY_OFF: [u8; 3] = [0, 0, 0];

/// "Fake" identify for wired devices: 3 on/off toggles of a Static effect
/// between yellow and literal black, through whatever zone/scope addressing
/// the device actually renders correctly (ring-aware `render_profile`
/// devices go through zone 0 with per-fan colors, like the RGB Editor's
/// Segments apply; plain devices get one `scope: All` Static per zone).
/// Toggling `RgbEffect.disabled` instead of swapping the color was tried
/// first — on real ENE6K77 hardware it doesn't force an immediate
/// re-render, so the zone just stayed lit yellow the whole sequence.
/// Finishes by restoring the last applied effect the same way
/// `reapply_last_effect` does for wireless — this toasts on its own if
/// nothing was recorded.
async fn identify_wired(ctx: &Rc<Ctx>, device_id: &str, dev_caps: &RgbDeviceCapabilities) {
    let brightness = percent_to_brightness4(25.0);
    let speed = percent_to_speed4(30.0);
    let zone_count = dev_caps.zones.len().max(1) as u8;

    // Tells any running client-driven Segments animation loop on these
    // zones to stop, same as a normal RGB Editor apply — otherwise it keeps
    // sending its own frames throughout the blink, fighting the toggle.
    if dev_caps.render_profile.is_some() {
        ctx.bump_segment_anim(&format!("{device_id}:0"));
    } else {
        for zone in 0..zone_count {
            ctx.bump_segment_anim(&format!("{device_id}:{zone}"));
        }
    }

    for lit in [true, false, true, false, true, false] {
        let color = if lit { IDENTIFY_COLOR } else { IDENTIFY_OFF };
        if let Some(profile) = dev_caps.render_profile.as_ref() {
            let per_fan = (profile.fan_count as usize).max(1);
            let effect = RgbEffect {
                mode: RgbMode::Static,
                colors: vec![color; per_fan],
                speed,
                brightness,
                direction: RgbDirection::default(),
                scope: RgbScope::All,
                disabled: false,
            };
            let request = IpcRequest::SetRgbEffect { device_id: dev_caps.device_id.clone(), zone: 0, effect };
            let _ = ctx.client.call_unit(request).await;
        } else {
            for zone in 0..zone_count {
                let effect = RgbEffect {
                    mode: RgbMode::Static,
                    colors: vec![color],
                    speed,
                    brightness,
                    direction: RgbDirection::default(),
                    scope: RgbScope::All,
                    disabled: false,
                };
                let request = IpcRequest::SetRgbEffect { device_id: dev_caps.device_id.clone(), zone, effect };
                let _ = ctx.client.call_unit(request).await;
                send_delay().await;
            }
        }
        glib::timeout_future(Duration::from_millis(IDENTIFY_STEP_MS as u64)).await;
    }

    reapply_last_effect(ctx, device_id).await;
}
