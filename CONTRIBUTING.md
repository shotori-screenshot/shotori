# Contributing to Shotori

Thanks for your interest! Bug reports, compositor-compatibility
feedback and patches are all welcome.

## Ways to help

- **Bug reports** — open an
  [issue](https://github.com/shotori-screenshot/shotori/issues) with your
  compositor + version, output layout (monitors, scales, rotation) and
  steps to reproduce. A screenshot or the terminal output of the run
  helps a lot.
- **Compositor reports** — Shotori is currently tested on niri and
  Hyprland only. Reports from other wlroots-adjacent compositors
  (sway, KWin, labwc, river, Wayfire, COSMIC, …) are valuable even
  when everything works.
- **Patches** — the checklist below applies.

## Getting set up

Fresh machines need the native build deps (what CI installs):

```sh
# Debian/Ubuntu
sudo apt install pkg-config libfontconfig1-dev libfreetype-dev \
                 libxkbcommon-dev libwayland-dev
# Fedora
sudo dnf install pkgconf fontconfig-devel freetype-devel \
                 libxkbcommon-devel wayland-devel
```

```sh
git clone https://github.com/shotori-screenshot/shotori
cd shotori
cargo build            # the app (root package only)
cargo test             # unit tests — no compositor needed
```

## Before you open a PR

CI runs exactly these — match it locally:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features perf -- -D warnings
cargo test
```

Keep commits to concise English imperative messages, one logical
change per commit.

## Finding your way around

- [`src/lib.rs`](src/lib.rs) header — the module map and the
  dependency contract: `ui → model → platform`, never back up.
  `actions.rs` is the shared vocabulary between keybindings, toolbar
  buttons and overlay handlers.
- `model/` is pure logic and state, unit-tested without a compositor —
  new interactive logic belongs there, not in overlay assembly.
- `ui/e2e.rs` owns the `SHOTORI_DEBUG_*` backdoors for headless e2e
  runs; production assembly stays free of them.

## Code conventions

- English everywhere: doc comments, inline comments, UI strings, test
  names.
- Comments explain **why**, not what — especially traps and rejected
  alternatives, next to the code they concern.
- No `unwrap()`/`expect()` on runtime paths; no `let _ =` on fallible
  operations (write `drop(...)` when the drop is intended).
- Tests live in-file next to the logic (`mod tests`); prefer extending
  existing files over creating new small ones.
- One geometry source per concept — visual, cursor and hit-test rects
  derive from the same constants (`model/placement.rs`).
- Failure paths (offline, corrupt files, missing protocols) are
  mandatory testing for lazy-loading designs; a success-path test
  alone is not enough.
- App icon assets live in `assets/app/`; after changing the SVG,
  regenerate the PNG/ICO set with `python3 tools/generate-icons.py`
  (requires `rsvg-convert`).
