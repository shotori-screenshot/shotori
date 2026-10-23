# Headless screenshots (development aid)

How an agent — or a maintainer — can **see** a real shotori window without
putting anything on the live desktop. Written after a session that spawned
windows onto the maintainer's niri session and stole focus: never do that.

What this buys you: real GPU rendering, real layer-shell surfaces, real
screencopy, in a compositor that exists only in memory. What it does **not**
buy you is input — see "No hands" below.

## The whole thing

Run it from the compositor's session (see "Why the `niri msg action spawn`
wrapper"), with `$APP` pointing at `target/release/shotori`:

```sh
niri msg action spawn -- sh -c '
APP=/abs/path/to/target/release/shotori
OUT=/abs/path/to/target/headless/settings.png
RT=$(mktemp -d /tmp/shotori-headless.XXXXXX)
printf "output HEADLESS-1 resolution 1600x1000\noutput HEADLESS-1 bg #ff00ff solid_color\ndefault_border none\n" > $RT/sway.conf
export XDG_RUNTIME_DIR=$RT
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER=gles2 WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128
sway -c $RT/sway.conf > $RT/sway.log 2>&1 &
SW=$!
sleep 2
export WAYLAND_DISPLAY=wayland-1
"$APP" settings > $RT/app.log 2>&1 &
AP=$!
sleep 6
grim -o HEADLESS-1 "$OUT"
grim -o HEADLESS-1 "${OUT%.png}-b.png"     # a second frame: animations need >1
kill $AP $SW 2>/dev/null
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
`SHOTORI_DEBUG_EDITING_SCENE=hover|selected|reselect|cancel` to inspect the
annotation hover outline, selection handles, crop reselection and cancellation.
The scenes create synthetic marks inside the injected selection. `selected`
includes a keyboard-sized translation; `cancel` restores the crop after a
reselection drag. Use an isolated compositor and configuration directory.

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

`SHOTORI_DEBUG_SETTINGS_PAGE` takes `appearance|capture|keys|ocr|advanced|about`.
`toolbar` and `general` are aliases for `capture` and `advanced`. A headless seat
has no pointer, so this selects the initial page without clicking the sidebar.

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

## No hands: a headless session has no input

Measured, with the app focused (`swaymsg -t get_tree` reports `"focused": true`
for app_id `shotori`):

```text
$ swaymsg -t get_seats
[{ "name": "seat0", "capabilities": 0, "focus": 5, "devices": [] }]
$ swaymsg -t get_inputs
[]
```

`WLR_LIBINPUT_NO_DEVICES=1` is what keeps the user's real keyboard and mouse
out of the test session, and the price is a seat with **zero capabilities**:

- `zwlr_virtual_pointer_v1` motion is accepted and never becomes a `wl_pointer`
  event. This is the same wall [wdotool](https://github.com/cushycush/wdotool/blob/main/docs/testing.md)
  documents for sway/labwc/river: wlroots seeds pointer capability from real
  input devices.
- `wtype` (i.e. `zwp_virtual_keyboard_v1`) did not reach the app either: three
  `Tab` presses produced a 0-pixel difference, and `Escape` never closed the
  overlay.

So a headless run can **only** drive the app through the `SHOTORI_DEBUG_*`
backdoors in `src/ui/e2e.rs`. If a test needs a click on a particular section,
add a backdoor for it — do not fabricate clicks, and do not "just use the real
desktop". `/dev/uinput` would sidestep this (sway cannot tell it from real
hardware) but it needs device permissions and an injector; that is the only
known route to wheel/click e2e, including long-capture scrolling.

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
- `SHOT_RENDERER=pixman` boots faster but cannot import the client's GPU
  buffers, so shotori paints nothing. Keep `gles2`.
- The session contains no other toplevels: omit `SHOTORI_DEBUG_TARGET` (it
  restricts overlays to a named output) and expect
  `window snap: no visible windows` in the app log.
- Sway names its socket `wayland-1` inside a fresh `XDG_RUNTIME_DIR`. If a
  future sway changes that, `$(ls $RT | grep wayland)` is the robust lookup.
- If a session ever looks stuck, list processes from the compositor session:
  `niri msg action spawn -- sh -c 'pgrep -a sway > /abs/path/in/repo/procs.txt'`.

## Related

- `tools/vptr` — pointer and keyboard into a **live** session (needs a real
  seat); the counterpart for the maintainer's own desktop, useless headless.
- `tools/scrollprobe` — frame-sequence driver for the long-capture matcher.
- `src/ui/e2e.rs` — the `SHOTORI_DEBUG_*` backdoors, the only input surface a
  headless run has.
