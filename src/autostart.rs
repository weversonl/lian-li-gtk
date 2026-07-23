//! "Start with system" — a standard XDG autostart `.desktop` file dropped
//! into `~/.config/autostart/`, the same mechanism every other autostart
//! toggle on Linux uses (GNOME Tweaks, KDE's own autostart settings page,
//! etc.) — no daemon/systemd unit involved, just a file the session's
//! autostart implementation reads on login.

use std::fs;
use std::path::PathBuf;

const DESKTOP_FILE_NAME: &str = "io.github.weversonl.LianLiGTK.desktop";

fn autostart_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("autostart")
}

fn desktop_path() -> PathBuf {
    autostart_dir().join(DESKTOP_FILE_NAME)
}

pub fn is_enabled() -> bool {
    desktop_path().is_file()
}

/// Writes (or removes) the autostart entry. `Exec` points at the actual
/// running binary's path (`current_exe`) with `--start-hidden`, since
/// starting at login should land straight in the tray, not pop the window
/// in the user's face before they've even opened anything.
pub fn set_enabled(enabled: bool) {
    let path = desktop_path();
    if !enabled {
        let _ = fs::remove_file(path);
        return;
    }

    let Ok(exe) = std::env::current_exe() else { return };
    // `current_exe` reads `/proc/self/exe`, a symlink the kernel appends
    // " (deleted)" to (as literal text baked into the path, not something
    // `PathBuf` knows to strip) whenever the file it points at was
    // replaced out from under the running process — e.g. this exact
    // process was already running when `install.sh` overwrote the binary
    // at the same path. Without stripping it, this wrote an `Exec=` line
    // pointing at a nonexistent path with " (deleted)" literally in it, so
    // the autostart entry silently failed to launch anything on next
    // login despite `is_enabled()` correctly reporting the toggle as on.
    let exe_str = exe.to_string_lossy();
    let exe_str = exe_str.strip_suffix(" (deleted)").unwrap_or(&exe_str);
    let dir = autostart_dir();
    let _ = fs::create_dir_all(&dir);

    let contents = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=LianLiGTK\n\
         Icon=io.github.weversonl.LianLiGTK\n\
         Exec={exe_str} --start-hidden\n\
         X-GNOME-Autostart-enabled=true\n\
         NoDisplay=false\n"
    );
    let _ = fs::write(path, contents);
}
