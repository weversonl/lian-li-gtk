mod app_prefs;
mod app_state;
mod autostart;
mod context;
mod device_names;
mod device_order;
mod device_rgb_prefs;
mod direction;
mod editor_snapshots;
mod effects;
mod i18n;
mod identify;
mod ipc_client;
mod pages;
mod rgb_persist;
mod tray;
mod widgets;
mod window;

use gtk::glib;
use gtk::prelude::*;

const APP_ID: &str = "io.github.weversonl.LianLiGTK";

/// Re-execs the same binary, then exits this process — used by the
/// language-change toast's "Restart" button. There's no in-place "rebuild
/// every open page in the new language" path (the whole UI is built once
/// from Rust string literals, not bound reactively to a language signal),
/// so a real process restart is the actual mechanism, not just a suggestion
/// to the user to do it themselves.
pub fn restart() -> ! {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
    std::process::exit(0);
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(window::build_ui);
    app.run()
}
