//! System tray icon (StatusNotifierItem via `ksni`) — lets the app keep
//! running with the window hidden (see `window.rs`'s `close-request`
//! handler) and be brought back or fully quit from the tray, instead of
//! closing the window always ending the process.
//!
//! `ksni`'s tray runs its D-Bus service on its own background thread — the
//! `Tray` impl below (`AppTray`) is genuinely executed there, not on the
//! GTK main thread, so it must not touch any GTK/glib object directly
//! (almost everything in gtk4-rs is `Rc`-based, not `Send`). Every click
//! just posts a `TrayEvent` through an `async_channel` (a `Send` sender is
//! fine to hand to another thread) and returns immediately; `window.rs`
//! is the one actually reading that channel back on the main thread via
//! `glib::spawn_future_local` and touching the window there.

use gdk::prelude::{TextureExt, TextureExtManual};
use gtk::gdk;
use ksni::blocking::TrayMethods;

/// Same 3-blade pinwheel as `window::FAN_SVG`, just recolored white instead
/// of that gray — the sidebar's dim gray reads fine sitting next to dim
/// text on a card, but the same gray as a tiny standalone tray icon read as
/// washed-out/hard to see against a panel, unlike every other (symbolic,
/// effectively white/theme-colored) tray icon next to it.
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

    // Only used as a fallback for a host that ignores `icon_pixmap`
    // entirely, or for the (practically never hit) case where rasterizing
    // `FAN_SVG` itself failed — see `rasterize_fan_icon`.
    fn icon_name(&self) -> String {
        if self.icon.is_some() {
            String::new()
        } else {
            "network-wireless-symbolic".into()
        }
    }

    // Raw pixel data, not a named theme icon — the same fan glyph
    // `window.rs` draws for every fan device in the sidebar, just recolored
    // white (`FAN_SVG_TRAY`) for a tray-sized icon, rasterized once at
    // spawn time (see `spawn`) so the tray always shows it regardless of
    // whether any icon theme has this app's icon installed at all.
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

/// Spawns the tray icon on its own background thread. Labels are baked in
/// at spawn time (once, at startup) rather than looked up live — same as
/// every other piece of UI text in this app, the language only actually
/// changes on the next restart (see `Lang`'s doc comment), so there's
/// nothing to keep in sync here.
/// Rasterizes `FAN_SVG_TRAY` into the raw ARGB32 pixel data `ksni::Icon`
/// (and the underlying StatusNotifierItem D-Bus property) wants — has to
/// happen here, on the main GTK thread, since `AppTray`'s `Tray` impl runs
/// on ksni's own background thread and can't touch `gdk::Texture` at all
/// (see this module's doc comment).
fn rasterize_fan_icon() -> Option<ksni::Icon> {
    let bytes = gdk::glib::Bytes::from_static(FAN_SVG_TRAY.as_bytes());
    let texture = gdk::Texture::from_bytes(&bytes).ok()?;
    let width = texture.width();
    let height = texture.height();
    let stride = (width * 4) as usize;
    let mut data = vec![0u8; stride * height as usize];
    texture.download(&mut data, stride);

    // `download` writes Cairo's `ARGB32` format, i.e. each pixel is a
    // native-endian `0xAARRGGBB` word — but the StatusNotifierItem spec
    // (and `ksni::Icon`'s own doc comment) wants network byte order
    // (big-endian) ARGB bytes instead. Re-encoding through `u32` handles
    // the swap correctly regardless of the host's actual endianness.
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
    // Errors here just mean there's no StatusNotifierItem host available
    // (e.g. a GNOME session with no AppIndicator extension installed) —
    // not fatal, the app still works fully from its window, it just won't
    // have a tray icon to minimize to.
    let _ = tray.spawn();
}
