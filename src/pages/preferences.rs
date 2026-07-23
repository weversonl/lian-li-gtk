//! Preferences window: General (daemon status, OpenRGB port, language) and
//! Saved Presets (delete-only management of `RgbPreset`s).
//!
//! Deleting a preset here only ever sends `DeleteRgbPreset` — confirmed
//! against the daemon source (`ipc_server.rs::handle_request`, the
//! `DeleteRgbPreset` arm) that this is a catalog-only operation: it retains
//! the presets Vec minus the deleted entry and rewrites `rgb_presets.json`,
//! nothing else. It never touches `AppConfig`, never calls
//! `RgbController::apply_config`, and never sends a device command. So a
//! deleted preset does not reset the hardware — whatever effect is
//! currently running (e.g. set once via the RGB Editor or Windows/L-Connect
//! before this app ever touched the device) keeps running untouched.

use crate::app_prefs::Lang;
use crate::context::Ctx;
use crate::widgets::segmented_control;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::ipc::{IpcRequest, TelemetrySnapshot};
use lianli_shared::rgb::RgbPreset;
use std::rc::Rc;

pub fn open(ctx: &Rc<Ctx>, parent: &impl IsA<gtk::Window>) {
    let window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(520)
        .build();

    window.add(&general_page(ctx));
    let (presets_page, refresh_presets) = presets_page(ctx);
    window.add(&presets_page);

    refresh_presets();
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

    // Applied at next launch, not live — see `Lang`'s doc comment.
    let lang_group = adw::PreferencesGroup::new();
    let lang_row = adw::ActionRow::builder()
        .title(ctx.t("prefs.language"))
        .subtitle(ctx.t("prefs.language_subtitle"))
        .build();
    let lang_names = [ctx.t("prefs.lang_en"), ctx.t("prefs.lang_pt")];
    let initial_lang_index = if ctx.lang() == Lang::PtBr { 1 } else { 0 };
    {
        let ctx = ctx.clone();
        lang_row.add_suffix(&segmented_control::build(&lang_names, initial_lang_index, move |i| {
            let new_lang = if i == 1 { Lang::PtBr } else { Lang::En };
            // `segmented_control::build` activates the initially-selected
            // button as part of constructing it, which fires this same
            // callback once up front — skip it when nothing actually
            // changed, otherwise just opening Preferences popped the
            // restart toast every time.
            if new_lang == ctx.lang() {
                return;
            }
            ctx.set_lang(new_lang);
            ctx.toast_with_button(ctx.t("prefs.language_toast"), ctx.t("prefs.restart_now"), || {
                crate::restart();
            });
        }));
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

    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        match ctx.client.call::<serde_json::Value>(IpcRequest::Ping).await {
            // AdwActionRow subtitles render Pango markup, but plain "●
            // Running" text has no color of its own — it just inherits the
            // dim subtitle gray, so "Running" never actually looked green.
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

fn presets_page(ctx: &Rc<Ctx>) -> (adw::PreferencesPage, impl Fn() + 'static) {
    let page = adw::PreferencesPage::builder()
        .title(ctx.t("prefs.saved_presets"))
        .icon_name("starred-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder().title(ctx.t("prefs.saved_rgb_presets")).build();
    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    group.add(&list_box);
    page.add(&group);

    let note_group = adw::PreferencesGroup::new();
    let note_row = adw::ActionRow::builder()
        .title(ctx.t("prefs.presets_banner"))
        .subtitle(ctx.t("prefs.presets_banner_subtitle"))
        .build();
    note_row.add_prefix(&gtk::Image::from_icon_name("dialog-information-symbolic"));
    note_group.add(&note_row);
    page.add(&note_group);

    let ctx_for_refresh = ctx.clone();
    let list_box_for_refresh = list_box.clone();
    let refresh = move || {
        let ctx = ctx_for_refresh.clone();
        let list_box = list_box_for_refresh.clone();
        glib::spawn_future_local(async move {
            match ctx.client.call::<Vec<RgbPreset>>(IpcRequest::ListRgbPresets).await {
                Ok(presets) => populate_presets(&list_box, &ctx, presets),
                Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("prefs.failed_load_presets"))),
            }
        });
    };

    (page, refresh)
}

fn populate_presets(list_box: &gtk::Box, ctx: &Rc<Ctx>, presets: Vec<RgbPreset>) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    if presets.is_empty() {
        list_box.append(&adw::ActionRow::builder().title(ctx.t("prefs.no_presets")).build());
        return;
    }

    for preset in presets {
        let row = adw::ActionRow::builder()
            .title(preset.name.clone())
            .subtitle(preset.device_id.clone())
            .build();

        let delete_button = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();

        let ctx_cb = ctx.clone();
        let list_box_cb = list_box.clone();
        let name = preset.name.clone();
        let device_id = preset.device_id.clone();
        delete_button.connect_clicked(move |button| {
            let root = button.root();
            let Some(window) = root.and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                return;
            };
            confirm_delete(&window, &ctx_cb, &list_box_cb, name.clone(), device_id.clone());
        });

        row.add_suffix(&delete_button);
        list_box.append(&row);
    }
}

fn confirm_delete(
    parent: &gtk::Window,
    ctx: &Rc<Ctx>,
    list_box: &gtk::Box,
    name: String,
    device_id: String,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(format!("{} “{name}”?", ctx.t("prefs.delete_preset_q")))
        .body(ctx.t("prefs.delete_preset_body"))
        .build();
    dialog.add_response("cancel", ctx.t("prefs.cancel"));
    dialog.add_response("delete", ctx.t("prefs.delete"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let ctx = ctx.clone();
    let list_box = list_box.clone();
    dialog.connect_response(None, move |dialog, response| {
        if response != "delete" {
            return;
        }
        let ctx = ctx.clone();
        let list_box = list_box.clone();
        let name = name.clone();
        let device_id = device_id.clone();
        dialog.close();
        glib::spawn_future_local(async move {
            let request = IpcRequest::DeleteRgbPreset {
                name: name.clone(),
                device_id: device_id.clone(),
            };
            match ctx.client.call_unit(request).await {
                Ok(()) => {
                    ctx.toast(&format!("{} “{name}”", ctx.t("prefs.preset_deleted")));
                    if let Ok(presets) =
                        ctx.client.call::<Vec<RgbPreset>>(IpcRequest::ListRgbPresets).await
                    {
                        populate_presets(&list_box, &ctx, presets);
                    }
                }
                Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("prefs.failed_delete_preset"))),
            }
        });
    });

    dialog.present(Some(parent));
}
