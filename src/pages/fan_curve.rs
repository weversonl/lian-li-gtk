//! Fan curve editor: named temperature→PWM curves, stored in
//! `AppConfig.fan_curves`. No dedicated Save/Delete IPC call — editing
//! round-trips through `GetConfig` → mutate → `SetConfig`.
//!
//! Curves are scoped to a single fan hub each (see `fan_curve_owners`) —
//! the daemon itself only knows curves as named entries in one global
//! list, referenced by name from any hub's `FanGroup`, but this client
//! never lets two hubs share a curve object, since editing one hub's
//! curve would otherwise silently affect another hub using the same name.

use crate::app_prefs::Lang;
use crate::context::Ctx;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::config::AppConfig;
use lianli_shared::fan::{FanConfig, FanCurve, FanGroup, FanSpeed};
use lianli_shared::ipc::IpcRequest;
use lianli_shared::sensors::{SensorInfo, SensorSource};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
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

/// Always entered from a specific device's dashboard (see
/// `window.rs::build_quick_actions`) — the hub is never ambiguous, so
/// there's no in-page hub picker; the page title carries the hub's name.
pub fn page(ctx: &Rc<Ctx>, device_id: &str) -> adw::NavigationPage {
    let hub_name = ctx
        .state
        .borrow()
        .devices
        .iter()
        .find(|d| d.device_id == device_id)
        .map(|d| ctx.display_name(d))
        .unwrap_or_else(|| device_id.to_string());

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

    let ctx = ctx.clone();
    let device_id = device_id.to_string();
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
                build_editor(&root, &ctx, config, sensors, device_id, &save_button);
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

    adw::NavigationPage::builder().title(hub_name).child(&toolbar).build()
}

/// One hub's currently-active curve gets adopted as owning that curve if
/// nothing owns it yet — recovers pre-existing daemon assignments made
/// before this app tracked ownership. Anything left with no owner at all
/// (an orphan with no hub referencing it) is adopted by the first hub, so
/// it stays visible and editable instead of becoming unreachable.
fn adopt_orphan_owners(config: &AppConfig, this_hub: &str, owners: &mut HashMap<String, String>) {
    if let Some(fans) = &config.fans {
        for group in &fans.speeds {
            let Some(device_id) = &group.device_id else { continue };
            for slot in &group.speeds {
                if let FanSpeed::Curve(name) = slot {
                    owners.entry(name.clone()).or_insert_with(|| device_id.clone());
                }
            }
        }
    }
    // Anything still ownerless (no hub ever referenced it) is claimed by
    // whichever hub's page happens to load first — keeps it visible and
    // editable instead of becoming unreachable.
    for curve in &config.fan_curves {
        owners.entry(curve.name.clone()).or_insert_with(|| this_hub.to_string());
    }
}

/// Guarantees every curve name is globally unique before it's sent to the
/// daemon — `fan_controller.rs` keys curves by name in a `HashMap`, so two
/// curves sharing a name would collide there and silently share state
/// across hubs, exactly what per-hub ownership is meant to prevent.
/// Renames later duplicates, fixing up any `FanGroup` reference and the
/// ownership map to match. Returns the new names assigned, if any.
fn dedupe_curve_names(config: &mut AppConfig, owners: &mut HashMap<String, String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut renames: Vec<(String, String)> = Vec::new();
    for curve in config.fan_curves.iter_mut() {
        let mut candidate = curve.name.clone();
        let mut n = 2;
        while seen.contains(&candidate) {
            candidate = format!("{} ({n})", curve.name);
            n += 1;
        }
        if candidate != curve.name {
            renames.push((curve.name.clone(), candidate.clone()));
            curve.name = candidate.clone();
        }
        seen.insert(curve.name.clone());
    }
    if !renames.is_empty() {
        if let Some(fans) = config.fans.as_mut() {
            for group in fans.speeds.iter_mut() {
                for slot in group.speeds.iter_mut() {
                    if let FanSpeed::Curve(nm) = slot {
                        if let Some((_, new)) = renames.iter().find(|(old, _)| old == nm) {
                            *nm = new.clone();
                        }
                    }
                }
            }
        }
        // `old` is kept by whichever curve didn't get renamed, so its owner
        // entry must stay put — moving it (as an earlier version of this
        // function did) left that curve ownerless. The renamed duplicate's
        // owner is instead derived from the `FanGroup` that now actually
        // references its new name, if any.
        for (_, new) in &renames {
            let owner = config.fans.as_ref().and_then(|fans| {
                fans.speeds.iter().find_map(|group| {
                    let references_new =
                        group.speeds.iter().any(|s| matches!(s, FanSpeed::Curve(nm) if nm == new));
                    references_new.then(|| group.device_id.clone()).flatten()
                })
            });
            if let Some(owner) = owner {
                owners.insert(new.clone(), owner);
            }
        }
    }
    renames.into_iter().map(|(_, new)| new).collect()
}

/// Bundles every widget and piece of shared state the editor's callbacks
/// need, so each callback just clones this (cheap — `Rc`/GObject clones)
/// instead of threading a dozen parameters through every function.
#[derive(Clone)]
struct Ui {
    ctx: Rc<Ctx>,
    lang: Lang,
    config: Rc<RefCell<AppConfig>>,
    sensors: Rc<Vec<SensorInfo>>,
    /// The one hub this page instance is scoped to (see `page`'s doc comment).
    hub_id: String,
    /// Curve name → owning hub's `device_id`. Local-only, see `fan_curve_owners`.
    owners: Rc<RefCell<HashMap<String, String>>>,
    /// Index into `config.fan_curves` — global, not hub-local.
    selected_curve: Rc<RefCell<Option<usize>>>,

    curves_group: adw::PreferencesGroup,
    curves_list: gtk::ListBox,
    activate_button: gtk::Button,
    deactivate_button: gtk::Button,

    name_row: adw::EntryRow,
    name_handler: Rc<RefCell<Option<glib::SignalHandlerId>>>,

    source_widgets: SourceWidgets,
    source_handler: Rc<RefCell<Option<glib::SignalHandlerId>>>,

    profile_group: adw::PreferencesGroup,
    profile_widget: Rc<RefCell<Option<adw::ComboRow>>>,

    points_list: gtk::Box,
    graph_area: gtk::DrawingArea,
}

impl Ui {
    /// `config.fan_curves` indices owned by this page's hub, in list order.
    fn owned_indices(&self) -> Vec<usize> {
        let owners = self.owners.borrow();
        self.config
            .borrow()
            .fan_curves
            .iter()
            .enumerate()
            .filter(|(_, c)| owners.get(&c.name).map(|o| o == &self.hub_id).unwrap_or(false))
            .map(|(i, _)| i)
            .collect()
    }

    /// The curve name currently driving this hub, if its `FanGroup` has
    /// all 4 slots pointing at the same curve.
    fn active_curve_name(&self) -> Option<String> {
        let cfg = self.config.borrow();
        let group = cfg.fans.as_ref()?.speeds.iter().find(|g| g.device_id.as_deref() == Some(self.hub_id.as_str()))?;
        match &group.speeds[0] {
            FanSpeed::Curve(n) if group.speeds.iter().all(|s| matches!(s, FanSpeed::Curve(m) if m == n)) => {
                Some(n.clone())
            }
            _ => None,
        }
    }

    fn persist_owners(&self) {
        crate::fan_curve_owners::save(&self.owners.borrow());
    }

    /// Sets (or clears) which curve drives this hub.
    fn set_active(&self, name: Option<String>) {
        let mut cfg = self.config.borrow_mut();
        let fans = cfg.fans.get_or_insert_with(FanConfig::default);
        fans.speeds.retain(|g| g.device_id.as_deref() != Some(self.hub_id.as_str()));
        if let Some(name) = name {
            let speed = FanSpeed::Curve(name);
            fans.speeds.push(FanGroup { device_id: Some(self.hub_id.clone()), speeds: [speed.clone(), speed.clone(), speed.clone(), speed] });
        }
        drop(cfg);
        self.ctx.toast(self.ctx.t("fc.assignment_changed"));
        self.refresh_curve_list();
    }

    /// Rebuilds the hub-scoped curve list: badges for "currently applied"
    /// and "currently being edited" (can differ), and the active-curve
    /// status line on the group description.
    fn refresh_curve_list(&self) {
        while let Some(child) = self.curves_list.first_child() {
            self.curves_list.remove(&child);
        }

        let active_name = self.active_curve_name();
        self.curves_group.set_description(Some(&match &active_name {
            Some(name) => format!("{}: {name}", self.ctx.t("fc.active_curve")),
            None => self.ctx.t("fc.no_active_curve").to_string(),
        }));

        let owned = self.owned_indices();
        if owned.is_empty() {
            self.curves_list
                .append(&adw::ActionRow::builder().title(self.ctx.t("fc.no_curves")).build());
        }

        let curves = self.config.borrow().fan_curves.clone();
        let selected = *self.selected_curve.borrow();
        let mut selected_is_active = false;
        for global_idx in owned {
            let Some(curve) = curves.get(global_idx) else { continue };
            let is_active = active_name.as_deref() == Some(curve.name.as_str());
            let is_selected = selected == Some(global_idx);
            if is_selected && is_active {
                selected_is_active = true;
            }

            let row = adw::ActionRow::builder()
                .title(curve.name.clone())
                .subtitle(format!("{} {}", curve.curve.len(), self.ctx.t("fc.points_suffix")))
                .activatable(true)
                .build();
            if is_selected {
                row.add_css_class("selected-curve");
            }
            if is_active {
                let badge = gtk::Label::builder().label(self.ctx.t("fc.active_badge")).valign(gtk::Align::Center).build();
                badge.add_css_class("applied-curve-badge");
                row.add_suffix(&badge);
            } else if is_selected {
                let badge = gtk::Label::builder().label(self.ctx.t("fc.editing_badge")).valign(gtk::Align::Center).build();
                badge.add_css_class("editing-curve-badge");
                row.add_suffix(&badge);
            }

            let ui = self.clone();
            row.connect_activated(move |_| ui.select_curve(Some(global_idx)));
            self.curves_list.append(&row);
        }

        self.activate_button.set_sensitive(selected.is_some() && !selected_is_active);
        self.deactivate_button.set_sensitive(active_name.is_some());
    }

    /// Refreshes everything that depends on which curve is selected, but
    /// not the curve list itself (used while the rename field has focus).
    fn refresh_curve_dependent(&self) {
        refresh_points(&self.points_list, &self.config, &self.selected_curve, &self.graph_area, self.lang);
        populate_source_row(&self.source_widgets, &self.sensors, &self.config, &self.selected_curve, &self.source_handler, self.lang);
        populate_profile_row(&self.profile_group, &self.profile_widget, &self.config, &self.selected_curve, &self.points_list, &self.graph_area, self.lang);
        populate_name_row(&self.name_row, &self.name_handler, &self.config, &self.selected_curve);
    }

    fn select_curve(&self, idx: Option<usize>) {
        *self.selected_curve.borrow_mut() = idx;
        self.refresh_curve_list();
        self.refresh_curve_dependent();
    }

    /// Picks a sensible starting curve: the hub's active one if it has
    /// one, else its first owned curve, else nothing. Called once, right
    /// after the page is built.
    fn select_initial_curve(&self) {
        let active_name = self.active_curve_name();
        let owned = self.owned_indices();
        let default_idx = active_name
            .and_then(|name| owned.iter().copied().find(|&i| self.config.borrow().fan_curves.get(i).map(|c| c.name == name).unwrap_or(false)))
            .or_else(|| owned.first().copied());
        *self.selected_curve.borrow_mut() = default_idx;
        self.refresh_curve_list();
        self.refresh_curve_dependent();
    }
}

fn build_editor(
    root: &gtk::Box,
    ctx: &Rc<Ctx>,
    config: AppConfig,
    sensors: Vec<SensorInfo>,
    hub_id: String,
    save_button: &gtk::Button,
) {
    let lang = ctx.lang();
    let config = Rc::new(RefCell::new(config));
    let sensors = Rc::new(sensors);

    let mut owners = crate::fan_curve_owners::load();
    adopt_orphan_owners(&config.borrow(), &hub_id, &mut owners);
    let owners = Rc::new(RefCell::new(owners));
    crate::fan_curve_owners::save(&owners.borrow());

    let selected_curve: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(700).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let curves_group = adw::PreferencesGroup::builder().title(ctx.t("fc.hub_curves")).build();
    // A real `ListBox`, not a plain `Box` — `AdwActionRow.activatable` only
    // fires its "activated" signal on click when the row is a direct child
    // of an actual `GtkListBox` (see `global_effects.rs`'s device list for
    // the same fix), which nesting inside a plain `Box` silently defeats.
    let curves_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    curves_group.add(&curves_list);
    content.append(&curves_group);

    let activate_row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).homogeneous(true).build();
    let activate_button = gtk::Button::builder().label(ctx.t("fc.set_active")).hexpand(true).css_classes(["suggested-action"]).build();
    let deactivate_button = gtk::Button::builder().label(ctx.t("fc.unassign")).hexpand(true).build();
    activate_row.append(&activate_button);
    activate_row.append(&deactivate_button);
    content.append(&activate_row);

    let curve_buttons = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).homogeneous(true).build();
    let new_button = gtk::Button::builder().label(ctx.t("fc.new_curve")).hexpand(true).build();
    let delete_curve_button = gtk::Button::builder()
        .label(ctx.t("fc.delete_curve"))
        .hexpand(true)
        .css_classes(["destructive-action"])
        .build();
    curve_buttons.append(&new_button);
    curve_buttons.append(&delete_curve_button);
    content.append(&curve_buttons);

    let name_group = adw::PreferencesGroup::new();
    let name_row = adw::EntryRow::builder().title(ctx.t("fc.curve_name")).build();
    name_group.add(&name_row);
    content.append(&name_group);

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

    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    root.append(&scrolled);

    let ui = Ui {
        ctx: ctx.clone(),
        lang,
        config: config.clone(),
        sensors: sensors.clone(),
        hub_id: hub_id.clone(),
        owners: owners.clone(),
        selected_curve: selected_curve.clone(),
        curves_group: curves_group.clone(),
        curves_list: curves_list.clone(),
        activate_button: activate_button.clone(),
        deactivate_button: deactivate_button.clone(),
        name_row: name_row.clone(),
        name_handler: Rc::new(RefCell::new(None)),
        source_widgets: source_widgets.clone(),
        source_handler: Rc::new(RefCell::new(None)),
        profile_group: profile_group.clone(),
        profile_widget: Rc::new(RefCell::new(None)),
        points_list: points_list.clone(),
        graph_area: graph_area.clone(),
    };

    {
        let ui_cb = ui.clone();
        let id = source_row.connect_selected_notify(move |row| {
            let Some(idx) = *ui_cb.selected_curve.borrow() else { return };
            let selected = row.selected() as usize;
            let is_custom_command = selected == ui_cb.sensors.len() + 1;
            ui_cb.source_widgets.command_row.set_visible(is_custom_command);
            ui_cb.source_widgets.command_help.set_visible(is_custom_command);
            let mut cfg = ui_cb.config.borrow_mut();
            let Some(curve) = cfg.fan_curves.get_mut(idx) else { return };
            if selected == 0 {
                curve.temp_source = None;
                curve.temp_command.clear();
            } else if is_custom_command {
                let cmd = ui_cb.source_widgets.command_row.text().to_string();
                curve.temp_source = Some(SensorSource::Command { cmd: cmd.clone() });
                curve.temp_command = cmd;
            } else if let Some(sensor) = ui_cb.sensors.get(selected - 1) {
                curve.temp_source = Some(sensor.source.clone());
                curve.temp_command.clear();
            }
            drop(cfg);
            if !is_custom_command {
                ui_cb.ctx.toast(ui_cb.ctx.t("fc.source_changed"));
            }
        });
        *ui.source_handler.borrow_mut() = Some(id);
    }

    {
        let ui = ui.clone();
        command_row.connect_changed(move |entry| {
            let Some(idx) = *ui.selected_curve.borrow() else { return };
            let cmd = entry.text().to_string();
            if let Some(curve) = ui.config.borrow_mut().fan_curves.get_mut(idx) {
                curve.temp_source = Some(SensorSource::Command { cmd: cmd.clone() });
                curve.temp_command = cmd;
            }
        });
    }

    {
        let ui_cb = ui.clone();
        let id = name_row.connect_changed(move |entry| {
            let Some(idx) = *ui_cb.selected_curve.borrow() else { return };
            let new_name = entry.text().to_string();
            let mut cfg = ui_cb.config.borrow_mut();
            let old_name = cfg.fan_curves.get(idx).map(|c| c.name.clone());
            if let Some(curve) = cfg.fan_curves.get_mut(idx) {
                curve.name = new_name.clone();
            }
            if let Some(old_name) = old_name.filter(|old| *old != new_name) {
                if let Some(fans) = cfg.fans.as_mut() {
                    for group in fans.speeds.iter_mut() {
                        for slot in group.speeds.iter_mut() {
                            if let FanSpeed::Curve(n) = slot {
                                if *n == old_name {
                                    *n = new_name.clone();
                                }
                            }
                        }
                    }
                }
                let owner = ui_cb.owners.borrow_mut().remove(&old_name);
                if let Some(owner) = owner {
                    ui_cb.owners.borrow_mut().insert(new_name.clone(), owner);
                }
                ui_cb.persist_owners();
            }
            drop(cfg);
            ui_cb.refresh_curve_list();
        });
        *ui.name_handler.borrow_mut() = Some(id);
    }

    {
        let ui = ui.clone();
        activate_button.connect_clicked(move |_| {
            let Some(idx) = *ui.selected_curve.borrow() else { return };
            let name = ui.config.borrow().fan_curves.get(idx).map(|c| c.name.clone());
            ui.set_active(name);
        });
    }

    {
        let ui = ui.clone();
        deactivate_button.connect_clicked(move |_| ui.set_active(None));
    }

    {
        let ui = ui.clone();
        new_button.connect_clicked(move |_| {
            let hub_id = ui.hub_id.clone();
            let mut cfg = ui.config.borrow_mut();
            let existing_names: HashSet<String> = cfg.fan_curves.iter().map(|c| c.name.clone()).collect();
            let mut name = format!("{} {}", ui.ctx.t("fc.new_curve"), cfg.fan_curves.len() + 1);
            let mut n = 2;
            while existing_names.contains(&name) {
                name = format!("{} {} ({n})", ui.ctx.t("fc.new_curve"), cfg.fan_curves.len() + 1);
                n += 1;
            }
            let index = cfg.fan_curves.len();
            cfg.fan_curves.push(FanCurve {
                name: name.clone(),
                temp_source: None,
                temp_command: String::new(),
                curve: vec![(20.0, 20.0), (50.0, 60.0), (80.0, 100.0)],
            });
            drop(cfg);
            ui.owners.borrow_mut().insert(name, hub_id);
            ui.persist_owners();
            ui.select_curve(Some(index));
        });
    }

    {
        let ui = ui.clone();
        delete_curve_button.connect_clicked(move |_| {
            let Some(idx) = *ui.selected_curve.borrow() else { return };
            let removed_name = {
                let mut cfg = ui.config.borrow_mut();
                if idx >= cfg.fan_curves.len() {
                    return;
                }
                let removed = cfg.fan_curves.remove(idx);
                if let Some(fans) = cfg.fans.as_mut() {
                    for group in fans.speeds.iter_mut() {
                        for slot in group.speeds.iter_mut() {
                            if matches!(slot, FanSpeed::Curve(n) if *n == removed.name) {
                                *slot = FanSpeed::Constant(0);
                            }
                        }
                    }
                    fans.speeds.retain(|g| !g.speeds.iter().all(|s| matches!(s, FanSpeed::Constant(0))));
                }
                removed.name
            };
            ui.owners.borrow_mut().remove(&removed_name);
            ui.persist_owners();
            let owned = ui.owned_indices();
            *ui.selected_curve.borrow_mut() = owned.first().copied();
            ui.refresh_curve_list();
            ui.refresh_curve_dependent();
            ui.ctx.toast(ui.ctx.t("fc.curve_removed"));
        });
    }

    {
        let ui = ui.clone();
        graph_reset_button.connect_clicked(move |_| {
            let Some(idx) = *ui.selected_curve.borrow() else { return };
            if let Some(curve) = ui.config.borrow_mut().fan_curves.get_mut(idx) {
                curve.curve = PRESET_BALANCED.to_vec();
            }
            refresh_points(&ui.points_list, &ui.config, &ui.selected_curve, &ui.graph_area, ui.lang);
        });
    }

    {
        let ui = ui.clone();
        add_point_button.connect_clicked(move |_| {
            let Some(idx) = *ui.selected_curve.borrow() else { return };
            if let Some(curve) = ui.config.borrow_mut().fan_curves.get_mut(idx) {
                curve.curve.push((50.0, 50.0));
            }
            refresh_points(&ui.points_list, &ui.config, &ui.selected_curve, &ui.graph_area, ui.lang);
        });
    }

    {
        let ui = ui.clone();
        save_button.connect_clicked(move |_| {
            let ui = ui.clone();
            glib::spawn_future_local(async move {
                let mut config = (*ui.config.borrow()).clone();
                let mut owners = ui.owners.borrow().clone();
                let renamed = dedupe_curve_names(&mut config, &mut owners);
                if !renamed.is_empty() {
                    *ui.owners.borrow_mut() = owners.clone();
                    ui.persist_owners();
                    *ui.config.borrow_mut() = config.clone();
                    ui.refresh_curve_list();
                    ui.ctx.toast(ui.ctx.t("fc.names_deduped"));
                }
                match ui.ctx.client.call_unit(IpcRequest::SetConfig { config }).await {
                    Ok(()) => ui.ctx.toast(ui.ctx.t("fc.curves_saved")),
                    Err(e) => ui.ctx.toast(&format!("{}: {e}", ui.ctx.t("fc.failed_save"))),
                }
            });
        });
    }

    ui.select_initial_curve();
}

/// Rebuilds the curve-rename `EntryRow` to reflect the selected curve's
/// current name — blocked while set, so this isn't mistaken for the user
/// typing a rename.
fn populate_name_row(
    name_row: &adw::EntryRow,
    name_handler: &Rc<RefCell<Option<glib::SignalHandlerId>>>,
    config: &Rc<RefCell<AppConfig>>,
    selected_curve: &Rc<RefCell<Option<usize>>>,
) {
    if let Some(id) = name_handler.borrow().as_ref() {
        name_row.block_signal(id);
    }
    let idx = *selected_curve.borrow();
    let name = idx.and_then(|i| config.borrow().fan_curves.get(i).map(|c| c.name.clone())).unwrap_or_default();
    name_row.set_text(&name);
    name_row.set_sensitive(idx.is_some());
    if let Some(id) = name_handler.borrow().as_ref() {
        name_row.unblock_signal(id);
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
        curve.curve.sort_by(|a, b| a.0.total_cmp(&b.0));
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
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
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
            points.sort_by(|a, b| a.0.total_cmp(&b.0));

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

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(name: &str, points: &[(f32, f32)]) -> FanCurve {
        FanCurve {
            name: name.to_string(),
            temp_source: None,
            temp_command: String::new(),
            curve: points.to_vec(),
        }
    }

    fn group(device_id: &str, curve_name: &str) -> FanGroup {
        FanGroup {
            device_id: Some(device_id.to_string()),
            speeds: [
                FanSpeed::Curve(curve_name.to_string()),
                FanSpeed::Constant(0),
                FanSpeed::Constant(0),
                FanSpeed::Constant(0),
            ],
        }
    }

    #[test]
    fn adopt_orphan_owners_claims_curve_referenced_by_a_hub() {
        let mut config = AppConfig::default();
        config.fan_curves.push(curve("Silent", &[(20.0, 20.0)]));
        config.fans = Some(FanConfig { speeds: vec![group("wireless:AA", "Silent")], ..FanConfig::default() });

        let mut owners = HashMap::new();
        adopt_orphan_owners(&config, "wireless:BB", &mut owners);

        assert_eq!(owners.get("Silent"), Some(&"wireless:AA".to_string()));
    }

    #[test]
    fn adopt_orphan_owners_claims_unreferenced_curve_as_this_hub() {
        let mut config = AppConfig::default();
        config.fan_curves.push(curve("Orphan", &[(20.0, 20.0)]));
        // No FanGroup references "Orphan" at all.

        let mut owners = HashMap::new();
        adopt_orphan_owners(&config, "wireless:BB", &mut owners);

        assert_eq!(owners.get("Orphan"), Some(&"wireless:BB".to_string()));
    }

    #[test]
    fn adopt_orphan_owners_never_overwrites_an_existing_owner() {
        let mut config = AppConfig::default();
        config.fan_curves.push(curve("Silent", &[(20.0, 20.0)]));
        config.fans = Some(FanConfig { speeds: vec![group("wireless:AA", "Silent")], ..FanConfig::default() });

        let mut owners = HashMap::new();
        owners.insert("Silent".to_string(), "wireless:CC".to_string());
        adopt_orphan_owners(&config, "wireless:BB", &mut owners);

        assert_eq!(owners.get("Silent"), Some(&"wireless:CC".to_string()));
    }

    #[test]
    fn dedupe_curve_names_renames_later_duplicates() {
        let mut config = AppConfig::default();
        config.fan_curves.push(curve("Silent", &[(20.0, 20.0)]));
        config.fan_curves.push(curve("Silent", &[(30.0, 30.0)]));
        config.fan_curves.push(curve("Balanced", &[(40.0, 40.0)]));
        let mut owners = HashMap::new();

        let renamed = dedupe_curve_names(&mut config, &mut owners);

        assert_eq!(renamed, vec!["Silent (2)".to_string()]);
        assert_eq!(config.fan_curves[0].name, "Silent");
        assert_eq!(config.fan_curves[1].name, "Silent (2)");
        assert_eq!(config.fan_curves[2].name, "Balanced");
    }

    #[test]
    fn dedupe_curve_names_updates_fan_group_references() {
        let mut config = AppConfig::default();
        config.fan_curves.push(curve("Silent", &[(20.0, 20.0)]));
        config.fan_curves.push(curve("Silent", &[(30.0, 30.0)]));
        config.fans = Some(FanConfig { speeds: vec![group("wireless:AA", "Silent")], ..FanConfig::default() });
        let mut owners = HashMap::new();

        dedupe_curve_names(&mut config, &mut owners);

        // The FanGroup referenced the name "Silent" — after dedup it must
        // still resolve to *some* real curve, not a name that no longer
        // exists in `fan_curves`.
        let referenced = match &config.fans.as_ref().unwrap().speeds[0].speeds[0] {
            FanSpeed::Curve(name) => name.clone(),
            _ => panic!("expected a Curve slot"),
        };
        assert!(config.fan_curves.iter().any(|c| c.name == referenced));
    }

    #[test]
    fn dedupe_curve_names_does_not_strip_the_unrenamed_curves_owner() {
        // Regression test: the original curve that keeps its name must keep
        // its existing owner — the renamed duplicate must not steal it.
        let mut config = AppConfig::default();
        config.fan_curves.push(curve("Silent", &[(20.0, 20.0)]));
        config.fan_curves.push(curve("Silent", &[(30.0, 30.0)]));
        let mut owners = HashMap::new();
        owners.insert("Silent".to_string(), "wireless:AA".to_string());

        dedupe_curve_names(&mut config, &mut owners);

        assert_eq!(owners.get("Silent"), Some(&"wireless:AA".to_string()));
    }

    #[test]
    fn dedupe_curve_names_assigns_renamed_curve_owner_from_its_fan_group() {
        let mut config = AppConfig::default();
        config.fan_curves.push(curve("Silent", &[(20.0, 20.0)]));
        config.fan_curves.push(curve("Silent", &[(30.0, 30.0)]));
        config.fans = Some(FanConfig { speeds: vec![group("wireless:AA", "Silent")], ..FanConfig::default() });
        let mut owners = HashMap::new();
        owners.insert("Silent".to_string(), "wireless:CC".to_string());

        dedupe_curve_names(&mut config, &mut owners);

        // The only FanGroup referencing "Silent" gets redirected to the
        // renamed duplicate; the pre-existing owner of the unrenamed name
        // is left untouched rather than being stolen by the duplicate.
        assert_eq!(owners.get("Silent"), Some(&"wireless:CC".to_string()));
        assert_eq!(owners.get("Silent (2)"), Some(&"wireless:AA".to_string()));
    }

    #[test]
    fn dedupe_curve_names_no_op_when_all_unique() {
        let mut config = AppConfig::default();
        config.fan_curves.push(curve("Silent", &[(20.0, 20.0)]));
        config.fan_curves.push(curve("Balanced", &[(30.0, 30.0)]));
        let mut owners = HashMap::new();

        let renamed = dedupe_curve_names(&mut config, &mut owners);

        assert!(renamed.is_empty());
        assert_eq!(config.fan_curves[0].name, "Silent");
        assert_eq!(config.fan_curves[1].name, "Balanced");
    }

    #[test]
    fn graph_plot_rect_subtracts_margins() {
        let (x0, y0, w, h) = graph_plot_rect(300, 200);
        assert_eq!((x0, y0), (GRAPH_MARGIN_LEFT, GRAPH_MARGIN_TOP));
        assert_eq!(w, 300.0 - GRAPH_MARGIN_LEFT - GRAPH_MARGIN_RIGHT);
        assert_eq!(h, 200.0 - GRAPH_MARGIN_TOP - GRAPH_MARGIN_BOTTOM);
    }

    #[test]
    fn graph_plot_rect_never_shrinks_below_1px() {
        let (_, _, w, h) = graph_plot_rect(1, 1);
        assert_eq!(w, 1.0);
        assert_eq!(h, 1.0);
    }

    #[test]
    fn graph_to_pixel_and_back_round_trips() {
        let plot = graph_plot_rect(300, 200);
        for &(temp, pwm) in &[(0.0, 0.0), (50.0, 50.0), (100.0, 100.0), (30.0, 90.0)] {
            let (x, y) = graph_to_pixel(temp, pwm, plot);
            let (t2, p2) = graph_to_data(x, y, plot);
            assert!((t2 - temp).abs() < 0.01, "temp round-trip: {temp} -> {t2}");
            assert!((p2 - pwm).abs() < 0.01, "pwm round-trip: {pwm} -> {p2}");
        }
    }

    #[test]
    fn graph_to_pixel_origin_is_bottom_left() {
        let plot = graph_plot_rect(300, 200);
        let (x0, y0, _, ph) = plot;
        // 0°C/0% PWM sits at the plot's bottom-left corner (y is flipped:
        // pixel-space grows downward, PWM grows upward).
        let (x, y) = graph_to_pixel(0.0, 0.0, plot);
        assert_eq!(x, x0);
        assert_eq!(y, y0 + ph);
    }

    #[test]
    fn graph_to_data_clamps_outside_plot_area() {
        let plot = graph_plot_rect(300, 200);
        let (x0, y0, pw, ph) = plot;
        let (temp, pwm) = graph_to_data(x0 - 500.0, y0 - 500.0, plot);
        assert_eq!(temp, 0.0);
        assert_eq!(pwm, GRAPH_MAX_PWM);
        let (temp, pwm) = graph_to_data(x0 + pw + 500.0, y0 + ph + 500.0, plot);
        assert_eq!(temp, GRAPH_MAX_TEMP);
        assert_eq!(pwm, 0.0);
    }

    #[test]
    fn graph_hit_test_finds_nearest_point_within_radius() {
        let plot = graph_plot_rect(300, 200);
        let points = [(20.0, 30.0), (50.0, 60.0), (80.0, 90.0)];
        let (x, y) = graph_to_pixel(50.0, 60.0, plot);
        assert_eq!(graph_hit_test(&points, x, y, plot), Some(1));
        assert_eq!(graph_hit_test(&points, x + 2.0, y + 2.0, plot), Some(1));
    }

    #[test]
    fn graph_hit_test_returns_none_outside_hit_radius() {
        let plot = graph_plot_rect(300, 200);
        let points = [(20.0, 30.0), (50.0, 60.0)];
        assert_eq!(graph_hit_test(&points, -1000.0, -1000.0, plot), None);
    }
}
