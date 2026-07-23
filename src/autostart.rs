//! "Start with system" via a standard XDG autostart `.desktop` file.

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

/// Writes (or removes) the autostart entry, launching hidden into the tray.
pub fn set_enabled(enabled: bool) {
    let path = desktop_path();
    if !enabled {
        let _ = fs::remove_file(path);
        return;
    }

    let Ok(exe) = std::env::current_exe() else { return };
    // `/proc/self/exe` gets " (deleted)" appended if the binary was
    // replaced (e.g. reinstalled) while this process was still running.
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
