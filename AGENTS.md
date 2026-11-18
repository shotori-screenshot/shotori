# AGENTS.md

Guidance for coding agents working in this repository (`CLAUDE.md`
imports this file so every harness shares one source of truth). Keep it
high-signal: root rules are traps we have actually fallen into plus a
task router into [`docs/agents/`](docs/agents/README.md). Detailed
reasoning lives in code comments next to the logic it guards.

## Project Overview

Shotori is a Wayland-native screenshot tool built with
[gpui-kit](https://crates.io/crates/gpui-kit) and on-device OCR
(PP-OCRv5/v6 via `rapidocr-core`). It freezes every screen (one
layer-shell overlay per output), supports rich in-canvas annotations,
and routes a selection to six exits: **clipboard**, **save** (portal
dialog or direct save), **OCR**, **QR scan**, **pin** (floating
always-on-top crop), and **long-capture** (`scroll`).

Cargo workspace: root `shotori` binary/lib crate plus dev-only Wayland
tools `tools/vptr` (virtual pointer/keyboard injector) and
`tools/scrollprobe` (screencopy/pointer repro harness).

## Read Before Editing (Progressive Disclosure)

Load **only** the docs relevant to your task:

| Task / Area | Read First |
| --- | --- |
| Any task / orientation | `src/lib.rs` header · [`docs/agents/README.md`](docs/agents/README.md) |
| Module boundaries, CLI, actions, coordinates, adding settings/shortcuts | [`docs/agents/architecture.md`](docs/agents/architecture.md) |
| Wayland screencopy, pixel formats, window snap IPC, exits & daemons | [`docs/agents/platform-and-exits.md`](docs/agents/platform-and-exits.md) |
| Overlays, pins, settings window, Wayland layer-shell, themes, i18n | [`docs/agents/ui-and-wayland.md`](docs/agents/ui-and-wayland.md) · [`docs/theme.example.toml`](docs/theme.example.toml) |
| Canvas annotations, toolbar options, rasterizer, undo/selection/focus | [`docs/agents/annotations.md`](docs/agents/annotations.md) · [`docs/annotation-parity.md`](docs/annotation-parity.md) |
| Unit tests, `#[gpui_kit::test]`, headless pixel review, `SHOTORI_DEBUG_*`, perf | [`docs/agents/testing-and-verification.md`](docs/agents/testing-and-verification.md) · [`docs/headless-e2e.md`](docs/headless-e2e.md) |
| Long-screenshot stitching, foreground matcher, scroll preview | [`docs/long-capture.md`](docs/long-capture.md) |
| Custom ONNX / `rapidocr-core` model pipelines | [`docs/ocr-custom-model.md`](docs/ocr-custom-model.md) |
| Parallel `jj` workspaces, CodeGraph, CI gate, config/doc sync | [`docs/agents/workflow.md`](docs/agents/workflow.md) · [`docs/settings.example.toml`](docs/settings.example.toml) |

If `.codegraph/` exists at the workspace root, query CodeGraph
(`codegraph_explore` MCP tool or `codegraph explore "<symbol or question>"`)
before grepping or reading broad file trees.

## Commands & CI Gate

```bash
cargo build                                                # root package only (NOT --workspace)
cargo build --workspace                                    # includes tools/vptr + tools/scrollprobe
cargo fmt --all --check                                    # CI gate 1
cargo clippy --all-targets -- -D warnings                  # CI gate 2
cargo clippy --all-targets --features perf -- -D warnings  # CI gate 3 (perf feature)
cargo test                                                 # CI gate 4 (unit + headless widget tests)
cargo run --release -- --bench                             # deterministic pixel/encode microbenchmarks
cargo run --release --features perf -- --perf              # E2E latency & RSS harness
```

CI (`.github/workflows/ci.yml`) runs the exact `fmt` + both `clippy` +
`test` commands above — run all four locally before finishing a task.
Fresh Linux machines need `pkg-config libfontconfig1-dev libfreetype-dev
libxkbcommon-dev libwayland-dev`.

## Architecture Invariants

- **Strict dependency DAG** (`src/lib.rs`): `ui → model → platform`,
  never back up (save for `platform::scroll_capture` driving
  `model::scroll_sheet`). `actions.rs` and `render_image.rs` are leaf
  modules referenced across layers; never introduce upward imports.
- **Pure logic in `model/`**: state machines, placement math, smart
  regions, scroll stitching, and settings persistence stay compositor-free
  and unit-tested in `model/` (or `annotation/`), never buried in `ui/`
  assembly.
- **Debug backdoors stay isolated**: `ui/e2e.rs` owns `SHOTORI_DEBUG_*`
  entry points; keep production assembly free of ad-hoc test branches.

## Hard Rules

### Windows & the Wayland compositor

- **Overlays always use bare `cx.open_window` with `LayerShellOptions` —
  never `base::Root` / `component::Root`.** `Root`'s `WindowState` paints
  a themed background over layer-shell windows and its border calls
  `set_client_inset(20)`, growing the surface ("the Root/CSD poisoning
  case"). Only the regular `shotori settings` toplevel wraps in `Root`.
- **Pin every layer-shell window to its output via `display_id`.**
  Without it the compositor picks an output and mixed-DPI multi-monitor
  setups land overlays on the wrong screen ("the 240Hz invisibility case").
- **`cx.displays()` is always empty during synchronous startup** (zed
  #46378). Open windows from a spawned async task after the first
  event-loop pass.
- **A portal dialog cannot coexist with overlays.** Layer-shell surfaces
  with exclusive keyboard sit above regular toplevels; unmap overlays
  before opening `rfd` save/file dialogs.
- **`cx.quit()` does not unmap Wayland surfaces.** Requests may sit
  unflushed in the socket buffer. When surfaces or handoffs must flush
  cleanly (save dialog, tray): use `QuitMode::Explicit`, remove every
  window, then quit from a ~150 ms timer.
- **Implicit grab delivers the entire gesture — including release — to
  the press window**, with out-of-bounds local coordinates after a
  cross-screen drag. Handle pointer gestures in window-level listeners,
  not element `on_mouse_down` (hit-testing drops out-of-bounds events).
- **Pointer position is desktop-global session state**
  (`session.pointer_global`), updated by whichever window receives an
  event. Per-window pointer caches go stale when chrome appears under a
  window the pointer has not moved over yet.

### Multi-monitor geometry & pixels

- **`window.scale_factor()` misreports across monitors.** Always compute
  crop/raster scale as `captured_physical_width / window_logical_width`.
- **gpui display bounds = output logical position ÷ `wl_output` integer
  scale.** Match displays on scaled position, never on size (`wl_output`
  scale is `ceil`, making size matching infeasible on fractional DPI).
- **Round each edge independently** (`round_px`); never compute opposite
  edges as `round(l) + round(w)` ("the 1px white-line bleed bug").
- **`RenderImage` expects BGRA** (Vulkan); PNG/annotation buffers use
  RGBA. Route every `RenderImage` conversion through `src/render_image.rs`
  (re-exported at `ui/image_util.rs`) — never hand-swap channels elsewhere.
- **Single geometry source per concept.** Visual layout, cursor regions,
  and hit-test rects must read the same constants (e.g.
  `model/placement.rs` and `session.toolbar_bounds()`), never layout side
  effects ("the grip/cursor alignment trap").

### gpui API & runtime pitfalls

- **`dispatch_action` stops at the end of the window's focus path** — it
  does not bubble to `App::on_action`. Handle actions on the window view.
- **`handle.update()` on the window currently handling an action is a
  no-op** (gpui temporarily removes the window from its map); mutate via
  the handler's own `window` reference.
- **`window_handle.update` yields `AnyView`** — mutate concrete view
  state from async tasks via `entity.update`.
- **`#[cfg]` cannot hang mid-method-chain.** Absorb platform branches
  via `.children(Option)` or a precomputed `Option<AnyElement>`.
- **Runtime keystrokes go through `actions::parse_keystroke` and
  `KeyBinding::load`.** `gpui-pre` 0.3.10 restricted `Keystroke::parse`
  and `KeyBinding::new` to `'static` literals without `secondary-` OS
  resolution; never `Box::leak` runtime strings to fake `'static`.
- **Never panic on gpui's background executor.** Fallible lazy init (OCR
  engine) returns `Result` and leaves `OnceLock` unset so retries work.
- **`reqwest::blocking` panics inside async executors** — run model
  downloads on a dedicated OS thread (`std::thread::spawn`).
- **Outliving work runs as re-exec'd child processes** (`--notify`,
  `--clipboard-daemon`), matched on `argv[1]` before `clap` runs so
  `process::exit` in the parent never kills active notifications or
  Wayland clipboard offers.

### Dependencies

- Keep `image` locked to the exact minor version `gpui-pre` builds
  `RenderImage` from.
- Keep `reqwest` on `0.12` (matches `rapidocr-core`; `0.13` duplicates
  the entire HTTP/TLS dependency tree).

## Code Style

- **English everywhere**: doc comments, inline comments, identifiers,
  commit messages, test names. **The sole exception is user-facing UI
  copy**, which MUST live in `locales/ui.yml` (`en` + `zh-CN`, fallback
  `en`) and resolve via `crate::i18n::t!` — never inline UI strings.
- **No god files**: when a file mixes a shell with pages or a core type
  with large vocabularies (~1000+ lines of non-test code), split into
  `foo.rs` + `foo/*.rs` children and keep external paths stable with
  `pub use` (`ui/overlay`, `ui/settings`, `model/settings`, `annotation`).
- **Comments explain *why*, not *what*** — preserve war stories,
  measured numbers, and rejected alternatives next to the code.
- **Zero runtime panics / silent drops**: no `unwrap()`/`expect()` on
  runtime paths; no `let _ =` on fallible operations (use explicit
  `drop(...)` when intentionally releasing a guard/value).
- **In-file unit tests**: keep `mod tests` next to the logic it covers;
  prefer extending existing modules over proliferating tiny test files.

## Testing Summary

- **Never open windows on the maintainer's live desktop** — it steals
  focus and switches workspaces. For pixel inspection, always run a
  private headless sway session via `niri msg action spawn` writing to
  `target/headless/` with isolated `HOME`/`XDG_*` dirs
  ([`docs/headless-e2e.md`](docs/headless-e2e.md)).
- Use pure `#[test]` for model/geometry/config logic,
  `#[gpui_kit::test]` for widget focus/menus/events without a compositor,
  and `SHOTORI_DEBUG_*` backdoors for headless pixel verification. Full
  tier guide and env var table:
  [`docs/agents/testing-and-verification.md`](docs/agents/testing-and-verification.md).
- Always test failure paths (corrupt TOML, missing files, offline/cancel,
  fractional scales, empty/1px bounds) alongside happy paths.

## Documentation Duties & Rules Hygiene

- **Use `jj` exclusively for all VCS operations** (`jj status`, `jj diff`,
  `jj commit` / `jj describe`, `jj bookmark set`, `jj git fetch`,
  `jj git push`) — never mutate commits, branches, or working trees with
  raw `git` commands in this colocated repo (running `git commit` behind
  `jj`'s back leaves `@` on the old parent and snapshots a duplicate
  side-branch commit on the next `jj` invocation).
- **Never edit `README.md` or `README.zh-CN.md`** (maintainer-owned;
  note user-visible changes in commit/PR descriptions instead).
- Keep [`docs/settings.example.toml`](docs/settings.example.toml),
  [`docs/theme.example.toml`](docs/theme.example.toml), `locales/ui.yml`,
  and [`docs/agents/`](docs/agents/README.md) in sync whenever settings,
  theme keys, shortcuts, `SHOTORI_DEBUG_*` knobs, or architecture seams
  change.
- Root `AGENTS.md` accepts only non-obvious, high-cost, actionable traps.
  Put subsystem maps, extension checklists, and reference tables in
  `docs/agents/` or inline rustdoc comments next to the code.
