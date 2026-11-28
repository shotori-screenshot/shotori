# Shotori

<p align="center">
  <img src="assets/app/shotori-128.png" alt="Shotori" width="96" height="96">
</p>

<p align="center">
  <strong>Freeze the desktop. Route every pixel locally.</strong><br>
  A Wayland-native screenshot instrument built in Rust with <a href="https://crates.io/crates/gpui-kit">gpui-kit</a>.
</p>

<p align="center">
  <a href="https://shotori.mengh04.top"><img src="https://img.shields.io/badge/Website_%26_Live_Demos-shotori.mengh04.top-ff6a00?style=flat-square" alt="Website"></a>
  <a href="https://shotori.mengh04.top/getting-started/installation/"><img src="https://img.shields.io/badge/Docs-English%20%7C%20%E7%AE%80%E4%BD%93%E4%B8%AD%E6%96%87-202028?style=flat-square" alt="Documentation"></a>
  <a href="https://github.com/shotori-screenshot/shotori/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/shotori-screenshot/shotori/ci.yml?style=flat-square&label=CI" alt="CI"></a>
  <a href="https://crates.io/crates/shotori"><img src="https://img.shields.io/crates/v/shotori.svg?style=flat-square" alt="Crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square" alt="License: MIT"></a>
</p>

<p align="center">
  <a href="https://shotori.mengh04.top"><strong>Website & Live Demos</strong></a> ·
  <a href="https://shotori.mengh04.top/getting-started/quick-start/"><strong>Quick Start</strong></a> ·
  <a href="https://shotori.mengh04.top/reference/shortcuts/"><strong>Shortcuts</strong></a> ·
  <a href="https://shotori.mengh04.top/reference/cli/"><strong>CLI Reference</strong></a> ·
  <a href="README.zh-CN.md"><strong>简体中文</strong></a>
</p>

---

Shotori freezes every connected Wayland monitor at native physical resolution and turns a single keybind into your complete capture workflow: snap windows across mixed-HiDPI outputs, edit 14 live vector & optical annotations after placement, float always-on-top pins, stitch scrolling pages with a live side preview, and decode QR codes or extract text with private on-device OCR — all inside one native binary with zero cloud uploads.

<p align="center">
  <a href="https://shotori.mengh04.top/#stage">
    <img src="https://shotori.mengh04.top/media/hero-workflow-poster.webp" alt="Shotori interactive overlay, annotations, and dual-row GPUI toolbar" width="100%">
  </a>
</p>

> Visit **[shotori.mengh04.top](https://shotori.mengh04.top)** to watch 60fps real Wayland workflow demos, try the interactive browser sandbox, preview all 19 built-in themes, and generate compositor keybinds.

## Highlights

- **Wayland-native & HiDPI-exact** — built on `zwlr-screencopy-v1` and `wlr-layer-shell-v1`. Daily-driven on **niri** and **Hyprland**, and compatible with **sway**, **river**, **labwc**, **Wayfire**, **KWin**, and **COSMIC**. Handles mixed `1× / 1.25× / 2×` scales, rotated outputs, cross-monitor selections, and compositor IPC window snapping.
- **14 post-editable annotation tools** — Rectangle, Ellipse, Line, Polyline, 7 Arrow silhouettes, Rubik Sequence Badges (`1/A/I` with drag-out callouts & auto-renumbering), CJK/IME Text, smooth Pencil, Multiply Highlighter, Mosaic/Blur, `1–8×` Magnifier Lens, Spotlight Mask, Select/Move, and Whole-Object Eraser. Every mark stays editable and undoable until export.
- **Live long screenshots (`Ctrl+L`)** — frame the scrolling region, scroll naturally with your wheel or touchpad, and watch the stitched page grow in a `254px` live side preview (vertical or horizontal). Strips sticky headers and rejects ambiguous frames instead of corrupting rows.
- **Always-on-top floating pins (`Ctrl+P`)** — detach any capture (or local image via `shotori pin`) into a borderless floating window that outlives the overlay. Drag across mixed-scale monitors and roll the mouse wheel to zoom smoothly.
- **Private on-device OCR (`Ctrl+O`) & QR scanner (`Ctrl+Q`)** — ships 6 SHA-256-verified PP-OCRv6 model tiers (one-time ~31 MB download, 0 bytes network afterwards) plus custom ONNX pipeline support, alongside instant local QR decoding from screen, file, or `stdin`.
- **Zero-dialog direct save & 9-page native Settings** — press `Ctrl+S` to write directly with `strftime` + `{w}×{h}` templates (PNG/JPEG/WebP + optional `copy_on_save`), visually drag-reorder all 23 toolbar items, and rebind all 25 shortcuts in `shotori settings`.
- **19 built-in themes** — Shotori Dark/Light, Capsule Dark/Light, Catppuccin (`Mocha`, `Macchiato`, `Frappé`, `Latte`), Tokyo Night, Rosé Pine, Gruvbox, Nord, Dracula, Solarized Light, GitHub Light, and High Contrast — plus single-field `theme.toml` overrides with automatic WCAG 4.5:1 contrast.

<table>
  <tr>
    <td width="50%">
      <a href="https://shotori.mengh04.top/guides/long-screenshots/">
        <img src="https://shotori.mengh04.top/media/long-capture-poster.webp" alt="Live Long-Capture with 254px side preview">
      </a>
      <br><sub><strong>Live Long-Capture (Ctrl+L)</strong> — manual scroll with 254px live side preview</sub>
    </td>
    <td width="50%">
      <a href="https://shotori.mengh04.top/guides/annotating/">
        <img src="https://shotori.mengh04.top/media/annotations-suite-poster.webp" alt="14 Editable Annotation Tools, Magnifier Lens, and Whole-Object Eraser">
      </a>
      <br><sub><strong>14 Editable Tools</strong> — magnifier lens, mosaic, highlighter, badges & object eraser</sub>
    </td>
  </tr>
  <tr>
    <td width="50%">
      <a href="https://shotori.mengh04.top/guides/pin/">
        <img src="https://shotori.mengh04.top/media/floating-pin-poster.webp" alt="Always-on-top Floating Desktop Pin">
      </a>
      <br><sub><strong>Floating Pins (Ctrl+P)</strong> — cross-monitor drag & smooth wheel zoom</sub>
    </td>
    <td width="50%">
      <a href="https://shotori.mengh04.top/reference/settings/">
        <img src="https://shotori.mengh04.top/media/settings/capture.webp" alt="Shotori 9-Page Native Settings GUI">
      </a>
      <br><sub><strong>9-Page Settings GUI</strong> — drag-and-drop toolbar editor, 25 shortcuts & direct save</sub>
    </td>
  </tr>
</table>

## Requirements

- A Wayland compositor implementing `zwlr-screencopy-v1` (niri, Hyprland, sway, river, labwc, Wayfire, KWin, COSMIC…). *GNOME Mutter and X11 are not supported.*
- `xdg-desktop-portal` for the native save dialog (installed by default on most desktops; skipped when `export.auto_save = true`).
- A notification daemon (`mako`, `dunst`, `swaync`, …) is optional.

## Installation

```bash
paru -S shotori              # Arch Linux AUR (prebuilt binary + desktop entry)
cargo install shotori        # crates.io (Rust Edition 2024)
```

Or download a prebuilt binary from [GitHub Releases](https://github.com/shotori-screenshot/shotori/releases).

For a launcher entry and desktop icon after manual installation, run from the source tree or extracted archive:

```bash
sh scripts/install-desktop.sh   # installs to ~/.local/share; no root needed
```

Packagers can pass `DESTDIR="$pkgdir" sh scripts/install-desktop.sh /usr`. Tray and notification icons are embedded directly in the binary; AUR packages install desktop resources automatically.

### Bind in your compositor

Need a custom multi-bind config? Try the interactive **[Keybind Builder](https://shotori.mengh04.top/#cli)** on the website.

<details>
<summary><strong>niri</strong> (<code>~/.config/niri/config.kdl</code>)</summary>

```kdl
spawn-at-startup "shotori" "tray"

binds {
    Mod+Shift+S { spawn "shotori"; }
    Print       { spawn "shotori" "full" "--screen" "--clipboard"; }
    Mod+Print   { spawn "shotori" "gui" "--window" "--tool" "arrow"; }
}
```
</details>

<details>
<summary><strong>Hyprland</strong> (<code>~/.config/hypr/hyprland.conf</code>)</summary>

```ini
exec-once = shotori tray

bind = SUPER SHIFT, S, exec, shotori
bind = , Print, exec, shotori full --screen --clipboard
bind = SUPER, Print, exec, shotori gui --window --tool arrow
```
</details>

<details>
<summary><strong>sway</strong> (<code>~/.config/sway/config</code>)</summary>

```ini
exec shotori tray

bindsym $mod+Shift+s exec shotori
bindsym Print exec shotori full --screen --clipboard
bindsym $mod+Print exec shotori gui --window --tool arrow
```
</details>

## Quick CLI Overview

```bash
shotori                                     # Interactive overlay (freeze + snap/drag + annotate)
shotori gui --window --tool arrow           # Pre-select focused window via IPC & arm Arrow
shotori full --screen --clipboard           # Silent full-screen copy (no UI)
shotori full --output DP-1 --stdout > a.png # Pipe raw PNG bytes over stdout
shotori ocr --download --window --clipboard # Offline PP-OCRv6 from focused window
shotori qr invoice.png                      # Decode QR code from file (or `-` for stdin)
shotori pin mockup.png --at 420,180         # Float an image as an always-on-top pin
shotori outputs --json                      # Inspect Wayland outputs, scales & bounds
shotori settings [PAGE]                     # Open the 9-page native Settings window
shotori tray                                # Resident StatusNotifierItem tray + pin host
```

Full documentation for every subcommand, shortcut, TOML key, theme, and custom OCR model lives at **[shotori.mengh04.top](https://shotori.mengh04.top/getting-started/installation/)**.

## Status

Shotori is in active, pre-1.0 development. Core workflows — capture, annotations, copy, direct save, pins, live long screenshots, OCR, QR scanning, themes, and GUI settings — are in daily use, while CLI flags and config keys may still evolve between releases. Tested daily on **niri** and **Hyprland**; feedback from other Wayland compositors is warmly welcome on the [issue tracker](https://github.com/shotori-screenshot/shotori/issues).

## Development & Contributing

```bash
git clone https://github.com/shotori-screenshot/shotori
cd shotori
cargo build --release
cargo test    # unit tests, no compositor needed
```

Build dependencies, CI checks, code conventions, and the PR checklist live in [CONTRIBUTING.md](CONTRIBUTING.md). Issues, bug reports, and feature ideas are just as valuable as pull requests.

## License

[MIT](LICENSE)
