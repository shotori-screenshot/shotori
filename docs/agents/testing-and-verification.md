# Testing, Verification & Debug Backdoors

Compilation alone never proves correctness in a Wayland/GPU application.
Match the verification tier to what your change touches.

## 1. The Four Verification Tiers

| Tier | Command / Tool | Compositor Needed? | What It Proves |
| --- | --- | --- | --- |
| **1. Pure Unit Tests** | `cargo test` (`mod tests` in `model/`, `annotation/`, `args.rs`, `i18n.rs`, `ocr.rs`) | No | State machines, placement sweep, smart-region detection, `scroll_sheet` pixel-prefix invariants, rasterizer math, config round-trips, CLI parsing |
| **2. Headless Widget Tests** | `cargo test` (`#[gpui_kit::test]` + `gpui_kit::test::TestWindowExt`) | No | Real GPUI widget layout, hit-testing, menu clicks, focus restoration, shortcut dispatch, `TextInput` / `TextEditor` / `Overlay` interaction |
| **3. Private Headless Wayland E2E** | Headless `sway` (`WLR_RENDERER=vulkan`) + `grim` via `niri msg action spawn` + `vptr keep` or `SHOTORI_DEBUG_*` ([`docs/headless-e2e.md`](../headless-e2e.md)) | Private in-memory `sway` (`WLR_BACKENDS=headless`) | Real Vulkan (`blade`) rendering, `BGRA` swap, `wlr-layer-shell` geometry, `wlr-screencopy`, real pointer drag/click/wheel & keyboard focus, fractional HiDPI (`1.25×`) |
| **4. Perf & Live Protocol Probes** | `--bench`, `--perf` (`--features perf`), `SHOTORI_BOOT=1`, `tools/scrollprobe`, `tools/vptr` | `--bench`: No · `--perf` / `scrollprobe` / `vptr`: Yes | Pixel/PNG throughput, cold-start phase latency & RSS, screencopy/pointer protocol diagnostics |

> **Important notes on `cargo test`:**
> - `Cargo.toml` scopes `[profile.test] opt-level = 2` to the workspace and
>   pixel/font leaf crates (`[profile.test.package."*"] opt-level = 0`), keeping
>   `scroll_sheet` and rasterizer tests fast (~5 s) while sharing cached `dev`
>   dependency artifacts with `cargo build`.
> - `#[gpui_kit::test]` works cleanly on pinned `gpui-kit 0.8` (see
>   `src/ui/text_input.rs`, `src/ui/text_editor.rs`,
>   `src/ui/overlay/interaction_tests.rs`,
>   `src/ui/overlay/multi_output_tests.rs`, `src/model/session.rs`). If a
>   dependency bump ever reintroduces a `rustc` SIGSEGV in macro expansion,
>   stop and report the regression — never silently downgrade widget tests.

## 2. Headless Input (`vptr keep`) & `SHOTORI_DEBUG_*` Reference

Headless Wayland sessions (`WLR_LIBINPUT_NO_DEVICES=1`) start `seat0` with
`capabilities: 0`. You have two ways to drive them:
- **Interactive pointer & keyboard (`tools/vptr`)**: start
  `vptr keep <width> <height> < input.fifo &` **before** launching `shotori` so
  `seat0` acquires `capabilities: 3` (`keyboard | pointer`), then stream
  `move`, `click`/`dclick`/`rclick`, `drag`, `scroll`/`hscroll`, `mods`, and
  `key` commands over the FIFO ([`docs/headless-e2e.md`](../headless-e2e.md)).
- **Deterministic scene/action seeds (`SHOTORI_DEBUG_*`)**: fast one-shot
  state injection in `src/ui/e2e.rs` when pointer choreography is unnecessary.

### Overlay & Capture Backdoors

| Variable | Accepted Values | Behavior |
| --- | --- | --- |
| `SHOTORI_DEBUG_TARGET` | `<output_name>` (e.g. `HEADLESS-1`) | Scopes debug selection/scenes/actions to a single output so multi-monitor overlays do not fight each other |
| `SHOTORI_DEBUG_SELECTION` | `x,y,w,h` (logical px) | Seeds `Selection::Selected` at startup on the targeted output |
| `SHOTORI_DEBUG_EDITING_SCENE` | `hover` \| `selected` \| `reselect` \| `cancel` \| `rectangle` \| `ellipse` \| `numbers` \| `lens` \| `multiselect` \| `spotlight` \| `textstyles` | Seeds deterministic annotation marks, selection states, or crop-reselection states inside `SHOTORI_DEBUG_SELECTION` |
| `SHOTORI_DEBUG_ACTION` | **Exits / commands** (fire at 1.5 s): `copy` \| `save` \| `pin` \| `ocr` \| `qr` \| `scroll` \| `clear` \| `quit` \| `ocrsetup` (triggers `OcrSelection` at 1.5 s, `OcrSetupConfirm` at 6.0 s)<br>**Inline text editors**: `textnew` \| `textmiddle` \| `textedit` \| `textempty`<br>**Held-drag async replays**: `spotlightdraw` \| `lensdrag` (16-step drag with a 1.5 s mid-drag hold at step 8) | Dispatches real actions, synthetic text keystrokes, or multi-step pointer drags inside the targeted overlay window |
| `SHOTORI_DEBUG_SAVE_PATH` | `/abs/path/out.png` | Bypasses the `xdg-desktop-portal` file dialog in overlay and scroll-capture save flows, writing directly to the path |

### Settings Window Backdoors (`shotori settings`)

Always run headless settings checks with isolated `HOME`,
`XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, and `XDG_DATA_HOME` under
`target/headless/` because settings write through immediately.

| Variable | Accepted Values | Behavior |
| --- | --- | --- |
| `SHOTORI_DEBUG_SETTINGS_PAGE` | `appearance` \| `capture` (`toolbar`) \| `annotation` (`annotations`) \| `export` (`save`) \| `desktop` (`notifications`, `notify`, `pin`, `pins`) \| `keys` (`shortcuts`) \| `ocr` \| `advanced` (`general`) \| `about` | Opens `SettingsWindow` directly on that section (equivalent to `shotori settings --page <section>`) |
| `SHOTORI_DEBUG_SETTINGS_SCROLL` | `<logical_px>` (non-negative `f32`) | Scrolls the active settings page vertically to inspect below-the-fold controls |
| `SHOTORI_DEBUG_SETTINGS_QUERY` | `<search_string>` | Seeds the settings sidebar search query |
| `SHOTORI_DEBUG_SETTINGS_FONT` | `12.0` ..= `24.0` | Overrides base UI font size in-memory for text-scaling layout review (not persisted) |
| `SHOTORI_DEBUG_SETTINGS_CUSTOM_COLORS` | `1` | Expands custom color override rows on Appearance without clicking |
| `SHOTORI_DEBUG_SETTINGS_COLOR` | `<ColorKey>` or `<ColorKey>:hsla` (e.g. `accent`, `accent:hsla`) | Opens that `ColorKey`'s inline picker on Appearance (palette or HSLA tab) |
| `SHOTORI_DEBUG_SETTINGS_RECORDING` | Any id in `actions::CONFIGURABLE_KEYS` (e.g. `copy`, `tool_arrow`) | Opens the keycap shortcut recorder in active recording state |

### Diagnostics, Long-Capture & Perf Knobs

| Variable | Accepted Values | Behavior |
| --- | --- | --- |
| `SHOTORI_BOOT` | `1` | Prints cumulative millisecond startup phase traces (`[boot] ... ms`) to stderr |
| `SHOTORI_DEBUG_SHEET_LOG` | `/abs/path/sheet.log` | Logs every pushed frame and stitch decision in `model::scroll_sheet` |
| `SHOTORI_SCROLL_TEST_ARTIFACTS` | `/abs/path/dir` | Writes diagnostic PNGs from `model/scroll_sheet/scene_tests.rs` |
| `SHOTORI_NOTIFY_TIMEOUT_SECS` | `<seconds>` | Overrides notification child auto-exit timeout (`src/notify.rs`) |

## 3. Mandatory Verification Gates by Change Type

- **Lazy loading / external I/O (OCR models, config files, themes, IPC)**:
  - Test missing files, corrupt/truncated files, invalid TOML types,
    unknown future keys, and retry after failure (`OnceLock` must stay
    unset on `Err`).
- **Locale / `locales/ui.yml` edits**:
  - Verify both `"en"` and `"zh-CN"` resolve every added key (a missing
    key returns the raw key path string — assert inequality against the
    key name).
  - Serialize any test calling `rust_i18n::set_locale` behind a process
    mutex (`LOCALE_LOCK` in `src/i18n.rs`), because cargo runs unit tests
    on parallel threads.
- **Annotation / pixel / chrome geometry edits**:
  - Cover fractional scale (`1.25`), `0×0` / `1×1` degenerate bounds,
    inverted drag directions (bottom-right → top-left), transparent source
    pixels, and undo/redo symmetry.
  - Never hardcode fragile absolute pixel coordinates in headless `grim`
    assertions: prove the surface rendered first (against sway's `#ff00ff`
    sentinel background), then assert relative geometry.
  - Capture animations or async raster handoffs with a frame burst, never
    a single screenshot.
- **Safety rules during headless runs**:
  - Always write artifacts inside the repo (`target/headless/`); sandbox
    `/tmp` is isolated per command and unreadable across shells.
  - Never run `pkill -f <pattern>` when `<pattern>` appears in your own
    shell command — kill exact PIDs (`kill $AP $SW`).
