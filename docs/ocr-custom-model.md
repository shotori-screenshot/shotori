# Custom OCR models

Shotori ships a curated set of PP-OCR model configs (Settings → OCR).
If you have your own ONNX models — a fine-tuned detector, a
community recognizer, a different dictionary — you can point shotori
at a [rapidocr-core](https://crates.io/crates/rapidocr-core) config
TOML and it will run exactly that pipeline instead.

## Enabling

In `~/.config/shotori/settings.toml`:

```toml
[ocr]
custom_config = "~/models/my-ocr.toml"
```

- A leading `~` expands to your home directory; relative paths in the
  setting itself are resolved against your home directory too.
- While a custom config is active it **overrides** the bundled
  `model` choice entirely: the settings window's dropdown disables
  and the OCR page shows the config's path and status (ready, or the
  first problem found).
- No download ever happens in this mode — the files are yours.
- Delete the key (or leave it empty) to go back to the bundled models.
- Reopen the settings window after editing `settings.toml` by hand:
  the window reads the file once at open and writes its snapshot back
  on every toggle, so a stale window would erase your edit.

## The config format

The file is a `rapidocr_core::RapidOcrConfig`. The scalar knobs are
required **at the top level** (not under a section):

```toml
# ── top-level pipeline knobs (all required) ──────────────────
text_score = 0.5          # minimum recognition confidence
min_side_len = 30         # high-level resize bounds
max_side_len = 2000
min_height = 30           # vertical padding gate
width_height_ratio = 8.0  # -1.0 disables the ratio gate

# ── which stages run ─────────────────────────────────────────
[pipeline]
use_det = true
use_cls = false           # orientation classifier (shotori's
                          # "skip orientation detection" toggle can
                          # also turn this off, never on)
use_rec = true

# ── one section per enabled stage ────────────────────────────
[det]
model_path = "det.onnx"
limit_side_len = 960
limit_type = "Max"        # or "Min"
mean = [0.485, 0.456, 0.406]
std = [0.229, 0.224, 0.225]
thresh = 0.3
box_thresh = 0.6
max_candidates = 1000
unclip_ratio = 1.6
min_size = 3

[rec]
model_path = "rec.onnx"
dict_path = "dict.txt"    # one character/token per line
image_shape = [3, 48, 320]
batch_size = 16
```

Rules of thumb:

- A stage's section is **required iff its `use_*` flag is true**.
  A recognition-only config (`use_det = false`, `use_cls = false`,
  `use_rec = true`) needs just `[rec]` — that is the common shape for
  feeding shotori's already-cropped selection into your own
  recognizer.
- Every path is resolved as follows: a path starting with `~` expands
  against your home directory, otherwise if it is relative it is
  rebased against the **directory containing the config TOML** (the
  process working directory of a desktop-launched app is arbitrary,
  so don't rely on it). Absolute paths pass through untouched.
- The config is validated when the engine is built. When something is
  wrong, the OCR page shows `Unusable: <reason>` (parse error, or the
  first missing model file) and OCR failures carry the same reason in
  their notification.

## Where things live

- Bundled-model cache: `$XDG_DATA_HOME/shotori/ocr-models` (usually
  `~/.local/share/shotori/ocr-models`); open it from Settings → OCR →
  Cache → Open. Custom-config files are **not** stored there — shotori
  only reads whatever path you configured.

## Troubleshooting

- *"Unusable: …"* on the OCR page — the reason is the parse error or
  the first missing file; fix the TOML or the paths and the status
  refreshes (a change to a model *file* alone doesn't touch the TOML,
  so the cached status updates on the next settings save or restart —
  the engine itself re-validates every file at OCR time).
- A working config that suddenly breaks after an editor save —
  the status line picks up the parse error immediately on the next
  engine build; no settings restart needed.
- Orientation: if your recognizer wants rotated text handled
  elsewhere, keep `use_cls = false` and leave
  `assume_upright = false`; shotori never re-enables a classifier
  your config skipped.
