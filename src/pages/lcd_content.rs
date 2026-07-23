//! LCD content editor. Loads the device's existing `LcdConfig` (matched by
//! serial) from `GetConfig` and lets the user switch media type / solid
//! color, then pushes just that one entry back via `SetLcdMedia` — cheaper
//! and safer than reconstructing the whole `LcdConfig` (which has a dozen
//! optional fields for sensor gauges, H264 tuning, etc. that this simple
//! editor doesn't touch) from scratch.

use crate::context::Ctx;
use crate::widgets::segmented_control;
use adw::prelude::*;
use gtk::glib;
use lianli_shared::config::AppConfig;
use lianli_shared::media::MediaType;
use lianli_shared::ipc::IpcRequest;
use std::cell::RefCell;
use std::rc::Rc;

const MEDIA_TYPES: [MediaType; 4] = [MediaType::Color, MediaType::Image, MediaType::Video, MediaType::Gif];

fn media_type_label(t: MediaType, ctx: &Ctx) -> &'static str {
    match t {
        MediaType::Image => ctx.t("lcd.type_image"),
        MediaType::Video => ctx.t("lcd.type_video"),
        MediaType::Color => ctx.t("lcd.type_solid_color"),
        MediaType::Gif => ctx.t("lcd.type_gif"),
        MediaType::Sensor => ctx.t("lcd.type_sensor"),
        MediaType::Doublegauge => ctx.t("lcd.type_dual_gauge"),
        MediaType::Cooler => ctx.t("lcd.type_cooler"),
        MediaType::Custom => ctx.t("lcd.type_custom"),
    }
}

pub fn page(ctx: &Rc<Ctx>, device_id: &str) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let apply_button = gtk::Button::builder().label(ctx.t("lcd.apply")).css_classes(["suggested-action"]).build();
    header.pack_end(&apply_button);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let loading = adw::StatusPage::builder().icon_name("content-loading-symbolic").title(ctx.t("lcd.loading")).build();
    root.append(&loading);
    toolbar.set_content(Some(&root));
    let page_title = ctx.t("lcd.title");

    let ctx = ctx.clone();
    let device_id = device_id.to_string();
    glib::spawn_future_local(async move {
        let config = ctx.client.call::<AppConfig>(IpcRequest::GetConfig).await;
        root.remove(&loading);

        let device_serial = ctx
            .state
            .borrow()
            .devices
            .iter()
            .find(|d| d.device_id == device_id)
            .and_then(|d| d.serial.clone());

        let entry = match (&config, &device_serial) {
            (Ok(cfg), Some(serial)) => cfg.lcds.iter().find(|l| l.serial.as_deref() == Some(serial.as_str())).cloned(),
            _ => None,
        };

        match entry {
            Some(lcd_config) => build_editor(&root, &ctx, &device_id, lcd_config, &apply_button),
            None => root.append(
                &adw::StatusPage::builder()
                    .icon_name("dialog-warning-symbolic")
                    .title(ctx.t("lcd.no_config_title"))
                    .description(ctx.t("lcd.no_config_desc"))
                    .build(),
            ),
        }
    });

    adw::NavigationPage::builder().title(page_title).child(&toolbar).build()
}

fn build_editor(
    root: &gtk::Box,
    ctx: &Rc<Ctx>,
    device_id: &str,
    lcd_config: lianli_shared::config::LcdConfig,
    apply_button: &gtk::Button,
) {
    let lcd_config = Rc::new(RefCell::new(lcd_config));

    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();
    let clamp = adw::Clamp::builder().maximum_size(600).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let group = adw::PreferencesGroup::new();

    let type_names: Vec<&str> = MEDIA_TYPES.iter().map(|t| media_type_label(*t, ctx)).collect();
    let selected_index = MEDIA_TYPES
        .iter()
        .position(|t| *t == lcd_config.borrow().media_type)
        .unwrap_or(0);
    let type_row = adw::ActionRow::builder().title(ctx.t("lcd.media_type")).build();
    {
        let lcd_config = lcd_config.clone();
        type_row.add_suffix(&segmented_control::build(&type_names, selected_index, move |i| {
            if let Some(t) = MEDIA_TYPES.get(i) {
                lcd_config.borrow_mut().media_type = *t;
            }
        }));
    }
    group.add(&type_row);

    let default_rgb = lcd_config.borrow().rgb.unwrap_or([0, 0, 0]);
    let color_dialog = gtk::ColorDialog::builder().with_alpha(false).build();
    let rgba = gtk::gdk::RGBA::new(
        default_rgb[0] as f32 / 255.0,
        default_rgb[1] as f32 / 255.0,
        default_rgb[2] as f32 / 255.0,
        1.0,
    );
    let color_button = gtk::ColorDialogButton::builder().dialog(&color_dialog).rgba(&rgba).build();
    let color_row = adw::ActionRow::builder().title(ctx.t("lcd.type_solid_color")).build();
    color_row.add_suffix(&color_button);
    {
        let lcd_config = lcd_config.clone();
        color_button.connect_rgba_notify(move |b| {
            let rgba = b.rgba();
            lcd_config.borrow_mut().rgb = Some([
                (rgba.red() * 255.0) as u8,
                (rgba.green() * 255.0) as u8,
                (rgba.blue() * 255.0) as u8,
            ]);
        });
    }
    group.add(&color_row);

    let path_row = adw::ActionRow::builder()
        .title(ctx.t("lcd.file"))
        .subtitle(
            lcd_config
                .borrow()
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| ctx.t("lcd.no_file_selected").to_string()),
        )
        .build();
    let choose_button = gtk::Button::builder().label(ctx.t("lcd.choose")).valign(gtk::Align::Center).build();
    path_row.add_suffix(&choose_button);
    {
        let lcd_config = lcd_config.clone();
        let path_row = path_row.clone();
        let select_media_file_title = ctx.t("lcd.select_media_file");
        choose_button.connect_clicked(move |button| {
            let lcd_config = lcd_config.clone();
            let path_row = path_row.clone();
            let dialog = gtk::FileDialog::builder().title(select_media_file_title).build();
            let root = button.root().and_then(|r| r.downcast::<gtk::Window>().ok());
            dialog.open(root.as_ref(), gtk::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        path_row.set_subtitle(&path.display().to_string());
                        lcd_config.borrow_mut().path = Some(path);
                    }
                }
            });
        });
    }
    group.add(&path_row);

    content.append(&group);
    clamp.set_child(Some(&content));
    scrolled.set_child(Some(&clamp));
    root.append(&scrolled);

    let ctx = ctx.clone();
    let device_id = device_id.to_string();
    apply_button.connect_clicked(move |_| {
        let ctx = ctx.clone();
        let device_id = device_id.clone();
        let config = (*lcd_config.borrow()).clone();
        glib::spawn_future_local(async move {
            let request = IpcRequest::SetLcdMedia { device_id, config };
            match ctx.client.call_unit(request).await {
                Ok(()) => ctx.toast(ctx.t("lcd.content_applied")),
                Err(e) => ctx.toast(&format!("{}: {e}", ctx.t("lcd.failed_apply"))),
            }
        });
    });
}
