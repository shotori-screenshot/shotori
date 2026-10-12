# Shotori application icon

`shotori.svg` is the canonical artwork. Keep the rounded background, ribbon,
colors and capture corners together; action icons in `assets/icons` are a
separate UI vocabulary.

| Asset | Use |
| --- | --- |
| `shotori.svg` | Scalable desktop icon and editable master |
| `shotori-{16,20,24,32,48,64}.png` | Small launcher/panel sizes; the tray embeds 64px |
| `shotori-{96,128,192,256}.png` | Desktop/HiDPI sizes; notifications embed 128px and the README displays 256px |
| `shotori-{512,1024}.png` | Large previews and high-resolution artwork |
| `shotori.ico` | Portable multi-resolution ICO: 16, 24, 32, 48, 64, 128, 256px |

Regenerate from the repository root with Python 3 and librsvg's `rsvg-convert`:

```sh
python3 scripts/generate-icons.py
```

Every PNG is rendered directly from the SVG. The ICO embeds the corresponding
PNG frames without resizing. Assets are checked in; ordinary Rust builds need
neither Python nor librsvg. The canvas fits the dark background exactly;
rounded corners have transparent pixels outside the background, with no white border.

Install desktop metadata and all hicolor sizes from the source/release root:

```sh
sh scripts/install-desktop.sh
# Package staging (the binary must be installed separately):
DESTDIR=/tmp/shotori-package sh scripts/install-desktop.sh /usr
```

The installer defaults to `~/.local`, accepts a prefix, and respects `DESTDIR`.
It does not install the executable or start Shotori. Desktop `Exec` resolves
`shotori` through PATH. Tray and notification icons are embedded in the binary.
