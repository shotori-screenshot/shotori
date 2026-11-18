# Agent Specifications (`docs/agents/`)

Progressive-disclosure engineering specifications for AI coding agents
working on Shotori.

## Context-Loading Protocol

1. **Always start at [`AGENTS.md`](../../AGENTS.md)** (`CLAUDE.md` points
   to it). It carries universal Wayland/gpui invariants and the task
   router.
2. **Load only the subsystem spec(s) matching your slice.** Do not dump
   every file in `docs/` into context on routine edits.
3. **Query CodeGraph before broad file searches** whenever `.codegraph/`
   is present (`codegraph_explore` MCP tool or
   `codegraph explore "<symbol or question>"`).
4. **Read module-level `//!` comments in target source files.** Shotori
   documents rejected designs, measured latencies, and compositor quirks
   directly next to the code they guard.

## Hierarchy of Truth

When two sources appear to disagree:

1. **Executable code, tests, and inline `//!` / `//` war-story comments**
   win — verify what compiles and passes today.
2. **[`AGENTS.md`](../../AGENTS.md)** defines non-negotiable repository
   constraints and safety rules.
3. **Subsystem specs in `docs/agents/` and `docs/*.md`** map seams,
   contracts, and verification procedures. If code intentionally outgrows
   a spec, update the spec in the same change.

## Specification Catalog

### Core Agent Specs (`docs/agents/`)

| File | Scope |
| --- | --- |
| [`architecture.md`](architecture.md) | Dependency DAG, multi-process architecture, coordinate spaces, and step-by-step checklists for adding actions, settings, CLI flags, or module splits |
| [`platform-and-exits.md`](platform-and-exits.md) | `wlr-screencopy` v3/shm pixel rules, display matching, compositor window-snap IPC, and the 6 exits + tray/clipboard/notify child processes |
| [`ui-and-wayland.md`](ui-and-wayland.md) | Window surface taxonomy (layer-shell vs regular toplevel), placement math (`model/placement.rs`), theme (`theme.toml` + `gpui-component` sync), and i18n (`locales/ui.yml`) |
| [`annotations.md`](annotations.md) | `src/annotation*` data model, live preview vs `raster.rs` export parity, spotlight/magnifier compositing rules, undo/selection/focus invariants |
| [`testing-and-verification.md`](testing-and-verification.md) | 4-tier test strategy (`#[test]`, `#[gpui_kit::test]`, headless sway + `grim`, `--bench`/`--perf`), complete `SHOTORI_*` env var table, mandatory edge-case gates |
| [`workflow.md`](workflow.md) | Parallel `jj` / Git worktree isolation, CodeGraph workflow, local CI gate, config/doc sync matrix, and maintainer boundaries |

### Domain & User-Format Docs (`docs/`)

| File | Scope |
| --- | --- |
| [`../headless-e2e.md`](../headless-e2e.md) | Private headless sway + `grim` recipe via `niri msg action spawn` (how to inspect pixels without touching the live desktop) |
| [`../annotation-parity.md`](../annotation-parity.md) | Annotation capability matrix, PixPin parity scope, interaction rules, and visual acceptance gates |
| [`../long-capture.md`](../long-capture.md) | Working design notes, open tradeoffs, and pixel-exact fixtures for `scroll_sheet` + `scroll_capture` |
| [`../ocr-custom-model.md`](../ocr-custom-model.md) | Custom `rapidocr-core` TOML schema, stage flags, and path resolution rules |
| [`../settings.example.toml`](../settings.example.toml) | Canonical user-facing `settings.toml` reference — keep synced with `src/model/settings/` |
| [`../theme.example.toml`](../theme.example.toml) | Canonical user-facing `theme.toml` reference — keep synced with `src/ui/theme/load.rs` |
