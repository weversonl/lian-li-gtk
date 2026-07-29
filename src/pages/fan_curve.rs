//! Fan curve editor: named temperature→PWM curves, stored in
//! `AppConfig.fan_curves`. No dedicated Save/Delete IPC call — editing
//! round-trips through `GetConfig` → mutate → `SetConfig`.

use crate::app_prefs::Lang;
use crate::context::Ctx;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::config::AppConfig;
use lianli_shared::fan::FanCurve;
use lianli_shared::ipc::IpcRequest;
use lianli_shared::sensors::{SensorInfo, SensorSource};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Canned curve shapes for the "Curve Profile" picker; Custom keeps whatever's there.
const PRESET_SILENT: &[(f32, f32)] = &[(30.0, 20.0), (45.0, 25.0), (55.0, 35.0), (65.0, 50.0), (75.0, 70.0), (85.0, 100.0)];
const PRESET_BALANCED: &[(f32, f32)] = &[(20.0, 20.0), (40.0, 35.0), (55.0, 50.0), (70.0, 70.0), (80.0, 85.0), (90.0, 100.0)];
const PRESET_PERFORMANCE: &[(f32, f32)] = &[(20.0, 40.0), (35.0, 55.0), (50.0, 70.0), (65.0, 85.0), (75.0, 95.0), (85.0, 100.0)];

/// Fixed graph domain so axes don't jump around while dragging points.
const GRAPH_MAX_TEMP: f32 = 100.0;
const GRAPH_MAX_PWM: f32 = 100.0;
/// Pixel radius for grabbing a point vs. adding/missing.
const POINT_HIT_RADIUS_PX: f64 = 14.0;

#[derive(Clone)]
struct SourceWidgets {
    row: adw::ComboRow,
    command_row: adw::EntryRow,
    command_help: gtk::Label,
}

pub fn page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let save_button = gtk::Button::builder().label(ctx.t("fc.save")).css_classes(["suggested-action"]).build();
    header.pack_end(&save_button);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let loading = adw::StatusPage::builder()
        .icon_name("content-loading-symbolic")
        .title(ctx.t("fc.loading"))
        .build();
    root.append(&loading);
    toolbar.set_content(Some(&root));
    let page_title = ctx.t("fc.title");

    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        let config_result = ctx.client.call::<AppConfig>(IpcRequest::GetConfig).await;
        // Filtered to temperature readings only — ListSensors also reports
        // usage/RPM/rate sensors meant for other pickers.
        let sensors: Vec<SensorInfo> = ctx
            .client
            .call::<Vec<SensorInfo>>(IpcRequest::ListSensors)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.unit == lianli_shared::sensors::Unit::C)
            .collect();
        match config_result {
            Ok(config) => {
                root.remove(&loading);
                build_editor(&root, &ctx, config, sensors, &save_button);
            }
            Err(e) => {
                root.remove(&loading);
                root.append(
                    &adw::StatusPage::builder()
                        .icon_name("dialog-error-symbolic")
                        .title(ctx.t("fc.failed_load_config"))
                        .description(e.to_string())
                        .build(),
                );
            }
        }
    });

    adw::NavigationPage::builder().title(page_title).child(&toolbar).build()
}

fn build_editor(
    root: &gtk::Box,
    ctx: &Rc<Ctx>,
    config: AppConfig,
    sensors: Vec<SensorInfo>,
    save_button: &gtk::Button,
) {
    let lang = ctx.lang();
    let config = Rc::new(RefCell::new(config));
    let sensors = Rc::new(sensors);
    let selected_curve: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(
        if config.borrow().fan_curves.is_empty() { None } else { Some(0) },
    ));

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(700).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let curves_group = adw::PreferencesGroup::builder().title(ctx.t("fc.saved_curves")).build();
    let curves_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    curves_group.add(&curves_list);
    content.append(&curves_group);

    let curve_buttons = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).build();
    let new_button = gtk::Button::builder().label(ctx.t("fc.new_curve")).hexpand(true).build();
    let delete_curve_button = gtk::Button::builder()
        .label(ctx.t("fc.delete_curve"))
        .hexpand(true)
        .css_classes(["destructive-action"])
        .build();
    curve_buttons.append(&new_button);
    curve_buttons.append(&delete_curve_button);
    content.append(&curve_buttons);

    let source_group = adw::PreferencesGroup::builder()
        .title(ctx.t("fc.temp_source"))
        .description(ctx.t("fc.temp_source_desc"))
        .build();
    let source_row = adw::ComboRow::builder().title(ctx.t("fc.sensor")).build();
    // Custom factory so long hwmon sensor names get a tooltip instead of
    // being ellipsized unreadably.
    let source_item_factory = gtk::SignalListItemFactory::new();
    source_item_factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else { return };
        let label = gtk::Label::builder().halign(gtk::Align::Start).ellipsize(gtk::pango::EllipsizeMode::End).build();
        list_item.set_child(Some(&label));
    });
    source_item_factory.connect_bind(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else { return };
        let Some(text) = list_item.item().and_downcast::<gtk::StringObject>().map(|s| s.string()) else { return };
        let Some(label) = list_item.child().and_downcast::<gtk::Label>() else { return };
        label.set_tooltip_text(Some(&text));
        label.set_label(&text);
    });
    source_row.set_factory(Some(&source_item_factory));
    source_row.set_list_factory(Some(&source_item_factory));
    source_group.add(&source_row);

    // Shown only when "Custom Command..." is selected: a shell command run
    // via `sh -c`. The daemon parses only the first whitespace-separated
    // token of stdout as an `f32` in °C — no unit conversion.
    let command_row = adw::EntryRow::builder().title(ctx.t("fc.command")).visible(false).build();
    let command_help = gtk::Label::builder()
        .label(ctx.t("fc.command_help"))
        .css_classes(["caption", "dim-label"])
        .halign(gtk::Align::Start)
        .wrap(true)
        .xalign(0.0)
        .visible(false)
        .build();
    source_group.add(&command_row);
    content.append(&source_group);
    content.append(&command_help);

    let source_widgets = SourceWidgets { row: source_row.clone(), command_row: command_row.clone(), command_help };

    let profile_group = adw::PreferencesGroup::new();
    content.append(&profile_group);

    let view_group = adw::PreferencesGroup::new();
    let view_names = gtk::StringList::new(&[ctx.t("fc.view_list"), ctx.t("fc.view_graph")]);
    let view_row = adw::ComboRow::builder().title(ctx.t("fc.view")).model(&view_names).build();
    view_group.add(&view_row);
    content.append(&view_group);

    let points_group = adw::PreferencesGroup::builder().title(ctx.t("fc.temp_points")).build();
    let points_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    points_group.add(&points_list);
    content.append(&points_group);

    let add_point_button = gtk::Button::builder().label(ctx.t("fc.add_point")).build();
    content.append(&add_point_button);

    let graph_area = build_curve_graph(&config, &selected_curve);
    let graph_reset_button = gtk::Button::builder().label(ctx.t("fc.restore_default")).halign(gtk::Align::End).build();
    content.append(&graph_area);
    content.append(&graph_reset_button);
    let starts_in_graph = ctx.fan_curve_graph_view();
    points_group.set_visible(!starts_in_graph);
    add_point_button.set_visible(!starts_in_graph);
    graph_area.set_visible(starts_in_graph);
    graph_reset_button.set_visible(starts_in_graph);

    view_row.set_selected(if starts_in_graph { 1 } else { 0 });
    {
        let points_group = points_group.clone();
        let add_point_button = add_point_button.clone();
        let graph_area = graph_area.clone();
        let graph_reset_button = graph_reset_button.clone();
        let ctx = ctx.clone();
        view_row.connect_selected_notify(move |row| {
            let is_graph = row.selected() == 1;
            points_group.set_visible(!is_graph);
            add_point_button.set_visible(!is_graph);
            graph_area.set_visible(is_graph);
            graph_reset_button.set_visible(is_graph);
            ctx.set_fan_curve_graph_view(is_graph);
            if is_graph {
                graph_area.queue_draw();
            }
        });
    }

    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let points_list = points_list.clone();
        let graph_area = graph_area.clone();
        graph_reset_button.connect_clicked(move |_| {
            let Some(idx) = *selected_curve.borrow() else { return };
            if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
                curve.curve = PRESET_BALANCED.to_vec();
            }
            refresh_points(&points_list, &config, &selected_curve, &graph_area, lang);
        });
    }

    // Rebuilt fresh on every curve switch by `populate_profile_row`, since
    // reselecting a segmented_control button without retriggering
    // `on_change` isn't possible. Tracked so a rebuild can remove the old one.
    let profile_widget: Rc<RefCell<Option<adw::ComboRow>>> = Rc::new(RefCell::new(None));
    populate_profile_row(&profile_group, &profile_widget, &config, &selected_curve, &points_list, &graph_area, lang);

    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    root.append(&scrolled);

    // Blocked while `populate_source_row` reflects the current curve's
    // source, so that call isn't mistaken for the user picking a new one.
    let source_handler_id = Rc::new(RefCell::new(None::<glib::SignalHandlerId>));
    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let sensors = sensors.clone();
        let command_row = command_row.clone();
        let command_help = source_widgets.command_help.clone();
        let ctx = ctx.clone();
        let id = source_row.connect_selected_notify(move |row| {
            let Some(idx) = *selected_curve.borrow() else { return };
            let selected = row.selected() as usize;
            let is_custom_command = selected == sensors.len() + 1;
            command_row.set_visible(is_custom_command);
            command_help.set_visible(is_custom_command);
            let mut cfg = config.borrow_mut();
            let Some(curve) = cfg.fan_curves.get_mut(idx) else { return };
            if selected == 0 {
                curve.temp_source = None;
                curve.temp_command.clear();
            } else if is_custom_command {
                let cmd = command_row.text().to_string();
                curve.temp_source = Some(SensorSource::Command { cmd: cmd.clone() });
                curve.temp_command = cmd;
            } else if let Some(sensor) = sensors.get(selected - 1) {
                curve.temp_source = Some(sensor.source.clone());
                curve.temp_command.clear();
            }
            drop(cfg);
            if !is_custom_command {
                ctx.toast(ctx.t("fc.source_changed"));
            }
        });
        *source_handler_id.borrow_mut() = Some(id);
    }

    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        command_row.connect_changed(move |entry| {
            let Some(idx) = *selected_curve.borrow() else { return };
            let cmd = entry.text().to_string();
            if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
                curve.temp_source = Some(SensorSource::Command { cmd: cmd.clone() });
                curve.temp_command = cmd;
            }
        });
    }

    refresh_curve_list(&curves_list, &config, &selected_curve, &points_list, &source_widgets, &sensors, &source_handler_id, &graph_area, &profile_group, &profile_widget, lang);
    refresh_points(&points_list, &config, &selected_curve, &graph_area, lang);
    populate_source_row(&source_widgets, &sensors, &config, &selected_curve, &source_handler_id, lang);
    populate_profile_row(&profile_group, &profile_widget, &config, &selected_curve, &points_list, &graph_area, lang);

    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let source_widgets = source_widgets.clone();
        let sensors = sensors.clone();
        let source_handler_id = source_handler_id.clone();
        let graph_area = graph_area.clone();
        let profile_group = profile_group.clone();
        let profile_widget = profile_widget.clone();
        new_button.connect_clicked({
            let curves_list = curves_list.clone();
            let points_list = points_list.clone();
            move |_| {
                let mut cfg = config.borrow_mut();
                let index = cfg.fan_curves.len();
                cfg.fan_curves.push(FanCurve {
                    name: format!("Curve {}", index + 1),
                    temp_source: None,
                    temp_command: String::new(),
                    curve: vec![(20.0, 20.0), (50.0, 60.0), (80.0, 100.0)],
                });
                drop(cfg);
                *selected_curve.borrow_mut() = Some(index);
                refresh_curve_list(&curves_list, &config, &selected_curve, &points_list, &source_widgets, &sensors, &source_handler_id, &graph_area, &profile_group, &profile_widget, lang);
                refresh_points(&points_list, &config, &selected_curve, &graph_area, lang);
                populate_source_row(&source_widgets, &sensors, &config, &selected_curve, &source_handler_id, lang);
                populate_profile_row(&profile_group, &profile_widget, &config, &selected_curve, &points_list, &graph_area, lang);
            }
        });
    }

    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let curves_list = curves_list.clone();
        let points_list = points_list.clone();
        let source_widgets = source_widgets.clone();
        let sensors = sensors.clone();
        let source_handler_id = source_handler_id.clone();
        let graph_area = graph_area.clone();
        let profile_group = profile_group.clone();
        let profile_widget = profile_widget.clone();
        let ctx = ctx.clone();
        delete_curve_button.connect_clicked(move |_| {
            let Some(idx) = *selected_curve.borrow() else { return };
            {
                let mut cfg = config.borrow_mut();
                if idx < cfg.fan_curves.len() {
                    cfg.fan_curves.remove(idx);
                }
            }
            let remaining = config.borrow().fan_curves.len();
            *selected_curve.borrow_mut() = if remaining == 0 {
                None
            } else {
                Some(idx.min(remaining - 1))
            };
            refresh_curve_list(&curves_list, &config, &selected_curve, &points_list, &source_widgets, &sensors, &source_handler_id, &graph_area, &profile_group, &profile_widget, lang);
            refresh_points(&points_list, &config, &selected_curve, &graph_area, lang);
            populate_source_row(&source_widgets, &sensors, &config, &selected_curve, &source_handler_id, lang);
            populate_profile_row(&profile_group, &profile_widget, &config, &selected_curve, &points_list, &graph_area, lang);
            ctx.toast(ctx.t("fc.curve_removed"));
        });
    }

    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let graph_area = graph_area.clone();
        add_point_button.connect_clicked(move |_| {
            let Some(idx) = *selected_curve.borrow() else { return };
            if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
                curve.curve.push((50.0, 50.0));
            }
            refresh_points(&points_list, &config, &selected_curve, &graph_area, lang);
        });
    }

    let ctx = ctx.clone();
    save_button.connect_clicked(move |_| {
        let ctx = ctx.clone();
        let config = (*config.borrow()).clone();
        glib::spawn_future_local(async move {
            match ctx.client.call_unit(IpcRequest::SetConfig { config }).await {
                Ok(()) => ctx.toast(ctx.t("fc.curves_saved")),
                Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("fc.failed_save"))),
            }
        });
    });
}

fn refresh_curve_list(
    list_box: &gtk::Box,
    config: &Rc<RefCell<AppConfig>>,
    selected_curve: &Rc<RefCell<Option<usize>>>,
    points_list: &gtk::Box,
    source_widgets: &SourceWidgets,
    sensors: &Rc<Vec<SensorInfo>>,
    source_handler_id: &Rc<RefCell<Option<glib::SignalHandlerId>>>,
    graph_area: &gtk::DrawingArea,
    profile_group: &adw::PreferencesGroup,
    profile_widget: &Rc<RefCell<Option<adw::ComboRow>>>,
    lang: Lang,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let curves = config.borrow().fan_curves.clone();
    if curves.is_empty() {
        list_box.append(&adw::ActionRow::builder().title(crate::i18n::t(lang, "fc.no_curves")).build());
        return;
    }

    for (i, curve) in curves.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(curve.name.clone())
            .subtitle(format!("{} {}", curve.curve.len(), crate::i18n::t(lang, "fc.points_suffix")))
            .activatable(true)
            .build();
        if Some(i) == *selected_curve.borrow() {
            row.add_css_class("selected-curve");
        }
        let selected_curve_cb = selected_curve.clone();
        let config_cb = config.clone();
        let list_box_cb = list_box.clone();
        let points_list_cb = points_list.clone();
        let source_widgets_cb = source_widgets.clone();
        let sensors_cb = sensors.clone();
        let source_handler_id_cb = source_handler_id.clone();
        let graph_area_cb = graph_area.clone();
        let profile_row_cb = profile_group.clone();
        let profile_widget_cb = profile_widget.clone();
        row.connect_activated(move |_| {
            *selected_curve_cb.borrow_mut() = Some(i);
            refresh_curve_list(
                &list_box_cb,
                &config_cb,
                &selected_curve_cb,
                &points_list_cb,
                &source_widgets_cb,
                &sensors_cb,
                &source_handler_id_cb,
                &graph_area_cb,
                &profile_row_cb,
                &profile_widget_cb,
                lang,
            );
            refresh_points(&points_list_cb, &config_cb, &selected_curve_cb, &graph_area_cb, lang);
            populate_source_row(&source_widgets_cb, &sensors_cb, &config_cb, &selected_curve_cb, &source_handler_id_cb, lang);
            populate_profile_row(&profile_row_cb, &profile_widget_cb, &config_cb, &selected_curve_cb, &points_list_cb, &graph_area_cb, lang);
        });
        list_box.append(&row);
    }
}

/// Rebuilds the "Curve Profile" control, detecting which preset (if any)
/// the current curve's points exactly match.
fn populate_profile_row(
    profile_group: &adw::PreferencesGroup,
    profile_widget: &Rc<RefCell<Option<adw::ComboRow>>>,
    config: &Rc<RefCell<AppConfig>>,
    selected_curve: &Rc<RefCell<Option<usize>>>,
    points_list: &gtk::Box,
    graph_area: &gtk::DrawingArea,
    lang: Lang,
) {
    if let Some(old) = profile_widget.borrow_mut().take() {
        profile_group.remove(&old);
    }

    let selected_index = selected_curve
        .borrow()
        .and_then(|idx| config.borrow().fan_curves.get(idx).map(|c| c.curve.clone()))
        .map(|points| {
            if points.as_slice() == PRESET_SILENT {
                1
            } else if points.as_slice() == PRESET_BALANCED {
                2
            } else if points.as_slice() == PRESET_PERFORMANCE {
                3
            } else {
                0
            }
        })
        .unwrap_or(0);

    let config = config.clone();
    let selected_curve = selected_curve.clone();
    let points_list = points_list.clone();
    let graph_area = graph_area.clone();
    let profile_names = [
        crate::i18n::t(lang, "fc.profile_custom"),
        crate::i18n::t(lang, "fc.profile_silent"),
        crate::i18n::t(lang, "fc.profile_balanced"),
        crate::i18n::t(lang, "fc.profile_performance"),
    ];
    let profile_model = gtk::StringList::new(&profile_names);
    let widget = adw::ComboRow::builder()
        .title(crate::i18n::t(lang, "fc.curve_profile"))
        .model(&profile_model)
        .build();
    widget.set_selected(selected_index as u32);
    widget.connect_selected_notify(move |row| {
        let preset = match row.selected() {
            1 => Some(PRESET_SILENT),
            2 => Some(PRESET_BALANCED),
            3 => Some(PRESET_PERFORMANCE),
            _ => None,
        };
        let Some(preset) = preset else { return };
        let Some(idx) = *selected_curve.borrow() else { return };
        if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
            curve.curve = preset.to_vec();
        }
        refresh_points(&points_list, &config, &selected_curve, &graph_area, lang);
    });
    profile_group.add(&widget);
    *profile_widget.borrow_mut() = Some(widget);
}

/// Rebuilds the "Temperature Source" combo's model and selects whichever
/// entry the current curve is reading from, matched via
/// `FanCurve::effective_source()`.
fn populate_source_row(
    source_widgets: &SourceWidgets,
    sensors: &Rc<Vec<SensorInfo>>,
    config: &Rc<RefCell<AppConfig>>,
    selected_curve: &Rc<RefCell<Option<usize>>>,
    source_handler_id: &Rc<RefCell<Option<glib::SignalHandlerId>>>,
    lang: Lang,
) {
    let source_row = &source_widgets.row;
    let custom_command_index = sensors.len() + 1;

    // Blocked for the whole rebuild, not just `set_selected` — `set_model`
    // itself resets `selected` to 0 and fires the signal before the real
    // value is restored below.
    if let Some(id) = source_handler_id.borrow().as_ref() {
        source_row.block_signal(id);
    }

    let mut names: Vec<String> = vec![crate::i18n::t(lang, "fc.system_default").to_string()];
    names.extend(sensors.iter().map(|s| s.get_display_name()));
    names.push(crate::i18n::t(lang, "fc.custom_command").to_string());
    let model = gtk::StringList::new(&names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    source_row.set_model(Some(&model));

    let Some(idx) = *selected_curve.borrow() else {
        if let Some(id) = source_handler_id.borrow().as_ref() {
            source_row.unblock_signal(id);
        }
        source_row.set_sensitive(false);
        source_widgets.command_row.set_visible(false);
        source_widgets.command_help.set_visible(false);
        return;
    };
    source_row.set_sensitive(true);
    let (selected_index, command_text) = config
        .borrow()
        .fan_curves
        .get(idx)
        .map(|curve| {
            if curve.temp_source.is_none() && curve.temp_command.is_empty() {
                return (0, String::new());
            }
            let effective = curve.effective_source();
            if let SensorSource::Command { cmd } = &effective {
                return (custom_command_index, cmd.clone());
            }
            let idx = sensors.iter().position(|s| s.source == effective).map(|i| i + 1).unwrap_or(0);
            (idx, String::new())
        })
        .unwrap_or((0, String::new()));

    source_row.set_selected(selected_index as u32);
    if let Some(id) = source_handler_id.borrow().as_ref() {
        source_row.unblock_signal(id);
    }

    // Only touched when active — `set_text` fires `connect_changed`, which
    // writes into `temp_command` and would clobber a sensor selection.
    let is_custom_command = selected_index == custom_command_index;
    if is_custom_command {
        source_widgets.command_row.set_text(&command_text);
    }
    source_widgets.command_row.set_visible(is_custom_command);
    source_widgets.command_help.set_visible(is_custom_command);
}

fn refresh_points(
    list_box: &gtk::Box,
    config: &Rc<RefCell<AppConfig>>,
    selected_curve: &Rc<RefCell<Option<usize>>>,
    graph_area: &gtk::DrawingArea,
    lang: Lang,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    graph_area.queue_draw();

    let Some(idx) = *selected_curve.borrow() else {
        list_box.append(&adw::ActionRow::builder().title(crate::i18n::t(lang, "fc.select_curve")).build());
        return;
    };

    // Sorted in place so `point_index` below matches ascending order.
    sort_curve(config, idx);

    let points = config
        .borrow()
        .fan_curves
        .get(idx)
        .map(|c| c.curve.clone())
        .unwrap_or_default();

    for (point_index, (temp, pwm)) in points.iter().enumerate() {
        let row = adw::ActionRow::new();

        let temp_adj = gtk::Adjustment::new(*temp as f64, 0.0, 120.0, 1.0, 5.0, 0.0);
        let temp_spin = gtk::SpinButton::new(Some(&temp_adj), 1.0, 0);
        temp_spin.set_valign(gtk::Align::Center);
        {
            let config = config.clone();
            let graph_area = graph_area.clone();
            temp_adj.connect_value_changed(move |adj| {
                if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
                    if let Some(p) = curve.curve.get_mut(point_index) {
                        p.0 = adj.value() as f32;
                    }
                }
                graph_area.queue_draw();
            });
        }

        let pwm_adj = gtk::Adjustment::new(*pwm as f64, 0.0, 100.0, 1.0, 5.0, 0.0);
        let pwm_spin = gtk::SpinButton::new(Some(&pwm_adj), 1.0, 0);
        pwm_spin.set_valign(gtk::Align::Center);
        {
            let config = config.clone();
            let graph_area = graph_area.clone();
            pwm_adj.connect_value_changed(move |adj| {
                if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
                    if let Some(p) = curve.curve.get_mut(point_index) {
                        p.1 = adj.value() as f32;
                    }
                }
                graph_area.queue_draw();
            });
        }

        row.add_prefix(&gtk::Label::new(Some("°C")));
        row.add_prefix(&temp_spin);
        row.add_suffix(&pwm_spin);
        row.add_suffix(&gtk::Label::new(Some("%")));

        let remove_button = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build();
        {
            let config = config.clone();
            let list_box = list_box.clone();
            let selected_curve = selected_curve.clone();
            let graph_area = graph_area.clone();
            remove_button.connect_clicked(move |_| {
                if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
                    if point_index < curve.curve.len() {
                        curve.curve.remove(point_index);
                    }
                }
                refresh_points(&list_box, &config, &selected_curve, &graph_area, lang);
            });
        }
        row.add_suffix(&remove_button);

        list_box.append(&row);
    }
}

const GRAPH_MARGIN_LEFT: f64 = 42.0;
const GRAPH_MARGIN_BOTTOM: f64 = 26.0;
const GRAPH_MARGIN_TOP: f64 = 14.0;
const GRAPH_MARGIN_RIGHT: f64 = 14.0;

fn graph_plot_rect(width: i32, height: i32) -> (f64, f64, f64, f64) {
    let w = width as f64;
    let h = height as f64;
    (
        GRAPH_MARGIN_LEFT,
        GRAPH_MARGIN_TOP,
        (w - GRAPH_MARGIN_LEFT - GRAPH_MARGIN_RIGHT).max(1.0),
        (h - GRAPH_MARGIN_TOP - GRAPH_MARGIN_BOTTOM).max(1.0),
    )
}

fn graph_to_pixel(temp: f32, pwm: f32, plot: (f64, f64, f64, f64)) -> (f64, f64) {
    let (x0, y0, pw, ph) = plot;
    let x = x0 + (temp as f64 / GRAPH_MAX_TEMP as f64) * pw;
    let y = y0 + ph - (pwm as f64 / GRAPH_MAX_PWM as f64) * ph;
    (x, y)
}

fn graph_to_data(x: f64, y: f64, plot: (f64, f64, f64, f64)) -> (f32, f32) {
    let (x0, y0, pw, ph) = plot;
    let temp = ((x - x0) / pw * GRAPH_MAX_TEMP as f64).clamp(0.0, GRAPH_MAX_TEMP as f64) as f32;
    let pwm = ((1.0 - (y - y0) / ph) * GRAPH_MAX_PWM as f64).clamp(0.0, GRAPH_MAX_PWM as f64) as f32;
    (temp, pwm)
}

/// Sorts curve `idx`'s points by temperature ascending, in place.
fn sort_curve(config: &Rc<RefCell<AppConfig>>, idx: usize) {
    if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
        curve.curve.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    }
}

/// Nearest point to widget-space `(x, y)`, within `POINT_HIT_RADIUS_PX`.
fn graph_hit_test(points: &[(f32, f32)], x: f64, y: f64, plot: (f64, f64, f64, f64)) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, &(temp, pwm))| {
            let (px, py) = graph_to_pixel(temp, pwm, plot);
            (i, (px - x).hypot(py - y))
        })
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .filter(|(_, dist)| *dist <= POINT_HIT_RADIUS_PX)
        .map(|(i, _)| i)
}

/// Drag-to-edit curve graph. Drag moves a point, double-click adds one,
/// right-click removes one (kept at 2+ points). Doesn't rebuild the List view.
fn build_curve_graph(config: &Rc<RefCell<AppConfig>>, selected_curve: &Rc<RefCell<Option<usize>>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_height(260).hexpand(true).vexpand(false).build();

    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        area.set_draw_func(move |_, cr, width, height| {
            let plot @ (x0, y0, pw, ph) = graph_plot_rect(width, height);

            cr.set_line_width(1.0);
            for i in 0..=4 {
                let t = i as f64 / 4.0;
                let y = y0 + ph * (1.0 - t);
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
                cr.move_to(x0, y);
                cr.line_to(x0 + pw, y);
                let _ = cr.stroke();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
                cr.move_to(2.0, y + 4.0);
                let _ = cr.show_text(&format!("{:.0}%", t * GRAPH_MAX_PWM as f64));
            }
            for i in 0..=5 {
                let t = i as f64 / 5.0;
                let x = x0 + pw * t;
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
                cr.move_to(x, y0);
                cr.line_to(x, y0 + ph);
                let _ = cr.stroke();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
                cr.move_to((x - 10.0).max(0.0), y0 + ph + 18.0);
                let _ = cr.show_text(&format!("{:.0}°", t * GRAPH_MAX_TEMP as f64));
            }

            let Some(idx) = *selected_curve.borrow() else { return };
            let mut points = config.borrow().fan_curves.get(idx).map(|c| c.curve.clone()).unwrap_or_default();
            if points.is_empty() {
                return;
            }
            points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

            cr.set_source_rgba(0.31, 0.55, 0.95, 0.9);
            cr.set_line_width(2.0);
            for (i, &(temp, pwm)) in points.iter().enumerate() {
                let (x, y) = graph_to_pixel(temp, pwm, plot);
                if i == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            let _ = cr.stroke();

            // Dotted extension of the last point's PWM% to the right edge.
            if let Some(&(last_temp, last_pwm)) = points.last() {
                let (lx, ly) = graph_to_pixel(last_temp, last_pwm, plot);
                cr.set_dash(&[4.0, 4.0], 0.0);
                cr.move_to(lx, ly);
                cr.line_to(x0 + pw, ly);
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
            }

            for &(temp, pwm) in &points {
                let (x, y) = graph_to_pixel(temp, pwm, plot);
                cr.arc(x, y, 5.0, 0.0, std::f64::consts::TAU);
                cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(0.31, 0.55, 0.95, 1.0);
                cr.set_line_width(2.0);
                let _ = cr.stroke();
            }
        });
    }

    let dragging: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

    let motion = gtk::EventControllerMotion::new();
    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let dragging = dragging.clone();
        let area_cb = area.clone();
        motion.connect_motion(move |_, x, y| {
            let Some(point_index) = dragging.get() else { return };
            let Some(idx) = *selected_curve.borrow() else { return };
            let plot = graph_plot_rect(area_cb.width(), area_cb.height());
            let (temp, pwm) = graph_to_data(x, y, plot);
            if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
                if let Some(p) = curve.curve.get_mut(point_index) {
                    *p = (temp, pwm);
                }
            }
            area_cb.queue_draw();
        });
    }
    area.add_controller(motion);

    let primary_click = gtk::GestureClick::new();
    primary_click.set_button(1);
    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let dragging = dragging.clone();
        let area_cb = area.clone();
        primary_click.connect_pressed(move |_, n_press, x, y| {
            let Some(idx) = *selected_curve.borrow() else { return };
            let plot = graph_plot_rect(area_cb.width(), area_cb.height());
            let mut cfg = config.borrow_mut();
            let Some(curve) = cfg.fan_curves.get_mut(idx) else { return };
            match graph_hit_test(&curve.curve, x, y, plot) {
                Some(index) => dragging.set(Some(index)),
                None if n_press == 2 => {
                    let (temp, pwm) = graph_to_data(x, y, plot);
                    curve.curve.push((temp, pwm));
                    dragging.set(Some(curve.curve.len() - 1));
                }
                None => {}
            }
            drop(cfg);
            area_cb.queue_draw();
        });
    }
    {
        let dragging = dragging.clone();
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let area_cb = area.clone();
        primary_click.connect_released(move |_, _, _, _| {
            dragging.set(None);
            // Re-sort now that the drag is done, not mid-drag — resorting
            // while a point's still being dragged would swap which index
            // `dragging` refers to out from under the gesture.
            if let Some(idx) = *selected_curve.borrow() {
                sort_curve(&config, idx);
            }
            area_cb.queue_draw();
        });
    }
    area.add_controller(primary_click);

    let secondary_click = gtk::GestureClick::new();
    secondary_click.set_button(3);
    {
        let config = config.clone();
        let selected_curve = selected_curve.clone();
        let area_cb = area.clone();
        secondary_click.connect_pressed(move |_, _, x, y| {
            let Some(idx) = *selected_curve.borrow() else { return };
            let plot = graph_plot_rect(area_cb.width(), area_cb.height());
            if let Some(curve) = config.borrow_mut().fan_curves.get_mut(idx) {
                // Keep at least 2 points — otherwise there's nothing left
                // to draw a line between.
                if curve.curve.len() > 2 {
                    if let Some(index) = graph_hit_test(&curve.curve, x, y, plot) {
                        curve.curve.remove(index);
                    }
                }
            }
            area_cb.queue_draw();
        });
    }
    area.add_controller(secondary_click);

    area
}
