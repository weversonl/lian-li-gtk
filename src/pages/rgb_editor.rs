//! RGB Effect editor: zone selector, effect mode, direction, scope, up to 4
//! colors, speed/brightness, and Apply.
//!
//! Wired devices use exactly what the daemon reports in
//! `GetRgbCapabilities.supported_modes` via `SetRgbEffect`.
//!
//! Wireless devices only ever report `["Static", "Direct"]`, since the
//! daemon just pushes a solid color to them regardless of mode. This editor
//! adds a client-rendered set (Rainbow, Rainbow Morph, Breathing, Gradient
//! Wave) turned into frames and shipped via `SetRgbFrames`, which addresses
//! the whole device's LED buffer, so the zone selector is hidden for those.
//!
//! "Gradient Wave" isn't a real `RgbMode` — it's tagged onto
//! `RgbMode::ColorCycle`, repurposed only when wireless (see
//! `effect_mode_label`), since `SetRgbFrames` has no mode field at all. A
//! wired device with a genuine native "Color Cycle" mode is unaffected.

use crate::context::Ctx;
use crate::direction::{direction_label, ALL_DIRECTIONS};
use crate::effects::{
    apply_edge_frame, apply_edge_strips_frame, breathing_frames, color_count_for_mode, custom_gradient_wave_frames,
    fps_to_interval_ms, frame_count_for, meteor_chase_frames, meteor_frames, meteor_pause_frames, mode_uses_color,
    percent_to_brightness4, percent_to_cycle_ms, percent_to_speed4, rainbow_frames, rainbow_morph_frames,
    scale_color,
};
use adw::prelude::*;
use gtk::glib;
use lianli_shared::ipc::IpcRequest;
use lianli_shared::rgb::{RgbDeviceCapabilities, RgbDirection, RgbEffect, RgbMode, RgbScope};
use std::cell::RefCell;
use std::rc::Rc;

/// Client-rendered modes offered for wireless devices, beyond the two the
/// daemon's own capabilities report (Static, Direct).
const WIRELESS_ANIMATED_MODES: [RgbMode; 7] = [
    RgbMode::Rainbow,
    RgbMode::RainbowMorph,
    RgbMode::Breathing,
    RgbMode::ColorCycle,
    RgbMode::Meteor,
    RgbMode::MeteorShower,
    RgbMode::Runway,
];

/// `MeteorShower` and `Runway` are repurposed here for wireless devices,
/// same trick as `ColorCycle` → "Gradient Wave": `Meteor` treats the whole
/// buffer as one continuous strip, merged across physical strip boundaries
/// (`meteor_frames`) — a single meteor, not one per strip. `MeteorShower` is
/// its rainbow-headed variant. `Runway` is the opposite: a strip-by-strip
/// relay (`meteor_chase_frames`) — one physical strip at a time, in
/// sequence, restarting from the first once the last finishes. A real wired
/// device with genuine native support for any of these three is unaffected.
fn effect_mode_label(mode: RgbMode, is_wireless: bool) -> &'static str {
    if is_wireless && mode == RgbMode::ColorCycle {
        "Gradient Wave"
    } else if is_wireless && mode == RgbMode::MeteorShower {
        "Meteor (Rainbow)"
    } else if is_wireless && mode == RgbMode::Runway {
        "Meteor (Split)"
    } else {
        mode.display_name()
    }
}

/// Whether `mode` is one of the Meteor family, for which the Linear/Circular
/// shape toggle is meaningful.
fn is_meteor_mode(mode: RgbMode) -> bool {
    matches!(mode, RgbMode::Meteor | RgbMode::MeteorShower | RgbMode::Runway)
}

const SLIDER_WIDTH: i32 = 260;
const SLIDER_VALUE_LABEL_WIDTH: i32 = 44;

#[derive(Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum MiddleKind {
    Solid,
    #[default]
    Effect,
}

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
    /// Per-device correction for a wiring order that doesn't match the
    /// direction label — see `DeviceRgbPrefs::invert_direction`.
    invert_direction: bool,
    /// Whether Meteor treats this device's strip as a closed ring — see
    /// `DeviceRgbPrefs::meteor_circular`.
    meteor_circular: bool,
    scope: RgbScope,
    colors: [[u8; 3]; 8],
    speed_percent: f64,
    brightness_percent: f64,
    fps: f64,
    meteor_pause_secs: f64,
    zone: u8,
    /// Physical LED strips concatenated in this device's flat buffer. `1`
    /// treats it as one continuous strip.
    strip_count: usize,
    segments_enabled: bool,
    /// Whether `edge_count` counts whole physical strips (side by side, e.g.
    /// a Strimer cable's concatenated strands) or LEDs along a single
    /// strip's length. Only meaningful when `strip_count > 1`.
    edge_by_strip: bool,
    edge_count: u32,
    edge_color: [u8; 3],
    edge_start: bool,
    edge_end: bool,
    edge_brightness_percent: f64,
    edge_kind: MiddleKind,
    edge_mode: RgbMode,
    edge_colors: [[u8; 3]; 8],
    middle_kind: MiddleKind,
    middle_color: [u8; 3],
    middle_mode: RgbMode,
    middle_colors: [[u8; 3]; 8],
    middle_brightness_percent: f64,
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
    let apply_all_button = gtk::Button::builder()
        .label(ctx.t("rgb_editor.apply_all"))
        .visible(false)
        .build();
    header.pack_end(&apply_button);
    header.pack_end(&apply_all_button);

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
        build_editor(&root, &ctx, &device_id, &caps, &apply_button, &apply_all_button);
    });

    nav_page
}

fn build_editor(
    root: &gtk::Box,
    ctx: &Rc<Ctx>,
    device_id: &str,
    caps: &RgbDeviceCapabilities,
    apply_button: &gtk::Button,
    apply_all_button: &gtk::Button,
) {
    // Owned so it can be captured by 'static signal-handler closures below.
    let device_id = device_id.to_string();
    let is_wireless = device_id.starts_with("wireless:");

    // A slider with marks (like FPS) or next to a shorter one-line title
    // can still end up allocated more or less than the others, since the
    // row hands over whatever space is left after the label. Wrapping each
    // in its own fixed-`width_request` `Box`, with the scale filling that
    // exact box, pins every slider to the same width regardless of marks
    // or label length. The value is drawn in a separate fixed-width label
    // *outside* the scale (rather than `draw_value`) so the trough itself
    // never shrinks to make room for a wider number like "100%".
    let slider_wrap = |scale: &gtk::Scale, format: fn(f64) -> String| -> gtk::Box {
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        let value_label = gtk::Label::builder()
            .label(format(scale.value()))
            .width_request(SLIDER_VALUE_LABEL_WIDTH)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        {
            let value_label = value_label.clone();
            scale.connect_value_changed(move |s| value_label.set_label(&format(s.value())));
        }
        let wrap = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        wrap.set_hexpand(false);
        wrap.set_width_request(SLIDER_WIDTH);
        wrap.append(scale);
        wrap.append(&value_label);
        wrap
    };

    let mut selectable_modes = caps.supported_modes.clone();
    if is_wireless {
        selectable_modes.extend(WIRELESS_ANIMATED_MODES);
    }

    let saved_prefs = ctx.rgb_prefs_for(&device_id);
    let snapshot = ctx.editor_snapshot_for(&device_id);
    let initial_mode = snapshot
        .as_ref()
        .map(|s| s.mode)
        .filter(|m| selectable_modes.contains(m))
        .unwrap_or_else(|| selectable_modes.first().copied().unwrap_or(RgbMode::Static));
    let state = Rc::new(RefCell::new(EditorState {
        mode: initial_mode,
        direction: snapshot.as_ref().map(|s| s.direction).unwrap_or(saved_prefs.direction),
        invert_direction: saved_prefs.invert_direction,
        meteor_circular: saved_prefs.meteor_circular,
        scope: snapshot.as_ref().map(|s| s.scope).unwrap_or(RgbScope::All),
        colors: snapshot.as_ref().map(|s| s.colors).unwrap_or_else(|| ctx.gradient_colors()),
        speed_percent: snapshot.as_ref().map(|s| s.speed_percent).unwrap_or(50.0),
        brightness_percent: snapshot.as_ref().map(|s| s.brightness_percent).unwrap_or(100.0),
        fps: snapshot.as_ref().map(|s| s.fps).unwrap_or(30.0),
        meteor_pause_secs: snapshot.as_ref().map(|s| s.meteor_pause_secs).unwrap_or(5.0),
        zone: snapshot.as_ref().map(|s| s.zone).unwrap_or(0),
        strip_count: snapshot.as_ref().map(|s| s.strip_count).unwrap_or(saved_prefs.strip_count),
        segments_enabled: snapshot.as_ref().map(|s| s.segments_enabled).unwrap_or(false),
        edge_by_strip: snapshot.as_ref().map(|s| s.edge_by_strip).unwrap_or(true),
        edge_count: snapshot.as_ref().map(|s| s.edge_count).unwrap_or(1),
        edge_color: snapshot.as_ref().map(|s| s.edge_color).unwrap_or([255, 255, 255]),
        edge_start: snapshot.as_ref().map(|s| s.edge_start).unwrap_or(true),
        edge_end: snapshot.as_ref().map(|s| s.edge_end).unwrap_or(true),
        edge_brightness_percent: snapshot.as_ref().map(|s| s.edge_brightness_percent).unwrap_or(100.0),
        edge_kind: snapshot.as_ref().map(|s| s.edge_kind).unwrap_or(MiddleKind::Solid),
        edge_mode: snapshot.as_ref().map(|s| s.edge_mode).unwrap_or(RgbMode::Rainbow),
        edge_colors: snapshot.as_ref().map(|s| s.edge_colors).unwrap_or_else(|| ctx.gradient_colors()),
        middle_kind: snapshot.as_ref().map(|s| s.middle_kind).unwrap_or(MiddleKind::Effect),
        middle_color: snapshot.as_ref().map(|s| s.middle_color).unwrap_or([255, 255, 255]),
        middle_mode: snapshot.as_ref().map(|s| s.middle_mode).unwrap_or(RgbMode::Rainbow),
        middle_colors: snapshot.as_ref().map(|s| s.middle_colors).unwrap_or_else(|| ctx.gradient_colors()),
        middle_brightness_percent: snapshot.as_ref().map(|s| s.middle_brightness_percent).unwrap_or(100.0),
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

    let colors_label = gtk::Label::builder()
        .label(ctx.t("rgb_editor.colors"))
        .css_classes(["caption-heading", "dim-label"])
        .halign(gtk::Align::Start)
        .hexpand(true)
        .visible(mode_uses_color(initial_mode))
        .build();
    let colors_reset_button = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text(ctx.t("rgb_editor.reset_rainbow"))
        .css_classes(["flat"])
        .visible(color_count_for_mode(initial_mode) > 1)
        .build();
    let colors_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    colors_header.append(&colors_label);
    colors_header.append(&colors_reset_button);
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
    {
        let color_buttons = color_buttons.clone();
        colors_reset_button.connect_clicked(move |_| {
            let rainbow = crate::app_prefs::default_gradient_colors();
            for (button, color) in color_buttons.iter().zip(rainbow) {
                button.set_rgba(&gtk::gdk::RGBA::new(
                    color[0] as f32 / 255.0,
                    color[1] as f32 / 255.0,
                    color[2] as f32 / 255.0,
                    1.0,
                ));
            }
        });
    }

    let mode_group = adw::PreferencesGroup::new();

    let meteor_shape_row = adw::ActionRow::builder()
        .title(ctx.t("rgb_editor.meteor_shape"))
        .subtitle(ctx.t("rgb_editor.meteor_shape_subtitle"))
        .title_lines(1)
        .visible(is_meteor_mode(initial_mode))
        .build();
    let meteor_shape_switch =
        gtk::Switch::builder().valign(gtk::Align::Center).active(state.borrow().meteor_circular).build();
    {
        let state = state.clone();
        let ctx = ctx.clone();
        let device_id = device_id.clone();
        meteor_shape_switch.connect_state_set(move |_, circular| {
            let mut s = state.borrow_mut();
            s.meteor_circular = circular;
            // Persisted immediately, not gated behind "Aplicar" — this is
            // hardware topology (like Global Effects' equivalent control),
            // not part of the effect being previewed, so it shouldn't be
            // silently lost if the user navigates away before applying.
            ctx.set_rgb_prefs(&device_id, s.direction, s.strip_count, s.invert_direction, circular);
            drop(s);
            glib::Propagation::Proceed
        });
    }
    meteor_shape_row.add_suffix(&meteor_shape_switch);

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
        let colors_reset_button = colors_reset_button.clone();
        let color_buttons = color_buttons.clone();
        let meteor_shape_row = meteor_shape_row.clone();
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
                colors_reset_button.set_visible(count > 1);
                for (i, button) in color_buttons.iter().enumerate() {
                    button.set_visible(i < count);
                }
                meteor_shape_row.set_visible(is_meteor_mode(*mode));
            }
        });
    }
    // `set_selected` doesn't fire the signal if the index is already 0.
    let initial_whole_device_only = is_wireless && WIRELESS_ANIMATED_MODES.contains(&initial_mode);
    if caps.zones.len() > 1 {
        zone_box.set_sensitive(!initial_whole_device_only);
    }
    zone_note.set_visible(initial_whole_device_only);
    mode_group.add(&mode_row);

    let direction_names: Vec<&str> = ALL_DIRECTIONS.iter().map(|d| direction_label(*d, ctx.lang())).collect();
    let direction_model = gtk::StringList::new(&direction_names);
    let direction_row =
        adw::ComboRow::builder().title(ctx.t("rgb_editor.direction")).model(&direction_model).build();
    let direction_selected_index =
        ALL_DIRECTIONS.iter().position(|d| *d == state.borrow().direction).unwrap_or(0);
    direction_row.set_selected(direction_selected_index as u32);
    {
        let state = state.clone();
        let ctx = ctx.clone();
        let device_id = device_id.clone();
        direction_row.connect_selected_notify(move |row| {
            let Some(d) = ALL_DIRECTIONS.get(row.selected() as usize) else { return };
            let mut s = state.borrow_mut();
            s.direction = *d;
            // Persisted immediately — see the meteor-shape switch's comment.
            ctx.set_rgb_prefs(&device_id, *d, s.strip_count, s.invert_direction, s.meteor_circular);
            drop(s);
        });
    }
    mode_group.add(&direction_row);

    let invert_direction_row = adw::ActionRow::builder()
        .title(ctx.t("rgb_editor.invert_direction"))
        .subtitle(ctx.t("rgb_editor.invert_direction_subtitle"))
        .build();
    let invert_direction_switch =
        gtk::Switch::builder().valign(gtk::Align::Center).active(state.borrow().invert_direction).build();
    {
        let state = state.clone();
        let ctx = ctx.clone();
        let device_id = device_id.clone();
        invert_direction_switch.connect_state_set(move |_, enabled| {
            let mut s = state.borrow_mut();
            s.invert_direction = enabled;
            // Persisted immediately — see the meteor-shape switch's comment.
            ctx.set_rgb_prefs(&device_id, s.direction, s.strip_count, enabled, s.meteor_circular);
            drop(s);
            glib::Propagation::Proceed
        });
    }
    invert_direction_row.add_suffix(&invert_direction_switch);
    mode_group.add(&invert_direction_row);
    mode_group.add(&meteor_shape_row);

    let zone_scopes = caps.supported_scopes.first().cloned().unwrap_or_default();
    let mut scope_row_opt: Option<adw::ComboRow> = None;
    if !zone_scopes.is_empty() {
        let scope_names: Vec<&str> = zone_scopes.iter().map(|s| scope_label(*s, ctx)).collect();
        let scope_model = gtk::StringList::new(&scope_names);
        let scope_row =
            adw::ComboRow::builder().title(ctx.t("rgb_editor.scope")).model(&scope_model).build();
        let scopes = zone_scopes.clone();
        let initial_scope_index = zone_scopes.iter().position(|s| *s == state.borrow().scope).unwrap_or(0);
        scope_row.set_selected(initial_scope_index as u32);
        {
            let state = state.clone();
            scope_row.connect_selected_notify(move |row| {
                if let Some(s) = scopes.get(row.selected() as usize) {
                    state.borrow_mut().scope = *s;
                }
            });
        }
        mode_group.add(&scope_row);
        scope_row_opt = Some(scope_row);
    }
    content.append(&mode_group);

    content.append(&colors_header);
    content.append(&colors_box);

    let sliders_group = adw::PreferencesGroup::new();

    let speed_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.speed")).build();
    let speed_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    speed_scale.set_value(state.borrow().speed_percent);
    speed_scale.set_hexpand(false);
    {
        let state = state.clone();
        speed_scale.connect_value_changed(move |s| {
            state.borrow_mut().speed_percent = s.value();
        });
    }
    speed_row.add_suffix(&slider_wrap(&speed_scale, |v| format!("{v:.0}%")));
    sliders_group.add(&speed_row);

    let brightness_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.brightness")).build();
    let brightness_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    brightness_scale.set_value(state.borrow().brightness_percent);
    brightness_scale.set_hexpand(false);
    {
        let state = state.clone();
        brightness_scale.connect_value_changed(move |s| {
            state.borrow_mut().brightness_percent = s.value();
        });
    }
    brightness_row.add_suffix(&slider_wrap(&brightness_scale, |v| format!("{v:.0}%")));
    sliders_group.add(&brightness_row);

    let fps_row = adw::ActionRow::builder()
        .title(ctx.t("ge.fps"))
        .subtitle(ctx.t("ge.fps_subtitle"))
        .build();
    let fps_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 5.0, 60.0, 1.0);
    fps_scale.set_value(state.borrow().fps);
    fps_scale.set_hexpand(false);
    fps_scale.add_mark(15.0, gtk::PositionType::Bottom, Some("15"));
    fps_scale.add_mark(30.0, gtk::PositionType::Bottom, Some("30"));
    fps_scale.add_mark(60.0, gtk::PositionType::Bottom, Some("60"));
    if state.borrow().fps > 60.0 {
        fps_scale.set_range(5.0, 120.0);
        fps_scale.add_mark(120.0, gtk::PositionType::Bottom, Some("120"));
        fps_scale.add_css_class("fps-warning-zone");
    }
    {
        let state = state.clone();
        fps_scale.connect_value_changed(move |s| {
            state.borrow_mut().fps = s.value();
        });
    }

    let fps_range_row = adw::ActionRow::builder().title(ctx.t("ge.over_60fps")).build();
    let initial_extended = state.borrow().fps > 60.0;
    let fps_range_switch =
        gtk::Switch::builder().valign(gtk::Align::Center).active(initial_extended).build();
    {
        let fps_scale = fps_scale.clone();
        fps_range_switch.connect_state_set(move |_, extended| {
            fps_scale.clear_marks();
            if extended {
                fps_scale.set_range(5.0, 120.0);
                fps_scale.add_mark(15.0, gtk::PositionType::Bottom, Some("15"));
                fps_scale.add_mark(30.0, gtk::PositionType::Bottom, Some("30"));
                fps_scale.add_mark(60.0, gtk::PositionType::Bottom, Some("60"));
                fps_scale.add_mark(120.0, gtk::PositionType::Bottom, Some("120"));
                fps_scale.add_css_class("fps-warning-zone");
            } else {
                if fps_scale.value() > 60.0 {
                    fps_scale.set_value(60.0);
                }
                fps_scale.set_range(5.0, 60.0);
                fps_scale.add_mark(15.0, gtk::PositionType::Bottom, Some("15"));
                fps_scale.add_mark(30.0, gtk::PositionType::Bottom, Some("30"));
                fps_scale.add_mark(60.0, gtk::PositionType::Bottom, Some("60"));
                fps_scale.remove_css_class("fps-warning-zone");
            }
            glib::Propagation::Proceed
        });
    }
    fps_range_row.add_suffix(&fps_range_switch);
    sliders_group.add(&fps_range_row);

    fps_row.add_suffix(&slider_wrap(&fps_scale, |v| format!("{v:.0}")));
    sliders_group.add(&fps_row);

    let meteor_pause_row = adw::ActionRow::builder()
        .title(ctx.t("rgb_editor.meteor_pause"))
        .subtitle(ctx.t("rgb_editor.meteor_pause_subtitle"))
        .build();
    let meteor_pause_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 30.0, 1.0);
    meteor_pause_scale.set_value(state.borrow().meteor_pause_secs);
    meteor_pause_scale.set_hexpand(false);
    {
        let state = state.clone();
        meteor_pause_scale.connect_value_changed(move |s| {
            state.borrow_mut().meteor_pause_secs = s.value();
        });
    }
    meteor_pause_row.add_suffix(&slider_wrap(&meteor_pause_scale, |v| format!("{v:.0}s")));
    sliders_group.add(&meteor_pause_row);

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
            let ctx = ctx.clone();
            let device_id = device_id.clone();
            strip_adj.connect_value_changed(move |adj| {
                let mut s = state.borrow_mut();
                let strip_count = adj.value() as usize;
                s.strip_count = strip_count;
                // Persisted immediately — same reasoning as the meteor-shape
                // switch above, see its comment.
                ctx.set_rgb_prefs(&device_id, s.direction, strip_count, s.invert_direction, s.meteor_circular);
                drop(s);
            });
        }
        strip_row.add_suffix(&strip_spin);
        sliders_group.add(&strip_row);
    }

    content.append(&sliders_group);

    let supports_segments = is_wireless || caps.supports_direct;
    // Multi-zone devices (e.g. a 4-fan hub, one zone per fan) apply Segments
    // to whichever zone is picked above via `SetRgbDirect` (static-only —
    // safe to touch one zone without resetting the others). A single-zone
    // wireless device (e.g. a Strimer cable) has no per-zone addressing at
    // all, so it always goes through `SetRgbFrames` for the whole buffer,
    // which also allows an animated middle.
    let segments_per_zone = caps.zones.len() > 1;
    if supports_segments {
        let segments_row = adw::ExpanderRow::builder()
            .title(ctx.t("rgb_editor.segments"))
            .subtitle(ctx.t("rgb_editor.segments_subtitle"))
            .show_enable_switch(true)
            .enable_expansion(state.borrow().segments_enabled)
            .expanded(state.borrow().segments_enabled)
            .build();
        {
            let state = state.clone();
            let mode_row = mode_row.clone();
            let colors_label = colors_label.clone();
            let colors_box = colors_box.clone();
            let colors_reset_button = colors_reset_button.clone();
            let brightness_row = brightness_row.clone();
            let scope_row_opt = scope_row_opt.clone();
            segments_row.connect_enable_expansion_notify(move |row| {
                let enabled = row.enables_expansion();
                state.borrow_mut().segments_enabled = enabled;
                row.set_expanded(enabled);
                mode_row.set_sensitive(!enabled);
                colors_label.set_sensitive(!enabled);
                colors_box.set_sensitive(!enabled);
                colors_reset_button.set_sensitive(!enabled);
                brightness_row.set_sensitive(!enabled);
                if let Some(scope_row) = &scope_row_opt {
                    scope_row.set_sensitive(!enabled);
                }
            });
        }
        if state.borrow().segments_enabled {
            mode_row.set_sensitive(false);
            colors_label.set_sensitive(false);
            colors_box.set_sensitive(false);
            colors_reset_button.set_sensitive(false);
            brightness_row.set_sensitive(false);
            if let Some(scope_row) = &scope_row_opt {
                scope_row.set_sensitive(false);
            }
        }

        let edge_count_adj = gtk::Adjustment::new(state.borrow().edge_count as f64, 0.0, 64.0, 1.0, 1.0, 0.0);
        let edge_count_row = adw::SpinRow::new(Some(&edge_count_adj), 1.0, 0);
        let set_edge_count_labels = {
            let ctx = ctx.clone();
            let edge_count_row = edge_count_row.clone();
            move |by_strip: bool| {
                if by_strip {
                    edge_count_row.set_title(ctx.t("rgb_editor.edge_strip_count"));
                    edge_count_row.set_subtitle(ctx.t("rgb_editor.edge_strip_count_subtitle"));
                } else {
                    edge_count_row.set_title(ctx.t("rgb_editor.edge_led_count"));
                    edge_count_row.set_subtitle(ctx.t("rgb_editor.edge_led_count_subtitle"));
                }
            }
        };
        set_edge_count_labels(state.borrow().edge_by_strip);
        {
            let state = state.clone();
            edge_count_adj.connect_value_changed(move |adj| {
                state.borrow_mut().edge_count = adj.value() as u32;
            });
        }

        if !segments_per_zone {
            let orientation_names =
                gtk::StringList::new(&[ctx.t("rgb_editor.edge_by_strip"), ctx.t("rgb_editor.edge_by_length")]);
            let orientation_row = adw::ComboRow::builder()
                .title(ctx.t("rgb_editor.edge_orientation"))
                .model(&orientation_names)
                .build();
            orientation_row.set_selected(if state.borrow().edge_by_strip { 0 } else { 1 });
            let state = state.clone();
            let set_edge_count_labels = set_edge_count_labels.clone();
            orientation_row.connect_selected_notify(move |row| {
                let by_strip = row.selected() == 0;
                state.borrow_mut().edge_by_strip = by_strip;
                set_edge_count_labels(by_strip);
            });
            segments_row.add_row(&orientation_row);
        }

        segments_row.add_row(&edge_count_row);

        let edge_sides_names: Vec<&str> = [
            ctx.t("rgb_editor.edge_both"),
            ctx.t("rgb_editor.edge_start_only"),
            ctx.t("rgb_editor.edge_end_only"),
        ]
        .to_vec();
        let edge_sides_model = gtk::StringList::new(&edge_sides_names);
        let edge_sides_row =
            adw::ComboRow::builder().title(ctx.t("rgb_editor.edge_sides")).model(&edge_sides_model).build();
        let initial_sides_index = match (state.borrow().edge_start, state.borrow().edge_end) {
            (true, false) => 1,
            (false, true) => 2,
            _ => 0,
        };
        edge_sides_row.set_selected(initial_sides_index as u32);
        {
            let state = state.clone();
            edge_sides_row.connect_selected_notify(move |row| {
                let mut s = state.borrow_mut();
                (s.edge_start, s.edge_end) = match row.selected() {
                    1 => (true, false),
                    2 => (false, true),
                    _ => (true, true),
                };
            });
        }
        segments_row.add_row(&edge_sides_row);

        let edge_color_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.edge_color")).build();
        let edge_default = state.borrow().edge_color;
        let edge_dialog = gtk::ColorDialog::builder().with_alpha(false).build();
        let edge_rgba = gtk::gdk::RGBA::new(
            edge_default[0] as f32 / 255.0,
            edge_default[1] as f32 / 255.0,
            edge_default[2] as f32 / 255.0,
            1.0,
        );
        let edge_color_button = gtk::ColorDialogButton::builder().dialog(&edge_dialog).rgba(&edge_rgba).build();
        {
            let state = state.clone();
            edge_color_button.connect_rgba_notify(move |b| {
                let rgba = b.rgba();
                state.borrow_mut().edge_color =
                    [(rgba.red() * 255.0) as u8, (rgba.green() * 255.0) as u8, (rgba.blue() * 255.0) as u8];
            });
        }
        edge_color_row.add_suffix(&edge_color_button);
        edge_color_row.set_visible(state.borrow().edge_kind == MiddleKind::Solid);

        let edge_brightness_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.edge_brightness")).build();
        let edge_brightness_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        edge_brightness_scale.set_value(state.borrow().edge_brightness_percent);
        edge_brightness_scale.set_hexpand(false);
        {
            let state = state.clone();
            edge_brightness_scale.connect_value_changed(move |s| {
                state.borrow_mut().edge_brightness_percent = s.value();
            });
        }
        edge_brightness_row.add_suffix(&slider_wrap(&edge_brightness_scale, |v| format!("{v:.0}%")));

        let edge_mode_names: Vec<&str> =
            WIRELESS_ANIMATED_MODES.iter().map(|m| effect_mode_label(*m, true)).collect();
        let edge_mode_model = gtk::StringList::new(&edge_mode_names);
        let edge_mode_row =
            adw::ComboRow::builder().title(ctx.t("rgb_editor.edge_mode")).model(&edge_mode_model).build();
        if let Some(idx) = WIRELESS_ANIMATED_MODES.iter().position(|m| *m == state.borrow().edge_mode) {
            edge_mode_row.set_selected(idx as u32);
        }

        let edge_colors_label = gtk::Label::builder()
            .label(ctx.t("rgb_editor.edge_colors"))
            .css_classes(["caption-heading", "dim-label"])
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        let edge_colors_reset_button = gtk::Button::builder()
            .icon_name("edit-undo-symbolic")
            .tooltip_text(ctx.t("rgb_editor.reset_rainbow"))
            .css_classes(["flat"])
            .build();
        let edge_colors_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        edge_colors_header.append(&edge_colors_label);
        edge_colors_header.append(&edge_colors_reset_button);
        let edge_colors_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .row_spacing(8)
            .column_spacing(8)
            .halign(gtk::Align::Start)
            .max_children_per_line(8)
            .build();
        let mut edge_color_buttons: Vec<gtk::ColorDialogButton> = Vec::with_capacity(8);
        for i in 0..8usize {
            let dialog = gtk::ColorDialog::builder().with_alpha(false).build();
            let default = state.borrow().edge_colors[i];
            let rgba = gtk::gdk::RGBA::new(
                default[0] as f32 / 255.0,
                default[1] as f32 / 255.0,
                default[2] as f32 / 255.0,
                1.0,
            );
            let button = gtk::ColorDialogButton::builder().dialog(&dialog).rgba(&rgba).build();
            let state = state.clone();
            button.connect_rgba_notify(move |b| {
                let rgba = b.rgba();
                state.borrow_mut().edge_colors[i] = [
                    (rgba.red() * 255.0) as u8,
                    (rgba.green() * 255.0) as u8,
                    (rgba.blue() * 255.0) as u8,
                ];
            });
            edge_colors_box.append(&button);
            edge_color_buttons.push(button);
        }
        {
            let edge_color_buttons = edge_color_buttons.clone();
            edge_colors_reset_button.connect_clicked(move |_| {
                let rainbow = crate::app_prefs::default_gradient_colors();
                for (button, color) in edge_color_buttons.iter().zip(rainbow) {
                    button.set_rgba(&gtk::gdk::RGBA::new(
                        color[0] as f32 / 255.0,
                        color[1] as f32 / 255.0,
                        color[2] as f32 / 255.0,
                        1.0,
                    ));
                }
            });
        }
        let edge_colors_wrap = gtk::Box::new(gtk::Orientation::Vertical, 8);
        edge_colors_wrap.set_margin_top(8);
        edge_colors_wrap.set_margin_bottom(8);
        edge_colors_wrap.set_margin_start(12);
        edge_colors_wrap.set_margin_end(12);
        edge_colors_wrap.append(&edge_colors_header);
        edge_colors_wrap.append(&edge_colors_box);
        let edge_is_effect = state.borrow().edge_kind == MiddleKind::Effect;
        edge_colors_wrap.set_visible(edge_is_effect && color_count_for_mode(state.borrow().edge_mode) > 1);
        for (i, button) in edge_color_buttons.iter().enumerate() {
            button.set_visible(i < color_count_for_mode(state.borrow().edge_mode));
        }

        // `edge_mode_row`/`edge_color_row` are real `AdwActionRow`/`AdwComboRow`
        // instances added directly to `segments_row` — being genuine
        // `GtkListBoxRow` subtypes, `set_visible` on them collapses cleanly
        // with no residual separator. `edge_colors_wrap` is a plain
        // Label+FlowBox `gtk::Box` with no row equivalent, so it's appended
        // directly to the outer `content` column instead of `add_row()`'d —
        // libadwaita auto-wraps non-row widgets passed to `add_row()` in a
        // synthetic row that has no visibility handle, leaving a phantom
        // separator behind when the content inside collapses.
        edge_mode_row.set_visible(edge_is_effect);

        {
            let state = state.clone();
            let edge_colors_wrap = edge_colors_wrap.clone();
            let edge_color_buttons = edge_color_buttons.clone();
            edge_mode_row.connect_selected_notify(move |row| {
                if let Some(mode) = WIRELESS_ANIMATED_MODES.get(row.selected() as usize) {
                    state.borrow_mut().edge_mode = *mode;
                    let count = color_count_for_mode(*mode);
                    let is_effect = state.borrow().edge_kind == MiddleKind::Effect;
                    edge_colors_wrap.set_visible(is_effect && count > 1);
                    for (i, button) in edge_color_buttons.iter().enumerate() {
                        button.set_visible(i < count);
                    }
                }
            });
        }

        {
            let kind_names = gtk::StringList::new(&[ctx.t("rgb_editor.kind_solid"), ctx.t("rgb_editor.kind_effect")]);
            let edge_kind_row = adw::ComboRow::builder()
                .title(ctx.t("rgb_editor.edge_kind"))
                .model(&kind_names)
                .build();
            edge_kind_row.set_selected(if state.borrow().edge_kind == MiddleKind::Effect { 1 } else { 0 });
            let state_for_kind = state.clone();
            let state_for_count = state.clone();
            let edge_color_row_c = edge_color_row.clone();
            let edge_mode_row_c = edge_mode_row.clone();
            let edge_colors_wrap_c = edge_colors_wrap.clone();
            edge_kind_row.connect_selected_notify(move |row| {
                let is_effect = row.selected() == 1;
                let kind = if is_effect { MiddleKind::Effect } else { MiddleKind::Solid };
                state_for_kind.borrow_mut().edge_kind = kind;
                edge_color_row_c.set_visible(!is_effect);
                edge_mode_row_c.set_visible(is_effect);
                let count = color_count_for_mode(state_for_count.borrow().edge_mode);
                edge_colors_wrap_c.set_visible(is_effect && count > 1);
            });
            segments_row.add_row(&edge_kind_row);
        }
        segments_row.add_row(&edge_color_row);
        segments_row.add_row(&edge_mode_row);
        segments_row.add_row(&edge_brightness_row);

        let middle_color_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.middle_color")).build();
        let middle_default = state.borrow().middle_color;
        let middle_dialog = gtk::ColorDialog::builder().with_alpha(false).build();
        let middle_rgba = gtk::gdk::RGBA::new(
            middle_default[0] as f32 / 255.0,
            middle_default[1] as f32 / 255.0,
            middle_default[2] as f32 / 255.0,
            1.0,
        );
        let middle_color_button =
            gtk::ColorDialogButton::builder().dialog(&middle_dialog).rgba(&middle_rgba).build();
        {
            let state = state.clone();
            middle_color_button.connect_rgba_notify(move |b| {
                let rgba = b.rgba();
                state.borrow_mut().middle_color =
                    [(rgba.red() * 255.0) as u8, (rgba.green() * 255.0) as u8, (rgba.blue() * 255.0) as u8];
            });
        }
        middle_color_row.add_suffix(&middle_color_button);
        middle_color_row.set_visible(state.borrow().middle_kind == MiddleKind::Solid);

        let middle_brightness_row = adw::ActionRow::builder().title(ctx.t("rgb_editor.middle_brightness")).build();
        let middle_brightness_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        middle_brightness_scale.set_value(state.borrow().middle_brightness_percent);
        middle_brightness_scale.set_hexpand(false);
        {
            let state = state.clone();
            middle_brightness_scale.connect_value_changed(move |s| {
                state.borrow_mut().middle_brightness_percent = s.value();
            });
        }
        middle_brightness_row.add_suffix(&slider_wrap(&middle_brightness_scale, |v| format!("{v:.0}%")));

        let middle_mode_names: Vec<&str> =
            WIRELESS_ANIMATED_MODES.iter().map(|m| effect_mode_label(*m, true)).collect();
        let middle_mode_model = gtk::StringList::new(&middle_mode_names);
        let middle_mode_row =
            adw::ComboRow::builder().title(ctx.t("rgb_editor.middle_mode")).model(&middle_mode_model).build();
        if let Some(idx) = WIRELESS_ANIMATED_MODES.iter().position(|m| *m == state.borrow().middle_mode) {
            middle_mode_row.set_selected(idx as u32);
        }

        let middle_colors_label = gtk::Label::builder()
            .label(ctx.t("rgb_editor.middle_colors"))
            .css_classes(["caption-heading", "dim-label"])
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        let middle_colors_reset_button = gtk::Button::builder()
            .icon_name("edit-undo-symbolic")
            .tooltip_text(ctx.t("rgb_editor.reset_rainbow"))
            .css_classes(["flat"])
            .build();
        let middle_colors_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        middle_colors_header.append(&middle_colors_label);
        middle_colors_header.append(&middle_colors_reset_button);
        let middle_colors_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .row_spacing(8)
            .column_spacing(8)
            .halign(gtk::Align::Start)
            .max_children_per_line(8)
            .build();
        let mut middle_color_buttons: Vec<gtk::ColorDialogButton> = Vec::with_capacity(8);
        for i in 0..8usize {
            let dialog = gtk::ColorDialog::builder().with_alpha(false).build();
            let default = state.borrow().middle_colors[i];
            let rgba = gtk::gdk::RGBA::new(
                default[0] as f32 / 255.0,
                default[1] as f32 / 255.0,
                default[2] as f32 / 255.0,
                1.0,
            );
            let button = gtk::ColorDialogButton::builder().dialog(&dialog).rgba(&rgba).build();
            let state = state.clone();
            button.connect_rgba_notify(move |b| {
                let rgba = b.rgba();
                state.borrow_mut().middle_colors[i] = [
                    (rgba.red() * 255.0) as u8,
                    (rgba.green() * 255.0) as u8,
                    (rgba.blue() * 255.0) as u8,
                ];
            });
            middle_colors_box.append(&button);
            middle_color_buttons.push(button);
        }
        {
            let middle_color_buttons = middle_color_buttons.clone();
            middle_colors_reset_button.connect_clicked(move |_| {
                let rainbow = crate::app_prefs::default_gradient_colors();
                for (button, color) in middle_color_buttons.iter().zip(rainbow) {
                    button.set_rgba(&gtk::gdk::RGBA::new(
                        color[0] as f32 / 255.0,
                        color[1] as f32 / 255.0,
                        color[2] as f32 / 255.0,
                        1.0,
                    ));
                }
            });
        }
        let middle_colors_wrap = gtk::Box::new(gtk::Orientation::Vertical, 8);
        middle_colors_wrap.set_margin_top(8);
        middle_colors_wrap.set_margin_bottom(8);
        middle_colors_wrap.set_margin_start(12);
        middle_colors_wrap.set_margin_end(12);
        middle_colors_wrap.append(&middle_colors_header);
        middle_colors_wrap.append(&middle_colors_box);
        let middle_is_effect = state.borrow().middle_kind == MiddleKind::Effect;
        middle_colors_wrap.set_visible(middle_is_effect && color_count_for_mode(state.borrow().middle_mode) > 1);
        for (i, button) in middle_color_buttons.iter().enumerate() {
            button.set_visible(i < color_count_for_mode(state.borrow().middle_mode));
        }

        // See `edge_mode_row`/`edge_colors_wrap` above for why `middle_mode_row`
        // stays a direct `add_row()` child while `middle_colors_wrap` is
        // appended straight to `content` instead.
        middle_mode_row.set_visible(middle_is_effect);

        {
            let state = state.clone();
            let middle_colors_wrap = middle_colors_wrap.clone();
            let middle_color_buttons = middle_color_buttons.clone();
            middle_mode_row.connect_selected_notify(move |row| {
                if let Some(mode) = WIRELESS_ANIMATED_MODES.get(row.selected() as usize) {
                    state.borrow_mut().middle_mode = *mode;
                    let count = color_count_for_mode(*mode);
                    let is_effect = state.borrow().middle_kind == MiddleKind::Effect;
                    middle_colors_wrap.set_visible(is_effect && count > 1);
                    for (i, button) in middle_color_buttons.iter().enumerate() {
                        button.set_visible(i < count);
                    }
                }
            });
        }

        {
            let kind_names = gtk::StringList::new(&[ctx.t("rgb_editor.kind_solid"), ctx.t("rgb_editor.kind_effect")]);
            let middle_kind_row = adw::ComboRow::builder()
                .title(ctx.t("rgb_editor.middle"))
                .model(&kind_names)
                .build();
            middle_kind_row.set_selected(if state.borrow().middle_kind == MiddleKind::Effect { 1 } else { 0 });
            let state_for_kind = state.clone();
            let state_for_count = state.clone();
            let middle_color_row_c = middle_color_row.clone();
            let middle_mode_row_c = middle_mode_row.clone();
            let middle_colors_wrap_c = middle_colors_wrap.clone();
            middle_kind_row.connect_selected_notify(move |row| {
                let is_effect = row.selected() == 1;
                let kind = if is_effect { MiddleKind::Effect } else { MiddleKind::Solid };
                state_for_kind.borrow_mut().middle_kind = kind;
                middle_color_row_c.set_visible(!is_effect);
                middle_mode_row_c.set_visible(is_effect);
                let count = color_count_for_mode(state_for_count.borrow().middle_mode);
                middle_colors_wrap_c.set_visible(is_effect && count > 1);
            });
            segments_row.add_row(&middle_kind_row);
        }
        segments_row.add_row(&middle_color_row);
        segments_row.add_row(&middle_mode_row);
        segments_row.add_row(&middle_brightness_row);

        if segments_per_zone {
            let per_zone_note = adw::ActionRow::builder()
                .title(ctx.t("rgb_editor.segments_wired_note"))
                .css_classes(["dim-label"])
                .build();
            segments_row.add_row(&per_zone_note);
        }

        let segments_group = adw::PreferencesGroup::new();
        segments_group.add(&segments_row);
        content.append(&segments_group);
        content.append(&edge_colors_wrap);
        content.append(&middle_colors_wrap);
    }

    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    root.append(&scrolled);

    let zone_count = caps.zones.len();
    let ctx = ctx.clone();
    let device_id = device_id.to_string();
    let do_apply = {
        let ctx = ctx.clone();
        let device_id = device_id.clone();
        let state = state.clone();
        move |apply_to_all: bool| {
            let s = state.borrow();
            let mode = s.mode;
            let zone = s.zone;
            let colors = s.colors;
            let speed_percent = s.speed_percent;
            let brightness_percent = s.brightness_percent;
            let fps = s.fps;
            let meteor_pause_secs = s.meteor_pause_secs;
            let direction = s.direction;
            let invert_direction = s.invert_direction;
            let meteor_circular = s.meteor_circular;
            let scope = s.scope;
            let strip_count = s.strip_count;
            let segments_enabled = s.segments_enabled;
            let edge_by_strip = s.edge_by_strip;
            let edge_count = s.edge_count as usize;
            let edge_color = s.edge_color;
            let edge_start = s.edge_start;
            let edge_end = s.edge_end;
            let edge_brightness_percent = s.edge_brightness_percent;
            let edge_kind = s.edge_kind;
            let edge_mode = s.edge_mode;
            let edge_colors = s.edge_colors;
            let middle_kind = s.middle_kind;
            let middle_color = s.middle_color;
            let middle_mode = s.middle_mode;
            let middle_colors = s.middle_colors;
            let middle_brightness_percent = s.middle_brightness_percent;
            drop(s);

            ctx.set_rgb_prefs(&device_id, direction, strip_count, invert_direction, meteor_circular);
            ctx.save_editor_snapshot(
                &device_id,
                crate::context::RgbEditorSnapshot {
                    mode,
                    direction,
                    scope,
                    colors,
                    speed_percent,
                    brightness_percent,
                    fps,
                    meteor_pause_secs,
                    zone,
                    strip_count,
                    segments_enabled,
                    edge_by_strip,
                    edge_count: edge_count as u32,
                    edge_color,
                    edge_start,
                    edge_end,
                    edge_brightness_percent,
                    edge_kind,
                    edge_mode,
                    edge_colors,
                    middle_kind,
                    middle_color,
                    middle_mode,
                    middle_colors,
                    middle_brightness_percent,
                },
            );

            let zones: Vec<u8> =
                if apply_to_all { (0..zone_count as u8).collect() } else { vec![zone] };

            // Each target zone gets its own task — a Segments Effect zone
            // loops forever (see `apply_segments`), so awaiting them in
            // sequence would starve every zone after the first.
            for target_zone in zones {
                let ctx = ctx.clone();
                let device_id = device_id.clone();
                glib::spawn_future_local(async move {
                    if segments_enabled {
                        apply_segments(
                            &ctx,
                            &device_id,
                            is_wireless,
                            target_zone,
                            edge_by_strip,
                            edge_count,
                            edge_color,
                            edge_start,
                            edge_end,
                            edge_brightness_percent,
                            edge_kind,
                            edge_mode,
                            edge_colors,
                            middle_kind,
                            middle_color,
                            middle_mode,
                            middle_colors,
                            middle_brightness_percent,
                            speed_percent,
                            fps,
                            meteor_pause_secs,
                            direction,
                            invert_direction,
                            meteor_circular,
                            strip_count,
                        )
                        .await;
                        return;
                    }

                    if is_wireless && WIRELESS_ANIMATED_MODES.contains(&mode) {
                        apply_wireless_animation(
                            &ctx,
                            &device_id,
                            mode,
                            colors,
                            speed_percent,
                            brightness_percent,
                            fps,
                            meteor_pause_secs,
                            direction,
                            invert_direction,
                            meteor_circular,
                            strip_count,
                        )
                        .await;
                        return;
                    }

                    // Wireless ignores `RgbEffect.brightness`, so bake it into the color.
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
                    ctx.bump_segment_anim(&format!("{device_id}:{target_zone}"));
                    let request = IpcRequest::SetRgbEffect {
                        device_id: device_id.clone(),
                        zone: target_zone,
                        effect: effect.clone(),
                    };
                    match ctx.client.call_unit(request).await {
                        Ok(()) => {
                            if is_wireless {
                                ctx.record_static_effect(&device_id, vec![(target_zone, effect.clone())]);
                            }
                            crate::rgb_persist::persist_rgb_effect(&ctx, &device_id, vec![(target_zone, effect)])
                                .await;
                            ctx.toast(ctx.t("rgb_editor.effect_applied"));
                        }
                        Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("rgb_editor.failed_apply"))),
                    }
                });
            }
        }
    };

    {
        let do_apply = do_apply.clone();
        apply_button.connect_clicked(move |_| do_apply(false));
    }
    if zone_count > 1 {
        apply_all_button.set_visible(true);
        apply_all_button.connect_clicked(move |_| do_apply(true));
    }
}

async fn apply_wireless_animation(
    ctx: &Rc<Ctx>,
    device_id: &str,
    mode: RgbMode,
    colors: [[u8; 3]; 8],
    speed_percent: f64,
    brightness_percent: f64,
    fps: f64,
    meteor_pause_secs: f64,
    direction: RgbDirection,
    invert_direction: bool,
    meteor_circular: bool,
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

    // Clears any stale Static entry from `AppConfig.rgb` before frames
    // start — see `rgb_persist::clear_wireless_rgb_configs`.
    crate::rgb_persist::clear_wireless_rgb_configs(ctx, std::slice::from_ref(&device_id.to_string())).await;

    let cycle_ms = percent_to_cycle_ms(speed_percent);
    let frame_count = frame_count_for(fps, cycle_ms);
    let interval_ms = fps_to_interval_ms(fps);
    let pause_frames = meteor_pause_frames(interval_ms, meteor_pause_secs);
    let reverse = crate::direction::effective_reverse(direction, invert_direction);
    let brightness = (brightness_percent / 100.0).clamp(0.0, 1.0) as f32;
    let frames = match mode {
        RgbMode::Rainbow => rainbow_frames(led_count, frame_count, reverse, strip_count, brightness),
        RgbMode::RainbowMorph => rainbow_morph_frames(led_count, frame_count, brightness),
        RgbMode::Breathing => breathing_frames(led_count, frame_count, colors[0], brightness),
        RgbMode::ColorCycle => custom_gradient_wave_frames(led_count, frame_count, &colors, reverse, strip_count, brightness),
        RgbMode::Meteor => meteor_frames(
            led_count, frame_count, pause_frames, colors[0], colors[1], false, reverse, strip_count, meteor_circular,
            brightness,
        ),
        RgbMode::MeteorShower => meteor_frames(
            led_count, frame_count, pause_frames, colors[0], colors[0], true, reverse, strip_count, meteor_circular,
            brightness,
        ),
        RgbMode::Runway => meteor_chase_frames(
            led_count, frame_count, pause_frames, colors[0], colors[1], false, reverse, strip_count, meteor_circular,
            brightness,
        ),
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
            // Not persisted to AppConfig — SetConfig would push a static
            // snapshot via RF and halt this animation. See rgb_persist.
            ctx.toast(ctx.t("rgb_editor.effect_applied"));
        }
        Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("rgb_editor.failed_apply"))),
    }
}

/// Colors the ends of every physical strip (or the wired zone, treated as a
/// Renders one side (edge or middle) as either a single solid frame or an
/// animated sequence, so both sides can be mixed independently in the
/// combined output.
#[allow(clippy::too_many_arguments)]
fn render_side_frames(
    kind: MiddleKind,
    solid_color: [u8; 3],
    mode: RgbMode,
    colors: [[u8; 3]; 8],
    brightness_percent: f64,
    led_count: usize,
    frame_count: usize,
    pause_frames: usize,
    reverse: bool,
    strip_count: usize,
    meteor_circular: bool,
) -> Vec<Vec<[u8; 3]>> {
    let brightness = (brightness_percent / 100.0).clamp(0.0, 1.0) as f32;
    match kind {
        MiddleKind::Solid => {
            let factor = (brightness_percent / 100.0).clamp(0.0, 1.0);
            vec![vec![scale_color(solid_color, factor); led_count]]
        }
        MiddleKind::Effect => match mode {
            RgbMode::Rainbow => rainbow_frames(led_count, frame_count, reverse, strip_count, brightness),
            RgbMode::RainbowMorph => rainbow_morph_frames(led_count, frame_count, brightness),
            RgbMode::Breathing => breathing_frames(led_count, frame_count, colors[0], brightness),
            RgbMode::ColorCycle => {
                custom_gradient_wave_frames(led_count, frame_count, &colors, reverse, strip_count, brightness)
            }
            RgbMode::Meteor => meteor_frames(
                led_count, frame_count, pause_frames, colors[0], colors[1], false, reverse, strip_count,
                meteor_circular, brightness,
            ),
            RgbMode::MeteorShower => meteor_frames(
                led_count, frame_count, pause_frames, colors[0], colors[0], true, reverse, strip_count,
                meteor_circular, brightness,
            ),
            RgbMode::Runway => meteor_chase_frames(
                led_count, frame_count, pause_frames, colors[0], colors[1], false, reverse, strip_count,
                meteor_circular, brightness,
            ),
            _ => vec![vec![solid_color; led_count]],
        },
    }
}

/// Combines independently-rendered edge/middle frame sequences (possibly
/// different lengths, e.g. one solid + one animated) into one sequence the
/// same length as the longer of the two.
fn combine_edge_middle_frames(
    middle_frames: &[Vec<[u8; 3]>],
    edge_frames: &[Vec<[u8; 3]>],
    edge_by_strip: bool,
    strip_count: usize,
    edge_count: usize,
    edge_start: bool,
    edge_end: bool,
) -> Vec<Vec<[u8; 3]>> {
    let total = middle_frames.len().max(edge_frames.len());
    (0..total)
        .map(|i| {
            let mut frame = middle_frames[i % middle_frames.len()].clone();
            let edge_src = &edge_frames[i % edge_frames.len()];
            if edge_by_strip {
                apply_edge_strips_frame(&mut frame, edge_src, strip_count, edge_count, edge_start, edge_end);
            } else {
                apply_edge_frame(&mut frame, edge_src, strip_count, edge_count, edge_start, edge_end);
            }
            frame
        })
        .collect()
}

/// Re-runs `apply_segments` from a saved `RgbEditorSnapshot` — used to
/// restore a Segments preset after Identify or a reconnect, since it's the
/// only way to correctly bring back an *animated* per-zone Segments effect
/// (the daemon has no storage for it — see `apply_segments` below — so
/// restoring it means re-deriving and restarting the same client-side
/// loop, not just resending one frame).
pub(crate) async fn apply_segments_from_snapshot(
    ctx: &Rc<Ctx>,
    device_id: &str,
    is_wireless: bool,
    snapshot: &crate::context::RgbEditorSnapshot,
) {
    let prefs = ctx.rgb_prefs_for(device_id);
    apply_segments(
        ctx,
        device_id,
        is_wireless,
        snapshot.zone,
        snapshot.edge_by_strip,
        snapshot.edge_count as usize,
        snapshot.edge_color,
        snapshot.edge_start,
        snapshot.edge_end,
        snapshot.edge_brightness_percent,
        snapshot.edge_kind,
        snapshot.edge_mode,
        snapshot.edge_colors,
        snapshot.middle_kind,
        snapshot.middle_color,
        snapshot.middle_mode,
        snapshot.middle_colors,
        snapshot.middle_brightness_percent,
        snapshot.speed_percent,
        snapshot.fps,
        snapshot.meteor_pause_secs,
        snapshot.direction,
        prefs.invert_direction,
        prefs.meteor_circular,
        snapshot.strip_count,
    )
    .await;
}

/// Colors each strip's (or, per-zone, the selected zone's) ends and middle
/// independently — each side can be a solid color or its own animated
/// effect. Wireless goes through `SetRgbFrames` (whole device, single-zone
/// only — see `whole_device_frames` below); everything else goes through
/// `SetRgbDirect` scoped to one zone, animated by a client-side loop when
/// needed (see `bump_segment_anim`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_segments(
    ctx: &Rc<Ctx>,
    device_id: &str,
    is_wireless: bool,
    zone: u8,
    edge_by_strip: bool,
    edge_count: usize,
    edge_color: [u8; 3],
    edge_start: bool,
    edge_end: bool,
    edge_brightness_percent: f64,
    edge_kind: MiddleKind,
    edge_mode: RgbMode,
    edge_colors: [[u8; 3]; 8],
    middle_kind: MiddleKind,
    middle_color: [u8; 3],
    middle_mode: RgbMode,
    middle_colors: [[u8; 3]; 8],
    middle_brightness_percent: f64,
    speed_percent: f64,
    fps: f64,
    meteor_pause_secs: f64,
    direction: RgbDirection,
    invert_direction: bool,
    meteor_circular: bool,
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

    // A single-zone wireless device (e.g. a Strimer cable) has no per-zone
    // addressing, so it always goes through `SetRgbFrames` for the whole
    // buffer. A multi-zone device (e.g. a fan hub, one zone per fan) applies
    // to just the selected zone via `SetRgbDirect`, static middle only.
    let whole_device_frames = is_wireless && dev_caps.zones.len() <= 1;

    if whole_device_frames {
        let led_count: usize = dev_caps.zones.iter().map(|z| z.led_count as usize).sum();
        if led_count == 0 {
            ctx.toast(ctx.t("rgb_editor.zero_leds"));
            return;
        }

        let reverse = crate::direction::effective_reverse(direction, invert_direction);
        let need_animation = middle_kind == MiddleKind::Effect || edge_kind == MiddleKind::Effect;
        let anim_interval_ms = fps_to_interval_ms(fps);
        let pause_frames = meteor_pause_frames(anim_interval_ms, meteor_pause_secs);
        let frame_count =
            if need_animation { frame_count_for(fps, percent_to_cycle_ms(speed_percent)) } else { 1 };
        let middle_frames = render_side_frames(
            middle_kind,
            middle_color,
            middle_mode,
            middle_colors,
            middle_brightness_percent,
            led_count,
            frame_count,
            pause_frames,
            reverse,
            strip_count,
            meteor_circular,
        );
        let edge_frames = render_side_frames(
            edge_kind,
            edge_color,
            edge_mode,
            edge_colors,
            edge_brightness_percent,
            led_count,
            frame_count,
            pause_frames,
            reverse,
            strip_count,
            meteor_circular,
        );
        let frames = combine_edge_middle_frames(
            &middle_frames,
            &edge_frames,
            edge_by_strip,
            strip_count,
            edge_count,
            edge_start,
            edge_end,
        );

        let interval_ms = if frames.len() > 1 { anim_interval_ms } else { 1000 };
        let request =
            IpcRequest::SetRgbFrames { device_id: device_id.to_string(), frames: frames.clone(), interval_ms };
        match ctx.client.call_unit(request).await {
            Ok(()) => {
                ctx.record_frames(device_id, frames, interval_ms, middle_mode);
                ctx.toast(ctx.t("rgb_editor.segments_applied"));
            }
            Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("rgb_editor.failed_apply"))),
        }
    } else {
        let Some(zone_info) = dev_caps.zones.get(zone as usize) else {
            ctx.toast(ctx.t("rgb_editor.caps_not_found"));
            return;
        };
        let led_count = zone_info.led_count as usize;
        if led_count == 0 {
            ctx.toast(ctx.t("rgb_editor.zero_leds"));
            return;
        }
        let key = format!("{device_id}:{zone}");
        let need_animation = middle_kind == MiddleKind::Effect || edge_kind == MiddleKind::Effect;

        if !need_animation {
            let my_gen = ctx.bump_segment_anim(&key);
            let middle_frames = render_side_frames(
                middle_kind,
                middle_color,
                middle_mode,
                middle_colors,
                middle_brightness_percent,
                led_count,
                1,
                0,
                false,
                1,
                false,
            );
            let edge_frames = render_side_frames(
                edge_kind,
                edge_color,
                edge_mode,
                edge_colors,
                edge_brightness_percent,
                led_count,
                1,
                0,
                false,
                1,
                false,
            );
            let colors = combine_edge_middle_frames(&middle_frames, &edge_frames, false, 1, edge_count, edge_start, edge_end)
                .into_iter()
                .next()
                .unwrap_or_default();

            let request = IpcRequest::SetRgbDirect { device_id: device_id.to_string(), zone, colors };
            match ctx.client.call_unit(request).await {
                Ok(()) => {
                    // Only claim the toast/gen if nothing newer pre-empted us meanwhile.
                    if ctx.segment_anim_current(&key) == my_gen {
                        ctx.toast(ctx.t("rgb_editor.segments_applied"));
                    }
                }
                Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("rgb_editor.failed_apply"))),
            }
            return;
        }

        // The daemon has no per-zone animation storage (unlike
        // `SetRgbFrames`, which is whole-device-only anyway — see
        // `whole_device_frames` above), so this zone is animated by
        // repeatedly pushing `SetRgbDirect` from the client. It keeps
        // running until this zone is re-applied (`bump_segment_anim`) or
        // the app closes.
        let reverse = crate::direction::effective_reverse(direction, invert_direction);
        let frame_count = frame_count_for(fps, percent_to_cycle_ms(speed_percent));
        let anim_interval_ms = fps_to_interval_ms(fps);
        let pause_frames = meteor_pause_frames(anim_interval_ms, meteor_pause_secs);
        let middle_frames = render_side_frames(
            middle_kind,
            middle_color,
            middle_mode,
            middle_colors,
            middle_brightness_percent,
            led_count,
            frame_count,
            pause_frames,
            reverse,
            1,
            meteor_circular,
        );
        let edge_frames = render_side_frames(
            edge_kind,
            edge_color,
            edge_mode,
            edge_colors,
            edge_brightness_percent,
            led_count,
            frame_count,
            pause_frames,
            reverse,
            1,
            meteor_circular,
        );
        let frames =
            combine_edge_middle_frames(&middle_frames, &edge_frames, edge_by_strip, 1, edge_count, edge_start, edge_end);
        let interval = std::time::Duration::from_millis(anim_interval_ms as u64);

        let my_gen = ctx.bump_segment_anim(&key);
        ctx.toast(ctx.t("rgb_editor.segments_applied"));
        let mut i = 0usize;
        loop {
            if ctx.segment_anim_current(&key) != my_gen {
                return;
            }
            let request = IpcRequest::SetRgbDirect {
                device_id: device_id.to_string(),
                zone,
                colors: frames[i % frames.len()].clone(),
            };
            if ctx.client.call_unit(request).await.is_err() {
                return;
            }
            i += 1;
            glib::timeout_future(interval).await;
        }
    }
}
