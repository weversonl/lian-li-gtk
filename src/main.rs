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
mod fan_curve_owners;
mod i18n;
mod identify;
mod ipc_client;
mod last_effect;
mod pages;
mod profiles;
mod rgb_persist;
mod tray;
mod window;

use gtk::glib;
use gtk::prelude::*;

const APP_ID: &str = "io.github.weversonl.LianLiGTK";

/// Re-execs the binary and exits — used by the language-change toast's
/// "Restart" button, since the UI isn't bound reactively to language.
pub fn restart() -> ! {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
    std::process::exit(0);
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(window::build_ui);
    // Not `app.run()` — that forwards argv to GApplication's own option
    // parser, which rejects our `--start-hidden` flag and exits. That flag
    // is read separately via `std::env::args()` in `window::build_ui`.
    let argv0: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&argv0)
}
