//! Persists effects applied via the RGB Editor / Global Effects into
//! `AppConfig.rgb` — the daemon's own wireless-drift auto-resync
//! (`rgb_controller::apply_rgb_config`, triggered by
//! `DaemonEvent::ResyncWirelessRgb` when a device's firmware idle-watchdog
//! resets its lighting) replays *that* config, not whatever this app most
//! recently pushed via a transient `SetRgbEffect`/`SetRgbFrames` call. Since
//! neither RGB Editor nor Global Effects used to write through to
//! `AppConfig`, a drifted device got resynced back to whatever was last
//! saved there — which could be old enough to predate this app entirely
//! (e.g. still whatever Windows/L-Connect wrote, if this app never once
//! called `SetConfig` for that device before). Writing through here means
//! the daemon's own auto-heal replays the *current* effect instead.
//!
//! Note this only round-trips a single `RgbEffect` per zone — there's no
//! frame-buffer field in this schema, so a wireless animation degrades to a
//! static solid color on resync regardless (a pre-existing daemon
//! limitation, documented at `rgb_controller/mod.rs`'s `set_rgb_frames`).
//! Persisting the animation's mode/color here still makes that fallback the
//! *current* pick instead of a stale unrelated one, which is the actual bug
//! being fixed — it doesn't make resync preserve the animation itself.

use crate::context::Ctx;
use lianli_shared::config::AppConfig;
use lianli_shared::ipc::IpcRequest;
use lianli_shared::rgb::{RgbDeviceConfig, RgbEffect, RgbMode, RgbZoneConfig};
use std::rc::Rc;

fn upsert_zones(config: &mut AppConfig, device_id: &str, zone_effects: Vec<(u8, RgbEffect)>) {
    // `rgb` is `null` in the daemon's config file until something writes it
    // for the first time (confirmed against the real config at
    // `~/.config/lianli/config.json` on this machine — every field but
    // `rgb` was already populated from the Fan Curve/OpenRGB work, `rgb`
    // itself was still `null`). `RgbAppConfig::default()` matches exactly
    // what the daemon itself would assume for a missing section
    // (`enabled: true`, `openrgb_server: false`, port `6743` — see
    // `lianli_shared::rgb::RgbAppConfig`'s own `Default` impl), so this
    // isn't guessing, it's the same default the daemon already has.
    let rgb = config.rgb.get_or_insert_with(Default::default);

    let dev_index = match rgb.devices.iter().position(|d| d.device_id == device_id) {
        Some(i) => i,
        None => {
            rgb.devices.push(RgbDeviceConfig {
                device_id: device_id.to_string(),
                mb_rgb_sync: false,
                active_preset: None,
                zones: Vec::new(),
            });
            rgb.devices.len() - 1
        }
    };
    let dev_cfg = &mut rgb.devices[dev_index];

    for (zone, effect) in zone_effects {
        match dev_cfg.zones.iter().position(|z| z.zone_index == zone) {
            Some(i) => dev_cfg.zones[i].effect = effect,
            None => dev_cfg.zones.push(RgbZoneConfig {
                zone_index: zone,
                effect,
                swap_lr: false,
                swap_tb: false,
            }),
        }
    }
}

/// Persists one device's zone effects. Best-effort and silent on failure —
/// this only affects what a *future* drift-resync would replay, it never
/// undoes the hardware command that was already sent successfully.
pub async fn persist_rgb_effect(ctx: &Rc<Ctx>, device_id: &str, zone_effects: Vec<(u8, RgbEffect)>) {
    persist_rgb_effects(ctx, vec![(device_id.to_string(), zone_effects)]).await;
}

/// Same, batched across multiple devices in one `GetConfig`/`SetConfig`
/// round-trip — Global Effects applies to every device at once, and doing
/// a separate round-trip per device would risk one save clobbering another
/// if they raced (plus it's just slower).
pub async fn persist_rgb_effects(ctx: &Rc<Ctx>, entries: Vec<(String, Vec<(u8, RgbEffect)>)>) {
    if entries.is_empty() {
        return;
    }
    let Ok(mut config) = ctx.client.call::<AppConfig>(IpcRequest::GetConfig).await else { return };
    for (device_id, zone_effects) in entries {
        upsert_zones(&mut config, &device_id, zone_effects);
    }
    let _ = ctx.client.call_unit(IpcRequest::SetConfig { config }).await;
}

/// One-time startup cleanup for a mistake this app used to make: earlier
/// builds persisted a representative static-color snapshot to `AppConfig`
/// for wireless *animated* effects too (Rainbow/Rainbow Morph/Breathing/
/// Custom Gradient Wave). Confirmed on real hardware that this actively
/// breaks those effects — `SetConfig` makes the daemon call
/// `apply_rgb_config()` synchronously, which for a wireless device pushes
/// that static color via a real RF command, immediately halting whatever
/// animation was just started via `SetRgbFrames` (and the 1s drift-resync
/// heartbeat repeats the same overwrite forever after). The client no
/// longer writes these entries going forward, but anyone who already hit
/// the bug has them sitting in their config file — this strips exactly
/// those pre-existing entries (leaving Static/Direct zones alone) so the
/// stale color stops getting silently replayed.
pub async fn clear_stale_wireless_animations(ctx: &Rc<Ctx>) {
    const ANIMATED: [RgbMode; 4] =
        [RgbMode::Rainbow, RgbMode::RainbowMorph, RgbMode::Breathing, RgbMode::ColorCycle];

    let Ok(mut config) = ctx.client.call::<AppConfig>(IpcRequest::GetConfig).await else { return };
    let Some(rgb) = config.rgb.as_mut() else { return };

    let mut changed = false;
    for dev in rgb.devices.iter_mut() {
        if !dev.device_id.starts_with("wireless:") {
            continue;
        }
        let before = dev.zones.len();
        dev.zones.retain(|z| !ANIMATED.contains(&z.effect.mode));
        if dev.zones.len() != before {
            changed = true;
        }
    }

    if changed {
        let _ = ctx.client.call_unit(IpcRequest::SetConfig { config }).await;
    }
}
