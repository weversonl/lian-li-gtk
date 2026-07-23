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
use std::collections::HashMap;
use std::rc::Rc;

pub fn build_ui(app: &adw::Application) {
    load_app_css();

    let client = IpcClient::new();
    let state = new_shared_state();

    // Loaded once, up front — this same `AppPrefs` (and the `lang` inside
    // it) is what `Ctx` gets constructed with below, since several widgets
    // here need a translated string before `Ctx` (and its `t()` helper)
    // exist yet.
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
    // Manual refresh, matching the reference app's sidebar header —
    // otherwise redundant with the 1s poll, but it's what "loading now"
    // looks like in that design instead of a silent wait.
    let refresh_button = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_button.set_tooltip_text(Some(crate::i18n::t(lang, "sidebar.refresh")));
    // Global Effects touches every RGB device at once — it isn't a
    // per-device action, so it lives here as one global entry point
    // instead of showing up as a card on each device's page.
    let sync_button = gtk::Button::from_icon_name("preferences-desktop-display-symbolic");
    sync_button.set_tooltip_text(Some(crate::i18n::t(lang, "sidebar.global_effects")));
    // Same reasoning as Global Effects: pairing isn't really "about" the
    // currently-selected device (it manages the whole dongle), so it moved
    // out of each wireless device's Quick Actions into one global entry
    // point too.
    let wireless_button = gtk::Button::from_icon_name("network-wireless-symbolic");
    wireless_button.set_tooltip_text(Some(crate::i18n::t(lang, "sidebar.wireless_pairing")));
    sidebar_header.pack_end(&sync_button);
    sidebar_header.pack_end(&wireless_button);
    sidebar_header.pack_end(&refresh_button);
    // `AdwHeaderBar` centers its title by default (picked up automatically
    // from `sidebar_page`'s title below) — a custom title widget overrides
    // that with a plain left-aligned label instead, matching the reference
    // app's sidebar header instead of GNOME's usual centered-title look.
    let sidebar_title = gtk::Label::builder()
        .label(crate::i18n::t(lang, "sidebar.title"))
        .css_classes(["title"])
        .halign(gtk::Align::Start)
        .build();
    sidebar_header.set_title_widget(Some(&sidebar_title));
    sidebar_toolbar.add_top_bar(&sidebar_header);

    // Preferences lives as a footer row below the device list — the
    // reference app's bottom-left "Preferências" row — instead of a header
    // icon.
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

    // "LianLiGTK" is the app's own name, not a translated phrase — it
    // stays the same in both languages the same way a product name would.
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
        last_effect: Rc::new(RefCell::new(HashMap::new())),
        editor_snapshots: Rc::new(RefCell::new(crate::editor_snapshots::load())),
    });

    {
        let ctx = ctx.clone();
        glib::spawn_future_local(async move {
            crate::rgb_persist::clear_stale_wireless_animations(&ctx).await;
        });
    }

    // Closing the window just hides it instead of ending the process — the
    // daemon connection and telemetry polling keep running, and the tray
    // icon (or relaunching the app, which GApplication just re-presents)
    // is what actually brings it back or quits for real. Without a tray
    // icon available (e.g. no AppIndicator extension on this GNOME
    // session), this still works fine, just with no obvious way back in
    // besides relaunching — no worse than before this existed.
    {
        let window = window.clone();
        let ctx = ctx.clone();
        window.connect_close_request(move |window| {
            window.set_visible(false);
            ctx.toast(ctx.t("window.minimized_to_tray"));
            glib::Propagation::Stop
        });
    }

    // Tray runs on its own thread (see `tray.rs`'s doc comment) — every
    // click there just posts a `TrayEvent` through this channel, read back
    // here on the main thread where it's actually safe to touch `window`.
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

    // Renaming a device (cosmetic, local-only) doesn't change the set of
    // known device_ids, so the sidebar-rebuild-on-device-change check below
    // would otherwise never notice it — this flag forces one rebuild.
    let names_dirty: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // rebuild_sidebar's own `list.select_row(...)` call (restoring whatever
    // was selected before the rebuild) fires this same row-selected signal.
    // Binding/unbinding a wireless device changes its device_id
    // (`wireless-unbound:<mac>` <-> `wireless:<mac>`), so a rebuild
    // triggered by that could "reselect" what looks like a different
    // device purely because of the ID change — without this flag, that was
    // read as a genuine device switch and popped the nav stack back to the
    // Dashboard, kicking the user out of Wireless Pairing (or any other
    // sub-page) right after a bind/unbind for no reason the user asked for.
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
                // Only jump back to the Dashboard for an actual device
                // change — this handler also re-fires when a background
                // sidebar rebuild re-selects the *same* device (e.g. after
                // a rename, or a reconnect that didn't change who's
                // selected), and popping the nav stack then would boot the
                // user out of whatever sub-page they're on for no reason.
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
        let devices = state_for_update.borrow().devices.clone();
        let telemetry = state_for_update.borrow().telemetry.clone();
        let new_ids: Vec<String> = devices.iter().map(|d| d.device_id.clone()).collect();

        if *known_ids.borrow() != new_ids || names_dirty_for_update.get() {
            // Snapshot-and-drop the borrow *before* calling rebuild_sidebar:
            // removing the currently-selected row from the ListBox fires
            // GtkListBox's "row-selected" signal synchronously with row=None,
            // which re-enters this same RefCell via the row-selected handler
            // below. Passing a live `Ref` in would double-borrow it and abort
            // the process (this can't unwind — it's across a GTK/C callback
            // boundary) instead of panicking recoverably.
            let selected_snapshot = selected_device_id_for_update.borrow().clone();
            rebuild_sidebar(
                &sidebar_list_for_update,
                &devices,
                &selected_snapshot,
                &ctx_for_update,
                &suppress_nav_pop_for_update,
            );
            *devices_for_update.borrow_mut() = devices.clone();
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

    // Set by the autostart `.desktop` entry (see `autostart.rs`) — landing
    // straight in the tray on login, not popping the window in the user's
    // face before they've asked for it.
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

    let mut row_to_select: Option<gtk::ListBoxRow> = None;

    for (index, device) in devices.iter().enumerate() {
        let row = build_device_row(device, &ctx.display_name(device), ctx);
        list.append(&row);

        if Some(&device.device_id) == selected_device_id.as_ref() {
            row_to_select = list.row_at_index(index as i32);
        }
    }

    let row_to_select = row_to_select.or_else(|| list.row_at_index(0));
    if let Some(row) = row_to_select {
        // Bracket the signal this fires synchronously so the row-selected
        // handler can tell this apart from a genuine user click.
        suppress_nav_pop.set(true);
        list.select_row(Some(&row));
        suppress_nav_pop.set(false);
    }
}

/// RGB-only cable/strip/ring families — no fan blade, no pump, no LCD, so
/// none of `has_lcd`/`has_pump`/the fan-icon fallback actually describe
/// them (a Strimer cable is not a fan).
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
    // AdwActionRow's subtitle renders Pango markup (see the same pattern in
    // `preferences.rs`'s "Daemon Status" row) — plain "Connected"/"Unpaired"
    // text has no color of its own, it just inherits the dim subtitle gray,
    // so it never actually looked "green = good" the way a status dot does
    // elsewhere in the app.
    let subtitle = if device.is_unbound_wireless {
        format!("<span foreground=\"#e5a50a\">● {}</span>", ctx.t("sidebar.unpaired"))
    } else {
        format!("<span foreground=\"#33d17a\">● {}</span>", ctx.t("sidebar.connected"))
    };
    let row = adw::ActionRow::builder().title(glib::markup_escape_text(display_name)).subtitle(subtitle).build();

    // Custom SVGs for fan and cable — Adwaita has neither (see `FAN_SVG`/
    // `CABLE_SVG`'s doc comments); "fan-symbolic" doesn't actually exist in
    // Adwaita's icon set at all (GTK was silently falling back to its
    // generic "icon not found" glyph, the circle-with-a-slash, which read
    // as if the app were broken). LCD/pump devices keep real Adwaita icons
    // since those fit well enough already.
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

/// A rotating-beacon "siren" glyph — Adwaita/GNOME's icon set has nothing
/// like it (closest is `alarm-symbolic`, an alarm clock, which read as
/// exactly that: a clock, not an alert light). Rendered in the same yellow
/// the Identify blink itself uses, so the icon visually previews what the
/// button is about to do.
// Rasterized at 128×128 (viewBox stays 0-16 so every coordinate below is
// unchanged) even though it's only ever displayed at 16px — `Texture`
// decodes an SVG once at its declared width/height and GTK then scales that
// raster to fit `set_pixel_size`, so a 16px-intrinsic texture had to be
// upscaled on any HiDPI display (or even at 1x, depending on how the
// compositor rounds it), which is what read as blurry. Downscaling from a
// much higher-resolution raster instead keeps it crisp everywhere.
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

/// A 3-blade pinwheel — Adwaita has no fan icon at all (see `is_cable_family`
/// and `build_device_row`'s doc comment on why the built-in icon set kept
/// coming up short here). Neutral gray rather than a theme accent, since
/// there's no particular semantic color for "this is a fan" the way yellow
/// meant something for the Identify blink. Also reused as-is for the tray
/// icon (see `tray.rs`) — same glyph the device sidebar already uses,
/// rather than the app's separate brand icon.
pub(crate) const FAN_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 16 16" fill="none" stroke="#9a9996" stroke-width="1" stroke-linecap="round" stroke-linejoin="round">
<circle cx="8" cy="8" r="6.5"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(0 8 8)"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(120 8 8)"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(240 8 8)"/>
<circle cx="8" cy="8" r="1.2" fill="#9a9996"/>
</svg>"##;

/// A USB-C plug (the stadium-shaped connector housing) at the end of a
/// short cable — for Strimer/LED-strip devices, which have no fan blade to
/// draw at all (see `is_cable_family`).
const CABLE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 16 16" fill="none" stroke="#9a9996" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round">
<path d="M1 8 H4"/>
<rect x="4" y="5.5" width="10.5" height="5" rx="2.5"/>
</svg>"##;

/// Rasterizes `svg` (see `SIREN_SVG`'s doc comment on why 128×128 for a
/// 16px-displayed icon) into a `gtk::Image`, falling back to a named system
/// icon if the SVG loader isn't available for some reason.
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
        // `Button`'s `icon-name` and `label` builder properties both just
        // set the button's one child, so whichever was applied last quietly
        // wins — setting both here previously left the button text-only
        // with no icon at all. A custom icon+label child box is the way to
        // actually show both together.
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

    // Whatever this app last applied to the device — from RGB Editor or
    // Global Effects, whichever happened most recently — rather than the
    // static "RGB Zones" count, which was accurate but not very useful to
    // see at a glance.
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
        // NB: margins on `card` itself would only push it away from its
        // siblings (outer spacing) — they don't add inner padding around
        // its content, which is why this was rendering with the text
        // flush against the card edges. The padding has to go on the
        // children instead.
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

/// Small green-check pills, one per capability the device actually has —
/// replaces a 4-row boxed list that always showed every capability
/// (including the unsupported ones as a plain X), which took up far more
/// vertical space than this information warranted.
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

/// Prompts for a new local nickname for `device_id`. Purely cosmetic — see
/// `crate::device_names` — so there's no daemon round-trip here, just a
/// write to the local JSON file and a flag telling the next poll tick to
/// refresh the sidebar with it.
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

/// Loads app-wide CSS. Currently just the ".fps-warning-zone" class used by
/// the Global Effects FPS slider: a soft yellow tint over the trough past
/// 60fps when "Over 60fps" is enabled — not a hard technical ceiling (the
/// daemon doesn't validate `interval_ms` at all), just a quiet visual nudge
/// that this range hasn't been confirmed to look any smoother in practice.
fn load_app_css() {
    let provider = gtk::CssProvider::new();
    // 60fps sits at (60-5)/(120-5) ≈ 47.8% of the extended 5-120 range.
    //
    // The `.segmented-pill` / row rules below force a compact density —
    // recent libadwaita versions default to noticeably taller rows/buttons
    // (larger touch targets) than the reference app this project's visual
    // style is modeled on. Left unstyled, `GtkToggleButton`s in the
    // segmented control inherit that larger default sizing with no upper
    // bound, which is why they were rendering oversized.
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
