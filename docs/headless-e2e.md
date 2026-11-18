# Headless screenshots (development aid)

How an agent — or a maintainer — can **see** a real shotori window without
putting anything on the live desktop. Written after a session that spawned
windows onto the maintainer's niri session and stole focus: never do that.

What this buys you: real GPU rendering, real layer-shell surfaces, real
screencopy, and — when paired with `vptr keep` — real pointer clicks, drags,
wheel scrolling, and keyboard shortcuts in a compositor that exists only in
memory.

## The whole thing

Run it from the compositor's session (see "Why the `niri msg action spawn`
wrapper"), with `$APP` pointing at `target/release/shotori` (and `$VPTR` at
`target/release/vptr` after `cargo build --release -p vptr`):

```sh
niri msg action spawn -- sh -c '
APP=/abs/path/to/target/release/shotori
VPTR=/abs/path/to/target/release/vptr
OUT=/abs/path/to/target/headless/interactive.png
RT=$(mktemp -d /tmp/shotori-headless.XXXXXX)
printf "output HEADLESS-1 resolution 1600x1000\noutput HEADLESS-1 bg #ff00ff solid_color\ndefault_border none\n" > $RT/sway.conf
export XDG_RUNTIME_DIR=$RT
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER=vulkan WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128
sway -c $RT/sway.conf > $RT/sway.log 2>&1 &
SW=$!
sleep 2
export WAYLAND_DISPLAY=wayland-1

# Start persistent virtual devices BEFORE shotori so seat0 gets capabilities=3:
mkfifo $RT/input.fifo
"$VPTR" keep 1600 1000 < $RT/input.fifo > $RT/vptr.log 2>&1 &
KP=$!
exec 3>"$RT/input.fifo"
sleep 0.3

"$APP" > $RT/app.log 2>&1 &
AP=$!
sleep 3.5
printf "drag 180 120 1060 660 20\nsleep 150\nkey r\nsleep 120\ndrag 240 190 520 390 16\nmove 380 190\nscroll 1 8\n" >&3
sleep 1.5
grim -o HEADLESS-1 "$OUT"

exec 3>&-
kill $AP $KP $SW 2>/dev/null
rm -rf $RT
'
```

Then read the PNG. `target/headless/` is under the gitignored `/target`, and
everything the agent needs to read must live inside the repository: the
sandbox gives the shell its own private `/tmp`.

Swap the last part for other runs:

```sh
"$APP" settings                                  # the settings window
SHOTORI_DEBUG_SELECTION=200,150,700,500 "$APP"    # the selection overlay
"$APP" full -p /abs/path/out.png -d 0.5           # the non-interactive capture path
```

`default_border none` is not cosmetic: with sway's default `border normal`,
sway draws its **own 28px title bar** into the window rect (the wayland tree
reports the toplevel at `y=28`), and every capture shows a phantom title strip
above the app's real one. Constrain `resolution` to the window's own size
(1120x780) when reviewing layout at its default size.

### Reviewing crop editing

With `SHOTORI_DEBUG_TARGET=HEADLESS-1` and
`SHOTORI_DEBUG_SELECTION=150,110,700,430`, set
`SHOTORI_DEBUG_EDITING_SCENE=hover|selected|reselect|cancel|rectangle|ellipse|numbers|lens|multiselect|spotlight|textstyles`
to inspect annotation hover outlines, handles, multi-selection, sequence
badges, magnifiers, spotlights, text styles, crop reselection, and
cancellation. Held-drag async preview transitions can also be replayed with
`SHOTORI_DEBUG_ACTION=spotlightdraw|lensdrag`.
Use an isolated compositor and configuration directory.

### Reviewing inline text

Use the same target and selection with
`SHOTORI_DEBUG_ACTION=textnew|textmiddle|textedit|textempty` to inspect a caret
at the end or middle of synthetic text, reopen placed text, or start an empty
editor. These actions use the overlay's input handlers and keep the editor open.
Check integer and fractional display scales with an isolated background.

### Reviewing the settings window

Settings write through immediately. Use a dedicated `HOME`, config, cache and
data directory with synthetic configuration for each review; never copy the
maintainer's settings or capture their active session.

```sh
export HOME=/abs/path/target/headless/home
export XDG_CONFIG_HOME=/abs/path/target/headless/cfg
export XDG_CACHE_HOME=/abs/path/target/headless/cache
export XDG_DATA_HOME=/abs/path/target/headless/data
mkdir -p "$HOME" "$XDG_CONFIG_HOME/shotori"
SHOTORI_DEBUG_SETTINGS_PAGE=appearance "$APP" settings
```

`SHOTORI_DEBUG_SETTINGS_PAGE` takes
`appearance|capture|annotation|export|desktop|keys|ocr|advanced|about`
(aliases: `toolbar` → `capture`, `annotations` → `annotation`, `save` →
`export`, `notifications|notify|pin|pins` → `desktop`, `shortcuts` → `keys`,
`general` → `advanced`). The CLI also accepts `shotori settings --page <page>`.
A headless seat has no pointer, so these select the initial page without
clicking the sidebar.

`SHOTORI_DEBUG_SETTINGS_SCROLL=<logical pixels>` sets the page's vertical offset.
`SHOTORI_DEBUG_SETTINGS_QUERY` seeds the navigation search. A 12–24 px base font
can be selected with `SHOTORI_DEBUG_SETTINGS_FONT` to review text scaling. These
inputs apply only to the review process and do not change saved preferences.
`SHOTORI_DEBUG_SETTINGS_CUSTOM_COLORS=1` expands custom colors without opening a
picker; `SHOTORI_DEBUG_SETTINGS_RECORDING=copy` displays that command's recorder.

`SHOTORI_DEBUG_SETTINGS_COLOR=accent` opens that color editor on Appearance;
use `accent:hsla` to inspect its component sliders. The key is a stable
`ColorKey::key()` name, and the run must still use an isolated config directory.

The `printf` line is load-bearing twice: `resolution` makes the virtual screen
big enough for the 1120×780 settings window, and `bg #ff00ff` makes a frozen
frame unmistakable — anything the app fails to paint shows as loud pink instead
of silent black. Write it with double quotes; the whole block sits inside single
quotes.

## Why the `niri msg action spawn` wrapper

The agent's sandbox masks `/dev/dri`, so a Vulkan client started from inside it
dies in `vkCreateInstance` (`Found no drivers!`) — blade has no software
fallback, and no lavapipe is installed. `niri msg action spawn` hands the command
to the compositor's own session, which sees the render node. It maps nothing:
it is a process launcher, not a window request.

- `systemd-run --user` would be the portable equivalent, but the sandbox has no
  session bus (`Failed to connect to user scope bus`).
- On a non-niri session, run the same block from a terminal inside the session.
- If you forget the wrapper, the app's log says `vkCreateInstance: Found no
  drivers!` and grim still writes a PNG — all magenta.

## Interactive input in headless sway: `vptr keep` vs `SHOTORI_DEBUG_*`

`WLR_LIBINPUT_NO_DEVICES=1` keeps the maintainer's physical keyboard and mouse
out of the headless session, starting `seat0` with `capabilities: 0`. Two
measured traps broke naive virtual-input attempts before `vptr keep` solved
both:

1. **The seat-capability timeline**: if `shotori` maps its `wlr-layer-shell`
   surface while `seat0` still has `capabilities: 0`, sway never grants focus.
   Starting `vptr keep W H < input.fifo &` **before** launching `shotori`
   attaches a persistent `zwlr_virtual_pointer_v1` and `zwp_virtual_keyboard_v1`,
   raising `seat0` to `capabilities: 3` (`keyboard | pointer`).
2. **Ephemeral pointer disconnect on drag / `axis_discrete` on wheel**:
   short-lived `vptr down` CLI calls destroy their virtual pointer on process
   exit, causing wlroots to auto-release held buttons before `vptr move` runs;
   and wlroots drops `AxisSource::Wheel` frames that lack `axis_discrete`.
   `vptr keep [W H]` executes the entire gesture stream on one persistent
   connection:
   - `move X Y [W H]`
   - `click | dclick | rclick X Y [W H]`
   - `drag X1 Y1 X2 Y2 [STEPS]` (and `down [left|right]` / `up [left|right]`)
   - `scroll | hscroll LINES [STEPS]` (positive = scroll/step up, negative = down)
   - `mods shift|ctrl|alt|super|shift+alt|none`
   - `key CHORD` (or bare `CHORD`), `extent W H`, `sleep MS`

When you only need a fast deterministic snapshot of a pre-built scene (without
scripting pointer coordinates), use the `SHOTORI_DEBUG_*` backdoors in
`src/ui/e2e.rs` directly; when testing end-to-end pointer drags, handle bends,
sidebar clicks, or wheel scrolling, drive `vptr keep`.

## When pixels are the wrong tool

Behaviour, focus, layout and callbacks are cheaper and sharper in-process:
`#[gpui_kit::test]` plus `gpui_kit::test::{TestWindowExt, …}` renders real
widgets with real hit testing and keyboard dispatch in a headless window, with
no compositor at all, under plain `cargo test`. In-tree examples:
`src/ui/text_input.rs`, `src/ui/text_editor.rs`, `src/model/session.rs`.

Linux has **no** pixel capture there (`is_screen_capture_supported()` is false;
the golden-image target is Metal-only), so the split is: `#[gpui_kit::test]` for
behaviour, this page for pixels.

## Gotchas

- **Never `pkill -f <pattern>` when the pattern appears in your own command
  line.** The sandboxed shell and the spawned process share a PID namespace, so
  `pkill -f 'shotori settings'` kills the shell that issued it. Kill by exact
  PID captured in the block (`$AP`, `$SW`).
- App boot plus font cache needs ~4–6 s. Capture earlier and the frame is pure
  magenta.
- **Use `WLR_RENDERER=vulkan`, not `pixman` or `gles2`.** `pixman` cannot import
  the client's GPU buffers, and wlroots's `gles2` headless readback announces
  24-bit `BGR888` (`stride = w * 3`) instead of 32-bit `XRGB8888` (`stride = w * 4`).
  `WLR_RENDERER=vulkan` announces 32-bit `XRGB8888` cleanly.
- The session contains no other toplevels: omit `SHOTORI_DEBUG_TARGET` (it
  restricts overlays to a named output) and expect
  `window snap: no visible windows` in the app log.
- Sway names its socket `wayland-1` inside a fresh `XDG_RUNTIME_DIR`. If a
  future sway changes that, `$(ls $RT | grep wayland)` is the robust lookup.
- If a session ever looks stuck, list processes from the compositor session:
  `niri msg action spawn -- sh -c 'pgrep -a sway > /abs/path/in/repo/procs.txt'`.

## Related

- `tools/vptr` — persistent (`vptr keep [W H]`) and one-shot virtual pointer +
  keyboard injector (`zwlr_virtual_pointer_v1` + `zwp_virtual_keyboard_v1`) for
  headless sway e2e and live sessions.
- `tools/scrollprobe` — frame-sequence driver for the long-capture matcher.
- `src/ui/e2e.rs` — deterministic `SHOTORI_DEBUG_*` scene/action backdoors for
  fast single-shot headless snapshots.
