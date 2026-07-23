//! Wireless pairing: lists devices seen through the RF dongle (both bound
//! and unbound) and lets the user Bind/Unbind by MAC.
//!
//! Unbound devices show up in `ListDevices` with `is_unbound_wireless: true`,
//! keyed by `wireless-unbound:<mac>` rather than `wireless:<mac>`.

use crate::context::Ctx;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::ipc::IpcRequest;
use std::rc::Rc;
use std::time::Duration;

/// `BindWirelessDevice`/`UnbindWirelessDevice` ack as soon as the RF command
/// is queued, before the handshake actually completes — poll until
/// `ListDevices` reflects the new state instead of trusting the first response.
const BIND_POLL_ATTEMPTS: u32 = 10;
const BIND_POLL_DELAY_MS: u64 = 400;

async fn wait_for_bind_state(ctx: &Rc<Ctx>, mac: &str, expect_unbound: bool) {
    for _ in 0..BIND_POLL_ATTEMPTS {
        glib::timeout_future(Duration::from_millis(BIND_POLL_DELAY_MS)).await;
        let Ok(devices) = ctx.client.call::<Vec<lianli_shared::ipc::DeviceInfo>>(IpcRequest::ListDevices).await
        else {
            continue;
        };
        let matched = devices.iter().any(|d| {
            let device_mac = d
                .device_id
                .strip_prefix("wireless-unbound:")
                .or_else(|| d.device_id.strip_prefix("wireless:"));
            device_mac == Some(mac) && d.is_unbound_wireless == expect_unbound
        });
        ctx.state.borrow_mut().devices = devices;
        if matched {
            return;
        }
    }
}

pub fn page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(640).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let group = adw::PreferencesGroup::builder()
        .title(ctx.t("wp.group_title"))
        .description(ctx.t("wp.group_desc"))
        .build();
    content.append(&group);

    let busy_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    busy_row.set_margin_top(4);
    busy_row.set_margin_bottom(4);
    let spinner = gtk::Spinner::new();
    busy_row.append(&spinner);
    busy_row.append(&gtk::Label::builder().label(ctx.t("wp.please_wait")).css_classes(["dim-label"]).build());
    busy_row.set_visible(false);
    content.append(&busy_row);

    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    group.add(&list_box);

    populate(&list_box, ctx, &busy_row, &spinner);

    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    toolbar.set_content(Some(&scrolled));

    adw::NavigationPage::builder().title(ctx.t("wp.title")).tag("wireless-pairing").child(&toolbar).build()
}

fn populate(list_box: &gtk::Box, ctx: &Rc<Ctx>, busy_row: &gtk::Box, spinner: &gtk::Spinner) {
    let devices: Vec<_> = ctx
        .state
        .borrow()
        .devices
        .iter()
        .filter(|d| d.device_id.starts_with("wireless:") || d.device_id.starts_with("wireless-unbound:"))
        .cloned()
        .collect();

    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    if devices.is_empty() {
        list_box.append(&adw::ActionRow::builder().title(ctx.t("wp.no_devices")).build());
        return;
    }

    for device in devices {
        let mac = device
            .device_id
            .strip_prefix("wireless-unbound:")
            .or_else(|| device.device_id.strip_prefix("wireless:"))
            .unwrap_or(&device.device_id)
            .to_string();

        let row = adw::ActionRow::builder()
            .title(ctx.display_name(&device))
            .subtitle(mac.clone())
            .build();

        let button = if device.is_unbound_wireless {
            gtk::Button::builder().label(ctx.t("wp.bind")).css_classes(["suggested-action"]).build()
        } else {
            gtk::Button::builder().label(ctx.t("wp.unbind")).css_classes(["destructive-action"]).build()
        };
        button.set_valign(gtk::Align::Center);

        let ctx_cb = ctx.clone();
        let list_box_cb = list_box.clone();
        let busy_row_cb = busy_row.clone();
        let spinner_cb = spinner.clone();
        let is_unbound = device.is_unbound_wireless;
        let old_device_id = device.device_id.clone();
        button.connect_clicked(move |_| {
            let ctx = ctx_cb.clone();
            let list_box = list_box_cb.clone();
            let busy_row = busy_row_cb.clone();
            let spinner = spinner_cb.clone();
            let mac = mac.clone();
            let old_device_id = old_device_id.clone();
            glib::spawn_future_local(async move {
                list_box.set_sensitive(false);
                busy_row.set_visible(true);
                spinner.start();

                let request = if is_unbound {
                    IpcRequest::BindWirelessDevice { mac: mac.clone() }
                } else {
                    IpcRequest::UnbindWirelessDevice { mac: mac.clone() }
                };
                match ctx.client.call_unit(request).await {
                    Ok(()) => {
                        ctx.toast(if is_unbound { ctx.t("wp.device_bound") } else { ctx.t("wp.device_unbound") });

                        wait_for_bind_state(&ctx, &mac, !is_unbound).await;

                        if is_unbound {
                            // device_id changes prefix on bind; migrate the
                            // recorded effect and resend it since the
                            // hardware doesn't remember across the cycle.
                            let new_device_id = format!("wireless:{mac}");
                            ctx.migrate_last_effect(&old_device_id, &new_device_id);
                            let reapplied = crate::identify::reapply_last_effect(&ctx, &new_device_id).await;
                            if reapplied {
                                ctx.toast(ctx.t("wp.effect_reapplied"));
                            }
                        }
                    }
                    Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("wp.failed"))),
                }

                spinner.stop();
                busy_row.set_visible(false);
                list_box.set_sensitive(true);
                populate(&list_box, &ctx, &busy_row, &spinner);
            });
        });

        row.add_suffix(&button);
        list_box.append(&row);
    }
}
