# Platform Capture, Window Snapping & Desktop Exits

Read before modifying `src/platform/`, `src/clipboard.rs`,
`src/save_dialog.rs`, `src/ocr.rs`, `src/qr.rs`, `src/notify.rs`, or
`src/tray.rs`.

## 1. Screen Capture & Pixel Normalization (`src/platform/capture/`)

- **Wayland protocol state machine (`capture/wayland.rs`)**:
  - Uses `zwlr_screencopy_manager_v1` + `wl_shm` + `zxdg_output_v1`.
  - **`v3+` `buffer_done` gate**: `zwlr_screencopy_frame_v1` v3 sends
    multiple buffer announcements (shm + dmabuf) terminated by
    `buffer_done`. Never issue `copy` before `buffer_done` on v3+, and
    guard with `copied: bool` so duplicate buffer batches never copy
    twice.
  - **Logical vs physical output geometry**: `zxdg_output_v1` provides
    true logical position and logical size (with fractional scale and
    transform applied); `wl_output` only reports integer `ceil` scale and
    unrotated mode size.
- **Format swizzling & rotation (`capture/pixels.rs`)**:
  - Kept Wayland/GPUI-free so every pixel format and all 8
    `wl_output::Transform` orientations (`Normal`, `Rot90/180/270`,
    `Flipped*`, calibrated against `grim`) are pure unit tests.
  - **`wl_shm::Format` byte-order trap**: format names describe 32-bit
    MSB→LSB words, so on little-endian hosts `Xrgb8888`/`Argb8888` sit in
    memory as `B, G, R, X/A` (and `wl_shm::Format` values are protocol
    ordinals like `1`, **not** DRM fourccs).
  - **Fail soft on unsupported formats / bad strides**: only 8-bit
    `Xrgb8888`, `Argb8888`, `Xbgr8888`, and `Abgr8888` are swizzled.
    10-bit/HDR shm formats (e.g. KWin HDR) or malformed compositor
    strides return `Vec::new()` so callers skip the frame/output cleanly
    instead of scrambling channels or panicking inside the Wayland event
    loop.

## 2. Display Matching (`src/platform/display.rs`)

- GPUI's Wayland connection and Shotori's capture connection do not share
  `wl_output` object IDs; outputs are matched to `DisplayId` purely by
  origin geometry:
  `gpui_display_origin == capture_logical_pos / wl_output_integer_scale`.
- Match **only** on scaled origin position, never on size (fractional
  scales make `wl_output` integer-scaled sizes diverge).
- `await_display_ids` polls asynchronously after startup (10 ms × 4 fast
  rounds, then 50 ms up to ~1 s) because `cx.displays()` is empty before
  the first event-loop turn.

## 3. Compositor Window Snapping (`src/platform/windowsnap/`)

Wayland exposes no cross-client window tree protocol, so Shotori queries
per-compositor IPC sockets (`first env var hit wins`):

| Compositor | Env Trigger | Mechanism & Schema Rules |
| --- | --- | --- |
| **niri** | `NIRI_SOCKET` | `niri-ipc` crate. Filters `Workspace::is_active`, requires `tile_pos_in_workspace_view: Some(...)` (floating windows on all versions; tiled scrolling columns since upstream `niri#4711`), and clips rects to `output.logical` bounds — never guess tiled rects from pixel edges. |
| **sway** | `SWAYSOCK` | i3 IPC `GET_TREE` over Unix socket, parsed tolerantly with `serde_json::Value`. |
| **Hyprland** | `HYPRLAND_INSTANCE_SIGNATURE` | `j/clients` over Unix socket, parsed tolerantly with `serde_json::Value` (schemas drift across releases; rigid `#[derive(Deserialize)]` structs break). |
| **Other compositors** | — | Returns `None` silently; hover/click degrades cleanly to smart-region / manual drag. |

All backends return `SnapRect` in **desktop-global logical coordinates**.

## 4. Selection Exits & Resident Helpers

### Clipboard (`src/clipboard.rs`)

- Wayland requires a live `zwlr_data_control_source_v1` owner to serve
  paste requests. `copy_image` (`image/png`) and `copy_text`
  (`text/plain;charset=utf-8`) spawn `shotori --clipboard-daemon <mime>`
  via `Command::new(current_exe())` (never `fork()` inside a
  multithreaded GPUI/Vulkan process) and pipe bytes over `stdin`.
- Error reporting distinguishes missing `WAYLAND_DISPLAY` from a
  compositor lacking `zwlr_data_control_manager_v1`.

### Save Flow (`src/save_dialog.rs` & `src/model/export.rs`)

- **Portal dialog path** (`auto_save = false`):
  1. Overlay crops RGBA8 pixels into `save_dialog::stash` (`PENDING`).
  2. `close_overlays(window, cx)` removes the active window via
     `window.remove_window()` (because `handle.update()` on the active
     window is a GPUI no-op) and removes sibling overlays via handles.
  3. After a ~150 ms flush timer, `cx.quit()` exits the GPUI loop.
  4. `main.rs` calls `save_dialog::complete_pending()`, opening `rfd`
     (`xdg-portal` / `ashpd`, zero GTK link dependency; `zenity`
     fallback) once layer-shell surfaces are gone, then encodes
     PNG/JPEG/WebP and triggers notification / optional `copy_on_save`.
- **Direct save** (`[export] auto_save = true`) or
  `SHOTORI_DEBUG_SAVE_PATH`: writes directly without opening the portal
  and supports `keep_open_after_save`.

### On-Device OCR (`src/ocr.rs` & `src/ui/ocr_setup.rs`)

- Runs `rapidocr-core` (`PP-OCRv5` / `PP-OCRv6` presets or user
  `[ocr] custom_config`, documented in
  [`docs/ocr-custom-model.md`](../ocr-custom-model.md)).
- Cached models in `$XDG_DATA_HOME/shotori/ocr-models` pre-warm in the
  background. Missing bundled assets open `OcrSetup` in-overlay.
- **Download & init invariants**:
  - Downloads run on a dedicated OS thread (`reqwest::blocking` panics
    inside GPUI's async executor), hold a cross-process lock, verify
    SHA-256 digests, and install atomically via temp files.
  - Engine `OnceLock` caches **only** `Ok` instances — initialization
    errors surface to the user and leave `OnceLock` unset so retries
    succeed after fixing files or finishing downloads.

### QR Code Scanner (`src/qr.rs`)

- Uses `rqrr` on cropped RGBA8 buffers:
  1. Alpha-composites transparent pixels over white before luma
     conversion.
  2. Pads a `16px` white border (`QUIET_ZONE_PX`) so tight crops touching
     finder patterns still decode.
  3. Retries on **inverted luma** (`255 - y`) when the normal pass finds
     nothing, because `rqrr` only binarizes dark-on-light modules.

### Desktop Notifications (`src/notify.rs`)

- Spawns detached `shotori --notify <summary> <body> [image] [open_path]`
  (`org.freedesktop.Notifications` over D-Bus) so `cx.quit()` never
  cuts off delivery or action clicks ("View image", "Open text").
- Parent translates `summary`/`body` up front and writes raw PNG bytes
  (zero parent downscale work); the child decodes/downscales thumbnails
  (`256px` max) and sweeps cached previews older than 24 hours.
- Missing/failing notification daemons log/ignore silently — notifications
  must never fail a capture exit.

### System Tray (`src/tray.rs`)

- `shotori tray` acquires an OS single-instance lock, runs under
  `QuitMode::Explicit` (zero windows), and hosts a `StatusNotifierItem`
  icon via `gpui-tray`.
- Menu actions launch isolated `shotori gui` or `shotori settings`
  processes via `crate::launch_detached`, which spawns a lightweight
  `child.wait()` reaper thread so long-lived tray sessions never leak
  zombie processes.
