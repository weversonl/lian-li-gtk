//! RGB Effect editor: zone selector, effect mode, direction, scope, up to 4
//! colors, speed/brightness, and Apply.
//!
//! Wired devices use exactly what the daemon reports in
//! `GetRgbCapabilities.supported_modes` and go out via `SetRgbEffect` (the
//! firmware runs the animation).
//!
//! Wireless devices only ever report `["Static", "Direct"]` there, because
//! `RgbController::set_effect` (daemon side) just pushes a solid color to
//! them regardless of mode — there's no firmware-side rainbow to ask for.
//! So for wireless devices this editor adds a client-rendered set (Rainbow,
//! Rainbow Morph, Breathing, Custom Gradient Wave) that gets turned into
//! frames here and shipped via `SetRgbFrames`, which is the only way those
//! devices actually animate. Frames address the whole device's LED buffer,
//! not a single zone, so the zone selector is hidden once one of those
//! modes is picked.
//!
//! "Custom Gradient Wave" (OpenRGB's name for the same effect, e.g. its
//! "Unicorn Vomit" preset) isn't a real mode in the Lian Li protocol's
//! `RgbMode` enum — that enum is fixed, vendored from the daemon's own
//! crate, not something this app can add a variant to. It's tagged onto
//! `RgbMode::ColorCycle` instead (repurposed only when the device is
//! wireless — see `effect_mode_label`), since the actual value sent to the
//! daemon for a wireless animation is just raw frames; `SetRgbFrames` has no
//! mode field at all, `mode` only exists client-side for bookkeping (this
//! dropdown, and the "Current Effect" stat). A wired device that genuinely
//! supports a native "Color Cycle" firmware mode is unaffected — that path
//! never sets `is_wireless`, so it keeps its real name and goes out via
//! `SetRgbEffect` untouched.

use crate::context::Ctx;
use crate::direction::{direction_label, ALL_DIRECTIONS};
use crate::effects::{
    breathing_frames, custom_gradient_wave_frames, fps_to_interval_ms, frame_count_for, mode_uses_color,
    percent_to_brightness4, percent_to_cycle_ms, percent_to_speed4, rainbow_frames, rainbow_morph_frames,
    scale_color,
};
use crate::widgets::segmented_control;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::ipc::IpcRequest;
use lianli_shared::rgb::{RgbDeviceCapabilities, RgbDirection, RgbEffect, RgbMode, RgbScope};
use std::cell::RefCell;
use std::rc::Rc;

/// Client-rendered modes offered for wireless devices, beyond the two the
/// daemon's own capabilities report (Static, Direct).
const WIRELESS_ANIMATED_MODES: [RgbMode; 4] =
    [RgbMode::Rainbow, RgbMode::RainbowMorph, RgbMode::Breathing, RgbMode::ColorCycle];

/// Real name for every mode except our repurposed `ColorCycle` tag, which
/// only means "Gradient Wave" for a wireless device — see this module's
/// doc comment.
fn effect_mode_label(mode: RgbMode, is_wireless: bool) -> &'static str {
    if is_wireless && mode == RgbMode::ColorCycle {
        "Gradient Wave"
    } else {
        mode.display_name()
    }
}

/// How many of the 8 color swatches are actually meaningful for `mode`.
/// Every other color-using mode (Static, Breathing, Direct, ...) only ever
/// reads `colors[0]` — showing all 8 swatches for those wasn't just visual
/// clutter, it was misleading, since picking swatches 2-8 silently did
/// nothing. Only Gradient Wave (`ColorCycle`) actually uses more than one.
fn color_count_for_mode(mode: RgbMode) -> usize {
    if mode == RgbMode::ColorCycle {
        8
    } else {
        1
    }
}

/// Fixed slider width, consistent with Global Effects' sliders — see that
/// module's `SLIDER_WIDTH` doc comment for why `hexpand` alone isn't enough.
const SLIDER_WIDTH: i32 = 220;

fn scope_label(s: RgbScope, ctx: &Ctx) -> &'static str {
    match s {
        RgbScope::All => ctx.t("rgb_editor.scope_all"),
        RgbScope::Top => ctx.t("rgb_editor.scope_top"),
        RgbScope::Bottom => ctx.t("rgb_editor.scope_bottom"),
        RgbScope::Inner => ctx.t("rgb_editor.scope_inner"),
        RgbScope::Outer => ctx.t("rgb_editor.scope_outer"),
    }
}

struct EditorState {
    mode: RgbMode,
    direction: RgbDirection,
    scope: RgbScope,
    colors: [[u8; 3]; 8],
    speed_percent: f64,
    brightness_percent: f64,
    zone: u8,
    /// How many physical LED strips this device's flat LED buffer is made
    /// of — confirmed on real hardware that Strimer cables concatenate
    /// several parallel strips in the buffer rather than laying out LEDs
    /// in physical order, so without this each strip plays its own
    /// independent slice of the gradient (looks tiled/chopped up) instead
    /// of every strip moving in sync, which is what L-Connect's "merge"
    /// mode actually does. `1` = treat the buffer as one continuous strip
    /// (today's plain gradient).
    strip_count: usize,
}

pub fn page(ctx: &Rc<Ctx>, device_id: &str) -> adw::NavigationPage {
    let device_name = ctx
        .state
        .borrow()
        .devices
        .iter()
        .find(|d| d.device_id == device_id)
        .map(|d| ctx.display_name(d))
        .unwrap_or_else(|| device_id.to_string());

    let header = adw::HeaderBar::new();
    let apply_button = gtk::Button::builder()
        .label(ctx.t("rgb_editor.apply"))
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&apply_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let loading = adw::StatusPage::builder()
        .icon_name("content-loading-symbolic")
        .title(ctx.t("rgb_editor.loading"))
        .build();
    root.append(&loading);
    toolbar.set_content(Some(&root));

    let nav_page = adw::NavigationPage::builder()
        .title(format!("{} — {device_name}", ctx.t("rgb_editor.title_prefix")))
        .child(&toolbar)
        .build();

    let ctx = ctx.clone();
    let device_id = device_id.to_string();
    glib::spawn_future_local(async move {
        let caps = ctx
            .client
            .call::<Vec<RgbDeviceCapabilities>>(IpcRequest::GetRgbCapabilities)
            .await;

        let caps = match caps {
            Ok(all) => all.into_iter().find(|c| c.device_id == device_id),
            Err(e) => {
                ctx.toast(&format!("{}: {e}", ctx.t("rgb_editor.failed_load_caps")));
                None
            }
        };

        let Some(caps) = caps else {
            root.remove(&loading);
            let error_status = adw::StatusPage::builder()
                .icon_name("dialog-error-symbolic")
                .title(ctx.t("rgb_editor.no_caps_title"))
                .description(ctx.t("rgb_editor.no_caps_desc"))
                .build();
            root.append(&error_status);
            return;
        };

        root.remove(&loading);
        build_editor(&root, &ctx, &device_id, &caps, &apply_button);
    });

    nav_page
}

fn build_editor(
    root: &gtk::Box,
    ctx: &Rc<Ctx>,
    device_id: &str,
    caps: &RgbDeviceCapabilities,
    apply_button: &gtk::Button,
) {
    let is_wireless = device_id.starts_with("wireless:");

    // Wireless devices only report Static/Direct as real hardware modes —
    // the animated ones here are rendered client-side (see module docs).
    let mut selectable_modes = caps.supported_modes.clone();
    if is_wireless {
        selectable_modes.extend(WIRELESS_ANIMATED_MODES);
    }

    // Wave direction/strip count are remembered per device (see
    // device_rgb_prefs) — no reason to make the user re-enter "8 strips,
    // Down" every time they reopen this cable's editor. Everything else
    // (mode, colors, speed, brightness, zone, scope) comes from the last
    // snapshot saved on Apply, if this device has one — without it, leaving
    // and reopening the editor always reset to Static/100% brightness/zone 0
    // no matter what was last applied.
    let saved_prefs = ctx.rgb_prefs_for(device_id);
    let snapshot = ctx.editor_snapshot_for(device_id);
    let initial_mode = snapshot
        .as_ref()
        .map(|s| s.mode)
        .filter(|m| selectable_modes.contains(m))
        .unwrap_or_else(|| selectable_modes.first().copied().unwrap_or(RgbMode::Static));
    let state = Rc::new(RefCell::new(EditorState {
        mode: initial_mode,
        direction: snapshot.as_ref().map(|s| s.direction).unwrap_or(saved_prefs.direction),
        scope: snapshot.as_ref().map(|s| s.scope).unwrap_or(RgbScope::All),
        colors: snapshot.as_ref().map(|s| s.colors).unwrap_or_else(|| ctx.gradient_colors()),
        speed_percent: snapshot.as_ref().map(|s| s.speed_percent).unwrap_or(50.0),
        brightness_percent: snapshot.as_ref().map(|s| s.brightness_percent).unwrap_or(100.0),
        zone: snapshot.as_ref().map(|s| s.zone).unwrap_or(0),
        strip_count: snapshot.as_ref().map(|s| s.strip_count).unwrap_or(saved_prefs.strip_count),
    }));

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(700).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let zone_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .css_classes(["linked"])
        .build();
    if caps.zones.len() > 1 {
        let initial_zone = (state.borrow().zone as usize).min(caps.zones.len() - 1);
        let mut first_button: Option<gtk::ToggleButton> = None;
        for (i, zone) in caps.zones.iter().enumerate() {
            let btn = gtk::ToggleButton::builder().label(zone.name.clone()).build();
            if let Some(ref first) = first_button {
                btn.set_group(Some(first));
            } else {
                first_button = Some(btn.clone());
            }
            if i == initial_zone {
                btn.set_active(true);
            }
            let state = state.clone();
            btn.connect_toggled(move |b| {
                if b.is_active() {
                    state.borrow_mut().zone = i as u8;
                }
            });
            zone_box.append(&btn);
        }
        content.append(&zone_box);
    }

    let zone_note = gtk::Label::builder()
        .label(ctx.t("rgb_editor.zone_note"))
        .css_classes(["caption", "dim-label"])
        .halign(gtk::Align::Start)
        .visible(false)
        .wrap(true)
        .build();
    content.append(&zone_note);

    // Built here (ahead of the mode selector below, which needs to toggle
    // their visibility) but not appended to `content` until after
    // `mode_group` — `Box::append` only controls visual order, not
    // construction order, so this doesn't move them on screen.
    let colors_label = gtk::Label::builder()
        .label(ctx.t("rgb_editor.colors"))
        .css_classes(["caption-heading", "dim-label"])
        .halign(gtk::Align::Start)
        .visible(mode_uses_color(initial_mode))
        .build();
    // `FlowBox`, not a plain `Box` — a plain `Box` overflowed past the
    // card's edge on a narrow window instead of wrapping. `max_children_per_line`
    // has to be raised from `FlowBox`'s own default of 7 to fit all 8 —
    // otherwise it force-wraps the 8th swatch onto its own line even with
    // plenty of width to spare.
    let colors_box = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .row_spacing(8)
        .column_spacing(8)
        .halign(gtk::Align::Start)
        .max_children_per_line(8)
        .visible(mode_uses_color(initial_mode))
        .build();
    let initial_color_count = color_count_for_mode(initial_mode);
    let mut color_buttons: Vec<gtk::ColorDialogButton> = Vec::with_capacity(8);
    for i in 0..8usize {
        let dialog = gtk::ColorDialog::builder().with_alpha(false).build();
        let default = state.borrow().colors[i];
        let rgba = gtk::gdk::RGBA::new(
            default[0] as f32 / 255.0,
            default[1] as f32 / 255.0,
            default[2] as f32 / 255.0,
            1.0,
        );
        let button = gtk::ColorDialogButton::builder()
            .dialog(&dialog)
            .rgba(&rgba)
            .visible(i < initial_color_count)
            .build();
        let state = state.clone();
        button.connect_rgba_notify(move |b| {
            let rgba = b.rgba();
            state.borrow_mut().colors[i] = [
                (rgba.red() * 255.0) as u8,
                (rgba.green() * 255.0) as u8,
                (rgba.blue() * 255.0) as u8,
            ];
        });
        colors_box.append(&button);
        color_buttons.push(button);
    }

    // Effect mode — daemon-reported modes for wired devices, plus the
    // client-rendered animated set for wireless ones.
    let mode_group = adw::PreferencesGroup::new();
    let mode_names: Vec<&str> = selectable_modes.iter().map(|m| effect_mode_label(*m, is_wireless)).collect();
    let mode_model = gtk::StringList::new(&mode_names);
    let mode_row = adw::ComboRow::builder().title(ctx.t("rgb_editor.effect_mode")).model(&mode_model).build();
    if let Some(idx) = selectable_modes.iter().position(|m| *m == initial_mode) {
        mode_row.set_selected(idx as u32);
    }
    {
        let state = state.clone();
        let modes = selectable_modes.clone();
        let zone_box = zone_box.clone();
        let zone_note = zone_note.clone();
        let colors_label = colors_label.clone();
        let colors_box = colors_box.clone();
        let color_buttons = color_buttons.clone();
        let has_multi_zone = caps.zones.len() > 1;
        mode_row.connect_selected_notify(move |row| {
            if let Some(mode) = modes.get(row.selected() as usize) {
                state.borrow_mut().mode = *mode;
                let whole_device_only = is_wireless && WIRELESS_ANIMATED_MODES.contains(mode);
                if has_multi_zone {
                    zone_box.set_sensitive(!whole_device_only);
                }
                zone_note.set_visible(whole_device_only);
                let uses_color = mode_uses_color(*mode);
                colors_label.set_visible(uses_color);
                colors_box.set_visible(uses_color);
                let count = color_count_for_mode(*mode);
                for (i, button) in color_buttons.iter().enumerate() {
                    button.set_visible(i < count);
                }
            }
        });
    }
    // `set_selected` above only fires `connect_selected_notify` when the
    // index actually changes from the widget's own default (0) — so restore
    // the zone-picker sensitivity/note for the initial mode by hand instead
    // of assuming the signal already did it.
    let initial_whole_device_only = is_wireless && WIRELESS_ANIMATED_MODES.contains(&initial_mode);
    if caps.zones.len() > 1 {
        zone_box.set_sensitive(!initial_whole_device_only);
    }
    zone_note.set_visible(initial_whole_device_only);
    mode_group.add(&mode_row);

    let direction_names: Vec<&str> = ALL_DIRECTIONS.iter().map(|d| direction_label(*d, ctx.lang())).collect();
    let direction_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.direction")).title_lines(1).build();
    let direction_selected_index =
        ALL_DIRECTIONS.iter().position(|d| *d == state.borrow().direction).unwrap_or(0);
    {
        let state = state.clone();
        direction_row.add_suffix(&segmented_control::build(
            &direction_names,
            direction_selected_index,
            move |i| {
                if let Some(d) = ALL_DIRECTIONS.get(i) {
                    state.borrow_mut().direction = *d;
                }
            },
        ));
    }
    mode_group.add(&direction_row);

    let zone_scopes = caps.supported_scopes.first().cloned().unwrap_or_default();
    if !zone_scopes.is_empty() {
        let scope_names: Vec<&str> = zone_scopes.iter().map(|s| scope_label(*s, ctx)).collect();
        let scope_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.scope")).title_lines(1).build();
        let scopes = zone_scopes.clone();
        let initial_scope_index = zone_scopes.iter().position(|s| *s == state.borrow().scope).unwrap_or(0);
        let state = state.clone();
        scope_row.add_suffix(&segmented_control::build(&scope_names, initial_scope_index, move |i| {
            if let Some(s) = scopes.get(i) {
                state.borrow_mut().scope = *s;
            }
        }));
        mode_group.add(&scope_row);
    }
    content.append(&mode_group);

    content.append(&colors_label);
    content.append(&colors_box);

    // Speed / brightness as 0-100% sliders (matches how the rest of the app
    // expresses these), converted to the firmware's 0-4 scale only when a
    // wired SetRgbEffect actually needs it.
    let sliders_group = adw::PreferencesGroup::new();

    let speed_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.speed")).build();
    let speed_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    speed_scale.set_value(state.borrow().speed_percent);
    speed_scale.set_hexpand(false);
    speed_scale.set_size_request(SLIDER_WIDTH, -1);
    speed_scale.set_draw_value(true);
    speed_scale.set_value_pos(gtk::PositionType::Right);
    speed_scale.set_format_value_func(|_, value| format!("{value:.0}%"));
    {
        let state = state.clone();
        speed_scale.connect_value_changed(move |s| {
            state.borrow_mut().speed_percent = s.value();
        });
    }
    speed_row.add_suffix(&speed_scale);
    sliders_group.add(&speed_row);

    let brightness_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.brightness")).build();
    let brightness_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    brightness_scale.set_value(state.borrow().brightness_percent);
    brightness_scale.set_hexpand(false);
    brightness_scale.set_size_request(SLIDER_WIDTH, -1);
    brightness_scale.set_draw_value(true);
    brightness_scale.set_value_pos(gtk::PositionType::Right);
    brightness_scale.set_format_value_func(|_, value| format!("{value:.0}%"));
    {
        let state = state.clone();
        brightness_scale.connect_value_changed(move |s| {
            state.borrow_mut().brightness_percent = s.value();
        });
    }
    brightness_row.add_suffix(&brightness_scale);
    sliders_group.add(&brightness_row);

    if is_wireless {
        let strip_row = adw::ActionRow::builder()
            .title(ctx.t("rgb_editor.led_strips"))
            .subtitle(ctx.t("rgb_editor.led_strips_subtitle"))
            .build();
        let strip_adj = gtk::Adjustment::new(state.borrow().strip_count as f64, 1.0, 32.0, 1.0, 1.0, 0.0);
        let strip_spin = gtk::SpinButton::new(Some(&strip_adj), 1.0, 0);
        strip_spin.set_valign(gtk::Align::Center);
        {
            let state = state.clone();
            strip_adj.connect_value_changed(move |adj| {
                state.borrow_mut().strip_count = adj.value() as usize;
            });
        }
        strip_row.add_suffix(&strip_spin);
        sliders_group.add(&strip_row);
    }

    content.append(&sliders_group);

    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    root.append(&scrolled);

    let ctx = ctx.clone();
    let device_id = device_id.to_string();
    apply_button.connect_clicked(move |_| {
        let s = state.borrow();
        let mode = s.mode;
        let zone = s.zone;
        let colors = s.colors;
        let speed_percent = s.speed_percent;
        let brightness_percent = s.brightness_percent;
        let direction = s.direction;
        let scope = s.scope;
        let strip_count = s.strip_count;
        drop(s);

        ctx.set_rgb_prefs(&device_id, direction, strip_count);
        ctx.save_editor_snapshot(
            &device_id,
            crate::context::RgbEditorSnapshot {
                mode,
                direction,
                scope,
                colors,
                speed_percent,
                brightness_percent,
                zone,
                strip_count,
            },
        );

        let ctx = ctx.clone();
        let device_id = device_id.clone();
        glib::spawn_future_local(async move {
            if is_wireless && WIRELESS_ANIMATED_MODES.contains(&mode) {
                apply_wireless_animation(
                    &ctx,
                    &device_id,
                    mode,
                    colors,
                    speed_percent,
                    brightness_percent,
                    direction,
                    strip_count,
                )
                .await;
                return;
            }

            // Wireless Static/Direct pushes forward these color bytes to the
            // device as-is — the daemon doesn't scale them by
            // `RgbEffect.brightness` (that field only means something to
            // wired firmware), so brightness has to be baked into the color
            // itself here, same as every other wireless effect already does.
            let effect_colors: Vec<[u8; 3]> = if is_wireless {
                let factor = brightness_percent / 100.0;
                colors.iter().map(|c| scale_color(*c, factor)).collect()
            } else {
                colors.to_vec()
            };
            let effect = RgbEffect {
                mode,
                colors: effect_colors,
                speed: percent_to_speed4(speed_percent),
                brightness: percent_to_brightness4(brightness_percent),
                direction,
                scope,
                disabled: false,
            };
            let request = IpcRequest::SetRgbEffect { device_id: device_id.clone(), zone, effect: effect.clone() };
            match ctx.client.call_unit(request).await {
                Ok(()) => {
                    if is_wireless {
                        ctx.record_static_effect(&device_id, vec![(zone, effect.clone())]);
                    }
                    // Written through to `AppConfig` so the daemon's own
                    // wireless-drift auto-resync replays this instead of
                    // whatever was last saved there — see `rgb_persist`'s
                    // module doc comment.
                    crate::rgb_persist::persist_rgb_effect(&ctx, &device_id, vec![(zone, effect)]).await;
                    ctx.toast(ctx.t("rgb_editor.effect_applied"));
                }
                Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("rgb_editor.failed_apply"))),
            }
        });
    });
}

async fn apply_wireless_animation(
    ctx: &Rc<Ctx>,
    device_id: &str,
    mode: RgbMode,
    colors: [[u8; 3]; 8],
    speed_percent: f64,
    brightness_percent: f64,
    direction: RgbDirection,
    strip_count: usize,
) {
    let caps = match ctx
        .client
        .call::<Vec<RgbDeviceCapabilities>>(IpcRequest::GetRgbCapabilities)
        .await
    {
        Ok(caps) => caps,
        Err(e) => {
            ctx.toast(&format!("{}: {e}", ctx.t("rgb_editor.failed_load_caps")));
            return;
        }
    };
    let Some(dev_caps) = caps.iter().find(|c| c.device_id == device_id) else {
        ctx.toast(ctx.t("rgb_editor.caps_not_found"));
        return;
    };
    let led_count: usize = dev_caps.zones.iter().map(|z| z.led_count as usize).sum();
    if led_count == 0 {
        ctx.toast(ctx.t("rgb_editor.zero_leds"));
        return;
    }

    // No separate FPS control in this single-device editor yet — 30fps is a
    // sane fluid default (see the Global Effects page for a literal FPS
    // slider).
    const DEFAULT_FPS: f64 = 30.0;
    let cycle_ms = percent_to_cycle_ms(speed_percent);
    let frame_count = frame_count_for(DEFAULT_FPS, cycle_ms);
    let interval_ms = fps_to_interval_ms(DEFAULT_FPS);
    let reverse = crate::direction::is_reverse(direction);
    let brightness = (brightness_percent / 100.0).clamp(0.0, 1.0) as f32;
    let frames = match mode {
        RgbMode::Rainbow => rainbow_frames(led_count, frame_count, reverse, strip_count, brightness),
        RgbMode::RainbowMorph => rainbow_morph_frames(led_count, frame_count, brightness),
        RgbMode::Breathing => breathing_frames(led_count, frame_count, colors[0], brightness),
        // "Custom Gradient Wave" — see this module's doc comment on why
        // `ColorCycle` is repurposed for it. Uses all 4 color pickers as
        // gradient stops, in order (repeat the first color as the last
        // stop yourself for a seamless loop, same convention OpenRGB uses).
        RgbMode::ColorCycle => custom_gradient_wave_frames(led_count, frame_count, &colors, reverse, strip_count, brightness),
        _ => return,
    };

    let request = IpcRequest::SetRgbFrames {
        device_id: device_id.to_string(),
        frames: frames.clone(),
        interval_ms,
    };
    match ctx.client.call_unit(request).await {
        Ok(()) => {
            ctx.record_frames(device_id, frames, interval_ms, mode);
            // Deliberately NOT persisted to `AppConfig`/`SetConfig` here.
            // Confirmed on real hardware: `SetConfig` makes the daemon call
            // `apply_rgb_config()` synchronously (`service/mod.rs`'s
            // `IpcUpdate` handler), which for a wireless device pushes a
            // single-color snapshot via `set_effect()` — that's a real RF
            // command, not a no-op, so it immediately halts whatever
            // animation this `SetRgbFrames` just started (and the 1s
            // heartbeat repeats the same overwrite if drift is detected).
            // There's no way to persist an actual frame buffer (no such
            // field in `AppConfig`'s schema), so persisting anything here
            // can only make a real animation freeze, never actually help a
            // future resync. See `rgb_persist`'s doc comment.
            ctx.toast(ctx.t("rgb_editor.effect_applied"));
        }
        Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("rgb_editor.failed_apply"))),
    }
}
