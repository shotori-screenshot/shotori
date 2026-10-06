# AGENTS.md

Guidance for coding agents working in this repository. Keep it
high-signal: the rules below are traps we have actually fallen into, not
a map of the code — read the code for the map. The reasoning behind each
rule lives in the code comments next to the logic it guards.

## Project Overview

Shotori is a Wayland-native screenshot tool with built-in, on-device OCR
(PP-OCRv6 via rapidocr-core), its UI hand-drawn with
[gpui-kit](https://crates.io/crates/gpui-kit). It freezes every screen
(one layer-shell overlay window per output), then a selection flows to
one of four exits: clipboard, native save dialog, OCR, pin. One `shotori`
crate plus `tools/vptr` (wlr virtual-pointer injector for interactive
e2e tests; a workspace member, not part of the binary).

## Read First

- `src/lib.rs` header — the module map and the dependency contract.

## Commands

```bash
cargo build                  # root package only (NOT --workspace)
cargo build --workspace      # includes tools/vptr
cargo test                   # unit tests, no compositor needed
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features perf -- -D warnings  # CI lints the perf gate too
cargo run --features perf -- --perf        # dev-only E2E perf harness
```

CI (`.github/workflows/ci.yml`) runs exactly the fmt + both clippy +
test steps above — match it locally before pushing. Fresh machines need
`pkg-config libfontconfig1-dev libfreetype-dev libxkbcommon-dev
libwayland-dev` (see CI).

## Architecture Invariants

- Dependency direction, documented in `src/lib.rs`: `ui → model`,
  `model → platform`, never back up. `actions.rs` is the shared
  vocabulary between keybindings, toolbar buttons and overlay handlers —
  everyone references it, it references no one. Don't reintroduce an
  import cycle by defining actions elsewhere.
- `model/` is pure logic and state, unit-tested without a compositor.
  New interactive logic goes there, not into overlay assembly.
- `ui/e2e.rs` owns the `SHOTORI_DEBUG_*` backdoors — they are the only
  entry point for headless e2e; keep production assembly free of them.

## Hard Rules

### Windows & the Wayland compositor

- **Overlays always use a bare `cx.open_window` with layer-shell
  options — never `base::Root`/component `Root`.** Root's WindowState
  plugin paints a themed background over layer-shell windows and its
  border calls `set_client_inset(20)`, growing the window ("the
  Root/CSD poisoning case").
- **Pin every layer-shell window to its output via `display_id`.**
  Without it the compositor picks an output and mixed-DPI multi-monitor
  setups get an overlay on the wrong screen ("the 240Hz invisibility
  case" — placement lottery, not rendering).
- **`cx.displays()` is always empty during synchronous startup** (zed
  #46378). Open windows from a spawned async task after the first
  event-loop pass.
- **A portal dialog cannot coexist with the overlays.** Layer-shell
  surfaces with exclusive keyboard sit above a regular toplevel; unmap
  the overlays first (the save flow does).
- **`cx.quit()` does not unmap wayland surfaces.** Requests may sit
  unflushed in the connection buffer. To keep surfaces alive past the
  run loop (save dialog, tray): `QuitMode::Explicit`, remove every
  window, then quit from a ~150 ms timer so the loop flushes.
- **Implicit grab delivers the whole gesture — including the release —
  to the window where the press happened**, with out-of-bounds local
  coordinates possible after a cross-screen release. Handle pointer
  events in window-level listeners, not element `on_mouse_down` (element
  hit-testing silently drops out-of-bounds presses).
- **Pointer position is desktop-global session state** (`session.
  pointer_global`), reported by whichever window receives an event.
  Per-window pointer caches go stale exactly when a state flip lands
  chrome under a window the pointer never moved over.

### Multi-monitor geometry

- **`window.scale_factor()` misreports across monitors** (it can report
  another output's). Compute the crop scale as captured physical width ÷
  window logical width; that is naturally consistent with rendering.
- **gpui display bounds coordinates = output logical position ÷
  `wl_output` integer scale.** Display matching must use the same
  division; match on position, not size (`wl_output.scale` is an
  integer — ceil — so size matching is infeasible).
- **Round each edge independently** (`round_px`); never derive opposite
  edges as `round(l) + round(w)`. Divergent rounding between dim bands,
  border and toolbar once produced a 1px raw-pixel bleed line ("the
  white-line bug").

### Rendering & pixels

- **`RenderImage` wants BGRA** (Vulkan backend); in-memory RGBA must
  swap bytes 0↔2. The PNG/annotation path is RGBA. `ui/image_util.rs`
  owns this contract — don't hand-build image buffers elsewhere.
- **Never compute a hit-test rect from layout side effects.** The
  element and its hit-test rect must derive from the same constants
  (the grip/cursor alignment trap). One geometry source per
  concept, e.g. `session.toolbar_bounds()` for render, cursor and drag
  clamp alike.

### gpui API pitfalls

- **`dispatch_action` inside a window stops at the end of the focus
  path** — it does not bubble to `App::on_action`. Handle an action
  where it is dispatched (overlay handler), not on an app-level
  backstop.
- **`handle.update()` on the window currently running an action handler
  is a no-op** (gpui removes the window from the map during its update);
  the current window must act through its own `window` reference.
- **`window_handle.update` closures receive an `AnyView`** — touching
  concrete view state from async requires the `Entity` handle's
  `entity.update`.
- **`#[cfg]` cannot hang in the middle of a method chain.** Absorb it
  with `.children(Option)` (Option is an IntoIterator) or precompute an
  `Option<AnyElement>`.
- **Never panic through gpui's background executor.** Fallible lazy init
  (the OCR engine) returns `Result` and leaves the `OnceLock` unset so
  the next call retries; failures surface to the overlay instead of
  bricking the process.
- **`reqwest::blocking` cannot run in an async context** (gpui's
  background executor is one) — downloads get a dedicated thread.
- Notifications and clipboard offers run as re-exec'd child processes
  (`--notify`, `--clipboard-daemon`): the child must outlive the parent,
  and a plain background thread would be killed by `process::exit`.

### Dependencies

- The `image` crate version must match the one gpui-pre builds
  `RenderImage` from — don't bump it independently.
- `reqwest` stays on 0.12 (rapidocr-core's line); 0.13 would compile a
  second hyper/tokio/rustls tree for one GET.

## Code Style

- English everywhere: doc comments, inline comments, UI strings, test
  names (the repo is published internationally).
- Comments explain **why**, not what — the war stories stay. Don't strip
  context when refactoring.
- No `unwrap()`/`expect()` on runtime paths; no `let _ =` on fallible
  operations. When the intent really is to drop a value (e.g. release a
  lock), write `drop(...)` explicitly.
- Tests live in-file next to the logic (`mod tests`). Prefer extending
  existing files over creating new small ones; follow the existing
  `mod.rs` organization.

## Testing

- `cargo test` covers pure logic (model/, annotation geometry, args) —
  no compositor required. New interactive logic should be extractable
  and tested there.
- Headless e2e drives the real pipeline through the `ui/e2e.rs`
  backdoors: `SHOTORI_DEBUG_TARGET=<output>` (restrict to one overlay —
  several firing fight each other), `SHOTORI_DEBUG_SELECTION=x,y,w,h`,
  `SHOTORI_DEBUG_ACTION=copy|quit|save|ocr|ocrsetup` (fires ~1.5 s after
  startup), `SHOTORI_DEBUG_SAVE_PATH=<file>` (skips the dialog). Read
  back with `wl-paste --type image/png`; compare captures with `grim`.
- Don't hardcode pixel coordinates in probe assertions — prove the
  overlay is up first, then probe relative geometry (a rounding change
  once turned every hardcoded check false-negative). Capture animations
  with a burst of frames; a single frame misses 0.3 s windows.
- Failure paths (offline, corrupt files, retry) are mandatory testing
  for lazy-loading designs; success-path e2e is not enough.
- `SHOTORI_BOOT=1` prints a startup phase timing trace.

## Documentation Duties

- `README.md` / `README.zh-CN.md` are maintained by the maintainer —
  agents do not edit them. When a change alters user-facing behavior,
  say so in the PR/commit message instead.
- Non-obvious findings (decisions, rejected paths, traps) are documented
  as English comments next to the code they explain, at the moment the
  code is written — not in a separate log that drifts out of reach.
- `docs/theme.example.toml` documents the user-facing theme file format;
  keep it in sync with the loader.

## Rules Hygiene

A new rule here must be non-obvious (an agent familiar with Rust would
still get it wrong), encountered more than once or expensive enough once,
and actionable. Architectural description does not belong in this file —
it goes stale; point to `src/lib.rs` instead. Keep the story with the
code (a comment), not duplicated here.
