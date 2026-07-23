//! System tray icon (StatusNotifierItem via `ksni`).
//!
//! `ksni`'s D-Bus service runs on its own thread — `AppTray` below executes
//! there, not on the GTK main thread, so it must never touch GTK/glib
//! objects directly. Clicks just post a `TrayEvent`; `window.rs` reads it
//! back on the main thread.

use gdk::prelude::{TextureExt, TextureExtManual};
use gtk::gdk;
use ksni::blocking::TrayMethods;

/// Same pinwheel as `window::FAN_SVG`, recolored white for visibility as a
/// small standalone tray icon.
const FAN_SVG_TRAY: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 16 16" fill="none" stroke="#f6f5f4" stroke-width="1" stroke-linecap="round" stroke-linejoin="round">
<circle cx="8" cy="8" r="6.5"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(0 8 8)"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(120 8 8)"/>
<path d="M8 8 C6.6 6.9 6.6 4.1 8 2.2 C9.4 4.1 9.4 6.9 8 8 Z" transform="rotate(240 8 8)"/>
<circle cx="8" cy="8" r="1.2" fill="#f6f5f4"/>
</svg>"##;

#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    ToggleShow,
    Quit,
}

struct AppTray {
    tx: async_channel::Sender<TrayEvent>,
    show_hide_label: &'static str,
    quit_label: &'static str,
    icon: Option<ksni::Icon>,
}

impl ksni::Tray for AppTray {
    fn id(&self) -> String {
        "io.github.weversonl.LianLiGTK".into()
    }

    fn title(&self) -> String {
        "LianLiGTK".into()
    }

    // Fallback for hosts that ignore `icon_pixmap`.
    fn icon_name(&self) -> String {
        if self.icon.is_some() {
            String::new()
        } else {
            "network-wireless-symbolic".into()
        }
    }

    // Raw pixel data instead of a named theme icon, so it shows regardless
    // of whether this app's icon is installed in any icon theme.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone().into_iter().collect()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send_blocking(TrayEvent::ToggleShow);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: self.show_hide_label.to_string(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send_blocking(TrayEvent::ToggleShow);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: self.quit_label.to_string(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send_blocking(TrayEvent::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Rasterizes `FAN_SVG_TRAY` into ARGB32 pixel data. Must run on the main
/// GTK thread — `AppTray`'s `Tray` impl can't touch `gdk::Texture`.
fn rasterize_fan_icon() -> Option<ksni::Icon> {
    let bytes = gdk::glib::Bytes::from_static(FAN_SVG_TRAY.as_bytes());
    let texture = gdk::Texture::from_bytes(&bytes).ok()?;
    let width = texture.width();
    let height = texture.height();
    let stride = (width * 4) as usize;
    let mut data = vec![0u8; stride * height as usize];
    texture.download(&mut data, stride);

    // Cairo's ARGB32 is native-endian; ksni wants big-endian.
    for chunk in data.chunks_exact_mut(4) {
        let pixel = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        chunk.copy_from_slice(&pixel.to_be_bytes());
    }

    Some(ksni::Icon { width, height, data })
}

pub fn spawn(tx: async_channel::Sender<TrayEvent>, lang: crate::app_prefs::Lang) {
    let icon = rasterize_fan_icon();

    let tray = AppTray {
        tx,
        show_hide_label: crate::i18n::t(lang, "tray.show_hide"),
        quit_label: crate::i18n::t(lang, "tray.quit"),
        icon,
    };
    // Fails silently if no StatusNotifierItem host is available.
    let _ = tray.spawn();
}
