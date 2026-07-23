//! Global Effects: apply one effect (Static, Rainbow, Rainbow Morph,
//! Breathing) with shared color/speed/brightness/orientation across every
//! RGB-capable device at once — the L-Connect 3 "apply to all" tab.
//!
//! Only Rainbow gets the cross-device wave treatment (wireless devices
//! treated as segments of one virtual strip, in the order shown below):
//! that's the only one of these that's spatial. Static/Breathing/Rainbow
//! Morph put every LED on every device through the *same* color/phase, so
//! there's nothing to slice — each device just renders its own copy.
//!
//! Wired devices with a native hardware mode get `SetRgbEffect` (the
//! firmware animates it); wireless devices get host-rendered frames via
//! `SetRgbFrames`, except Static, which wireless reports as a real
//! `SetRgbEffect`-capable mode (confirmed via `GetRgbCapabilities`) so it
//! goes out the same way as wired.

use crate::context::Ctx;
use crate::direction::{self, wave_direction_label, WAVE_DIRECTIONS};
use crate::effects::{
    breathing_frames, custom_gradient_wave_frames, fps_to_interval_ms, frame_count_for, mode_uses_color,
    percent_to_brightness4, percent_to_cycle_ms, percent_to_speed4, rainbow_morph_frames, rainbow_wave_frames,
    scale_color,
};
use crate::widgets::segmented_control;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::ipc::{DeviceInfo, IpcRequest};
use lianli_shared::rgb::{RgbDeviceCapabilities, RgbDirection, RgbEffect, RgbMode};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const MODES: [RgbMode; 5] =
    [RgbMode::Static, RgbMode::Rainbow, RgbMode::RainbowMorph, RgbMode::Breathing, RgbMode::ColorCycle];

/// `ColorCycle` is repurposed here exactly like in `rgb_editor` — see that
/// module's doc comment. Wireless-only: there's no wired firmware
/// equivalent, so wired devices are just skipped when this mode is picked
/// (see `apply_global_effect`).
fn mode_label(mode: RgbMode) -> &'static str {
    if mode == RgbMode::ColorCycle {
        "Gradient Wave"
    } else {
        mode.display_name()
    }
}

/// Fixed width for every slider on this page — left as `hexpand`, each one
/// ended up a different width depending on its row's other content (e.g.
/// the FPS row's longer subtitle left less room than Speed's), which read
/// as visually inconsistent even though nothing was actually wrong with any
/// one of them individually.
const SLIDER_WIDTH: i32 = 220;

/// Breathing room between back-to-back IPC sends to different wireless
/// devices/zones. The daemon returns `ok` as soon as it queues a command on
/// the RF dongle, not once the device firmware confirms receipt — firing
/// several sends with no gap risked the dongle dropping/colliding one of
/// them, which is why "Apply to All Devices" would silently skip whichever
/// device/zone landed last in the sequence.
const IPC_SEND_DELAY_MS: u64 = 100;

async fn ipc_send_delay() {
    glib::timeout_future(std::time::Duration::from_millis(IPC_SEND_DELAY_MS)).await;
}

pub fn page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(640).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
    content.set_margin_top(28);
    content.set_margin_bottom(28);
    content.set_margin_start(24);
    content.set_margin_end(24);

    // Plain heading, no card — matches the reference app's device-detail
    // style (bold title + dim description directly below, no boxed banner).
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
    // Remembers whichever mode was last applied here (see
    // `Ctx::global_effect_mode`'s doc comment) — without this, leaving and
    // reopening this page always reset the picker to Rainbow no matter what
    // had actually been applied.
    let initial_mode_index = MODES.iter().position(|m| *m == ctx.global_effect_mode()).unwrap_or(1);
    let mode_index = Rc::new(Cell::new(initial_mode_index));
    // `title_lines(1)` — without it, a narrow window squeezes this row's
    // title column down to almost nothing (the 5-button segmented control
    // suffix doesn't shrink), and "Efeito"/"Effect" wraps one character per
    // line instead of just eliding with "…".
    let mode_row = adw::ActionRow::builder().title(ctx.t("ge.effect")).title_lines(1).build();

    // Rainbow/Rainbow Morph auto-cycle the full hue wheel on their own — any
    // color picked here is ignored for those, so the row (and its "Used by
    // Static and Breathing" hint) only makes sense to show for the modes
    // that actually use a single color — Gradient Wave has its own 8-swatch
    // row below instead.
    let single_color_mode = |m: RgbMode| mode_uses_color(m) && m != RgbMode::ColorCycle;
    let color_row = adw::ActionRow::builder()
        .title(ctx.t("ge.color"))
        .subtitle(ctx.t("ge.color_subtitle"))
        .visible(single_color_mode(MODES[mode_index.get()]))
        .build();
    let color_dialog = gtk::ColorDialog::builder().with_alpha(false).build();
    let color_button = gtk::ColorDialogButton::builder()
        .dialog(&color_dialog)
        .rgba(&gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0))
        .valign(gtk::Align::Center)
        .build();
    color_row.add_suffix(&color_button);

    // Custom Gradient Wave's gradient stops — same swatch pattern as the RGB
    // Editor's `colors_box`, reused here so both pages build the same kind
    // of `RgbEffect.colors` payload for the repurposed `ColorCycle` tag.
    // Seeded from `ctx.gradient_colors()` (persisted client-side, see
    // `Ctx::gradient_colors`'s doc comment) instead of 8 identical white
    // swatches, and every pick is saved back immediately so reopening this
    // page doesn't lose whatever gradient was last set up here.
    let gradient_colors: Rc<RefCell<[[u8; 3]; 8]>> = Rc::new(RefCell::new(ctx.gradient_colors()));
    let gradient_row = adw::ActionRow::builder()
        .title(ctx.t("rgb_editor.colors"))
        .title_lines(1)
        .visible(MODES[mode_index.get()] == RgbMode::ColorCycle)
        .build();
    // `FlowBox`, not a plain `Box` — a plain `Box` overflowed past the
    // card's edge on a narrow window instead of wrapping (same fix as the
    // RGB Editor's `colors_box`). `max_children_per_line` has to be raised
    // from `FlowBox`'s own default of 7 to fit all 8 — otherwise it force-
    // wraps the 8th swatch onto its own line even with plenty of width to
    // spare, which is exactly what a bare default looked like here.
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
        let ctx = ctx.clone();
        mode_row.add_suffix(&segmented_control::build(&mode_names, initial_mode_index, move |i| {
            mode_index.set(i);
            ctx.set_global_effect_mode(MODES[i]);
            color_row.set_visible(single_color_mode(MODES[i]));
            gradient_row.set_visible(MODES[i] == RgbMode::ColorCycle);
        }));
    }
    controls_group.add(&mode_row);
    controls_group.add(&color_row);
    controls_group.add(&gradient_row);

    let speed_row = adw::ActionRow::builder().title(ctx.t("ge.speed")).build();
    let speed_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    speed_scale.set_value(30.0);
    speed_scale.set_hexpand(false);
    speed_scale.set_size_request(SLIDER_WIDTH, -1);
    speed_scale.set_draw_value(true);
    speed_scale.set_value_pos(gtk::PositionType::Right);
    speed_scale.set_format_value_func(|_, value| format!("{value:.0}%"));
    speed_row.add_suffix(&speed_scale);
    controls_group.add(&speed_row);

    // Literal FPS, not an abstract "smoothness %" — independent from Speed
    // (which only controls how long a full cycle takes). How many frames
    // that cycle actually needs falls out of fps × cycle length.
    let fps_row = adw::ActionRow::builder()
        .title(ctx.t("ge.fps"))
        .subtitle(ctx.t("ge.fps_subtitle"))
        .build();
    let fps_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 5.0, 60.0, 1.0);
    fps_scale.set_value(30.0);
    fps_scale.set_hexpand(false);
    fps_scale.set_size_request(SLIDER_WIDTH, -1);
    fps_scale.set_draw_value(true);
    fps_scale.set_value_pos(gtk::PositionType::Right);
    fps_scale.set_format_value_func(|_, value| format!("{value:.0} fps"));
    fps_scale.add_mark(15.0, gtk::PositionType::Bottom, Some("15"));
    fps_scale.add_mark(30.0, gtk::PositionType::Bottom, Some("30"));
    fps_scale.add_mark(60.0, gtk::PositionType::Bottom, Some("60"));
    fps_row.add_suffix(&fps_scale);
    controls_group.add(&fps_row);

    // "Over 60fps" is not a validated hardware ceiling — the daemon accepts
    // any interval_ms without checking it (confirmed by testing down to
    // 1ms/1000fps, which still returned ok). It's just an unverified range,
    // hence gated behind an explicit opt-in with a quiet yellow tint on the
    // upper half of the slider once enabled, rather than silently exposing
    // 120fps as if it were as proven as the 5-60 range below it.
    let range_row = adw::ActionRow::builder().title(ctx.t("ge.over_60fps")).build();
    {
        let fps_scale = fps_scale.clone();
        let control = segmented_control::build(&[ctx.t("ge.standard"), ctx.t("ge.extended")], 0, move |i| {
            fps_scale.clear_marks();
            if i == 1 {
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
        });
        range_row.add_suffix(&control);
    }
    controls_group.add(&range_row);

    let brightness_row = adw::ActionRow::builder().title(ctx.t("ge.brightness")).build();
    let brightness_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    brightness_scale.set_value(25.0);
    brightness_scale.set_hexpand(false);
    brightness_scale.set_size_request(SLIDER_WIDTH, -1);
    brightness_scale.set_draw_value(true);
    brightness_scale.set_value_pos(gtk::PositionType::Right);
    brightness_scale.set_format_value_func(|_, value| format!("{value:.0}%"));
    brightness_row.add_suffix(&brightness_scale);
    controls_group.add(&brightness_row);

    let direction_row = adw::ActionRow::builder()
        .title(ctx.t("ge.global_orientation"))
        .subtitle(ctx.t("ge.global_orientation_subtitle"))
        .title_lines(1)
        .build();
    let direction_names: Vec<&str> = WAVE_DIRECTIONS.iter().map(|d| wave_direction_label(*d, ctx.lang())).collect();
    let global_direction = Rc::new(Cell::new(RgbDirection::Up)); // matches segmented_control's initial index 0
    let direction_control = segmented_control::build(&direction_names, 0, {
        let global_direction = global_direction.clone();
        move |i| {
            if let Some(d) = WAVE_DIRECTIONS.get(i) {
                global_direction.set(*d);
            }
        }
    });
    // Every device already gets an explicit direction (its own saved
    // pref, or this default, eagerly recorded — see render_device_order),
    // so this switch is purely "let me touch the default" vs. "leave it
    // alone, don't tempt me to bump it by accident" — not a functional
    // on/off for the wave itself. Persisted like everything else here (see
    // app_prefs) — it kept resetting to enabled on every restart before.
    let direction_enabled = ctx.global_direction_enabled();
    let direction_enabled_switch =
        gtk::Switch::builder().valign(gtk::Align::Center).active(direction_enabled).build();
    direction_control.set_sensitive(direction_enabled);
    direction_row.add_suffix(&direction_enabled_switch);
    controls_group.add(&direction_row);

    // A proper AdwActionRow, not a bare GtkBox — that's what gets the same
    // gray boxed-list card background and automatic label-left/control-right
    // layout as every other row here, instead of floating outside the card
    // with no background.
    let direction_value_row = adw::ActionRow::builder().title(ctx.t("ge.direction")).build();
    direction_value_row.add_suffix(&direction_control);
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
    let devices_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
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
        .css_classes(["suggested-action", "pill"])
        .build();
    apply_button.set_margin_top(8);
    content.append(&apply_button);

    {
        let ctx = ctx.clone();
        let ordered_devices = ordered_devices.clone();
        let device_directions = device_directions.clone();
        let device_segments = device_segments.clone();
        let mode_index = mode_index.clone();
        let global_direction = global_direction.clone();
        let gradient_colors = gradient_colors.clone();
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
            let gradient = *gradient_colors.borrow();
            let speed_percent = speed_scale.value();
            let fps = fps_scale.value();
            let brightness_percent = brightness_scale.value();
            let global_dir = global_direction.get();
            glib::spawn_future_local(async move {
                apply_global_effect(
                    &ctx,
                    &devices,
                    mode,
                    color,
                    gradient,
                    speed_percent,
                    fps,
                    brightness_percent,
                    global_dir,
                    &directions,
                    &segments,
                )
                .await;
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
    list_box: &gtk::Box,
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
        // Two stacked rows per device instead of one: cramming a 6-character
        // name label, a direction picker, and 2 reorder buttons into a
        // single row line left almost no room for the name (it was
        // ellipsizing to 2-3 characters). Splitting them means the name
        // gets the row to itself.
        let row = adw::ActionRow::builder().title(ctx.display_name(device)).title_lines(1).build();
        row.add_prefix(&gtk::Label::new(Some(&(i + 1).to_string())));

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
        list_box.append(&row);

        // Direction override — checked in this order: touched already this
        // session (device_directions) → saved from a previous session
        // (device_rgb_prefs, e.g. "this cable always runs Down") → whatever
        // the global Orientation control is currently set to. Only devices
        // without an explicit pick anywhere follow the global default.
        let saved_prefs = ctx.rgb_prefs_for_opt(&device.device_id);
        let current_dir = device_directions
            .borrow()
            .get(&device.device_id)
            .copied()
            .or_else(|| saved_prefs.map(|p| p.direction))
            .unwrap_or_else(|| global_direction.get());
        let selected_index = WAVE_DIRECTIONS.iter().position(|d| *d == current_dir).unwrap_or(0);
        // Whatever ends up selected (session override, saved pref, or
        // global default) must land in the map immediately — not only when
        // the user touches the control — otherwise `Apply to All Devices`
        // reads an empty map on the very first click after opening the app
        // and silently falls back to the global default for every device,
        // even though the pills on screen already show the right thing.
        device_directions.borrow_mut().insert(device.device_id.clone(), current_dir);

        let direction_row2 = adw::ActionRow::builder().title(ctx.t("ge.direction")).build();
        // How many physical LED strips this device's flat LED buffer is
        // made of, concatenated strip-by-strip — confirmed on real hardware
        // (coloring the buffer in 4 equal quarters showed up as 2 solid
        // physical strips per quarter, i.e. 8 strips total for that cable).
        // Every strip reaches the same point in the gradient at the same
        // time instead of each one playing its own independent slice —
        // that's what makes a "merged" cable read as one clean band instead
        // of a tiled/chopped-up pattern. `1` = treat the buffer as one
        // continuous strip (the plain per-LED gradient). Same saved/session
        // priority as Direction above.
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
            direction_row2.add_suffix(&segmented_control::build(&direction_names, selected_index, move |idx| {
                if let Some(d) = WAVE_DIRECTIONS.get(idx) {
                    device_directions.borrow_mut().insert(device_id.clone(), *d);
                    let strips = device_segments.borrow().get(&device_id).copied().unwrap_or(current_strip_count);
                    ctx.set_rgb_prefs(&device_id, *d, strips);
                }
            }));
        }
        list_box.append(&direction_row2);

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
                ctx.set_rgb_prefs(&device_id, dir, strips);
            });
        }
        merge_row.add_suffix(&merge_spin);
        list_box.append(&merge_row);
    }
}

async fn apply_global_effect(
    ctx: &Rc<Ctx>,
    devices: &[DeviceInfo],
    mode: RgbMode,
    color: [u8; 3],
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
    let wired_speed = percent_to_speed4(speed_percent);
    let wired_brightness = percent_to_brightness4(brightness_percent);
    let brightness_factor = (brightness_percent / 100.0).clamp(0.0, 1.0) as f32;
    let resolve_direction = |device_id: &str| -> RgbDirection {
        device_directions.get(device_id).copied().unwrap_or(global_direction)
    };

    let wireless: Vec<&DeviceInfo> = devices.iter().filter(|d| d.device_id.starts_with("wireless:")).collect();
    let wired: Vec<&DeviceInfo> = devices.iter().filter(|d| !d.device_id.starts_with("wireless:")).collect();

    let mut ok = 0usize;
    let mut failed = 0usize;
    // Written through to `AppConfig` once at the end (one round-trip for
    // every device touched here) so the daemon's own wireless-drift
    // auto-resync replays the *current* effect instead of stale config —
    // see `rgb_persist`'s module doc comment.
    let mut to_persist: Vec<(String, Vec<(u8, RgbEffect)>)> = Vec::new();

    // Wired: always a direct SetRgbEffect — the firmware renders whichever
    // mode it natively supports; if this device can't map the mode to a
    // hardware byte the call errors and we count it as failed rather than
    // silently doing nothing. Custom Gradient Wave (`ColorCycle` repurposed
    // — see `mode_label`) is wireless-only, so wired devices are just
    // skipped for it rather than sent the repurposed tag as a real mode.
    for device in &wired {
        if mode == RgbMode::ColorCycle {
            continue;
        }
        let zone_count = caps
            .iter()
            .find(|c| c.device_id == device.device_id)
            .map(|c| c.zones.len().max(1))
            .unwrap_or(1);
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
            // Wireless reports Static as a genuine SetRgbEffect-capable
            // mode (see GetRgbCapabilities), so it goes out the same way as
            // wired via SetRgbEffect — but the daemon forwards the color
            // bytes to a wireless device as-is and never scales them by
            // `RgbEffect.brightness` (that field only does anything for
            // wired firmware), so brightness has to be baked into the color
            // itself here, same as Rainbow/Breathing's frame colors already
            // are via `brightness_factor`.
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
            // The one spatial effect: every wireless device is a segment of
            // one shared gradient, in the order given. The base gradient is
            // always built forward — per-device orientation is expressed
            // entirely through `device_reversed` below, so a device whose
            // direction resolves to e.g. Up/CCW/Gather gets its own slice
            // flipped without touching how the other devices render.
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
                .map(|d| direction::is_reverse(resolve_direction(&d.device_id)))
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
                // Deliberately not persisted to `AppConfig` — `SetConfig`
                // makes the daemon call `apply_rgb_config()` synchronously,
                // which for a wireless device pushes a single-color snapshot
                // via `set_effect()`, a real RF command that immediately
                // halts the animation these frames just started (confirmed
                // on real hardware: the effect played briefly then froze on
                // one color). See rgb_editor.rs's matching comment.
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
        RgbMode::RainbowMorph | RgbMode::Breathing | RgbMode::ColorCycle => {
            // Every LED on every device shows the same color at the same
            // time — no spatial slicing, each device just renders its own
            // copy of the same frame sequence. Custom Gradient Wave
            // (`ColorCycle`) still respects each device's own direction and
            // LED-strip count, same as it does in the RGB Editor.
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
                let frames = match mode {
                    RgbMode::RainbowMorph => rainbow_morph_frames(led_count, frame_count, brightness_factor),
                    RgbMode::ColorCycle => custom_gradient_wave_frames(
                        led_count,
                        frame_count,
                        &gradient_colors,
                        direction::is_reverse(resolve_direction(&device.device_id)),
                        device_segments.get(&device.device_id).copied().unwrap_or(1),
                        brightness_factor,
                    ),
                    _ => breathing_frames(led_count, frame_count, color, brightness_factor),
                };
                let request = IpcRequest::SetRgbFrames {
                    device_id: device.device_id.clone(),
                    frames: frames.clone(),
                    interval_ms,
                };
                // Not persisted — same reason as the Rainbow branch above:
                // `SetConfig` would immediately push a static-color snapshot
                // to the device and freeze this animation.
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
