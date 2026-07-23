#!/usr/bin/env bash
# Builds and installs lian-li-gtk for the current user (no root needed):
#   - binary          -> ~/.local/bin/lian-li-gtk
#   - app icon        -> ~/.local/share/icons/hicolor/scalable/apps/
#   - .desktop entry  -> ~/.local/share/applications/
#
# Usage:
#   ./install.sh              build + install
#   ./install.sh --uninstall  remove everything this script installed
#
# This only ever touches paths under $HOME — no sudo, no system-wide
# files. It does not install GTK4/libadwaita themselves; if the build
# fails, install this distro's GTK4 + libadwaita development packages
# first (see the README's Requirements section).

set -euo pipefail

APP_ID="io.github.weversonl.LianLiGTK"
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BIN_DIR="$HOME/.local/bin"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
DESKTOP_DIR="$HOME/.local/share/applications"

BIN_DEST="$BIN_DIR/lian-li-gtk"
ICON_DEST="$ICON_DIR/$APP_ID.svg"
DESKTOP_DEST="$DESKTOP_DIR/$APP_ID.desktop"

uninstall() {
    echo "Removing lian-li-gtk..."
    rm -f "$BIN_DEST" "$ICON_DEST" "$DESKTOP_DEST"
    # Autostart entry, if the user ever enabled "start with system" from
    # Preferences — same file this app itself writes (see src/autostart.rs).
    rm -f "$HOME/.config/autostart/$APP_ID.desktop"
    hash -r
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DESKTOP_DIR" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
    echo "Done."
    exit 0
}

if [[ "${1:-}" == "--uninstall" || "${1:-}" == "-u" ]]; then
    uninstall
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found — install Rust first (https://rustup.rs)" >&2
    exit 1
fi

echo "Building lian-li-gtk (release)..."
if ! (cd "$REPO_DIR" && cargo build --release); then
    echo >&2
    echo "error: build failed — this is almost always missing GTK4/libadwaita" >&2
    echo "development packages, not a problem with lian-li-gtk itself. See the" >&2
    echo "README's Requirements section for what your distro needs installed." >&2
    exit 1
fi

echo "Installing binary to $BIN_DEST"
mkdir -p "$BIN_DIR"
install -Dm755 "$REPO_DIR/target/release/lian-li-gtk" "$BIN_DEST"

echo "Installing icon to $ICON_DEST"
install -Dm644 "$REPO_DIR/data/icons/hicolor/scalable/apps/$APP_ID.svg" "$ICON_DEST"

echo "Installing desktop entry to $DESKTOP_DEST"
mkdir -p "$DESKTOP_DIR"
# Same template as data/*.desktop, but with Exec pointed at the actual
# installed binary path instead of a bare command name — otherwise this
# only works for users who already have ~/.local/bin on PATH.
sed "s|^Exec=.*|Exec=$BIN_DEST|" "$REPO_DIR/data/$APP_ID.desktop" > "$DESKTOP_DEST"
chmod 644 "$DESKTOP_DEST"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DESKTOP_DIR" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

echo
echo "Installed. Launch from your app launcher (\"Lian Li Control\") or run:"
echo "  $BIN_DEST"
if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo
    echo "Note: $BIN_DIR isn't on your PATH, so \"lian-li-gtk\" won't work directly"
    echo "in a terminal — the app launcher entry above works regardless."
fi
