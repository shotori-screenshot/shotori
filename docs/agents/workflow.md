# AI Agent Workflow, Parallel Workspaces & Hygiene

Operational rules for agents working concurrently across Shotori
workspaces.

## 1. Exploration Order

1. Read [`AGENTS.md`](../../AGENTS.md) and the single matching spec in
   [`docs/agents/`](README.md).
2. If `.codegraph/` exists at the repo root, use `codegraph_explore` (MCP)
   or `codegraph explore "<symbol or question>"` (CLI) before running broad
   `rg`/`find` sweeps — it resolves Rust trait/method dispatch and returns
   line-numbered definitions in one call.
3. Read the module header (`//!`) and neighboring tests in the target file
   before designing edits.

## 2. Parallel Workspaces (`jj` + Colocated Git)

Shotori uses colocated Jujutsu (`.jj`) + Git worktrees for parallel agent
streams (e.g. `../shotori-ui`, `../shotori-annotation`,
`../shotori-tests`, `../shotori-docs`):

```bash
# Spawn an isolated workspace rooted at main:
jj workspace add ../shotori-<topic> -r main

# Inspect active workspaces and their working-copy commits:
jj workspace list
git worktree list
```

### Multi-Agent Isolation Rules

- **Stay inside your assigned workspace.** Never edit or run `git checkout`
  / `jj edit` inside a sibling workspace where another agent or the
  maintainer has uncommitted changes.
- **Keep file scope tight to avoid merge conflicts.** An agent working on
  UI styles should not opportunistically reformat `src/annotation/`, and a
  docs-only workspace should avoid unnecessary code churn in active `.rs`
  hotspots unless fixing code/doc contract bugs explicitly in scope.
- **Never touch the live Wayland desktop.** Do not invoke `shotori`,
  `shotori settings`, or `tools/vptr` against the maintainer's active
  `WAYLAND_DISPLAY`. Always isolate GPU/pixel checks inside a private
  headless `sway` instance writing to `<your-workspace>/target/headless/`
  with throwaway `HOME` and `XDG_*` directories
  ([`docs/headless-e2e.md`](../headless-e2e.md)).

## 3. Pre-Completion CI Gate

Before declaring any code change ready or opening a commit/PR, run the
exact four checks enforced by `.github/workflows/ci.yml`:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features perf -- -D warnings
cargo test
```

If you modified `tools/vptr` or `tools/scrollprobe`, also run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 4. Mandatory Synchronization Matrix

Code changes that alter contracts **must** update their companion files in
the same commit/branch:

| When You Change... | You Must Also Update... |
| --- | --- |
| Any user-visible UI text, tooltip, or notification | `locales/ui.yml` (both `en` and `zh-CN` columns; quote YAML boolean-like keys such as `"off"`) |
| `Settings` schema, defaults, or `[keys]` / `[toolbar]` ids (`src/model/settings/`) | [`docs/settings.example.toml`](../settings.example.toml) |
| `Theme` fields or `ColorKey` / `ThemeFile` (`src/ui/theme/`) | [`docs/theme.example.toml`](../theme.example.toml) |
| Custom OCR TOML handling (`src/ocr.rs`) | [`docs/ocr-custom-model.md`](../ocr-custom-model.md) |
| `SHOTORI_DEBUG_*` env vars (`src/ui/e2e.rs`, `src/ui/settings.rs`) | [`docs/agents/testing-and-verification.md`](testing-and-verification.md) and [`docs/headless-e2e.md`](../headless-e2e.md) |
| Annotation tool parity or interaction rules | [`docs/annotation-parity.md`](../annotation-parity.md) and [`docs/agents/annotations.md`](annotations.md) |
| Long-capture resolved open questions (`src/model/scroll_sheet*`) | Delete resolved items from [`docs/long-capture.md`](../long-capture.md) and move durable war stories into inline code comments |
| Top-level modules in `src/` | Crate-root diagram in [`src/lib.rs`](../../src/lib.rs) and [`docs/agents/architecture.md`](architecture.md) |
| App icon SVG (`assets/app/`) | Regenerate PNG/ICO set via `python3 scripts/generate-icons.py` |
| User-facing features or CLI behavior | Describe clearly in commit/PR message — **never edit `README.md` or `README.zh-CN.md`** |

## 5. Commit & Documentation Style

- **Commits**: concise English imperative subject (`feat(qr): ...`,
  `fix(scroll): ...`, `docs(agents): ...`), one logical change per commit.
- **Inline comments vs Markdown docs**:
  - Put *local* traps, algorithm thresholds, and protocol quirks in code
    comments right next to the lines they protect — code comments move with
    refactors and are read at edit time.
  - Put *cross-cutting* contracts, module boundaries, workflows, and
    verification tables in `AGENTS.md` / `docs/agents/`.
