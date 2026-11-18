# UI Surfaces, Wayland Layer-Shell, Theme & i18n

Read before modifying anything under `src/ui/`, `src/model/placement.rs`,
or `locales/ui.yml`.

## 1. Window Surface Taxonomy

Not every Shotori window is the same kind of Wayland surface. Applying
toplevel patterns to layer-shell windows (or vice versa) breaks layout
and focus:

| Surface | Module | Wayland Role | `Root` Wrapper? | `display_id` Pinning? | Notes |
| --- | --- | --- | --- | --- | --- |
| Capture `Overlay` | `ui/overlay.rs` | `wlr-layer-shell` (`Layer::Overlay`, exclusive keyboard) | **NEVER** (bare `cx.open_window`) | **REQUIRED** (one window per output) | Unmap before launching any `xdg-desktop-portal` file dialog |
| Floating `Pin` | `ui/pin.rs` | `wlr-layer-shell` (`Layer::Top`, on-demand keyboard) | **NEVER** | **REQUIRED** (shared scene across registered `PinOutput`s) | Neon focus halo renders **only** on the focused pin while its surface holds focus; ghost pins exclude their body from `set_input_region` |
| `PinHistoryWindow` | `ui/pin_history.rs` | Regular XDG toplevel (`940×640` default, client `TitleBar`) | **YES** (`gpui_kit::component::Root`) | Compositor-placed | Launched via `shotori pin --history` / detached child so `Root` never touches layer-shell pin surfaces |
| Long-capture `ScrollBar` | `ui/scroll_bar.rs` | `wlr-layer-shell` (region frame + preview deck) | **NEVER** | **REQUIRED** | If no side strip fits on the capture output, docks preview on `other_output_than(display)` so `wlr-screencopy` never captures its own HUD |
| `SettingsWindow` | `ui/settings.rs` | Regular XDG toplevel (`1120×780` default, floating-island sidebar + client drag headers) | **YES** (`gpui_kit::component::Root`) | Compositor-placed | `Root` provides `WindowState` for managed tooltips on the collapsed sidebar; harmless on a regular toplevel |

### Startup & Teardown Rules

- **Open windows asynchronously after startup**: `cx.displays()` is empty
  during synchronous `App::run` setup. Always spawn an async task so one
  event-loop tick populates `cx.displays()` before matching `Capture`s to
  `DisplayId`s (`platform/display.rs`).
- **Flush Wayland requests before exit**: `cx.quit()` exits the loop
  without guaranteeing pending surface destroys or buffer releases have
  flushed to the Wayland socket. When surfaces must cleanly unmap or hand
  off (portal save flow, tray), set `QuitMode::Explicit`, remove windows,
  and quit after a ~150 ms timer.

## 2. Single-Source Chrome Placement (`model/placement.rs`)

Overlay chrome uses a strict **two-zone non-intersecting contract**:

- **Top zone — Dimension label (`label_anchor`)**: sits above the
  selection, or inside its top-left corner (`INSET = 12.`) when the
  selection hugs the top edge. Never flips below the selection.
- **Bottom zone — Toolbar (`toolbar_anchor`)**: sits below the selection,
  or inside its bottom-left corner when the selection reaches the screen
  bottom (`EDGE_B = 12.` margin).
  - Single-row width (no drawing tool active): `TB_W_ROW1` (`660.`),
    hugging row-one content so `flex_1` never opens a dead gap.
  - Two-row options width (tool active): `TB_W` (`672.`). Whenever row-one
    default buttons or row-two option controls grow, update the constants
    in `model/placement.rs` and keep the probe tests green
    (`toolbar_hugs_its_content` and the 35-geometry placement sweep).
  - Narrow selections/screens: toolbar rows must remain reachable via
    horizontal scrolling (`ui/toolbar/row.rs`), and both rows must stay
    clamped within display bounds.
- **Hit-testing & cursor alignment**: drag grips (`GRIP_W`, `BAR_PAD`),
  resize handles, and toolbar hover regions MUST read the exact bounds
  functions on `ScreenshotSession` / `placement.rs`. Never infer hit
  boxes from GPUI layout callbacks or ad-hoc padding constants.

## 3. Theme vs Settings & Component Sync

Shotori separates **chrome appearance** from **brush configuration**:

1. **UI Style Preset (`src/model/settings/style.rs`, `settings.toml` `ui_style`)**:
   - Orthogonal `UiStyle` (`classic` | `capsule`) controls chrome geometry and
     material treatment (frosted capsule bars, concentric rounded controls,
     circular SDF-masked loupe, elevated jewel handles) independently of the
     active color palette.
2. **Overlay & Chrome Palette (`src/ui/theme/`, `theme.toml`)**:
   - Stored as `Theme` (`Copy` struct of `0xRRGGBBAA` `u32` colors +
     `dim_opacity: f32`), installed once at startup via `theme::set` and
     read via `theme::c()`.
   - `theme.toml` picks a `base` (`shotori-dark`, `shotori-light`,
     `capsule-dark`, `capsule-light`, or a built-in community palette in
     `theme::load::BUILT_INS`) plus optional per-field overrides. Overriding
     `accent` automatically retints `pin_border` and `toolbar_selected`
     unless those keys are also explicitly overridden.
   - Any change to `ColorKey` or `ThemeFile` in `src/ui/theme/load.rs`
     **must** update [`docs/theme.example.toml`](../theme.example.toml).
3. **`gpui-component` Shell Sync (`ui::settings::sync_shell_theme(cx)`)**:
   - Both `SettingsWindow` and `Overlay` (which hosts `ColorPickerState`
     popovers from `gpui-component`) call `sync_shell_theme(cx)` so
     component popovers, selects, inputs, and tooltips match the active
     Shotori palette instead of `gpui-component`'s stock theme.
4. **Annotation Swatch Board (`src/model/settings.rs`)**:
   - The 7 brush colors (`PALETTE = 7`, `DEFAULT_ANNOTATION_COLORS`,
     `[toolbar] colors` in `settings.toml`) are user tool configuration,
     **not** theme fields. Palette slots are opaque (`0xRRGGBBFF`) so
     exported strokes stay solid; only text outlines/backgrounds expose
     alpha (`HSLA` picker tab).

## 4. Internationalization (`src/i18n.rs` & `locales/ui.yml`)

- **Zero inline UI copy**: every user-visible label, tooltip, placeholder,
  notification body, and status string belongs in `locales/ui.yml` with
  both `en` and `zh-CN` entries, resolved through `crate::i18n::t!("key")`.
- **Crate-root wiring (`src/lib.rs`)**:
  - `rust_i18n::i18n!("locales", fallback = "en")` must remain at crate
    root so `t!` macro expansions resolve `crate::_rust_i18n_translate`.
  - `const _LOCALES_UI_YML: &str = include_str!("../locales/ui.yml");`
    keeps `locales/ui.yml` in Cargo's rebuild dependency graph (proc
    macros alone do not emit `cargo:rerun-if-changed`, causing stale
    embedded tables if removed).
- **YAML 1.1 boolean-key trap**: quote keys like `"on"`, `"off"`,
  `"yes"`, `"no"` in `locales/ui.yml` — unquoted YAML 1.1 parses `off:` as
  boolean `false`, silently breaking `t!("...off")` lookups (returning the
  raw key string in UI).
- **Dynamic locale switching**: `t!` reads the global locale on each
  render, so `SettingsWindow` switches languages live by calling
  `i18n::set(lang)` + triggering re-render. Note that language self-names
  (`"English"`, `"简体中文"`) stay untranslated by design so users can
  always find their language back; only `"Auto"` (`settings.general.language.system`)
  is localized.
