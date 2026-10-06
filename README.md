# Shotori

<p align="center">
  <img src="assets/app/shotori-128.png" alt="Shotori">
</p>

[![CI](https://github.com/mengh04/shotori/actions/workflows/ci.yml/badge.svg)](https://github.com/mengh04/shotori/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/shotori.svg)](https://crates.io/crates/shotori)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Linux-8892bf)
![Status](https://img.shields.io/badge/status-beta-yellow)

A Wayland-native screenshot tool with built-in, on-device OCR — annotate,
pin, and stitch long pages without a browser extension, without a cloud
service, and without leaving the keyboard.

Shotori freezes every screen, you drag a selection, and the selection
flows to wherever it's needed: clipboard, save dialog, text (OCR), a
pinned floating copy — or a long screenshot that stitches itself while
you scroll. The entire UI is hand-drawn with
[gpui-kit](https://crates.io/crates/gpui-kit), and everything runs
locally on your machine.

**[简体中文](README.zh-CN.md)**

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
- **Pin (贴图)** — crop a selection into an always-on-top floating
  image that outlives the screenshot UI; drag it across screens,
  scroll to zoom
- **A full annotation kit** — shapes, arrows, numbered steps, freehand,
  highlighter, mosaic/blur and text, all editable after the fact
- **Keyboard-first** — every exit is one keystroke: copy, save, OCR,
  pin, long screenshot

## Status

Shotori is in active, pre-1.0 development. The core flows — capture,
annotate, copy, save, OCR, pin, long screenshots — are in daily use,
but the CLI, keybindings and theme format may still change between
releases. Tested on niri, sway and Hyprland. Bug reports and feedback
are welcome in the [issue tracker](https://github.com/mengh04/shotori/issues).

## Features

- Region selection, adjustable in place; multi-monitor aware, including mixed
  scales and rotated outputs, with selections spanning screens
- Annotations: rectangle, ellipse, line, polyline, arrow, numbered steps,
  pencil, highlighter, mosaic/blur, eraser, and text
- Select tool (`V`): drawing never selects — picking up, moving, resizing
  and retuning placed marks (double-click edits text, or a badge's value)
  all happen in this mode
- Object eraser: a brush (a live ring shows its exact footprint) or a
  rectangle sweep deletes whole annotations they touch — no half-erased
  pixels — and a single undo restores everything one sweep took
- Clear all annotations in one step (`Ctrl+Shift+Del` or the toolbar's
  trash button); the selection stays put and a single undo restores
  every mark
- Magnifier loupe while dragging a corner (or a shape's handle): a 3×
  inset of the frozen capture floats beside the point being placed —
  crosshair on the exact pixel, never covering the point itself
- Double-click existing text to edit it in place; live wrapping extends to the
  selection edge, with size/color controls and one-step undo of the edit. Text
  keeps a 2px inset; input, paste, or size changes that overflow the bottom
  are rejected rather than storing clipped text
- Pin: crop a selection into a floating always-on-top image that
  survives the overlay — drag across outputs, scroll to zoom
- Long screenshot (`Ctrl+L`): frame the scrollable content, then scroll
  it yourself, or hold the toolbar's ⇕ grab button and drag the frame's
  vertical position freely. The toolbar also carries Copy / Save /
  Cancel; a side panel streams the growing image with a highlight
  marking the current viewport position. Controls never overlap the
  capture region — with a full-screen selection the panel moves to
  another monitor, or the session runs keyboard-only (shortcuts
  announced in a notification). If part of your selection doesn't
  scroll with the rest (fixed bars, sidebars, video), the session stops
  early and keeps the partial capture instead of silently discarding it
- Copy to clipboard, save via the system "save as" dialog, or OCR to text
- Non-interactive full-screen capture from the CLI
- Optional tray icon; themes following the system light/dark appearance

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
[GitHub Releases](https://github.com/mengh04/shotori/releases).

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

Run `shotori` (or `shotori gui`): every screen freezes and a selection overlay
appears. A toolbar with equivalent buttons shows up below the selection after
release.

| Key                | Action                                                   |
| ------------------ | -------------------------------------------------------- |
| drag               | select a region                                          |
| `Ctrl+A`           | select this whole screen; again → every screen           |
| `Enter` / `Ctrl+C` | copy the selection to the clipboard                      |
| `Ctrl+S`           | save the selection — system "save as" dialog             |
| `Ctrl+O`           | OCR the selection → text to the clipboard                |
| `Ctrl+P`           | pin the selection to the screen                          |
| `Ctrl+L`           | long screenshot — scroll, or drag the frame over the      |
|                    | content; stitched live with a side preview (Enter/Ctrl+C  |
|                    | copy, Ctrl+S save, Esc cancel — same as selection).       |
|                    | Chrome-free on full-screen selections (single monitor):   |
|                    | keyboard-only, shortcuts announced at start               |
| `Esc`              | abandon the current drag / exit                          |

Annotation keys (`V` select, `R` rectangle, `E` ellipse, `L` line,
`A` arrow, `M` mosaic, `H` highlighter, `B` pencil, `N` numbered step,
`P` polyline, `T` text, `D` eraser) switch tools; undo/redo and
`Ctrl+Shift+Del` clear-all work as usual. Run `shotori --help` for the
full CLI surface.

Non-interactive capture, no overlay:

```sh
shotori full                 # capture every screen → clipboard
shotori full -p ~/Pictures   # → timestamped PNG in a directory
shotori full -d 2            # wait 2 s first
```

Themes: `shotori --theme light` (also `dark`, `high_contrast`; `auto` follows
the system). For a custom palette, copy
[`docs/theme.example.toml`](docs/theme.example.toml) to
`~/.config/shotori/theme.toml`.

## FAQ

**Which compositors are supported?**
Anything implementing `zwlr-screencopy` — niri, sway, Hyprland, KWin,
labwc, river, Wayfire, COSMIC and more. GNOME (Mutter) does not
implement it and is not supported; X11 is not supported.

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

Bug reports, compositor-compatibility feedback and patches are all
welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for build setup,
conventions and the PR checklist.

## Development

```bash
git clone https://github.com/mengh04/shotori
cd shotori
cargo build --release
cargo test    # unit tests, no compositor needed
```

Fresh machines need `pkg-config libfontconfig1-dev libfreetype-dev
libxkbcommon-dev libwayland-dev` (what CI installs). CI enforces
`cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings`
(plus the same with `--features perf`) — run all four before pushing.

- App icon: [SVG and multi-size PNG/ICO assets](assets/app/README.md); regenerate with
  `python3 tools/generate-icons.py` (requires `rsvg-convert`)
- Module map: the header of [`src/lib.rs`](src/lib.rs)

## License

[MIT](LICENSE)
