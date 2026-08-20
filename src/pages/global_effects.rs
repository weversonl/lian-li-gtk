//! Global Effects: apply one effect with shared color/speed/brightness/
//! orientation across every RGB-capable device at once.
//!
//! Only Rainbow gets the cross-device wave treatment (wireless devices as
//! segments of one virtual strip) — the other modes put every LED on every
//! device through the same color/phase, so each device just renders its
//! own copy. Wired devices with a native mode get `SetRgbEffect`; wireless
//! devices get host-rendered frames via `SetRgbFrames`, except Static,
//! which wireless reports as a real `SetRgbEffect`-capable mode.

use crate::context::Ctx;
use crate::direction::{self, wave_direction_label, WAVE_DIRECTIONS};
use crate::effects::{
    breathing_frames, custom_gradient_wave_frames, fps_to_interval_ms, frame_count_for, meteor_band_frames,
    meteor_frames, meteor_pause_frames, meteor_relay_across_devices, mode_uses_color, percent_to_brightness4,
    percent_to_cycle_ms, percent_to_speed4, rainbow_morph_frames, rainbow_wave_frames, scale_color,
    stagger_across_devices,
};
use adw::prelude::*;
use gtk::glib;
use lianli_shared::ipc::{DeviceInfo, IpcRequest};
use lianli_shared::rgb::{RgbDeviceCapabilities, RgbDirection, RgbEffect, RgbMode};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const MODES: [RgbMode; 9] = [
    RgbMode::Static,
    RgbMode::Rainbow,
    RgbMode::RainbowMorph,
    RgbMode::Breathing,
    RgbMode::ColorCycle,
    RgbMode::Meteor,
    RgbMode::MeteorShower,
    RgbMode::Runway,
    RgbMode::TailChasing,
];

/// `ColorCycle`, `MeteorShower`, `Runway` and `TailChasing` are repurposed
/// modes — wireless only, wired devices skipped (see `apply_global_effect`).
/// `Meteor`/`MeteorShower` band-cross each fan of a hub (`meteor_band_frames`)
/// or merge cable strips into one meteor (`meteor_frames`). `Runway` keeps
/// the older every-strip-in-sync look always. `TailChasing` relays the
/// band-crossing idea across whole *devices* (`meteor_relay_across_devices`).
fn mode_label(mode: RgbMode) -> &'static str {
    if mode == RgbMode::ColorCycle {
        "Gradient Wave"
    } else if mode == RgbMode::MeteorShower {
        "Meteor (Rainbow)"
    } else if mode == RgbMode::Runway {
        "Meteor (Synced)"
    } else if mode == RgbMode::TailChasing {
        "Meteor (Relay)"
    } else {
        mode.display_name()
    }
}

/// Modes where "Sincronizar Efeito" can chain devices one turn at a time
/// instead of each looping independently. Static has nothing to chain;
/// Rainbow already has its own cross-device wave; `TailChasing` already
/// is this by default.
fn supports_sync(mode: RgbMode) -> bool {
    matches!(
        mode,
        RgbMode::RainbowMorph
            | RgbMode::Breathing
            | RgbMode::ColorCycle
            | RgbMode::Meteor
            | RgbMode::MeteorShower
            | RgbMode::Runway
    )
}

const SLIDER_WIDTH: i32 = 260;
const SLIDER_VALUE_LABEL_WIDTH: i32 = 44;

/// Gap between back-to-back IPC sends — the daemon acks as soon as a
/// command is queued on the RF dongle, so no gap risks dropped commands.
const IPC_SEND_DELAY_MS: u64 = 100;

async fn ipc_send_delay() {
    glib::timeout_future(std::time::Duration::from_millis(IPC_SEND_DELAY_MS)).await;
}

pub fn page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    // A slider with marks (like FPS) or next to a short one-line title
    // (like "Brilho") can still end up allocated more or less than the
    // others, since the row hands over whatever space is left after the
    // label. Wrapping each in its own fixed-`width_request` `Box` pins
    // every slider to the same width regardless of marks or label length.
    // The value is drawn in a separate fixed-width label *outside* the
    // scale (rather than `draw_value`) so the trough itself never shrinks
    // to make room for a wider number like "100%".
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

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(640).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
    content.set_margin_top(28);
    content.set_margin_bottom(28);
    content.set_margin_start(24);
    content.set_margin_end(24);

    content.append(
        &gtk::Label::builder()
            .label(ctx.t("ge.title"))
            .css_classes(["title-1"])
            .halign(gtk::Align::Start)
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label(ctx.t("ge.description"))
            .css_classes(["dim-label"])
            .halign(gtk::Align::Start)
            .wrap(true)
            .build(),
    );

    let controls_group = adw::PreferencesGroup::new();

    let mode_names: Vec<&str> = MODES.iter().map(|m| mode_label(*m)).collect();
    let initial_mode_index = MODES.iter().position(|m| *m == ctx.global_effect_mode()).unwrap_or(1);
    let mode_index = Rc::new(Cell::new(initial_mode_index));
    // A `ComboRow`, not `segmented_control` — that widget is documented for
    // short option lists only, and 9 modes (some with long labels like
    // "Meteor (Rainbow)") overflowed the pill row wide enough that clicks
    // on the later options landed on the wrong one.
    let mode_model = gtk::StringList::new(&mode_names);
    let mode_row = adw::ComboRow::builder().title(ctx.t("ge.effect")).model(&mode_model).build();
    mode_row.set_selected(initial_mode_index as u32);

    let single_color_mode = |m: RgbMode| mode_uses_color(m) && m != RgbMode::ColorCycle;
    let color_row = adw::ActionRow::builder()
        .title(ctx.t("ge.color"))
        .subtitle(ctx.t("ge.color_subtitle"))
        .visible(single_color_mode(MODES[mode_index.get()]))
        .build();
    let saved_ge = ctx.global_effect_controls();
    let color_dialog = gtk::ColorDialog::builder().with_alpha(false).build();
    let color_button = gtk::ColorDialogButton::builder()
        .dialog(&color_dialog)
        .rgba(&gtk::gdk::RGBA::new(
            saved_ge.color[0] as f32 / 255.0,
            saved_ge.color[1] as f32 / 255.0,
            saved_ge.color[2] as f32 / 255.0,
            1.0,
        ))
        .valign(gtk::Align::Center)
        .build();
    color_row.add_suffix(&color_button);

    let meteor_tail_row = adw::ActionRow::builder()
        .title(ctx.t("ge.meteor_tail"))
        .subtitle(ctx.t("ge.meteor_tail_subtitle"))
        .visible(matches!(MODES[mode_index.get()], RgbMode::Meteor | RgbMode::Runway | RgbMode::TailChasing))
        .build();
    let meteor_tail_dialog = gtk::ColorDialog::builder().with_alpha(false).build();
    let meteor_tail_button = gtk::ColorDialogButton::builder()
        .dialog(&meteor_tail_dialog)
        .rgba(&gtk::gdk::RGBA::new(
            saved_ge.meteor_tail[0] as f32 / 255.0,
            saved_ge.meteor_tail[1] as f32 / 255.0,
            saved_ge.meteor_tail[2] as f32 / 255.0,
            1.0,
        ))
        .valign(gtk::Align::Center)
        .build();
    meteor_tail_row.add_suffix(&meteor_tail_button);

    let meteor_pause_row = adw::ActionRow::builder()
        .title(ctx.t("rgb_editor.meteor_pause"))
        .subtitle(ctx.t("rgb_editor.meteor_pause_subtitle"))
        .visible(matches!(MODES[mode_index.get()], RgbMode::Meteor | RgbMode::MeteorShower | RgbMode::Runway | RgbMode::TailChasing))
        .build();
    let meteor_pause_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 30.0, 1.0);
    meteor_pause_scale.set_value(saved_ge.meteor_pause_secs);
    meteor_pause_scale.set_hexpand(false);
    meteor_pause_row.add_suffix(&slider_wrap(&meteor_pause_scale, |v| format!("{v:.0}s")));

    let sync_devices_row = adw::ActionRow::builder()
        .title(ctx.t("ge.sync_devices"))
        .subtitle(ctx.t("ge.sync_devices_subtitle"))
        .visible(supports_sync(MODES[mode_index.get()]))
        .build();
    let sync_devices_switch =
        gtk::Switch::builder().valign(gtk::Align::Center).active(saved_ge.sync_devices).build();
    sync_devices_row.add_suffix(&sync_devices_switch);

    let gradient_colors: Rc<RefCell<[[u8; 3]; 8]>> = Rc::new(RefCell::new(ctx.gradient_colors()));
    let gradient_row = adw::ActionRow::builder()
        .title(ctx.t("rgb_editor.colors"))
        .title_lines(1)
        .visible(MODES[mode_index.get()] == RgbMode::ColorCycle)
        .build();
    let gradient_box = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .row_spacing(8)
        .column_spacing(8)
        .halign(gtk::Align::End)
        .max_children_per_line(8)
        .build();
    for i in 0..8usize {
        let dialog = gtk::ColorDialog::builder().with_alpha(false).build();
        let default = gradient_colors.borrow()[i];
        let rgba = gtk::gdk::RGBA::new(
            default[0] as f32 / 255.0,
            default[1] as f32 / 255.0,
            default[2] as f32 / 255.0,
            1.0,
        );
        let button = gtk::ColorDialogButton::builder().dialog(&dialog).rgba(&rgba).valign(gtk::Align::Center).build();
        let gradient_colors = gradient_colors.clone();
        let ctx = ctx.clone();
        button.connect_rgba_notify(move |b| {
            let rgba = b.rgba();
            gradient_colors.borrow_mut()[i] =
                [(rgba.red() * 255.0) as u8, (rgba.green() * 255.0) as u8, (rgba.blue() * 255.0) as u8];
            ctx.set_gradient_colors(*gradient_colors.borrow());
        });
        gradient_box.append(&button);
    }
    gradient_row.add_suffix(&gradient_box);

    {
        let mode_index = mode_index.clone();
        let color_row = color_row.clone();
        let gradient_row = gradient_row.clone();
        let meteor_tail_row = meteor_tail_row.clone();
        let meteor_pause_row = meteor_pause_row.clone();
        let sync_devices_row = sync_devices_row.clone();
        let ctx = ctx.clone();
        mode_row.connect_selected_notify(move |row| {
            let i = row.selected() as usize;
            let Some(mode) = MODES.get(i).copied() else { return };
            mode_index.set(i);
            ctx.set_global_effect_mode(mode);
            color_row.set_visible(single_color_mode(mode));
            gradient_row.set_visible(mode == RgbMode::ColorCycle);
            meteor_tail_row.set_visible(matches!(mode, RgbMode::Meteor | RgbMode::Runway | RgbMode::TailChasing));
            meteor_pause_row.set_visible(matches!(
                mode,
                RgbMode::Meteor | RgbMode::MeteorShower | RgbMode::Runway | RgbMode::TailChasing
            ));
            sync_devices_row.set_visible(supports_sync(mode));
        });
    }
    controls_group.add(&mode_row);
    controls_group.add(&color_row);
    controls_group.add(&meteor_tail_row);
    controls_group.add(&gradient_row);

    let speed_row = adw::ActionRow::builder().title(ctx.t("ge.speed")).build();
    let speed_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    speed_scale.set_value(saved_ge.speed_percent);
    speed_scale.set_hexpand(false);
    speed_row.add_suffix(&slider_wrap(&speed_scale, |v| format!("{v:.0}%")));
    controls_group.add(&speed_row);

    controls_group.add(&sync_devices_row);

    let fps_row = adw::ActionRow::builder()
        .title(ctx.t("ge.fps"))
        .subtitle(ctx.t("ge.fps_subtitle"))
        .build();
    let fps_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 5.0, 60.0, 1.0);
    fps_scale.set_value(saved_ge.fps);
    fps_scale.set_hexpand(false);
    fps_scale.add_mark(15.0, gtk::PositionType::Bottom, Some("15"));
    fps_scale.add_mark(30.0, gtk::PositionType::Bottom, Some("30"));
    fps_scale.add_mark(60.0, gtk::PositionType::Bottom, Some("60"));
    fps_row.add_suffix(&slider_wrap(&fps_scale, |v| format!("{v:.0}")));

    let range_row = adw::ActionRow::builder().title(ctx.t("ge.over_60fps")).build();
    let range_switch = gtk::Switch::builder().valign(gtk::Align::Center).active(false).build();
    {
        let fps_scale = fps_scale.clone();
        range_switch.connect_state_set(move |_, extended| {
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
    range_row.add_suffix(&range_switch);
    controls_group.add(&range_row);
    controls_group.add(&fps_row);

    let brightness_row = adw::ActionRow::builder().title(ctx.t("ge.brightness")).build();
    let brightness_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    brightness_scale.set_value(saved_ge.brightness_percent);
    brightness_scale.set_hexpand(false);
    brightness_row.add_suffix(&slider_wrap(&brightness_scale, |v| format!("{v:.0}%")));
    controls_group.add(&brightness_row);
    controls_group.add(&meteor_pause_row);

    let direction_row = adw::ActionRow::builder()
        .title(ctx.t("ge.global_orientation"))
        .subtitle(ctx.t("ge.global_orientation_subtitle"))
        .title_lines(1)
        .build();
    let direction_names: Vec<&str> = WAVE_DIRECTIONS.iter().map(|d| wave_direction_label(*d, ctx.lang())).collect();
    let global_direction = Rc::new(Cell::new(RgbDirection::Up));
    let direction_model = gtk::StringList::new(&direction_names);
    let direction_control = adw::ComboRow::builder().model(&direction_model).build();
    direction_control.set_selected(0);
    direction_control.connect_selected_notify({
        let global_direction = global_direction.clone();
        move |row| {
            if let Some(d) = WAVE_DIRECTIONS.get(row.selected() as usize) {
                global_direction.set(*d);
            }
        }
    });
    let direction_enabled = ctx.global_direction_enabled();
    let direction_enabled_switch =
        gtk::Switch::builder().valign(gtk::Align::Center).active(direction_enabled).build();
    direction_control.set_sensitive(direction_enabled);
    direction_row.add_suffix(&direction_enabled_switch);
    controls_group.add(&direction_row);

    let direction_value_row = direction_control.clone();
    direction_value_row.set_title(ctx.t("ge.direction"));
    controls_group.add(&direction_value_row);

    {
        let direction_control = direction_control.clone();
        let ctx = ctx.clone();
        direction_enabled_switch.connect_state_set(move |_, enabled| {
            direction_control.set_sensitive(enabled);
            ctx.set_global_direction_enabled(enabled);
            glib::Propagation::Proceed
        });
    }

    content.append(&controls_group);

    let devices_group = adw::PreferencesGroup::builder()
        .title(ctx.t("ge.included_devices"))
        .description(ctx.t("ge.included_devices_desc"))
        .build();
    let devices_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    devices_group.add(&devices_list);
    content.append(&devices_group);

    let rgb_devices: Vec<DeviceInfo> =
        ctx.state.borrow().devices.iter().filter(|d| d.has_rgb).cloned().collect();
    let ordered_devices: Rc<RefCell<Vec<DeviceInfo>>> =
        Rc::new(RefCell::new(ctx.sort_by_saved_order(rgb_devices)));
    let device_directions: Rc<RefCell<HashMap<String, RgbDirection>>> = Rc::new(RefCell::new(HashMap::new()));
    let device_segments: Rc<RefCell<HashMap<String, usize>>> = Rc::new(RefCell::new(HashMap::new()));

    render_device_order(
        &devices_list,
        &ordered_devices,
        &device_directions,
        &device_segments,
        &global_direction,
        ctx,
    );

    let apply_button = gtk::Button::builder()
        .label(ctx.t("ge.apply_all"))
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&apply_button);

    {
        let ctx = ctx.clone();
        let ordered_devices = ordered_devices.clone();
        let device_directions = device_directions.clone();
        let device_segments = device_segments.clone();
        let mode_index = mode_index.clone();
        let global_direction = global_direction.clone();
        let gradient_colors = gradient_colors.clone();
        let sync_devices_switch = sync_devices_switch.clone();
        let apply_button_for_reenable = apply_button.clone();
        apply_button.connect_clicked(move |_| {
            let ctx = ctx.clone();
            let devices = ordered_devices.borrow().clone();
            let directions = device_directions.borrow().clone();
            let segments = device_segments.borrow().clone();
            let mode = MODES[mode_index.get()];
            let rgba = color_button.rgba();
            let color = [
                (rgba.red() * 255.0) as u8,
                (rgba.green() * 255.0) as u8,
                (rgba.blue() * 255.0) as u8,
            ];
            let tail_rgba = meteor_tail_button.rgba();
            let meteor_tail = [
                (tail_rgba.red() * 255.0) as u8,
                (tail_rgba.green() * 255.0) as u8,
                (tail_rgba.blue() * 255.0) as u8,
            ];
            let gradient = *gradient_colors.borrow();
            let speed_percent = speed_scale.value();
            let fps = fps_scale.value();
            let brightness_percent = brightness_scale.value();
            let meteor_pause_secs = meteor_pause_scale.value();
            let sync_devices_value = sync_devices_switch.is_active();
            let global_dir = global_direction.get();
            ctx.set_global_effect_controls(crate::context::GlobalEffectControls {
                color,
                meteor_tail,
                speed_percent,
                fps,
                brightness_percent,
                meteor_pause_secs,
                sync_devices: sync_devices_value,
            });
            let apply_button = apply_button_for_reenable.clone();
            apply_button.set_sensitive(false);
            glib::spawn_future_local(async move {
                apply_global_effect(
                    &ctx,
                    &devices,
                    mode,
                    color,
                    meteor_tail,
                    meteor_pause_secs,
                    sync_devices_value,
                    gradient,
                    speed_percent,
                    fps,
                    brightness_percent,
                    global_dir,
                    &directions,
                    &segments,
                )
                .await;
                // Re-enable only once the transfer actually lands, so a
                // second click can't fire an overlapping one mid-transfer.
                apply_button.set_sensitive(true);
            });
        });
    }

    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    toolbar.set_content(Some(&scrolled));

    adw::NavigationPage::builder()
        .title(ctx.t("ge.title"))
        .tag("global-effects")
        .child(&toolbar)
        .build()
}

fn render_device_order(
    list_box: &gtk::ListBox,
    ordered_devices: &Rc<RefCell<Vec<DeviceInfo>>>,
    device_directions: &Rc<RefCell<HashMap<String, RgbDirection>>>,
    device_segments: &Rc<RefCell<HashMap<String, usize>>>,
    global_direction: &Rc<Cell<RgbDirection>>,
    ctx: &Rc<Ctx>,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let devices = ordered_devices.borrow().clone();
    if devices.is_empty() {
        list_box.append(&adw::ActionRow::builder().title(ctx.t("ge.no_devices")).build());
        return;
    }

    let direction_names: Vec<&str> = WAVE_DIRECTIONS.iter().map(|d| wave_direction_label(*d, ctx.lang())).collect();
    let count = devices.len();
    for (i, device) in devices.iter().enumerate() {
        let row = adw::ExpanderRow::builder()
            .title(glib::markup_escape_text(&ctx.display_name(device)))
            .title_lines(1)
            .build();
        let index_label = gtk::Label::builder()
            .label((i + 1).to_string())
            .css_classes(["dim-label"])
            .build();
        row.add_prefix(&index_label);

        let up_button = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .sensitive(i > 0)
            .build();
        let down_button = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .sensitive(i + 1 < count)
            .build();

        {
            let ordered_devices = ordered_devices.clone();
            let list_box = list_box.clone();
            let ctx = ctx.clone();
            let device_directions = device_directions.clone();
            let device_segments = device_segments.clone();
            let global_direction = global_direction.clone();
            up_button.connect_clicked(move |_| {
                ordered_devices.borrow_mut().swap(i, i - 1);
                ctx.set_device_order(ordered_devices.borrow().iter().map(|d| d.device_id.clone()).collect());
                render_device_order(
                    &list_box,
                    &ordered_devices,
                    &device_directions,
                    &device_segments,
                    &global_direction,
                    &ctx,
                );
            });
        }
        {
            let ordered_devices = ordered_devices.clone();
            let list_box = list_box.clone();
            let ctx = ctx.clone();
            let device_directions = device_directions.clone();
            let device_segments = device_segments.clone();
            let global_direction = global_direction.clone();
            down_button.connect_clicked(move |_| {
                ordered_devices.borrow_mut().swap(i, i + 1);
                ctx.set_device_order(ordered_devices.borrow().iter().map(|d| d.device_id.clone()).collect());
                render_device_order(
                    &list_box,
                    &ordered_devices,
                    &device_directions,
                    &device_segments,
                    &global_direction,
                    &ctx,
                );
            });
        }

        row.add_suffix(&up_button);
        row.add_suffix(&down_button);

        // Priority: touched this session, then saved prefs, then global default.
        let saved_prefs = ctx.rgb_prefs_for_opt(&device.device_id);
        let current_dir = device_directions
            .borrow()
            .get(&device.device_id)
            .copied()
            .or_else(|| saved_prefs.map(|p| p.direction))
            .unwrap_or_else(|| global_direction.get());
        let selected_index = WAVE_DIRECTIONS.iter().position(|d| *d == current_dir).unwrap_or(0);
        // Recorded immediately, not just on touch — Apply to All Devices
        // reads this map and must not see it empty for an untouched device.
        device_directions.borrow_mut().insert(device.device_id.clone(), current_dir);

        let direction_model2 = gtk::StringList::new(&direction_names);
        let direction_row2 =
            adw::ComboRow::builder().title(ctx.t("ge.direction")).model(&direction_model2).build();
        direction_row2.set_selected(selected_index as u32);
        // Physical LED strips concatenated in this device's flat buffer.
        // `1` treats it as one continuous strip.
        let current_strip_count = device_segments
            .borrow()
            .get(&device.device_id)
            .copied()
            .or_else(|| saved_prefs.map(|p| p.strip_count))
            .unwrap_or(1);
        device_segments.borrow_mut().insert(device.device_id.clone(), current_strip_count);

        {
            let device_directions = device_directions.clone();
            let device_segments = device_segments.clone();
            let device_id = device.device_id.clone();
            let ctx = ctx.clone();
            direction_row2.connect_selected_notify(move |row| {
                if let Some(d) = WAVE_DIRECTIONS.get(row.selected() as usize) {
                    device_directions.borrow_mut().insert(device_id.clone(), *d);
                    let strips = device_segments.borrow().get(&device_id).copied().unwrap_or(current_strip_count);
                    let prefs = ctx.rgb_prefs_for(&device_id);
                    ctx.set_rgb_prefs(&device_id, *d, strips, prefs.invert_direction, prefs.meteor_circular);
                }
            });
        }
        row.add_row(&direction_row2);

        let merge_row = adw::ActionRow::builder().title(ctx.t("ge.led_strips")).build();
        let merge_adj = gtk::Adjustment::new(current_strip_count as f64, 1.0, 32.0, 1.0, 1.0, 0.0);
        let merge_spin = gtk::SpinButton::new(Some(&merge_adj), 1.0, 0);
        merge_spin.set_valign(gtk::Align::Center);
        {
            let device_directions = device_directions.clone();
            let device_segments = device_segments.clone();
            let device_id = device.device_id.clone();
            let ctx = ctx.clone();
            let global_direction = global_direction.clone();
            merge_adj.connect_value_changed(move |adj| {
                let strips = adj.value() as usize;
                device_segments.borrow_mut().insert(device_id.clone(), strips);
                let dir = device_directions.borrow().get(&device_id).copied().unwrap_or_else(|| global_direction.get());
                let prefs = ctx.rgb_prefs_for(&device_id);
                ctx.set_rgb_prefs(&device_id, dir, strips, prefs.invert_direction, prefs.meteor_circular);
            });
        }
        merge_row.add_suffix(&merge_spin);
        row.add_row(&merge_row);

        if device.has_fan {
            // Fans of the identical model can be screwed into a hub at a
            // different physical rotation — this calibrates where local LED
            // index 0 really sits (0 = 12 o'clock), so the Meteor
            // band-crossing effect reads its horizontal position correctly.
            // Only meaningful for fan rings, not cables like a Strimer.
            let ring_row = adw::ActionRow::builder().title(ctx.t("ge.ring_offset")).build();
            let current_offset = saved_prefs.map(|p| p.ring_offset_deg).unwrap_or(0.0);
            let ring_adj = gtk::Adjustment::new(current_offset, -180.0, 180.0, 15.0, 15.0, 0.0);
            let ring_spin = gtk::SpinButton::new(Some(&ring_adj), 1.0, 0);
            ring_spin.set_valign(gtk::Align::Center);
            {
                let device_id = device.device_id.clone();
                let ctx = ctx.clone();
                ring_adj.connect_value_changed(move |adj| {
                    ctx.set_ring_offset_deg(&device_id, adj.value());
                });
            }
            ring_row.add_suffix(&ring_spin);
            row.add_row(&ring_row);
        }

        list_box.append(&row);
    }
}

async fn apply_global_effect(
    ctx: &Rc<Ctx>,
    devices: &[DeviceInfo],
    mode: RgbMode,
    color: [u8; 3],
    meteor_tail: [u8; 3],
    meteor_pause_secs: f64,
    sync_devices: bool,
    gradient_colors: [[u8; 3]; 8],
    speed_percent: f64,
    fps: f64,
    brightness_percent: f64,
    global_direction: RgbDirection,
    device_directions: &HashMap<String, RgbDirection>,
    device_segments: &HashMap<String, usize>,
) {
    let caps = match ctx
        .client
        .call::<Vec<RgbDeviceCapabilities>>(IpcRequest::GetRgbCapabilities)
        .await
    {
        Ok(caps) => caps,
        Err(e) => {
            ctx.toast(&format!("{}: {e}", ctx.t("ge.failed_load_caps")));
            return;
        }
    };

    let cycle_ms = percent_to_cycle_ms(speed_percent);
    let frame_count = frame_count_for(fps, cycle_ms);
    let interval_ms = fps_to_interval_ms(fps);
    let pause_frames = meteor_pause_frames(interval_ms, meteor_pause_secs);
    let wired_speed = percent_to_speed4(speed_percent);
    let wired_brightness = percent_to_brightness4(brightness_percent);
    let brightness_factor = (brightness_percent / 100.0).clamp(0.0, 1.0) as f32;
    let resolve_direction = |device_id: &str| -> RgbDirection {
        device_directions.get(device_id).copied().unwrap_or(global_direction)
    };
    // Per-device wiring-order fix set in the RGB Editor page (see
    // `DeviceRgbPrefs::invert_direction`) — applies here too, so it doesn't
    // need to be set separately per surface.
    let resolve_reverse =
        |device_id: &str| -> bool { direction::effective_reverse(resolve_direction(device_id), ctx.rgb_prefs_for(device_id).invert_direction) };
    // Per-device Meteor ring-vs-strip topology, same source as above.
    let resolve_circular = |device_id: &str| -> bool { ctx.rgb_prefs_for(device_id).meteor_circular };
    // Per-device fan-ring mount rotation, same source as above — see
    // `DeviceRgbPrefs::ring_offset_deg`.
    let resolve_ring_offset = |device_id: &str| -> f32 { ctx.rgb_prefs_for(device_id).ring_offset_deg as f32 };

    let wireless: Vec<&DeviceInfo> = devices.iter().filter(|d| d.device_id.starts_with("wireless:")).collect();
    let wired: Vec<&DeviceInfo> = devices.iter().filter(|d| !d.device_id.starts_with("wireless:")).collect();

    // Any stale `AppConfig.rgb` entry left over from an earlier Static
    // apply must go before an animated mode starts — otherwise the
    // daemon's own idle-watchdog auto-resync (see `rgb_persist`) keeps
    // reapplying that old static color via RF over these frames whenever
    // the device's firmware hiccups (the "flickers back to the old
    // color" symptom this exists to prevent).
    if mode != RgbMode::Static {
        let ids: Vec<String> = wireless.iter().map(|d| d.device_id.clone()).collect();
        crate::rgb_persist::clear_wireless_rgb_configs(ctx, &ids).await;
    }

    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut to_persist: Vec<(String, Vec<(u8, RgbEffect)>)> = Vec::new();

    // ColorCycle (Gradient Wave), MeteorShower (rainbow-headed Meteor),
    // Runway (strip-by-strip Meteor relay) and TailChasing (device-by-device
    // Meteor relay) are wireless-only repurposed modes, so wired devices
    // skip all four.
    for device in &wired {
        if matches!(mode, RgbMode::ColorCycle | RgbMode::MeteorShower | RgbMode::Runway | RgbMode::TailChasing) {
            continue;
        }
        // The daemon names ENE6K77 wired ports differently across endpoints
        // (`hid:<serial>:portN` from ListDevices, `hid:<serial>:groupN` from
        // GetRgbCapabilities) — see `rgb_caps_id_matches` in rgb_editor.rs.
        // `SetRgbEffect` needs the capabilities one, not `device.device_id`.
        let Some(dev_caps) =
            caps.iter().find(|c| crate::pages::rgb_editor::rgb_caps_id_matches(&c.device_id, &device.device_id))
        else {
            failed += 1;
            continue;
        };
        let rgb_device_id = dev_caps.device_id.clone();
        let zone_count = dev_caps.zones.len().max(1);
        let effect = RgbEffect {
            mode,
            colors: vec![color],
            speed: wired_speed,
            brightness: wired_brightness,
            direction: resolve_direction(&device.device_id),
            scope: Default::default(),
            disabled: false,
        };
        let mut device_ok = true;
        for zone in 0..zone_count as u8 {
            let request = IpcRequest::SetRgbEffect {
                device_id: rgb_device_id.clone(),
                zone,
                effect: effect.clone(),
            };
            if ctx.client.call_unit(request).await.is_err() {
                device_ok = false;
            }
            ipc_send_delay().await;
        }
        if device_ok {
            ok += 1;
            let zone_effects: Vec<(u8, RgbEffect)> =
                (0..zone_count as u8).map(|z| (z, effect.clone())).collect();
            to_persist.push((device.device_id.clone(), zone_effects));
        } else {
            failed += 1;
        }
    }

    match mode {
        RgbMode::Static => {
            let wireless_color = scale_color(color, brightness_percent / 100.0);
            for device in &wireless {
                let zone_count = caps
                    .iter()
                    .find(|c| c.device_id == device.device_id)
                    .map(|c| c.zones.len().max(1))
                    .unwrap_or(1);
                let effect = RgbEffect {
                    mode: RgbMode::Static,
                    colors: vec![wireless_color],
                    speed: wired_speed,
                    brightness: wired_brightness,
                    direction: resolve_direction(&device.device_id),
                    scope: Default::default(),
                    disabled: false,
                };
                let mut device_ok = true;
                for zone in 0..zone_count as u8 {
                    let request = IpcRequest::SetRgbEffect {
                        device_id: device.device_id.clone(),
                        zone,
                        effect: effect.clone(),
                    };
                    if ctx.client.call_unit(request).await.is_err() {
                        device_ok = false;
                    }
                    ipc_send_delay().await;
                }
                if device_ok {
                    let zone_effects: Vec<(u8, RgbEffect)> =
                        (0..zone_count as u8).map(|z| (z, effect.clone())).collect();
                    ctx.record_static_effect(&device.device_id, zone_effects.clone());
                    to_persist.push((device.device_id.clone(), zone_effects));
                    ok += 1;
                } else {
                    failed += 1;
                }
            }
        }
        RgbMode::Rainbow => {
            // Every wireless device is a segment of one shared gradient.
            let led_counts: Vec<usize> = wireless
                .iter()
                .map(|d| {
                    caps.iter()
                        .find(|c| c.device_id == d.device_id)
                        .map(|c| c.zones.iter().map(|z| z.led_count as usize).sum())
                        .unwrap_or(0)
                })
                .collect();
            let device_reversed: Vec<bool> = wireless
                .iter()
                .map(|d| resolve_reverse(&d.device_id))
                .collect();
            let device_seg_counts: Vec<usize> = wireless
                .iter()
                .map(|d| device_segments.get(&d.device_id).copied().unwrap_or(1))
                .collect();
            let wave_frames = rainbow_wave_frames(
                &led_counts,
                frame_count,
                false,
                &device_reversed,
                &device_seg_counts,
                brightness_factor,
            );
            for (device, frames) in wireless.iter().zip(wave_frames.into_iter()) {
                if frames.is_empty() {
                    failed += 1;
                    continue;
                }
                let request = IpcRequest::SetRgbFrames {
                    device_id: device.device_id.clone(),
                    frames: frames.clone(),
                    interval_ms,
                };
                // Not persisted to AppConfig — see rgb_editor.rs's comment
                // on why SetConfig would freeze this animation.
                match ctx.client.call_unit(request).await {
                    Ok(()) => {
                        ctx.record_frames(&device.device_id, frames, interval_ms, mode);
                        ok += 1;
                    }
                    Err(_) => failed += 1,
                }
                ipc_send_delay().await;
            }
        }
        RgbMode::TailChasing => {
            // Same "one virtual strip across every device" idea as
            // Rainbow's wave, but relayed device-by-device instead of
            // blended into one continuous gradient — see
            // `meteor_relay_across_devices`.
            let led_counts: Vec<usize> = wireless
                .iter()
                .map(|d| {
                    caps.iter()
                        .find(|c| c.device_id == d.device_id)
                        .map(|c| c.zones.iter().map(|z| z.led_count as usize).sum())
                        .unwrap_or(0)
                })
                .collect();
            let device_reversed: Vec<bool> = wireless.iter().map(|d| resolve_reverse(&d.device_id)).collect();
            let device_seg_counts: Vec<usize> = wireless
                .iter()
                .map(|d| device_segments.get(&d.device_id).copied().unwrap_or(1))
                .collect();
            let device_circular: Vec<bool> = wireless.iter().map(|d| resolve_circular(&d.device_id)).collect();
            let device_chase: Vec<bool> = wireless.iter().map(|d| d.has_fan).collect();
            // Real per-fan zone sizes, truncated to the user's configured
            // fan count — a hub's `caps.zones` can include ports with no
            // fan actually wired in (see `meteor_band_frames`).
            let device_zone_led_counts: Vec<Vec<usize>> = wireless
                .iter()
                .zip(device_seg_counts.iter())
                .map(|(d, &strip_count)| {
                    caps.iter()
                        .find(|c| c.device_id == d.device_id)
                        .map(|c| c.zones.iter().take(strip_count).map(|z| z.led_count as usize).collect())
                        .unwrap_or_default()
                })
                .collect();
            let device_ring_offset_deg: Vec<f32> =
                wireless.iter().map(|d| resolve_ring_offset(&d.device_id)).collect();
            let relay_frames = meteor_relay_across_devices(
                &led_counts,
                &device_zone_led_counts,
                frame_count,
                pause_frames,
                color,
                meteor_tail,
                false,
                &device_reversed,
                &device_seg_counts,
                &device_circular,
                &device_chase,
                &device_ring_offset_deg,
                brightness_factor,
            );
            for (device, frames) in wireless.iter().zip(relay_frames.into_iter()) {
                if frames.is_empty() {
                    failed += 1;
                    continue;
                }
                let request = IpcRequest::SetRgbFrames {
                    device_id: device.device_id.clone(),
                    frames: frames.clone(),
                    interval_ms,
                };
                match ctx.client.call_unit(request).await {
                    Ok(()) => {
                        ctx.record_frames(&device.device_id, frames, interval_ms, mode);
                        ok += 1;
                    }
                    Err(_) => failed += 1,
                }
                ipc_send_delay().await;
            }
        }
        RgbMode::RainbowMorph
        | RgbMode::Breathing
        | RgbMode::ColorCycle
        | RgbMode::Meteor
        | RgbMode::MeteorShower
        | RgbMode::Runway => {
            let mut valid_devices: Vec<&DeviceInfo> = Vec::new();
            let mut led_counts: Vec<usize> = Vec::new();
            for device in &wireless {
                let led_count: usize = caps
                    .iter()
                    .find(|c| c.device_id == device.device_id)
                    .map(|c| c.zones.iter().map(|z| z.led_count as usize).sum())
                    .unwrap_or(0);
                if led_count == 0 {
                    failed += 1;
                    continue;
                }
                valid_devices.push(device);
                led_counts.push(led_count);
            }

            // With "Sincronizar Efeito" on, each device's own turn plays
            // with no pause of its own — the single pause (`pause_frames`)
            // happens once, after the whole relay, via
            // `stagger_across_devices` below.
            let own_pause = if sync_devices { 0 } else { pause_frames };
            let own_turns: Vec<Vec<Vec<[u8; 3]>>> = valid_devices
                .iter()
                .zip(led_counts.iter())
                .map(|(device, &led_count)| match mode {
                    RgbMode::RainbowMorph => rainbow_morph_frames(led_count, frame_count, brightness_factor),
                    RgbMode::ColorCycle => custom_gradient_wave_frames(
                        led_count,
                        frame_count,
                        &gradient_colors,
                        resolve_reverse(&device.device_id),
                        device_segments.get(&device.device_id).copied().unwrap_or(1),
                        brightness_factor,
                    ),
                    RgbMode::Meteor => {
                        let strip_count = device_segments.get(&device.device_id).copied().unwrap_or(1);
                        if device.has_fan {
                            let zones: Vec<usize> = caps
                                .iter()
                                .find(|c| c.device_id == device.device_id)
                                .map(|c| c.zones.iter().take(strip_count).map(|z| z.led_count as usize).collect())
                                .unwrap_or_default();
                            meteor_band_frames(
                                led_count,
                                &zones,
                                resolve_ring_offset(&device.device_id),
                                frame_count,
                                own_pause,
                                color,
                                meteor_tail,
                                resolve_reverse(&device.device_id),
                                brightness_factor,
                            )
                        } else {
                            meteor_frames(
                                led_count,
                                frame_count,
                                own_pause,
                                color,
                                meteor_tail,
                                false,
                                resolve_reverse(&device.device_id),
                                strip_count,
                                resolve_circular(&device.device_id),
                                brightness_factor,
                            )
                        }
                    }
                    // Only one swatch (`color_row`) is shown for this mode —
                    // it's the tail here, since the head cycles the hue
                    // wheel on its own (see `meteor_tail_row`'s visibility).
                    RgbMode::MeteorShower => meteor_frames(
                        led_count,
                        frame_count,
                        own_pause,
                        color,
                        color,
                        true,
                        resolve_reverse(&device.device_id),
                        device_segments.get(&device.device_id).copied().unwrap_or(1),
                        resolve_circular(&device.device_id),
                        brightness_factor,
                    ),
                    RgbMode::Runway => meteor_frames(
                        led_count,
                        frame_count,
                        own_pause,
                        color,
                        meteor_tail,
                        false,
                        resolve_reverse(&device.device_id),
                        device_segments.get(&device.device_id).copied().unwrap_or(1),
                        resolve_circular(&device.device_id),
                        brightness_factor,
                    ),
                    _ => breathing_frames(led_count, frame_count, color, brightness_factor),
                })
                .collect();

            let per_device_frames = if sync_devices {
                stagger_across_devices(&own_turns, &led_counts, pause_frames, meteor_tail)
            } else {
                own_turns
            };

            for (device, frames) in valid_devices.iter().zip(per_device_frames.into_iter()) {
                let request = IpcRequest::SetRgbFrames {
                    device_id: device.device_id.clone(),
                    frames: frames.clone(),
                    interval_ms,
                };
                match ctx.client.call_unit(request).await {
                    Ok(()) => {
                        ctx.record_frames(&device.device_id, frames, interval_ms, mode);
                        ok += 1;
                    }
                    Err(_) => failed += 1,
                }
                ipc_send_delay().await;
            }
        }
        _ => {}
    }

    crate::rgb_persist::persist_rgb_effects(ctx, to_persist).await;

    if failed == 0 {
        ctx.toast(&format!(
            "{} {} {ok} {}",
            mode.display_name(),
            ctx.t("ge.applied_to"),
            ctx.t("ge.devices_suffix")
        ));
    } else {
        ctx.toast(&format!(
            "{} {} {ok} {}, {failed} {}",
            mode.display_name(),
            ctx.t("ge.applied_to"),
            ctx.t("ge.devices_suffix"),
            ctx.t("ge.failed_suffix")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_label_repurposed_modes_use_wireless_names() {
        assert_eq!(mode_label(RgbMode::ColorCycle), "Gradient Wave");
        assert_eq!(mode_label(RgbMode::MeteorShower), "Meteor (Rainbow)");
        assert_eq!(mode_label(RgbMode::Runway), "Meteor (Synced)");
        assert_eq!(mode_label(RgbMode::TailChasing), "Meteor (Relay)");
    }

    #[test]
    fn mode_label_other_modes_fall_back_to_display_name() {
        assert_eq!(mode_label(RgbMode::Static), RgbMode::Static.display_name());
        assert_eq!(mode_label(RgbMode::Rainbow), RgbMode::Rainbow.display_name());
    }

    #[test]
    fn supports_sync_matches_documented_modes() {
        for &mode in MODES.iter() {
            let expected = matches!(
                mode,
                RgbMode::RainbowMorph
                    | RgbMode::Breathing
                    | RgbMode::ColorCycle
                    | RgbMode::Meteor
                    | RgbMode::MeteorShower
                    | RgbMode::Runway
            );
            assert_eq!(supports_sync(mode), expected, "mismatch for {mode:?}");
        }
    }

    #[test]
    fn supports_sync_excludes_static_rainbow_and_tail_chasing() {
        assert!(!supports_sync(RgbMode::Static));
        assert!(!supports_sync(RgbMode::Rainbow));
        assert!(!supports_sync(RgbMode::TailChasing));
    }
}
