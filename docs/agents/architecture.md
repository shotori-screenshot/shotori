# Architecture, Process Model & Extension Playbooks

Read alongside the crate header in [`src/lib.rs`](../../src/lib.rs).

## 1. Dependency DAG

Imports flow strictly top-down; cycles and upward reaches are forbidden:

```text
main.rs / commands.rs / args.rs
        │
        ▼
      ui/* ──────────────► annotation/* (private crate module tree)
        │                        │
        ▼                        ▼
     model/* ────────────► render_image.rs  (leaf: RGBA → BGRA RenderImage)
        │
        ▼
   platform/*

actions.rs  (pure leaf: zero `crate::*` imports; referenced by main, ui, model)
Desktop exits / services: clipboard.rs, custom_action.rs, notify.rs, ocr.rs, qr.rs, save_dialog.rs, tray.rs, i18n.rs
Dev harnesses: bench.rs (`--bench`), perf.rs (`--perf`, behind `--features perf`)
```

### Boundary Rules

- **`actions.rs` is a pure leaf.** Keybindings (`main.rs`), toolbar
  controls (`ui/toolbar`), and window handlers (`ui/overlay`,
  `ui/scroll_bar`, `ui/pin`) all import `actions.rs`; `actions.rs`
  imports nothing from `crate::*`. Only dialog-private actions (e.g.
  `OcrSetupConfirm` in `ui/ocr_setup.rs`) stay local to their module.
- **`render_image.rs` is a pure leaf.** Both `model/` and `annotation/`
  build `RenderImage` caches and cannot import `ui/`; `ui/image_util.rs`
  re-exports `rgba_to_render_image` for UI callers.
- **`model/` is compositor-free.** Selection state (`selection.rs`),
  multi-output session orchestration (`session/`), chrome layout
  (`placement.rs`), smart-region proposals (`regiondetect/`), long-capture
  stitching (`scroll_sheet/`), PNG/JPEG/WebP export (`export.rs`), and
  config persistence (`settings/`) must compile and run under plain
  `cargo test` with no Wayland socket.
- **Single documented exception**: `platform::scroll_capture` drives
  `model::scroll_sheet` during live region polling. Do not add new
  `platform → model` or `model → ui` edges.

## 2. Multi-Process Execution Model

Shotori intentionally uses short-lived processes plus detached child
entry points rather than one monolithic background daemon:

| Entry Point | Dispatch Path | Lifecycle & Why |
| --- | --- | --- |
| `shotori` / `shotori gui` | `clap` → `main.rs` | **Run-and-exit capture session.** Loads `settings.toml`, `theme.toml`, and locale once at startup (`init`), freezes screens via `wlr-screencopy`, opens one layer-shell `Overlay` per output, runs one exit, and terminates. No live config reload is needed or wanted. |
| `shotori full \| ocr \| qr \| outputs` | `clap` → `commands.rs` | Non-interactive CLI pipelines (capture/decode/stdout/file/clipboard) that exit on completion. |
| `shotori pin` (or Pin exit) | `clap` / in-process transition | Keeps layer-shell pin surfaces alive across outputs (`ui/pin`) until the last pin closes; also dispatches `--clipboard`, `--restore`, `--unlock`, `--toggle-hide`, `--close-all`, and the regular-toplevel `shotori pin --history` manager (`ui/pin_history`). |
| `shotori settings` | `clap` or `launch_detached("settings")` | **Regular Wayland toplevel** (`ui/settings`). Writes every toggle/edit straight through to `settings.toml` / `theme.toml`; the next capture session picks changes up. |
| `shotori tray` | `clap` → `tray.rs` | Resident `StatusNotifierItem` process (`gpui-tray`). Spawns fresh `shotori` child sessions via `launch_detached` and reaps them on a helper thread to prevent zombies. |
| `shotori --notify ...` | `argv[1]` pre-`clap` in `main.rs` | Re-exec'd child owning D-Bus notification lifetime + action callbacks ("View image", "Open text") after the parent overlay exits. Calls `i18n::init_child()`. |
| `shotori --clipboard-daemon ...` | `argv[1]` pre-`clap` in `main.rs` (Linux) | Re-exec'd Wayland clipboard seat owner (`wlr-data-control`) that serves paste requests until another client replaces the selection. |
| `shotori --bench` / `--perf` | `argv[1]` pre-`clap` in `main.rs` | Release-build microbenchmarks (`bench.rs`) and feature-gated E2E latency/RSS suites (`perf.rs`). Matched before `clap` because trailing arguments are suite-specific. |

## 3. Coordinate Spaces & Scaling Contracts

Confusing coordinate spaces is the #1 source of multi-monitor bugs. Every
function boundary must know which of the three spaces its numbers inhabit:

1. **Desktop-global logical `Pixels`** (`Point<Pixels>`, `Bounds<Pixels>`):
   - Used by `ScreenshotSession`, `Selection`, `Shape` (`annotation.rs`),
     `PinOutput`, and cross-monitor window snap rects.
   - Pointer state (`session.pointer_global`) lives here.
   - Convert output-local coordinates via `session.to_global(output, p)`
     and back by subtracting the output's global origin.
2. **Output-local logical `Pixels`**:
   - Used inside a single `Overlay` window's GPUI element tree, window
     pointer events, and `model/placement.rs` (`ws: Size<Pixels>`).
   - Note: during an implicit pointer grab, cross-monitor drags produce
     negative or `> ws` output-local coordinates on the press window —
     never clamp before handing to `session`.
3. **Physical capture pixels (`u32`, RGBA8 row-major)**:
   - Used by `Capture` (`platform/capture`), `model/export.rs`,
     `annotation/raster.rs`, `model/regiondetect`, and `model/scroll_sheet`.
   - **Scale rule**: always compute physical-per-logical scale for an
     output as `capture.width as f32 / f32::from(window_logical_width)`.
     Never call `window.scale_factor()` for cropping or rasterization (it
     can report a sibling monitor's scale).
   - **Edge rounding rule**: convert logical edges (`left`, `top`,
     `right`, `bottom`) to physical integers independently with
     `round_px`; derive pixel width/height as `right - left` and
     `bottom - top`, never `round(left) + round(width)`.

## 4. Extension Playbooks

### Adding a New Configurable Overlay Action / Shortcut

1. Declare the GPUI action in `src/actions.rs` (`gpui_kit::actions!`).
2. If user-rebindable, add `("<id>", "<default_chord>")` to
   `CAPTURE_KEYS` or `ANNOTATION_KEYS` AND `CONFIGURABLE_KEYS` in
   `src/actions.rs` (`escape` and `enter` stay in `RESERVED_KEYS`).
3. Bind the keystroke in `main.rs` (via `actions::KeyBinding` /
   `parse_keystroke` path) and wire the handler on `Overlay`
   (`src/ui/overlay.rs` / `exits.rs`), **not** on `App::on_action`.
4. If exposed on the toolbar, add the variant to `ToolbarButton` in
   `src/model/settings/toolbar.rs`, wire its icon/dispatch in
   `src/ui/toolbar/`, and add UI labels/tooltips in `locales/ui.yml`.
5. Mirror the key in [`docs/settings.example.toml`](../settings.example.toml)
   under `[keys]` (and `[toolbar] items` comment if applicable).

### Adding a Persistent Setting (`settings.toml`)

1. Pick the domain child under `src/model/settings/` (`capture.rs`,
   `annotation.rs`, `export.rs`, `ocr.rs`, `toolbar.rs`) — colors of UI
   chrome belong in `src/ui/theme/`, not `settings.toml`.
2. **Forward/backward tolerance is mandatory**:
   - Missing keys default cleanly (`Default`).
   - Out-of-range or unknown enum strings degrade to defaults with a
     load warning — never fail `Settings::load`.
   - `Settings::save` must preserve unknown TOML keys/sections written by
     newer binaries (round-trip test in `src/model/settings.rs`).
3. Wire the control into the matching `src/ui/settings/*_page.rs` module
   using `persist` (write-through, no Apply/OK button) and localized
   strings in `locales/ui.yml`.
4. Document the key and valid values in
   [`docs/settings.example.toml`](../settings.example.toml).
5. Add unit tests for default, valid parse, invalid fallback, and
   unknown-key round-trip.

### Adding a CLI Flag or Subcommand

1. Define clap structs/enums in `src/args.rs` (respect existing
   `ArgGroup` mutual-exclusion groups such as `"target"`).
2. Implement non-GUI execution in `src/commands.rs` (or pre-selection
   plumbing in `main.rs` + `model/session/preset.rs` for `gui`).
3. Add argument-parsing unit tests in `src/args.rs` covering defaults,
   aliases, and conflicting-flag rejections.

### Splitting a Growing File ("No God Files")

When non-test code in a file approaches ~1000 lines or mixes distinct
responsibilities (e.g. a container shell + multiple pages/tools):

1. Keep `foo.rs` (or `foo/mod.rs` following neighboring convention; modern
   splits in this repo use `foo.rs` + `foo/child.rs`) as the thin root
   holding the core type and shared helpers.
2. Move each concern into `foo/<concern>.rs` along with the unit tests
   that cover it.
3. Keep external caller paths unchanged via `pub use` / `pub(crate) use`
   re-exports at `foo.rs`.
