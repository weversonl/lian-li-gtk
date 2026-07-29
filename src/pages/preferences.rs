//! Preferences window: daemon status, OpenRGB port, language, autostart,
//! default device.

use crate::app_prefs::Lang;
use crate::context::Ctx;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::ipc::{IpcRequest, TelemetrySnapshot};
use std::rc::Rc;

pub fn open(ctx: &Rc<Ctx>, parent: &impl IsA<gtk::Window>) {
    let window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(520)
        .build();

    window.add(&general_page(ctx));
    window.present();
}

fn general_page(ctx: &Rc<Ctx>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(ctx.t("prefs.general"))
        .icon_name("preferences-system-symbolic")
        .build();

    let daemon_group = adw::PreferencesGroup::builder().title(ctx.t("prefs.daemon_group")).build();
    let status_row = adw::ActionRow::builder()
        .title(ctx.t("prefs.daemon_status"))
        .subtitle(ctx.t("prefs.checking"))
        .use_markup(true)
        .build();
    daemon_group.add(&status_row);

    let port_row = adw::ActionRow::builder().title(ctx.t("prefs.openrgb_port")).build();
    daemon_group.add(&port_row);
    page.add(&daemon_group);

    let lang_group = adw::PreferencesGroup::new();
    const LANGS: [Lang; 2] = [Lang::En, Lang::PtBr];
    let lang_names = gtk::StringList::new(&[ctx.t("prefs.lang_en"), ctx.t("prefs.lang_pt")]);
    let lang_row = adw::ComboRow::builder()
        .title(ctx.t("prefs.language"))
        .subtitle(ctx.t("prefs.language_subtitle"))
        .model(&lang_names)
        .build();
    lang_row.set_selected(LANGS.iter().position(|l| *l == ctx.lang()).unwrap_or(0) as u32);
    {
        let ctx = ctx.clone();
        lang_row.connect_selected_notify(move |row| {
            let Some(new_lang) = LANGS.get(row.selected() as usize).copied() else { return };
            if new_lang == ctx.lang() {
                return;
            }
            ctx.set_lang(new_lang);
            ctx.toast_with_button(ctx.t("prefs.language_toast"), ctx.t("prefs.restart_now"), || {
                crate::restart();
            });
        });
    }
    lang_group.add(&lang_row);
    page.add(&lang_group);

    let background_group = adw::PreferencesGroup::builder().title(ctx.t("prefs.background")).build();
    let autostart_row = adw::ActionRow::builder()
        .title(ctx.t("prefs.start_with_system"))
        .subtitle(ctx.t("prefs.start_with_system_subtitle"))
        .build();
    let autostart_switch = gtk::Switch::builder()
        .valign(gtk::Align::Center)
        .active(crate::autostart::is_enabled())
        .build();
    autostart_switch.connect_state_set(|_, enabled| {
        crate::autostart::set_enabled(enabled);
        glib::Propagation::Proceed
    });
    autostart_row.add_suffix(&autostart_switch);
    background_group.add(&autostart_row);
    page.add(&background_group);

    let startup_group = adw::PreferencesGroup::builder().title(ctx.t("prefs.startup")).build();
    let default_device_row = adw::ComboRow::builder()
        .title(ctx.t("prefs.default_device"))
        .subtitle(ctx.t("prefs.default_device_subtitle"))
        .build();
    let devices = ctx.sort_by_saved_order(ctx.state.borrow().devices.clone());
    let mut option_labels: Vec<String> = vec![ctx.t("prefs.default_device_first_in_order").to_string()];
    option_labels.extend(devices.iter().map(|d| ctx.display_name(d)));
    let option_labels_ref: Vec<&str> = option_labels.iter().map(String::as_str).collect();
    let model = gtk::StringList::new(&option_labels_ref);
    default_device_row.set_model(Some(&model));
    let saved_default = ctx.default_device_id();
    let initial_index = saved_default
        .as_ref()
        .and_then(|id| devices.iter().position(|d| &d.device_id == id))
        .map(|i| i as u32 + 1)
        .unwrap_or(0);
    default_device_row.set_selected(initial_index);
    {
        let ctx = ctx.clone();
        let device_ids: Vec<String> = devices.iter().map(|d| d.device_id.clone()).collect();
        default_device_row.connect_selected_notify(move |row| {
            let selected = row.selected();
            let new_default =
                if selected == 0 { None } else { device_ids.get(selected as usize - 1).cloned() };
            ctx.set_default_device_id(new_default);
        });
    }
    startup_group.add(&default_device_row);
    page.add(&startup_group);

    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        match ctx.client.call::<serde_json::Value>(IpcRequest::Ping).await {
            Ok(_) => status_row.set_subtitle(&format!("<span foreground=\"#33d17a\">{}</span>", ctx.t("prefs.running"))),
            Err(_) => status_row.set_subtitle(&format!("<span foreground=\"#e01b24\">{}</span>", ctx.t("prefs.unreachable"))),
        }
        if let Ok(telemetry) = ctx.client.call::<TelemetrySnapshot>(IpcRequest::GetTelemetry).await {
            if let Some(port) = telemetry.openrgb_status.port {
                port_row.set_subtitle(&port.to_string());
            } else if telemetry.openrgb_status.enabled {
                port_row.set_subtitle(ctx.t("prefs.openrgb_enabled_not_bound"));
            } else {
                port_row.set_subtitle(ctx.t("prefs.openrgb_disabled"));
            }
        }
    });

    page
}

