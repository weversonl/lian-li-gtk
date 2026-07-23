//! Root window: `AdwNavigationSplitView` with a device sidebar and a
//! `AdwNavigationView` content column. The Dashboard detail is the root page
//! of that content column; RGB Editor / Sync Rainbow / Fan Curve / LCD /
//! Wireless Pairing push on top of it, Preferences opens as its own
//! `AdwPreferencesWindow`.

use crate::app_state::{new_shared_state, start_polling};
use crate::context::Ctx;
use crate::ipc_client::IpcClient;
use crate::pages::{fan_curve, global_effects, lcd_content, preferences, rgb_editor, wireless_pairing};
use adw::prelude::*;
use gtk::glib;
use lianli_shared::device_id::DeviceFamily;
use lianli_shared::ipc::{DeviceInfo, TelemetrySnapshot};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub fn build_ui(app: &adw::Application) {
    // GApplication routes a second launch to this same process via D-Bus,
    // but still fires `activate` every time — reuse the existing window
    // instead of building a second one.
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    load_app_css();

    let client = IpcClient::new();
    let state = new_shared_state();

    let app_prefs = crate::app_prefs::load();
    let lang = app_prefs.lang;

    let sidebar_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();

    let detail_status = adw::StatusPage::builder()
        .icon_name("dialog-information-symbolic")
        .title(crate::i18n::t(lang, "window.select_device_title"))
        .description(crate::i18n::t(lang, "window.select_device_desc"))
        .build();

    let detail_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    detail_box.append(&detail_status);

    let sidebar_toolbar = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
    let refresh_button = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_button.set_tooltip_text(Some(crate::i18n::t(lang, "sidebar.refresh")));
    let sync_button = gtk::Button::from_icon_name("preferences-desktop-display-symbolic");
    sync_button.set_tooltip_text(Some(crate::i18n::t(lang, "sidebar.global_effects")));
    let wireless_button = gtk::Button::from_icon_name("network-wireless-symbolic");
    wireless_button.set_tooltip_text(Some(crate::i18n::t(lang, "sidebar.wireless_pairing")));
    sidebar_header.pack_end(&sync_button);
    sidebar_header.pack_end(&wireless_button);
    sidebar_header.pack_end(&refresh_button);
    let sidebar_title = gtk::Label::builder()
        .label(crate::i18n::t(lang, "sidebar.title"))
        .css_classes(["title"])
        .halign(gtk::Align::Start)
        .build();
    sidebar_header.set_title_widget(Some(&sidebar_title));
    sidebar_toolbar.add_top_bar(&sidebar_header);

    let prefs_row = adw::ActionRow::builder().title(crate::i18n::t(lang, "sidebar.preferences")).activatable(true).build();
    prefs_row.add_prefix(&gtk::Image::from_icon_name("preferences-system-symbolic"));
    let sidebar_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_content.append(&build_sidebar_pane(&sidebar_list));
    sidebar_content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let prefs_list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
    prefs_list.append(&prefs_row);
    sidebar_content.append(&prefs_list);
    sidebar_toolbar.set_content(Some(&sidebar_content));

    let detail_header = adw::HeaderBar::new();
    let detail_toolbar = adw::ToolbarView::new();
    detail_toolbar.add_top_bar(&detail_header);
    detail_toolbar.set_content(Some(&detail_box));

    let dashboard_page = adw::NavigationPage::builder()
        .title("LianLiGTK")
        .tag("dashboard")
        .child(&detail_toolbar)
        .build();

    let content_nav = adw::NavigationView::new();
    content_nav.push(&dashboard_page);

    let sidebar_page = adw::NavigationPage::builder()
        .title(crate::i18n::t(lang, "sidebar.title"))
        .child(&sidebar_toolbar)
        .build();

    let content_page = adw::NavigationPage::builder()
        .title("LianLiGTK")
        .child(&content_nav)
        .build();

    let split_view = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content_page)
        .build();

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&split_view));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("LianLiGTK")
        .default_width(980)
        .default_height(640)
        .content(&toast_overlay)
        .build();

    let ctx: Rc<Ctx> = Rc::new(Ctx {
        client,
        state: state.clone(),
        toast_overlay: toast_overlay.clone(),
        nav: content_nav.clone(),
        custom_names: Rc::new(RefCell::new(crate::device_names::load())),
        rgb_prefs: Rc::new(RefCell::new(crate::device_rgb_prefs::load())),
        device_order: Rc::new(RefCell::new(crate::device_order::load())),
        app_prefs: Rc::new(RefCell::new(app_prefs)),
        last_effect: Rc::new(RefCell::new(crate::last_effect::load())),
        editor_snapshots: Rc::new(RefCell::new(crate::editor_snapshots::load())),
    });

    {
        let ctx = ctx.clone();
        glib::spawn_future_local(async move {
            crate::rgb_persist::clear_stale_wireless_animations(&ctx).await;
        });
    }

    // Closing just hides the window; the tray (or relaunching) brings it
    // back or quits for real.
    {
        let window = window.clone();
        let ctx = ctx.clone();
        window.connect_close_request(move |window| {
            window.set_visible(false);
            ctx.toast(ctx.t("window.minimized_to_tray"));
            glib::Propagation::Stop
        });
    }

    let (tray_tx, tray_rx) = async_channel::unbounded::<crate::tray::TrayEvent>();
    crate::tray::spawn(tray_tx, ctx.lang());
    {
        let window = window.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = tray_rx.recv().await {
                match event {
                    crate::tray::TrayEvent::ToggleShow => {
                        if window.is_visible() {
                            window.set_visible(false);
                        } else {
                            window.present();
                        }
                    }
                    crate::tray::TrayEvent::Quit => std::process::exit(0),
                }
            }
        });
    }

    {
        let ctx = ctx.clone();
        let window = window.clone();
        prefs_row.connect_activated(move |_| {
            preferences::open(&ctx, &window);
        });
    }

    {
        let ctx = ctx.clone();
        sync_button.connect_clicked(move |_| {
            let ctx = ctx.clone();
            ctx.push_singleton("global-effects", || global_effects::page(&ctx));
        });
    }

    {
        let ctx = ctx.clone();
        wireless_button.connect_clicked(move |_| {
            let ctx = ctx.clone();
            ctx.push_singleton("wireless-pairing", || wireless_pairing::page(&ctx));
        });
    }

    let devices_for_select: Rc<RefCell<Vec<DeviceInfo>>> = Rc::new(RefCell::new(Vec::new()));
    let selected_device_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // Forces a sidebar rebuild for a rename, which doesn't change device_ids.
    let names_dirty: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Suppresses the nav-pop-to-dashboard when a sidebar rebuild reselects
    // the same device under a changed id (bind/unbind changes the prefix).
    let suppress_nav_pop: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    {
        let devices_for_select = devices_for_select.clone();
        let selected_device_id = selected_device_id.clone();
        let detail_box = detail_box.clone();
        let detail_header = detail_header.clone();
        let ctx = ctx.clone();
        let names_dirty = names_dirty.clone();
        let suppress_nav_pop = suppress_nav_pop.clone();
        sidebar_list.connect_row_selected(move |_, row| {
            let Some(row) = row else {
                *selected_device_id.borrow_mut() = None;
                return;
            };
            let index = row.index();
            if index < 0 {
                return;
            }
            if let Some(device) = devices_for_select.borrow().get(index as usize) {
                let is_device_change = selected_device_id.borrow().as_deref() != Some(device.device_id.as_str());
                if is_device_change && !suppress_nav_pop.get() {
                    ctx.nav.pop_to_tag("dashboard");
                }
                *selected_device_id.borrow_mut() = Some(device.device_id.clone());
                detail_header.set_title_widget(Some(&gtk::Label::new(Some(&ctx.display_name(device)))));
                let telemetry = ctx.state.borrow().telemetry.clone();
                render_device_detail(&detail_box, device, &telemetry, &ctx, &names_dirty);
            }
        });
    }

    let known_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let state_for_update = state.clone();
    let sidebar_list_for_update = sidebar_list.clone();
    let devices_for_update = devices_for_select.clone();
    let detail_box_for_update = detail_box.clone();
    let selected_device_id_for_update = selected_device_id.clone();
    let ctx_for_update = ctx.clone();
    let names_dirty_for_update = names_dirty.clone();
    let suppress_nav_pop_for_update = suppress_nav_pop.clone();
    let on_update: Rc<dyn Fn()> = Rc::new(move || {
        // Sorted first: `ListDevices` doesn't guarantee stable order between
        // polls, so comparing the raw order alone triggered spurious sidebar
        // rebuilds even when the device set hadn't changed.
        let devices = ctx_for_update.sort_by_saved_order(state_for_update.borrow().devices.clone());
        let telemetry = state_for_update.borrow().telemetry.clone();
        // Sorted independently of display order, so a reorder alone never
        // counts as "the device set changed".
        let mut new_ids: Vec<String> = devices.iter().map(|d| d.device_id.clone()).collect();
        new_ids.sort();

        if *known_ids.borrow() != new_ids || names_dirty_for_update.get() {
            // Dropped before rebuild_sidebar: removing the selected row
            // fires "row-selected" synchronously with row=None, re-entering
            // this same RefCell.
            let selected_snapshot = selected_device_id_for_update.borrow().clone();
            // Written before the rebuild: rebuild_sidebar auto-selects a row
            // synchronously, and that fires the row-selected handler, which
            // reads this same map to look up the DeviceInfo to render.
            *devices_for_update.borrow_mut() = devices.clone();
            rebuild_sidebar(
                &sidebar_list_for_update,
                &devices,
                &selected_snapshot,
                &ctx_for_update,
                &suppress_nav_pop_for_update,
            );
            *known_ids.borrow_mut() = new_ids;
            names_dirty_for_update.set(false);
        } else {
            *devices_for_update.borrow_mut() = devices;
        }

        if let Some(ref device_id) = *selected_device_id_for_update.borrow() {
            if let Some(device) = devices_for_update
                .borrow()
                .iter()
                .find(|d| &d.device_id == device_id)
            {
                render_device_detail(
                    &detail_box_for_update,
                    device,
                    &telemetry,
                    &ctx_for_update,
                    &names_dirty_for_update,
                );
            }
        }
    });

    {
        let ctx = ctx.clone();
        let on_update = on_update.clone();
        refresh_button.connect_clicked(move |_| {
            let ctx = ctx.clone();
            let on_update = on_update.clone();
            glib::spawn_future_local(async move {
                let devices = ctx
                    .client
                    .call::<Vec<DeviceInfo>>(lianli_shared::ipc::IpcRequest::ListDevices)
                    .await;
                let telemetry = ctx
                    .client
                    .call::<TelemetrySnapshot>(lianli_shared::ipc::IpcRequest::GetTelemetry)
                    .await;
                if let Ok(devices) = devices {
                    ctx.state.borrow_mut().devices = devices;
                }
                if let Ok(telemetry) = telemetry {
                    ctx.state.borrow_mut().telemetry = telemetry;
                }
                on_update();
            });
        });
    }

    {
        let on_update = on_update.clone();
        start_polling(ctx.client.clone(), state, move || on_update());
    }

    let start_hidden = std::env::args().any(|arg| arg == "--start-hidden");
    if !start_hidden {
        window.present();
    }
}

fn build_sidebar_pane(list: &gtk::ListBox) -> gtk::Widget {
    let scrolled = gtk::ScrolledWindow::builder()
        .child(list)
        .vexpand(true)
        .build();
    scrolled.upcast()
}

fn rebuild_sidebar(
    list: &gtk::ListBox,
    devices: &[DeviceInfo],
    selected_device_id: &Option<String>,
    ctx: &Rc<Ctx>,
    suppress_nav_pop: &Rc<Cell<bool>>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    // Shared with the ▲▼ reorder buttons: mutated, persisted, and rebuilt
    // from immediately on click.
    let ordered_devices: Rc<RefCell<Vec<DeviceInfo>>> = Rc::new(RefCell::new(devices.to_vec()));

    let mut row_to_select: Option<gtk::ListBoxRow> = None;
    let count = devices.len();

    for (index, device) in devices.iter().enumerate() {
        let row = build_device_row(device, &ctx.display_name(device), ctx);

        if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
            let up_button = gtk::Button::builder()
                .icon_name("go-up-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "sidebar-reorder-btn"])
                .sensitive(index > 0)
                .tooltip_text(ctx.t("sidebar.move_up"))
                .build();
            let down_button = gtk::Button::builder()
                .icon_name("go-down-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "sidebar-reorder-btn"])
                .sensitive(index + 1 < count)
                .tooltip_text(ctx.t("sidebar.move_down"))
                .build();

            {
                let ordered_devices = ordered_devices.clone();
                let list = list.clone();
                let selected_device_id = selected_device_id.clone();
                let ctx = ctx.clone();
                let suppress_nav_pop = suppress_nav_pop.clone();
                up_button.connect_clicked(move |_| {
                    ordered_devices.borrow_mut().swap(index, index - 1);
                    let new_order = ordered_devices.borrow().iter().map(|d| d.device_id.clone()).collect();
                    ctx.set_device_order(new_order);
                    rebuild_sidebar(&list, &ordered_devices.borrow(), &selected_device_id, &ctx, &suppress_nav_pop);
                });
            }
            {
                let ordered_devices = ordered_devices.clone();
                let list = list.clone();
                let selected_device_id = selected_device_id.clone();
                let ctx = ctx.clone();
                let suppress_nav_pop = suppress_nav_pop.clone();
                down_button.connect_clicked(move |_| {
                    ordered_devices.borrow_mut().swap(index, index + 1);
                    let new_order = ordered_devices.borrow().iter().map(|d| d.device_id.clone()).collect();
                    ctx.set_device_order(new_order);
                    rebuild_sidebar(&list, &ordered_devices.borrow(), &selected_device_id, &ctx, &suppress_nav_pop);
                });
            }

            action_row.add_suffix(&up_button);
            action_row.add_suffix(&down_button);

            // A small drag handle so dragging doesn't fight click-to-select.
            let handle = gtk::Image::from_icon_name("list-drag-handle-symbolic");
            handle.set_css_classes(&["sidebar-drag-handle"]);
            handle.set_valign(gtk::Align::Center);
            action_row.add_prefix(&handle);

            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gtk::gdk::DragAction::MOVE);
            {
                let device_id = device.device_id.clone();
                drag_source.connect_prepare(move |_, _, _| {
                    Some(gtk::gdk::ContentProvider::for_value(&device_id.to_value()))
                });
            }
            handle.add_controller(drag_source);

            let drop_target = gtk::DropTarget::new(glib::types::Type::STRING, gtk::gdk::DragAction::MOVE);
            {
                let ordered_devices = ordered_devices.clone();
                let list = list.clone();
                let selected_device_id = selected_device_id.clone();
                let ctx = ctx.clone();
                let suppress_nav_pop = suppress_nav_pop.clone();
                drop_target.connect_drop(move |_, value, _, _| {
                    let Ok(dragged_id) = value.get::<String>() else { return false };
                    let devices = ordered_devices.borrow();
                    let Some(from) = devices.iter().position(|d| d.device_id == dragged_id) else {
                        return false;
                    };
                    drop(devices);
                    if from == index {
                        return true;
                    }
                    let mut devices = ordered_devices.borrow_mut();
                    let moved = devices.remove(from);
                    devices.insert(index, moved);
                    let new_order = devices.iter().map(|d| d.device_id.clone()).collect();
                    drop(devices);
                    ctx.set_device_order(new_order);
                    rebuild_sidebar(&list, &ordered_devices.borrow(), &selected_device_id, &ctx, &suppress_nav_pop);
                    true
                });
            }
            action_row.add_controller(drop_target);
        }

        list.append(&row);

        if Some(&device.device_id) == selected_device_id.as_ref() {
            row_to_select = list.row_at_index(index as i32);
        }
    }

    // Nothing selected yet — prefer the "Default device" from Preferences,
    // else the first row in order.
    let row_to_select = row_to_select.or_else(|| {
        let default_index = ctx
            .default_device_id()
            .and_then(|default_id| devices.iter().position(|d| d.device_id == default_id));
        list.row_at_index(default_index.unwrap_or(0) as i32)
    });
    if let Some(row) = row_to_select {
        suppress_nav_pop.set(true);
        list.select_row(Some(&row));
        suppress_nav_pop.set(false);
    }
}

/// RGB-only cable/strip/ring families — no fan blade, pump, or LCD.
fn is_cable_family(family: DeviceFamily) -> bool {
    matches!(
        family,
        DeviceFamily::WirelessStrimer
            | DeviceFamily::WirelessLc217
            | DeviceFamily::WirelessLed88
            | DeviceFamily::StrimerPlus
            | DeviceFamily::UniversalScreenLighting
    )
}

fn build_device_row(device: &DeviceInfo, display_name: &str, ctx: &Rc<Ctx>) -> gtk::Widget {
    let subtitle = if device.is_unbound_wireless {
        format!("<span foreground=\"#e5a50a\">● {}</span>", ctx.t("sidebar.unpaired"))
    } else {
        format!("<span foreground=\"#33d17a\">● {}</span>", ctx.t("sidebar.connected"))
    };
    let row = adw::ActionRow::builder().title(glib::markup_escape_text(display_name)).subtitle(subtitle).build();

    // Custom SVGs for fan/cable — Adwaita has no icon for either.
    let icon: gtk::Image = if device.has_lcd {
        gtk::Image::from_icon_name("video-display-symbolic")
    } else if device.has_pump {
        gtk::Image::from_icon_name("weather-few-clouds-symbolic")
    } else if is_cable_family(device.family) {
        svg_icon(CABLE_SVG, "network-wired-symbolic")
    } else {
        svg_icon(FAN_SVG, "weather-windy-symbolic")
    };
    row.add_prefix(&icon);

    row.upcast()
}

fn humanize_family(family: DeviceFamily) -> String {
    let raw = format!("{family:?}");
    let mut out = String::with_capacity(raw.len() + 4);
    for (i, c) in raw.chars().enumerate() {
        if i != 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Rotating-beacon "siren" glyph, in the Identify blink's yellow. Rasterized
/// at 128×128 (viewBox stays 0-16) so it stays crisp at the 16px display size.
const SIREN_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 16 16" fill="none" stroke="#f6d32d" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round">
<path d="M8 0.5 V2"/>
<path d="M3.5 2 L4.6 3.1"/>
<path d="M12.5 2 L11.4 3.1"/>
<path d="M1.3 6 H2.8"/>
<path d="M13.2 6 H14.7"/>
<path d="M4 10 V6.8 A4 3.2 0 0 1 12 6.8 V10 Z"/>
<rect x="2" y="10" width="12" height="4" rx="1"/>
<path d="M5.5 12 H10.5"/>
</svg>"##;

/// A 3-blade pinwheel — Adwaita has no fan icon. Also reused for the tray
/// icon (see `tray.rs`).
pub(crate) const FAN_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 16 16" fill="none" stroke="#9a9996" stroke-width="1" stroke-linecap="round" stroke-linejoin="round">
<circle cx="8" cy="8" r="6.5"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(0 8 8)"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(120 8 8)"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(240 8 8)"/>
<circle cx="8" cy="8" r="1.2" fill="#9a9996"/>
</svg>"##;

/// USB-C plug at the end of a short cable, for Strimer/LED-strip devices.
const CABLE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 16 16" fill="none" stroke="#9a9996" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round">
<path d="M1 8 H4"/>
<rect x="4" y="5.5" width="10.5" height="5" rx="2.5"/>
</svg>"##;

fn svg_icon(svg: &'static str, fallback_icon_name: &str) -> gtk::Image {
    let bytes = glib::Bytes::from_static(svg.as_bytes());
    match gtk::gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => {
            let image = gtk::Image::from_paintable(Some(&texture));
            image.set_pixel_size(16);
            image
        }
        Err(_) => gtk::Image::from_icon_name(fallback_icon_name),
    }
}

fn siren_icon() -> gtk::Image {
    svg_icon(SIREN_SVG, "alarm-symbolic")
}

fn render_device_detail(
    container: &gtk::Box,
    device: &DeviceInfo,
    telemetry: &TelemetrySnapshot,
    ctx: &Rc<Ctx>,
    names_dirty: &Rc<Cell<bool>>,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(700).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let title_row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).build();
    let title = gtk::Label::builder()
        .label(ctx.display_name(device))
        .css_classes(["title-2"])
        .halign(gtk::Align::Start)
        .build();
    title_row.append(&title);

    let rename_button = gtk::Button::builder()
        .icon_name("document-edit-symbolic")
        .css_classes(["flat", "circular"])
        .valign(gtk::Align::Center)
        .tooltip_text(ctx.t("dashboard.rename_tooltip"))
        .build();
    title_row.append(&rename_button);

    // Right-aligned spacer pushes Identify to the far end of the row.
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    title_row.append(&spacer);

    if device.has_rgb {
        let identify_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        identify_content.append(&siren_icon());
        identify_content.append(&gtk::Label::new(Some(ctx.t("dashboard.identify"))));
        let identify_button = gtk::Button::builder()
            .child(&identify_content)
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .tooltip_text(ctx.t("dashboard.identify_tooltip"))
            .build();
        let ctx = ctx.clone();
        let device_id = device.device_id.clone();
        identify_button.connect_clicked(move |_| {
            let ctx = ctx.clone();
            let device_id = device_id.clone();
            glib::spawn_future_local(async move {
                crate::identify::identify(&ctx, &device_id).await;
            });
        });
        title_row.append(&identify_button);
    }

    content.append(&title_row);

    {
        let ctx = ctx.clone();
        let names_dirty = names_dirty.clone();
        let device_id = device.device_id.clone();
        let current_name = ctx.display_name(device);
        rename_button.connect_clicked(move |button| {
            let Some(window) = button.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                return;
            };
            open_rename_dialog(&window, &ctx, &device_id, &current_name, &names_dirty);
        });
    }

    let subtitle_text = match &device.firmware_version {
        Some(fw) => format!("{} · Firmware {fw}", humanize_family(device.family)),
        None => humanize_family(device.family),
    };
    let subtitle = gtk::Label::builder()
        .label(subtitle_text)
        .css_classes(["dim-label"])
        .halign(gtk::Align::Start)
        .build();
    content.append(&subtitle);

    if let Some(stats) = build_stats_grid(device, telemetry, ctx) {
        content.append(&stats);
    }

    if let Some(actions) = build_quick_actions(device, ctx) {
        content.append(&actions);
    }

    if let Some(chips) = build_capability_chips(device, ctx) {
        content.append(&chips);
    }

    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    container.append(&scrolled);
}

fn build_stats_grid(device: &DeviceInfo, telemetry: &TelemetrySnapshot, ctx: &Rc<Ctx>) -> Option<gtk::Widget> {
    let mut stats: Vec<(&str, String)> = Vec::new();

    if device.has_fan {
        if let Some(rpms) = telemetry.fan_rpms.get(&device.device_id) {
            if !rpms.is_empty() {
                let avg = rpms.iter().map(|&r| r as u32).sum::<u32>() / rpms.len() as u32;
                stats.push((ctx.t("dashboard.fan_speed_avg"), format!("{avg} RPM")));
            }
        }
    }

    if let Some(temp) = telemetry.coolant_temps.get(&device.device_id) {
        stats.push((ctx.t("dashboard.coolant_temp"), format!("{temp:.1} °C")));
    }

    if device.has_rgb {
        if let Some(effect) = ctx.last_effect_for(&device.device_id) {
            stats.push((ctx.t("dashboard.current_effect"), effect.mode().display_name().to_string()));
        }
    }

    if stats.is_empty() {
        return None;
    }

    let grid = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .homogeneous(true)
        .build();

    for (label, value) in stats {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["card"])
            .build();

        let label_widget = gtk::Label::builder()
            .label(label)
            .css_classes(["caption", "dim-label"])
            .halign(gtk::Align::Start)
            .margin_top(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        let value_widget = gtk::Label::builder()
            .label(value)
            .css_classes(["title-3"])
            .halign(gtk::Align::Start)
            .margin_bottom(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        card.append(&label_widget);
        card.append(&value_widget);
        grid.append(&card);
    }

    Some(grid.upcast())
}

fn build_quick_actions(device: &DeviceInfo, ctx: &Rc<Ctx>) -> Option<gtk::Widget> {
    let group = adw::PreferencesGroup::builder().title(ctx.t("dashboard.quick_actions")).build();
    let mut any = false;

    if device.has_rgb {
        let device_id = device.device_id.clone();
        let ctx = ctx.clone();
        group.add(&nav_action_row(ctx.t("dashboard.rgb_effect_action"), move |ctx| {
            ctx.push(&rgb_editor::page(ctx, &device_id));
        }, &ctx));
        any = true;
    }
    if device.has_fan {
        let ctx = ctx.clone();
        group.add(&nav_action_row(ctx.t("dashboard.fan_curve"), move |ctx| {
            ctx.push(&fan_curve::page(ctx));
        }, &ctx));
        any = true;
    }
    if device.has_lcd {
        let device_id = device.device_id.clone();
        let ctx = ctx.clone();
        group.add(&nav_action_row(ctx.t("dashboard.lcd_content"), move |ctx| {
            ctx.push(&lcd_content::page(ctx, &device_id));
        }, &ctx));
        any = true;
    }
    any.then(|| group.upcast())
}

fn nav_action_row(
    title: &str,
    action: impl Fn(&Rc<Ctx>) + 'static,
    ctx: &Rc<Ctx>,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).activatable(true).build();
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let ctx = ctx.clone();
    row.connect_activated(move |_| action(&ctx));
    row
}

/// Small green-check pills, one per capability the device actually has.
fn build_capability_chips(device: &DeviceInfo, ctx: &Rc<Ctx>) -> Option<gtk::Widget> {
    let supported: Vec<&str> = [
        (ctx.t("dashboard.cap_fan"), device.has_fan),
        (ctx.t("dashboard.cap_rgb"), device.has_rgb),
        (ctx.t("dashboard.cap_lcd"), device.has_lcd),
        (ctx.t("dashboard.cap_pump"), device.has_pump),
    ]
    .into_iter()
    .filter(|(_, has)| *has)
    .map(|(label, _)| label)
    .collect();

    if supported.is_empty() {
        return None;
    }

    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.append(
        &gtk::Label::builder()
            .label(ctx.t("dashboard.capabilities"))
            .css_classes(["caption-heading", "dim-label"])
            .halign(gtk::Align::Start)
            .build(),
    );

    let chips = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .row_spacing(6)
        .column_spacing(6)
        .halign(gtk::Align::Start)
        .build();
    for label in supported {
        let chip = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .css_classes(["capability-chip"])
            .build();
        let icon = gtk::Image::from_icon_name("emblem-ok-symbolic");
        icon.add_css_class("success");
        chip.append(&icon);
        chip.append(&gtk::Label::new(Some(label)));
        chips.append(&chip);
    }
    container.append(&chips);

    Some(container.upcast())
}

/// Prompts for a new local nickname for `device_id` — see `crate::device_names`.
fn open_rename_dialog(
    parent: &gtk::Window,
    ctx: &Rc<Ctx>,
    device_id: &str,
    current_name: &str,
    names_dirty: &Rc<Cell<bool>>,
) {
    let entry = gtk::Entry::builder().text(current_name).hexpand(true).build();

    let dialog = adw::AlertDialog::builder()
        .heading(ctx.t("rename.title"))
        .body(ctx.t("rename.body"))
        .extra_child(&entry)
        .build();
    dialog.add_response("cancel", ctx.t("rename.cancel"));
    dialog.add_response("reset", ctx.t("rename.reset"));
    dialog.add_response("save", ctx.t("rename.save"));
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let ctx = ctx.clone();
    let device_id = device_id.to_string();
    let names_dirty = names_dirty.clone();
    dialog.connect_response(None, move |dialog, response| {
        match response {
            "save" => {
                let text = entry.text().to_string();
                ctx.set_custom_name(&device_id, Some(text));
                names_dirty.set(true);
            }
            "reset" => {
                ctx.set_custom_name(&device_id, None);
                names_dirty.set(true);
            }
            _ => {}
        }
        dialog.close();
    });

    dialog.present(Some(parent));
}

fn load_app_css() {
    let provider = gtk::CssProvider::new();
    // 60fps sits at (60-5)/(120-5) ≈ 47.8% of the extended 5-120 range.
    provider.load_from_string(
        ".fps-warning-zone trough { \
            background-image: linear-gradient(to right, \
                transparent 0%, transparent 47.8%, \
                alpha(#f6d32d, 0.30) 47.8%, alpha(#f6d32d, 0.30) 100%); \
        } \
        button.segmented-pill { \
            min-height: 0; \
            padding: 5px 12px; \
            font-size: 0.95em; \
        } \
        row { \
            min-height: 0; \
        } \
        list.boxed-list > row, list.navigation-sidebar > row { \
            padding: 2px 0; \
        } \
        .capability-chip { \
            padding: 4px 10px; \
            border-radius: 999px; \
            background-color: alpha(@success_color, 0.15); \
            font-size: 0.9em; \
        } \
        .sidebar-reorder-btn { \
            min-width: 16px; \
            min-height: 16px; \
            padding: 2px; \
        } \
        .sidebar-reorder-btn image { \
            -gtk-icon-size: 12px; \
        } \
        .sidebar-drag-handle { \
            min-width: 16px; \
            min-height: 16px; \
            opacity: 0.5; \
        }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
