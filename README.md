# Shotori

<p align="center">
  <img src="assets/app/shotori-128.png" alt="Shotori">
</p>

[![CI](https://github.com/shotori-screenshot/shotori/actions/workflows/ci.yml/badge.svg)](https://github.com/shotori-screenshot/shotori/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/shotori.svg)](https://crates.io/crates/shotori)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Linux-8892bf)
[![Built with gpui-kit](https://img.shields.io/badge/built_with-gpui--kit-8892bf)](https://gpui-kit.com)
![Status](https://img.shields.io/badge/status-beta-yellow)

A Wayland-native screenshot tool — select, annotate, pin, long-capture
scrolling pages, and read text with on-device OCR.

Shotori freezes every screen, you drag a selection, and the selection
flows to wherever it's needed: clipboard, save dialog, text, a pinned
floating copy — or a long screenshot that stitches itself while you
scroll. The entire UI is hand-drawn with
[gpui-kit](https://crates.io/crates/gpui-kit), and everything runs
locally on your machine.

**[简体中文](README.zh-CN.md)**

![Shotori — region selection with two pins floating over the desktop](assets/screenshots/selection-and-pins.png)

## Highlights

- **Wayland-native** — built on `wlr-screencopy` and layer-shell; at
  home on niri, sway, Hyprland and other wlroots-adjacent compositors.
  Multi-monitor setups with mixed scales and rotated outputs are
  handled, selections may span screens
- **Private OCR** — text recognition runs on-device; after a one-time
  ~31 MB model download, nothing ever leaves your machine
- **Long screenshots** — frame the scrolling region, scroll it
  yourself, and watch the stitched page grow in a live side preview.
  If part of the selection doesn't scroll along (fixed bars, video),
  the session stops safely and keeps what it captured
- **Pin** — crop a selection into an always-on-top floating image that
  outlives the screenshot UI; drag it across screens, scroll to zoom
- **A full annotation kit** — shapes, arrows, numbered steps, freehand,
  highlighter, mosaic/blur and text, all editable after the fact

## Status

Shotori is in active, pre-1.0 development. The core flows — capture,
annotate, copy, save, OCR, pin, long screenshots — are in daily use,
but the CLI, keybindings and theme format may still change between
releases. Tested on niri and Hyprland; other compositors are untested —
trying Shotori on yours and reporting how it goes is a great way to
help. The [issue tracker](https://github.com/shotori-screenshot/shotori/issues)
is open for bugs, rough edges and feature wishes alike.

## Features

- Region selection, adjustable in place; multi-monitor aware, including
  mixed scales and rotated outputs, with selections spanning screens
- Annotations: shapes, arrows, numbered steps, pencil, highlighter,
  mosaic and text — all editable and undoable after the fact
- Pin: crop a selection into an always-on-top floating image — drag it
  across screens, scroll to zoom
- Long screenshots: stitch scrolling content into one image, with a
  live side preview
- On-device OCR: turn a selection into plain text (one-time model
  download on first use)
- Copy to clipboard, or save via the system "save as" dialog
- Non-interactive full-screen capture from the CLI
- Optional tray icon

## Requirements

- A wlroots-adjacent Wayland compositor (niri, sway, Hyprland, …)
- `xdg-desktop-portal` for the save dialog (installed by default on most
  desktops)
- A notification daemon (dunst, mako, swaync, …) is optional

## Installation

```bash
cargo install shotori        # crates.io
paru -S shotori              # AUR (prebuilt binary)
```

Or grab a binary from
[GitHub Releases](https://github.com/shotori-screenshot/shotori/releases).

For a launcher entry and desktop icon, run the following from the source tree
or an extracted release archive, after putting `shotori` on your desktop's PATH:

```bash
sh tools/install-desktop.sh   # installs to ~/.local/share; no root needed
```

Packagers can use `DESTDIR="$pkgdir" sh tools/install-desktop.sh /usr`.
Tray and notification icons are embedded and work without this installation.
AUR packages install the desktop resources automatically.

Bind it to a key, e.g. in niri:

```kdl
Mod+Shift+S { spawn "shotori"; }
```

`shotori tray` stays resident with a tray icon (StatusNotifierItem; works
with waybar, KDE Plasma, and the GNOME appindicator extension).

## Usage

Run `shotori` (or `shotori gui`): every screen freezes, drag out a
selection, and pick an action from the toolbar that appears below it —
copy, save, OCR, pin, or long screenshot.

## FAQ

**Which compositors are supported?**
Shotori needs `zwlr-screencopy`, which most wlroots-adjacent
compositors implement (niri, sway, Hyprland, KWin, labwc, river,
Wayfire, COSMIC…). So far it has only been tested on niri and
Hyprland — reports from other compositors are welcome. GNOME (Mutter)
does not implement it; X11 is not supported.

**Where do the OCR models come from? Is my text uploaded anywhere?**
Models (PP-OCR) are downloaded once, on first use, and inference runs
entirely on your device. No telemetry, no network use afterwards.

**Why do I scroll myself in long screenshots?**
Compositors coalesce injected wheel events per client, which makes
reliable auto-scroll impossible to promise across apps. Manual scrolling
works everywhere, today; you can also drag the frame itself over
content that never scrolls (canvas viewers, image panes).

**A long screenshot stopped early with a notice.**
Part of the selection most likely doesn't scroll with the rest (a fixed
header, sidebar or video breaks the stitching assumption). The partial
capture is kept — re-select only the scrolling area for a clean result.

## Contributing

Issues are just as valuable as code. Found a bug, something that feels
off, or a feature you wish existed?
[Open an issue](https://github.com/shotori-screenshot/shotori/issues) —
patches are welcome too. [CONTRIBUTING.md](CONTRIBUTING.md) has the
build setup, conventions and the PR checklist.

## Development

```bash
git clone https://github.com/shotori-screenshot/shotori
cd shotori
cargo build --release
cargo test    # unit tests, no compositor needed
```

Build dependencies, the checks CI runs, code conventions and the PR
checklist live in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
