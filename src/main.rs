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
mod last_effect;
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
    // Not `app.run()` — that forwards the full `argv` (including our own
    // `--start-hidden`, read separately via `std::env::args()` in
    // `window::build_ui`) to `GApplication`'s own option parser, which
    // rejects anything it doesn't recognize as a registered `GOption` and
    // exits immediately with "Unknown option --start-hidden" — exactly
    // the flag the autostart `.desktop` entry passes on every login (see
    // `autostart.rs`), so the app silently failed to start with the
    // system every single time despite the entry itself being completely
    // correct. Passing only `argv[0]` sidesteps `GApplication`'s parsing
    // entirely while leaving our own `std::env::args()` check unaffected.
    let argv0: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&argv0)
}
