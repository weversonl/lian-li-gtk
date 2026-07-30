//! Persists effects into `AppConfig.rgb` so the daemon's own wireless-drift
//! auto-resync (triggered when a device's idle-watchdog resets its
//! lighting) replays the current effect instead of stale/pre-app state.
//!
//! Only a single `RgbEffect` round-trips per zone — no frame-buffer field
//! in this schema, so an animation still degrades to a static color on
//! resync. Persisting it here just makes that fallback color current.

use crate::context::Ctx;
use lianli_shared::config::AppConfig;
use lianli_shared::ipc::IpcRequest;
use lianli_shared::rgb::{RgbDeviceConfig, RgbEffect, RgbMode, RgbZoneConfig};
use std::rc::Rc;

fn upsert_zones(config: &mut AppConfig, device_id: &str, zone_effects: Vec<(u8, RgbEffect)>) {
    // `rgb` is `null` until something writes it; `RgbAppConfig::default()`
    // matches what the daemon itself assumes for a missing section.
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

pub async fn persist_rgb_effect(ctx: &Rc<Ctx>, device_id: &str, zone_effects: Vec<(u8, RgbEffect)>) {
    persist_rgb_effects(ctx, vec![(device_id.to_string(), zone_effects)]).await;
}

/// Batched across devices in one `GetConfig`/`SetConfig` round-trip.
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

/// Wipes persisted `AppConfig.rgb` zone effects for the given wireless
/// devices — call this before switching one to an animated mode. A stale
/// entry left over from an earlier Static apply otherwise sits there
/// forever, and the daemon's own idle-watchdog auto-resync (see this
/// module's top doc comment) keeps reapplying it via RF over the
/// client-driven animation whenever the device's firmware hiccups.
pub async fn clear_wireless_rgb_configs(ctx: &Rc<Ctx>, device_ids: &[String]) {
    if device_ids.is_empty() {
        return;
    }
    let Ok(mut config) = ctx.client.call::<AppConfig>(IpcRequest::GetConfig).await else { return };
    let Some(rgb) = config.rgb.as_mut() else { return };
    let mut changed = false;
    for dev in rgb.devices.iter_mut() {
        if device_ids.iter().any(|id| id == &dev.device_id) && !dev.zones.is_empty() {
            dev.zones.clear();
            changed = true;
        }
    }
    if changed {
        let _ = ctx.client.call_unit(IpcRequest::SetConfig { config }).await;
    }
}

/// Startup cleanup: strips any persisted animated-mode zone entries for
/// wireless devices. `SetConfig` makes the daemon push a static snapshot
/// via RF immediately, which halts a running animation — these entries
/// should never exist, but older builds wrote them.
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
