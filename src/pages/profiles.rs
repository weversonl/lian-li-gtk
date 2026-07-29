//! Named full-app snapshots ("Perfis") — capture the whole current setup
//! (daemon `AppConfig` + client-side wireless/segments replay state) under a
//! name, and reapply it later in one shot. See `crate::profiles` for the
//! data model and the exact rationale for why this can't just be a
//! daemon `AppConfig` snapshot.

use crate::context::Ctx;
use crate::profiles::Profile;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::config::AppConfig;
use lianli_shared::ipc::IpcRequest;
use std::rc::Rc;
use std::time::Duration;

/// Lets `SetConfig`'s immediate RF push to wired/LCD devices settle before
/// we start hitting wireless devices with frame/segment replays — same
/// rationale as `identify::reapply_last_effect`'s own settle delay.
const SETTLE_DELAY_MS: u64 = 400;

pub fn page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(600).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let intro_group = adw::PreferencesGroup::builder()
        .title(ctx.t("profiles.title"))
        .description(ctx.t("profiles.subtitle"))
        .build();
    let save_button = gtk::Button::builder()
        .label(ctx.t("profiles.save_current"))
        .css_classes(["suggested-action"])
        .halign(gtk::Align::Start)
        .build();
    intro_group.add(&save_button);
    content.append(&intro_group);

    let list_group = adw::PreferencesGroup::new();
    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list_group.add(&list_box);
    content.append(&list_group);

    let ctx_for_refresh = ctx.clone();
    let list_box_for_refresh = list_box.clone();
    let refresh: Rc<dyn Fn()> =
        Rc::new(move || populate_profiles(&list_box_for_refresh, &ctx_for_refresh));
    refresh();

    {
        let ctx = ctx.clone();
        let refresh = refresh.clone();
        save_button.connect_clicked(move |button| {
            let Some(window) = button.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                return;
            };
            open_save_dialog(&window, &ctx, refresh.clone());
        });
    }

    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    toolbar.set_content(Some(&scrolled));

    adw::NavigationPage::builder().title(ctx.t("profiles.title")).tag("profiles").child(&toolbar).build()
}

fn populate_profiles(list_box: &gtk::ListBox, ctx: &Rc<Ctx>) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let profiles = ctx.profiles();
    if profiles.is_empty() {
        list_box.append(&adw::ActionRow::builder().title(ctx.t("profiles.none_saved")).build());
        return;
    }

    for profile in profiles {
        let row = adw::ActionRow::builder().title(profile.name.clone()).build();

        let apply_button = gtk::Button::builder()
            .label(ctx.t("profiles.apply"))
            .valign(gtk::Align::Center)
            .css_classes(["suggested-action"])
            .build();
        let delete_button = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();

        {
            let ctx = ctx.clone();
            let name = profile.name.clone();
            apply_button.connect_clicked(move |_| {
                let ctx = ctx.clone();
                let name = name.clone();
                glib::spawn_future_local(async move {
                    let profiles = ctx.profiles();
                    let Some(profile) = profiles.into_iter().find(|p| p.name == name) else { return };
                    ctx.toast(ctx.t("profiles.applying"));
                    apply_profile(&ctx, &profile).await;
                });
            });
        }

        {
            let ctx = ctx.clone();
            let refresh: Rc<dyn Fn()> = {
                let list_box = list_box.clone();
                let ctx_for_refresh = ctx.clone();
                Rc::new(move || populate_profiles(&list_box, &ctx_for_refresh))
            };
            let name = profile.name.clone();
            delete_button.connect_clicked(move |button| {
                let Some(window) = button.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
                    return;
                };
                confirm_delete(&window, &ctx, name.clone(), refresh.clone());
            });
        }

        row.add_suffix(&apply_button);
        row.add_suffix(&delete_button);
        list_box.append(&row);
    }
}

fn open_save_dialog(parent: &gtk::Window, ctx: &Rc<Ctx>, refresh: Rc<dyn Fn()>) {
    let entry = gtk::Entry::builder()
        .placeholder_text(ctx.t("profiles.name_placeholder"))
        .hexpand(true)
        .build();

    let dialog = adw::AlertDialog::builder()
        .heading(ctx.t("profiles.name_dialog_title"))
        .body(ctx.t("profiles.name_dialog_body"))
        .extra_child(&entry)
        .build();
    dialog.add_response("cancel", ctx.t("profiles.cancel"));
    dialog.add_response("save", ctx.t("profiles.save"));
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let ctx = ctx.clone();
    dialog.connect_response(None, move |dialog, response| {
        if response != "save" {
            dialog.close();
            return;
        }
        let name = entry.text().trim().to_string();
        dialog.close();
        if name.is_empty() {
            return;
        }
        let ctx = ctx.clone();
        let refresh = refresh.clone();
        glib::spawn_future_local(async move {
            ctx.toast(ctx.t("profiles.capturing"));
            match capture_profile(&ctx, name).await {
                Ok(profile) => {
                    ctx.save_profile(profile);
                    ctx.toast(ctx.t("profiles.saved"));
                    refresh();
                }
                Err(_) => ctx.toast(ctx.t("profiles.failed_capture")),
            }
        });
    });

    dialog.present(Some(parent));
}

fn confirm_delete(parent: &gtk::Window, ctx: &Rc<Ctx>, name: String, refresh: Rc<dyn Fn()>) {
    let dialog = adw::AlertDialog::builder()
        .heading(format!("{} \u{201c}{name}\u{201d}?", ctx.t("profiles.delete_q")))
        .body(ctx.t("profiles.delete_body"))
        .build();
    dialog.add_response("cancel", ctx.t("profiles.cancel"));
    dialog.add_response("delete", ctx.t("profiles.delete"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let ctx = ctx.clone();
    dialog.connect_response(None, move |dialog, response| {
        dialog.close();
        if response != "delete" {
            return;
        }
        ctx.delete_profile(&name);
        ctx.toast(ctx.t("profiles.deleted"));
        refresh();
    });

    dialog.present(Some(parent));
}

/// Snapshots the daemon's current `AppConfig` plus every client-side piece
/// of state that doesn't round-trip through it — see `crate::profiles`.
async fn capture_profile(ctx: &Rc<Ctx>, name: String) -> Result<Profile, ()> {
    let app_config = ctx.client.call::<AppConfig>(IpcRequest::GetConfig).await.map_err(|_| ())?;

    Ok(Profile {
        name,
        app_config,
        last_effect: ctx.last_effect.borrow().clone(),
        editor_snapshots: ctx.editor_snapshots.borrow().clone(),
        rgb_prefs: ctx.rgb_prefs.borrow().clone(),
        device_order: ctx.device_order.borrow().clone(),
        app_prefs: ctx.profile_app_prefs(),
    })
}

/// Restores a Profile: `SetConfig` for everything the daemon can round-trip
/// (fan curves/assignments, LCD content, wired RGB zones, AIO/ENE config),
/// then the client-side extras that `SetConfig` doesn't cover, then replays
/// wireless animated/segments state via the same path `identify::reapply_last_effect`
/// already uses for reconnects.
async fn apply_profile(ctx: &Rc<Ctx>, profile: &Profile) {
    let mut ok = 0usize;
    let mut failed = 0usize;

    if ctx.client.call_unit(IpcRequest::SetConfig { config: profile.app_config.clone() }).await.is_err() {
        ctx.toast(ctx.t("profiles.failed_capture"));
        return;
    }

    ctx.replace_rgb_prefs(profile.rgb_prefs.clone());
    ctx.replace_editor_snapshots(profile.editor_snapshots.clone());
    ctx.replace_last_effect(profile.last_effect.clone());
    ctx.set_device_order(profile.device_order.clone());
    ctx.apply_profile_app_prefs(&profile.app_prefs);

    glib::timeout_future(Duration::from_millis(SETTLE_DELAY_MS)).await;

    let devices = ctx.state.borrow().devices.clone();
    for device in devices.iter().filter(|d| d.device_id.starts_with("wireless:")) {
        let has_state = profile.last_effect.contains_key(&device.device_id)
            || profile.editor_snapshots.contains_key(&device.device_id);
        if !has_state {
            continue;
        }
        if crate::identify::reapply_last_effect(ctx, &device.device_id).await {
            ok += 1;
        } else {
            failed += 1;
        }
    }

    if failed == 0 {
        ctx.toast(ctx.t("profiles.applied"));
    } else {
        ctx.toast(&format!("{} ({ok} ok, {failed} failed)", ctx.t("profiles.applied_with_failures")));
    }
}
