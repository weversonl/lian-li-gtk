# Lian Li GTK

A native GTK4 + Libadwaita control panel for Lian Li fans, AIOs, and Strimer RGB cables (wired and wireless) on Linux — built to fit naturally into a GNOME desktop.

## Why this exists

Lian Li only ships **L-Connect 3** for Windows. On Linux, the excellent [**lian-li-linux**](https://github.com/sgtaziz/lian-li-linux) project by [sgtaziz](https://github.com/sgtaziz) reverse-engineered the USB/HID protocol for wired devices and the RF protocol used by the wireless dongle, and ships it as a background daemon (`lianli-daemon`) with a Slint-based GUI.

I use a fully wireless Lian Li setup (fans + Strimer cables over the RF dongle, no wired RGB headers at all), and wanted a client that felt like a first-class GNOME app — Libadwaita widgets, a design consistent with the rest of my desktop, and a couple of quality-of-life features I kept wanting (a proper "apply one effect to every device at once" screen, and effects that don't randomly drift back to old L-Connect/Windows state when a wireless device's firmware idle-watchdog resets).

**This project does not reimplement the protocol.** It's a frontend only — all USB/HID/RF communication, fan curves, LCD streaming, etc. still happen in the upstream `lianli-daemon`. This app talks to that daemon exactly like the original Slint GUI does, over its local IPC socket.

## Screenshots

| Dashboard                     | RGB Editor                           |
| ----------------------------- | ------------------------------------ |
| ![Dashboard](assets/fans.png) | ![RGB Editor](assets/rgb-detail.png) |

| Global Effects                               | Fan Curve                          |
| -------------------------------------------- | ---------------------------------- |
| ![Global Effects](assets/global-effects.png) | ![Fan Curve](assets/fan-curve.png) |

| Wireless Pairing                                | Preferences                         |
| ----------------------------------------------- | ----------------------------------- |
| ![Wireless Pairing](assets/wireless-dongle.png) | ![Preferences](assets/settings.png) |

## Features

- **Dashboard** — sidebar of every detected device (wired and wireless) with live telemetry (RPM, temps, pump speed) and quick actions.
- **RGB Editor** — per-device, per-zone effect editor. The mode list is built dynamically from whatever the daemon reports via `GetRgbCapabilities` for that specific device, not a hardcoded list.
  - Wireless devices (which only ever report `Static`/`Direct` at the protocol level) additionally get a client-rendered animation set: Rainbow, Rainbow Morph, Breathing, and **Gradient Wave** (an OpenRGB-style custom multi-color wave, up to 8 gradient stops), all turned into raw frames on the client and streamed to the device.
  - Per-device wave direction and "LED strip count" (for cables that concatenate several physical strips into one flat buffer) are remembered individually.
- **Global Effects** — apply one effect (Static, Rainbow, Rainbow Morph, Breathing, Gradient Wave) with shared color/speed/brightness/direction across every RGB-capable device at once, wired and wireless alike. Rainbow treats every wireless device as a segment of one continuous virtual strip, in a user-configurable device order.
- **Fan Curve** — create, edit, and assign temperature → PWM curves per fan, with a draggable-point curve editor.
- **LCD Content** — preview and push image/video/GIF/color/sensor content to Strimer LCD displays.
- **Wireless Pairing** — bind/unbind devices to the RF dongle.
- **Preferences** — daemon status, OpenRGB bridge port, accent color, saved RGB presets (deleting a preset never resets the hardware — it only removes it from the list), language (English/Portuguese).
- **System tray** — minimize to tray instead of quitting; optional autostart on login.
- **Wireless-drift resilience** — applied effects are written through to the daemon's own config, so its existing auto-resync mechanism (which fixes a wireless device's firmware resetting its lighting on its own) replays the _current_ effect instead of stale leftover state.

## Tech stack

- **[Rust](https://www.rust-lang.org/)** — the whole client.
- **[gtk4-rs](https://gtk-rs.org/)** / **[libadwaita-rs](https://gitlab.gnome.org/World/Rust/libadwaita-rs)** — UI toolkit, using `AdwNavigationSplitView`, `AdwToolbarView`, `AdwPreferencesGroup`/`ActionRow`/`ComboRow`, `AdwToast`, and GNOME's standard widget vocabulary throughout.
- **[`lianli-shared`](https://github.com/sgtaziz/lian-li-linux)** — the upstream project's own IPC/config/RGB types, pulled in as a pinned git dependency (`Cargo.toml`), so the wire format with the daemon never drifts out of sync with hand-copied structs.
- **`async-net` + `futures-lite`**, driven by `glib::spawn_future_local` — the IPC client talks to the daemon's Unix socket asynchronously on GTK's own main loop, no separate thread/channel plumbing.
- **`serde` / `serde_json`** — IPC message (de)serialization, plus this app's own small local preference files (per-device RGB direction, editor snapshots, saved device order/names, language, etc.) under `~/.config/lian-li-gtk/`.
- **`ksni`** — the `StatusNotifierItem` tray icon, running on its own thread and communicating back to the GTK main thread over a channel.

## Architecture

```
┌─────────────────────┐        Unix socket, newline-delimited JSON
│   lian-li-gtk (this)│◄──────────────────────────────────────────┐
│  gtk4 / libadwaita   │                                          │
└─────────────────────┘                                           │
                                                                  ▼
                                                     ┌───────────────────────┐
                                                     │     lianli-daemon      │
                                                     │  (from lian-li-linux)  │
                                                     │  USB/HID + RF dongle   │
                                                     └───────────────────────┘
                                                                   │
                                                                   ▼
                                                     ┌───────────────────────┐
                                                     │  Fans / AIOs / Strimer │
                                                     └───────────────────────┘
```

This app is a pure IPC client of `$XDG_RUNTIME_DIR/lianli-daemon.sock`. No driver code, no protocol implementation, and no device access lives in this repository.

## Requirements

- A running `lianli-daemon` from [lian-li-linux](https://github.com/sgtaziz/lian-li-linux) (see that project's README for installation — AUR package or build from source).
- GTK 4.12+ and libadwaita 1.5+ (whatever your distro ships is almost certainly new enough on anything released in the last couple of years).

## Building

```sh
cargo build --release
./target/release/lian-li-gtk
```

The `lianli-shared` dependency is pulled straight from the upstream repo at a pinned commit, so a plain `cargo build` is enough — no need to clone `lian-li-linux` separately just to build this client.

## Installing

```sh
./install.sh
```

Builds the release binary and installs it entirely under `$HOME` — no root needed:

- binary → `~/.local/bin/lian-li-gtk`
- app icon → `~/.local/share/icons/hicolor/scalable/apps/`
- app launcher entry → `~/.local/share/applications/`

After that, "Lian Li Control" shows up in your app launcher with its own icon (window, dock, and app switcher all use it — the tray icon is deliberately the same fan glyph used for fan devices in the sidebar instead). To remove everything the script installed:

```sh
./install.sh --uninstall
```

## Credits

- [sgtaziz/lian-li-linux](https://github.com/sgtaziz/lian-li-linux) — the daemon, the protocol reverse-engineering, and the shared types this client depends on entirely. This project would not exist without that work.
- Lian Li's own **L-Connect 3** — the reference for feature parity and, in the case of Gradient Wave, the general shape of an OpenRGB-style custom effect.
