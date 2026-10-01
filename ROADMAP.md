# Shotori — Decision Log & Pitfall Archive

Conclusions from the research phase plus every pitfall and load-bearing
design decision recorded since. Organized by theme; entries keep their
dates. What the code does lives in the code (`src/lib.rs` header for the
module map); this file records **why** it is that way and which paths
were tried and rejected.

Principle: **trust live wayland-info probes over docs and over memory.**

How to use: read the sections touching your task before changing code.
When a change carries non-obvious findings, add a dated entry to the
matching section. `AGENTS.md` cites several entries here by name — keep
those phrases searchable when editing.

## Research phase (2026-09-25, before any code)

### Environment (measured)

- Arch Linux + niri 26.04 (zwlr_layer_shell_v1 v5, zwlr_screencopy_manager_v1
  v3, both old and new foreign-toplevel, virtual-pointer, data-control — all
  present)
- Mixed-DPI multi-monitor: HDMI-A-1 1920×1080@1.0 (0,0) + eDP-1 logical
  1536×960@1.25 (1920,0)
- `ext-image-copy-capture`: only merged into niri main on 2026-09-13 (and
  without window capture at that) — 26.04 **does not have it**. The official
  wiki documents main; don't be fooled.

### Capture backend matrix (origin of the CaptureBackend trait)

| Path | Protocol | Fits | Notes |
|------|----------|------|-------|
| ① portal | xdg-desktop-portal ScreenCast + PipeWire | every desktop (GNOME's only option) | consent dialog, video-stream frame extraction |
| ② wlr | zwlr_screencopy-unstable-v1 | niri/sway/Hyprland etc. | what grim uses; today's workhorse |
| ③ ext | the ext-image-copy-capture family | the standardized future | waiting for distro rollout; includes window capture |

Window geometry: foreign-toplevel gives a list but no coordinates →
prototyping used the `niri msg --json windows` backdoor.

### Overlay decisions

- A plain xdg window as an overlay is a disaster under a tiling compositor
  → layer-shell or nothing
- gpui-pre 0.3.6 has `WindowKind::LayerShell(LayerShellOptions)` (nobody
  in the gpui-kit ecosystem had used it; we went first — spike #1 verified)
- The toolbar must live inside the overlay window (layering deadlock: a
  normal window would be buried under its own dim layer)
- Pin (floating image) = a `Layer::Top` layer-shell window (Wayland has no
  "keep normal window on top" protocol)

### Spike #1 verification points

1. Can a layer-shell window open on niri at all (protocol handshake)
2. Is `WindowBackgroundAppearance::Transparent` actually transparent (EGL
   alpha)
3. Does the Esc focus chain work under `KeyboardInteractivity::Exclusive`
4. Does the `exclusive_zone: Some(px(-1.))` negative sentinel map to the
   protocol's -1 correctly
5. Do four-edge anchors + configure cover the whole output (including a
   1.25-scale screen)

## Pitfalls — Wayland & the compositors

### The Root/CSD poisoning case (2026-09-25)

base::Root's WindowState plugin (component) paints a themed background
onto layer-shell windows (white wall → gray haze 76 = 0.3×255),
WindowBorder calls set_client_inset(20) (window grows by +40), plus
inward padding. **Overlays must always use a bare cx.open_window; normal
windows can use Root.**

### The placement lottery, a.k.a. "the 240Hz invisibility case" (2026-09-25)

Nothing to do with the refresh rate — the real culprit was
**layer-surface placement**. When gpui doesn't pin a layer surface to an
output, niri picks one (the focused screen); the overlay sometimes landed
on DP-2 (720×1280) while grim photographed HDMI — hence "invisible". The
60Hz test windows happened to land on HDMI, manufacturing the
"refresh-rate-related" illusion. Fix: pass `display_id` when opening the
window to pin it to the captured screen (WindowOptions.display_id →
wl_outputs match → get_layer_surface(output)). **Lesson: in a
mixed-scale multi-monitor setup, when "some screen can't be captured",
check placement before rendering.**

### displays() is always empty during synchronous startup (2026-09-25)

zed#46378: all screens appear only after the first event-loop pass (first
cx.spawn update). Workaround: move window opening into a spawn'd async
task; first shot gets it. display_id matching: bounds size == capture
size (exact at scale=1; mixed-scale matching needs the vendor to expose
output names — backlog).

### A portal dialog cannot coexist with the overlay (2026-09-26)

The overlays are layer-shell surfaces on the Overlay layer with exclusive
keyboard — a regular toplevel (the dialog) renders below them and gets no
input. **The overlays must be unmapped first** (the save flow does).

### cx.quit() does not unmap wayland surfaces (2026-09-26)

It stops the run loop; the window-destroy requests may still be sitting
unflushed in the wayland connection buffer. With the process exiting
immediately nobody notices — the socket close cleans up. With the process
alive waiting on a dialog (or a tray), frozen frames stay mapped on
screen forever. Fix: `QuitMode::Explicit`, remove every window, then quit
from a 150 ms timer so the loop gets a few iterations to flush. Also
known: removing the last window auto-quits gpui on Linux
(`QuitMode::Default == LastWindowClosed` off macOS).

### Implicit grab: the gesture belongs to the press window (2026-09-27)

Three consecutive user reports, one root behavior — Wayland's implicit
grab delivers the whole gesture (including the release) to the window
where the press happened:

1. **Cross-screen release** dropped the chrome on the floor. The label
   and toolbar render only on `active_output`'s overlay, but only
   `pointer_down` ever re-hosts — a move/resize released over the seam
   left `active_output` on the press screen: old screen no longer
   intersects the selection, new one is not "active" — blank everywhere.
   Fix: `follow_selection_host()` after every finalized landing,
   re-hosting to the output holding the selection's largest intersection,
   STICKY (the incumbent wins ties, so an ambiguous straddle never churns
   the chrome).
2. **The cursor must not depend on a per-window pointer.** The window
   that physically holds the pointer receives no events at all under
   implicit grab, so its per-window pointer cache was stale exactly when
   a state flip landed chrome under a window the pointer never moved
   over. Fix: the pointer's GLOBAL desktop position is session state
   (`pointer_global`), recorded at the entry of every pointer event by
   whoever receives it; `cursor_style` derives from it via `pointer_in`.
   **Rule: pointer position is desktop-global truth shared by all
   windows.**
3. **The dropped press under stale pointer focus.** After the grab ends,
   niri keeps the pointer's surface focus on the press window until the
   next MOTION — a click resting on the new screen is delivered to the
   OLD window with OUT-OF-BOUNDS local coordinates, and element-level
   `on_mouse_down` hit-testing drops out-of-bounds positions silently.
   Fix: the left down is registered in the window-level
   `pointer_event_sink` beside move/up (element handlers that should own
   a press stop propagation during the bubble phase, which also skips the
   root listener). The session converts via the RECEIVING window's
   origin, so it doesn't matter which window delivered the event.

### niri's zwlr_virtual_pointer appears dead (2026-09-25)

motion_absolute/motion/button all vanish (WAYLAND_DEBUG confirmed
requests go on the wire, zero client events; with output/without,
absolute/relative — all the same). No GUI click automation on niri for
now (keyboard side untested). Worth reporting upstream. `tools/vptr`
exists as the injector for compositors where it works.

### wl_shm format is an ordinal (2026-09-25)

xrgb8888=1 — not a DRM fourcc; the format name describes the word's byte
order (MSB→LSB), little-endian memory is reversed.

### Transform semantics, measured (2026-09-26)

niri's "90° counter-clockwise" (Transform::_90) actually fills the panel
by rotating the buffer **clockwise** 90° — opposite of the protocol
wording. rotate_rgba was calibrated against grim; the Flipped family is
rare and unhandled.

**Wallpaper rotation destroys comparison testing** — a photo wallpaper
changes orientation; cross-time grim comparisons correlate as badly as
0.54. Verification posture: grim→screencap→grim within a one-second
window, three-way compare.

### Hyprland IPC: three live-measured protocol traps (2026-09-26)

The windowsnap Hyprland backend got its first live session and the
fixture-tested code hit three restructured-IPC traps in a row:

1. **Trailing newline = "unknown request"** for every exactly-matched
   command. The post-restructure dispatcher matches the raw string; only
   prefix-matched commands (`j/monitors`) happened to survive the '\n' —
   which made the bug look half-working. Requests now go out bare, with a
   400ms-silent fallback to newline for older line-based servers.
2. **Half-closing the write side drops the request.** The new event loop
   treats the EOF as a disconnect and never processes the buffered
   command (python probes without shutdown worked; Rust with
   `shutdown(Write)` silently lost every request). Connection stays
   open; the reply's EOF terminates the read.
3. **One request per connection.** The socket closes after each reply —
   pipelining `j/monitors` behind `j/clients` on the same stream fails.
   Each request opens its own connection.

With those fixed the backend lights up fully: `at`/`size` confirmed to
be global logical coordinates (cross-checked against the monitor layout)
— the same space the session state machine speaks, tiled and floating
alike.

### Hyprland layer sizing: the compositor that listens (2026-09-26)

On niri the overlay windows were always compositor-sized and fine; on
Hyprland the layers came up wrong (HDMI 1536×1080 instead of 1920×1080,
DP-2 960×540 landscape instead of 720×1280 portrait). Root cause chain,
all measured live:

1. gpui's wayland backend sends an explicit `set_size(w, h)` for layer
   surfaces, with the window's initial bounds as the value.
2. The layer-shell spec says fully-anchored surfaces are
   compositor-sized — niri ignores the request (why this never showed
   there). **Hyprland honors it**, exposing whatever bounds gpui
   computed: derived from the INTEGER wl_output scale and WITHOUT the
   transform, hence the garbage on fractional/rotated outputs.
3. `set_size(0, 0)` (the protocol's "compositor, you decide") was tried
   and is a dead end: Hyprland never sends a configure for the
   zero-sized surface, gpui never commits a first buffer, the surface
   never maps.

Fix: the capture connection binds `zxdg_output_v1` (one extra
roundtrip) and records each output's TRUE logical size — fractional
scale and transform included. `Overlay::window_options` forwards it as
the window bounds (→ `set_size`), with a `width÷scale` fallback for
compositors without xdg-output. The session's initial screen size uses
the same helper, so the pre-configure frame is no longer
integer-scale-wrong either.

## Pitfalls — multi-monitor geometry

### Display coordinates = logical position ÷ integer scale (2026-09-26)

gpui display bounds coordinates = output logical position ÷ wl_output
integer scale (the backend does the division; measured by comparison:
eDP 1920,0→960,0; DP-2 -720,-100→-360,-50). Display matching must use the
same algorithm.

**wl_output.scale is an integer**: a 1.5x screen reports 2 (ceil); the
true value needs the fractional protocol (unavailable per-output). Size
matching is therefore infeasible — **match on position** (layout origins
are unique).

### scale_factor() misreports across monitors (2026-09-25)

A window pinned to HDMI (rendering at 1.0) while `window.scale_factor()`
reports 1.5 (DP-2's). Cropping instead computes "captured physical width
÷ window logical width", naturally consistent with rendering. The same
self-computed scale naturally handles per-screen scales (eDP 2.0 / DP-2
1.5 / HDMI 1.0).

### The white-line bug: round every edge independently (2026-09-26)

Symptom: an occasional 1px full-width pure white line just under the
selection's bottom edge, only at certain positions. Forensics: remote
mice produce fractional selection coordinates → dim_strips (4 dim bands)
and selection_chrome (border) each round independently inside gpui → at
certain fractional phases the two roundings diverge → a 1px row covered
by neither → raw content bleeds through (pure white on light
backgrounds, invisible on dark). Fix: `round_px()` — all four edges
rounded once each (round(l)+round(w) ≠ round(r); edges must be rounded
independently), dim bands / border / toolbar share the same integer
bounds. Verified with a 10-phase fractional sweep — zero leak rows.

## Pitfalls — gpui & Taffy internals

### Scroll-wheel deltas are amplified ×3 by the wayland backend (2026-09-29)

A discrete notch on niri reaches the app as `Pixels(120)`, not 40:
gpui's wayland backend hard-codes `modifier = 3.0` on every `axis`
value (client.rs, a Zed-inherited speed hack), and the continuous
pixels path outranks `axis_discrete`, so `ScrollDelta::Lines` never
arrives for a mouse wheel. Dividing pixels by a per-notch constant is
therefore compositor-dependent; the robust posture for notch-stepping
is clamping each EVENT to ±1 line (one notch = one event = one step)
and letting sub-line events (touchpads, smaller per-notch values)
accumulate. Symptom before the fix: one notch stepped the value +3
(concealed until number editing made it visible; it silently affected
size stepping too).

### The RenderImage contract (2026-09-25)

BGRA bytes (Vulkan backend); feeding memory directly requires swap(0,2);
the PNG path is RGBA. `ui/image_util.rs` owns this contract.

### dispatch_action does not bubble past the focus path (2026-09-26)

A dispatch_action inside a gpui window **stops at the end of the focus
path and does not bubble to App::on_action**. Shotori's exit logic once
rode on an app-level backstop and had been silently dead since the
toolbar was born. Fix: handle the action where it is dispatched (the
overlay handler).

### Window-handle traps inside action handlers (2026-09-26)

- **`handle.update()` on the window currently running an action handler
  is a no-op** (gpui takes the window out of the map during its update,
  so the nested update finds nothing). The current window must act
  through its own `window` reference; `close_overlays` therefore takes
  both.
- **`window_handle.update`'s closure receives an `AnyView`** — it can't
  touch the concrete view's fields. Touching view state from async
  requires the `Entity` handle's `entity.update`.
- gpui-kit's `Entity::update` return = the closure's return passed
  through (not zed's Result wrapping); returning `()` trips clippy's
  `let_unit_value`.

### #[cfg] cannot hang in the middle of a method chain (2026-09-26)

Three separate offenses before it burned in. An attribute on a
`.child()` link isn't legal Rust. Absorb it with `.children(Option<E>)`
(children takes an IntoIterator; Option is one) or precompute an
`Option<AnyElement>` and attach unconditionally. gpui-kit doesn't
implement `IntoElement` for `Option<impl IntoElement>` (upstream gpui
does), nor for Infallible — a non-feature stub returning
`Option<&'static str>` is the cheapest way out.

### Never panic through the background executor (2026-09-26)

`get_or_init` + `expect` on failure paths (first OCR run offline,
unwritable dir) panics through gpui's background executor and behaves
unpredictably. Fix pattern: init returns `Result`, failures are not
cached (OnceLock stays unset) → the overlay prints the error and stays
usable; the next attempt retries. Related: **a corrupt model file must
not brick the feature permanently** — on init failure, wipe the model
cache dir so the next attempt re-downloads (an interrupted download that
leaves a truncated file otherwise fails every later hash check).

### reqwest::blocking cannot run in an async context (2026-09-26)

gpui's background executor is one — the OCR model download gets a
dedicated thread. (First-download progress polling is driven by an 80 ms
loop over the Entity handle + notify.)

### Taffy clamps absolute children to the parent's content box (2026-09-26)

A "W × H" size label inside the selection border box wrapped one
character per line on narrow selections: Taffy clamps the fit-content of
absolute children to the parent's content box, so a 22px-wide selection
left 22px of usable width. Fix: emit the label window-anchored and
absolutely positioned against the overlay root, content-sized,
independent of the selection's width.

### Dropping a RenderImage does not evict its atlas entry (2026-09-27)

Each overlay tracks its current preview images and calls
`Window::drop_image` for retired images during prepaint. Eviction is per
window so another output can finish displaying the shared image.

### Minor notes

- `with_animation` respects reduce-motion automatically; `max_fps 15`
  caps redraws (the OCR busy spinner).
- `let _ = engine()` trips the `let_underscore_lock` lint (even for
  deliberately dropping a lock) — explicit `drop(engine())` states the
  intent.
- `--print-theme` writes via `writeln!` and ignores stdout errors —
  piping into `head` used to panic on the broken pipe.
- JSON has no comments: exactly one `"//"` key is accepted (serde
  rename); a second one is a duplicate-field error.

## Design decisions

### The eraser deletes objects, and touches ink only (2026-10-01)

Issue #14: the pixel-restore eraser (rasterize coverage, blend back to
the capture) is GONE, replaced by an object eraser. The whole
subsystem it needed — `annotation/eraser.rs`, the `original` buffer
threaded through `rasterize`/`rasterize_shapes`/`StrokePreview`, the
eraser arms of `uses_raster_preview` — died with it. What replaced it:

- **Touch criterion is the visible INK, not the selectable region.**
  `shape_erased` (annotation/select.rs) dilates each kind's stroke
  geometry by the brush radius — deliberately narrower than
  `shape_hit`, because a closed polygon's interior is CLICKABLE
  (issue #16) but not erasable: a brush sweeping inside an enclosing
  ring must leave the ring alone, or nothing inside it could ever be
  erased individually. Hollow rect/ellipse outlines likewise erase
  only from their band.
- **One `RemoveMany` history entry per gesture, recorded LIVE.**
  Removals hit `shapes` as the brush touches them (the canvas updates
  mid-sweep) and extend a single trailing entry — the `apply_size`
  merge pattern — so an undo arriving mid-gesture cleanly reverts the
  sweep so far instead of corrupting history. Undo re-inserts in
  reverse recording order, redo removes forward; both reproduce the
  exact states the indices were recorded against (descending indices
  within one sample batch stay valid as the list shrinks).
- **Escape stops a sweep but does not revert it.** Unlike move/handle
  drags (whose snapshots are uncommitted preview state), eraser
  removals are committed as they happen — Escape just ends the
  gesture; Ctrl+Z is the revert.
- **The area eraser is bounds-intersection, deliberately coarse.**
  Brush = ink-touching precision; rect = "clear this area" (any shape
  whose bounds intersect the dragged rect, deleted on release when
  the rect is final). Two tools, two semantics, no ambiguity.
- **`parks_click_select` now excludes the erasers.** A press ON a
  shape must erase it, not park a click-select — the same exception
  polyline already had; `shape_hover` returns None for non-parking
  tools so the cursor never promises a selection the press won't
  deliver.
- **Fast flicks interpolate.** `drag_to` samples the segment between
  consecutive pointer positions every `radius` px — endpoint-only
  sampling lets a quick flick jump clean over thin ink.

### Loupe magnifies by composition, not by buffer (2026-09-29)

Issue #19: pixel-precise corner placement gets a floating 3× inset of
the frozen capture, crosshair on the focus pixel.

- **The zoom is pure element composition** — the SAME per-output
  `RenderImage`, sized `window × 3` and offset inside an
  `overflow_hidden` frame so the focus pixel lands at the frame's
  center. No cropped buffer, no `rgba_to_render_image` round-trip, no
  atlas churn per drag frame — and the math is all logical px, which
  is scale-factor-proof by construction because the img already fills
  the window 1:1. The alternative (re-buffered crops, like the pin
  path) would have to redo the physical/logical division per output
  (the crop-scale trap) for zero visual gain.
- **The loupe routes to the output that owns the focus point**, not
  the window receiving the drag events. Under implicit grab a
  cross-screen release drags events into the press window while the
  handle itself lives on the other output; each overlay magnifies
  only the pixels it froze, so `local_loupe` returns None elsewhere
  (pinned by the cross-output shape-handle test).
- **Corners and shape handles only.** An edge drag aims a line, not a
  pixel; a move drags a body. The loupe targets gestures whose whole
  meaning is "place THIS point" — selection corner resizes and every
  shape handle drag (endpoints, corners, vertices).
- **Content centers on the point; the inset floats away from it.**
  Two passes, two needs: fine-tuning wants the magnified view centered
  on the point (crosshair IS the pixel being placed); the coarse pass
  (dragging without fine-tuning) wants the point unobstructed. So the
  img inside the frame always maps the focus to the frame's center,
  while the frame itself floats `16px` out along the handle's outward
  diagonal (away from the resized body). **Near a screen edge the
  failing axis flips per-axis** — clamping back would re-cover the
  point (caught live: a corner dragged to the top-right had the
  clamped loupe squatting on it), while the flipped side only covers
  the dimmed resized body. A window too small for either side still
  clamps (accepted fallback). Fade-in is the spinner's
  `with_animation` posture (~100ms); disappearing with the gesture is
  instant by design — no chrome lingers over a finished edit.
- **Absolute-in-absolute is frame-local.** The zoomed img and the
  crosshair position against the loupe div's own origin, not the
  window: computing their offsets in window coordinates
  double-counts `frame.origin`, and the magnified content slides off
  the crosshair by exactly wherever the inset floats. Same class of
  bug as the hit-test-rect trap (two geometry sources for one
  concept): pick ONE coordinate space per element nesting level.

### Closed polygons select by interior; closure is structural (2026-09-29)

Issue #16: a placed polygon (polyline) was selectable only along its
stroke band — the interior click silently missed. The fix rides the
module's "what you see is what you can click" contract, with two
findings that shaped it:

- **The shape model has no closed flag, so closure must be inferred.**
  Sealing a polygon in this tool means the final click lands back on
  the first vertex; `ring_is_closed` accepts that within pointing
  slop (`CLOSURE_SLOP` 7 px — the handle-grab scale — plus the
  stroke's own footprint). Without the slop an exact `first == last`
  test would reject every human-sealed ring; with too much, zigzags
  whose endpoints merely sit nearby would gain a phantom interior.
- **Ray casting over the vertex list implicitly closes the ring**
  (last→first edge), which is exactly the sealed-ring interior — and
  stays exact for concave outlines (an L's notch) where a bounding-box
  test false-positives. Pencil/Highlighter loops remain band-only:
  their visual is a stroke, an interior is not meaningful, and their
  hit region shares nothing with the polyline arm anymore.

The hover probe (now `shape_hover`, see the #17 entry below) rides
the same `shape_hit`, so the hover cursor now covers polygon interiors
too — the affordance other selectable bodies already had. While the
polyline tool is active nothing changes: `parks_click_select` keeps
its clicks placing vertices, selection happens from any other tool.

### Annotation hover cursor: pointing hand picks, hands only hold (2026-09-29)

Issue #17. Hovering a shape used to show the open hand — but a hand
advertises "I'm holding something", and at hover time nothing is
grabbed yet; the affordance being promised is "click to pick". Now:

- **Unselected shape → pointing hand; selected shape → open hand;
  actively moving → closed hand** (toolbar grips and resize handles
  unchanged). The select-then-move sequence reads
  pick → press → hold → release.
- **The split lives in `Annotations::shape_hover`
  (`annotation/select.rs`), a pure `Pick`/`Move` function of the
  topmost hit, not a boolean.** The answer must match what a press
  would do, and `pointer_down` parks its click on the TOPMOST hit —
  so where a newer shape overlaps the selected one, the overlap
  probes as Pick (the press would pick the top shape). The old
  boolean probe (`pointer_on_annotation`/`hits_shape`) could not
  express that and was removed.
- The gpui pointing-hand variant is **`CursorStyle::PointingHand`**
  (CSS `pointer`), not `Pointer`.

### Clear-all is one whole-list history entry, not N removals (2026-09-29)

Issue #15's one-click wipe of every placed annotation:

- **`HistoryEntry::RemoveAll { shapes }` stores the entire committed
  sequence and restores it wholesale on undo**, instead of replaying N
  per-shape `Remove` entries. The saved sequence is self-describing:
  undo puts the exact list back no matter what interleaves after the
  clear (new strokes, further undos, redo), so the "indices in older
  entries stay valid" invariant needs no index arithmetic over a list
  that empties and refills. One entry is also the issue's contract —
  a single Ctrl+Z restores everything. An empty canvas records NO
  entry (pressing clear twice must not clobber the redo stack),
  mirroring `delete_selected`'s no-selection no-op.
- **The raster-preview path needed no new invalidation plumbing.**
  `filtered_preview` already diffs `cached.committed` against the live
  list, so a mass removal rebuilds from the immutable capture; once no
  filter kinds remain, `uses_raster_preview()` flipping false drops
  the cache outright — the session test pins both.
- **Placement on the toolbar: end of the tool cluster.** Undo/redo
  are deliberately keyboard-only (`geometry_toolbar_keyboard_and_export`
  asserts `tb-undo`/`tb-redo` never exist), so there is no undo/redo
  row to sit "near" — the trash button closes the tool cluster
  instead. `TB_W_ROW1` re-measured 562 → 595 (one probe-measured
  32px button pitch; the copy-clips assert would have caught a miss).
  This Lucide bundle has no trash-2, hence the plain `Trash` glyph.

### Number badge editing: wheel tunes the value, double-click opens free entry (2026-09-29)

Issue #2's second ask (post-placement value editing), riding the issue
#5 selection layer:

- **The wheel over a selected badge redirects to its VALUE** (±1,
  floored at 1 — 0 is not a badge). The diameter keeps the size-slider
  path, so the two edits never fight over one gesture.
- **Double-click opens the editor; it reuses the text-editing
  machinery wholesale.** `text_editing` still carries the editor, so
  blocked-canvas, Esc-cancel, Enter-commit and click-away-commit all
  apply unchanged; `number_edit: Option<(ix, before-snapshot)>` is the
  only new state. Live previews write `shape.number` with NO history
  entry (a half-typed buffer must stay outside undo); commit is exactly
  `commit_move`'s before→current contract, cancel re-previews the
  original. The NumberCache keys on `shape.number`, so previewing needs
  no cache plumbing.
- **Parse-or-hold previews.** A non-numeric or empty buffer previews
  nothing (the badge keeps its last value); commit only accepts a full
  `u32` — anything else restores.

### Same-number badges: repeat max(existing), not "last placed" (2026-09-29)

Alt+placement repeats the largest number on canvas (issue #2's first
ask). Two decisions worth keeping:

- **max(existing) over a remembered "last placed" value.** A
  last-placed field desyncs after undo or deletion; deriving from the
  shapes themselves keeps the semantics correct through
  undo/redo/delete for free. An empty canvas starts at 1 either way.
- **The press-through path samples Alt at PRESS time.** A press on an
  existing shape parks as a pending click; the stroke only begins when
  the drag crosses the click slop (a MOVE event, possibly seconds
  later). The modifier rides along in the `pending_click` tuple —
  reading modifiers at slop-crossing time would honor an Alt the user
  already released.

### Size controls: continuous slider over base primitives (2026-09-28)

The S/M/L preset buttons became a continuous slider with min/max per
tool family plus the three legacy rungs as clickable detents (issue #3
phase 2). Three traps shaped the design:

- **Never the component-library slider.** `gpui-component`'s Slider
  drags in the Root/WindowState plugin — poison for layer-shell
  overlays (see "the Root/CSD poisoning case"). The unstyled behavior
  root in `gpui_base::slider` (Slider + SliderTrack/Thumb/Indicator)
  provides drag/click/a11y with application-supplied presentation;
  component is now dropped from the tree entirely — BOTH the main and
  dev `gpui-kit` declarations need `default-features = false`, or
  feature unification resurrects it from either side.
- **SliderState's min/max are baked at entity build time.** Switching
  tools changes the range, so the overlay owns
  `Option<(ShapeKind, Entity<SliderState>)>` and rebuilds on family
  change; the `SliderEvent::Change` subscription writes through to
  `set_tool_size`.
- **External value sync must be change-gated.** Pushing the wheel /
  detent value into the state via `set_value` re-notifies, and render
  runs every notify — an unconditional sync is a render loop. Skip
  when the value already matches.

One spec per tool family (`annotation::size_spec`) is the single
source of range + detents; the wheel (±1 clamped), the slider and the
detent buttons all read it.

**Update (2026-09-28, post-detent-drop): the settings-row width trap.**
The filter/eraser settings rows carried fixed `.w()` values tuned for
their OLD content — two mode buttons plus the three S/M/L preset
buttons (~178px). When the slider bundle (track 108 + gap 8 + readout
26 = 142px, wider than the three buttons) replaced the presets, nobody
re-derived the constants: the row overflowed its own painted border by
~50px, readout hanging outside the panel. It survived testing because
the spill is near-black-on-near-black in the dark theme — a HiDPI
screenshot read by a vision model made it obvious. Fix: settings rows
hug their content like every other row (no fixed widths), with
`toolbar_hugs_its_content` asserting the readout sits inside the row's
border via `debug_selector` probes. General rule: a row whose children
can change must not carry a hand-tuned width.

### Module layout & dependency direction (2026-09-26)

`ui → model`, `model → platform`, never back up; `actions.rs` is the
shared vocabulary referenced by everyone, referencing no one (it broke
the codebase's only import cycle, toolbar ↔ overlay). Documented in the
`src/lib.rs` header. `core` was rejected as a directory name (bare
`core::` path collisions).

### Clipboard: the resident-offer twin (2026-09-25)

Copy = re-exec ourselves as a `--clipboard-daemon` twin, PNG bytes via
stdin; the twin serves pastes as a `zwlr_data_control` source and exits
on `cancelled` when replaced (same model as wl-copy — a plain client's
offers die with the process). Generalized to `--clipboard-daemon <MIME>`
so image and text offers share the framework; text offers
`text/plain;charset=utf-8` with UTF8_STRING/STRING fallbacks for old
xwayland apps, the Send handler writing on any offered-MIME hit.

### Application icon identity and delivery (2026-09-29)

`assets/app/shotori.svg` is the vector master; PNGs are rendered independently
at each size rather than enlarged from a small bitmap. `shotori.desktop`, the
hicolor icon name, GPUI window app IDs and the notification desktop-entry hint
must all agree on `shotori`. Layer-shell namespaces remain separate: they are
compositor rules, not desktop icon lookup keys.

Tray pixels come from an embedded PNG. Notifications atomically cache a separate
embedded app icon so cargo-installed binaries need no adjacent asset directory;
never use the app logo as `image-path`, which is reserved for the screenshot
preview. Desktop launchers still need installed hicolor/desktop files; the
release archive includes the same installer and assets as the source tree.
The AUR release hook preserves its existing package function and additionally
stages these assets through the installer (DESTDIR prevents host cache updates).

### Notifications: the detached child (2026-09-25 → 2026-09-27)

`shotori --notify <summary> <body> [image]`: the parent spawns it and
exits immediately; **the detached child outlives the parent** — a plain
background thread would be killed by the process::exit after cx.quit(),
cutting the notification mid-send. The child fails quietly (one stderr
line); a missing daemon never affects screenshots.

Body markup is escaped so recognized text and filenames remain literal.
Titles are result-oriented; empty OCR results are reported separately
from recognition failures. Copy/save notifications expose an Open image
action for a uniquely named, full-resolution cached PNG (never the
thumbnail), handled by the detached child after the screenshot process
exits; cache failures do not fail the copy. Cached images and
thumbnails are lazily removed after 24 hours. Image previews ride the
image-path hint + file:// URL (probe the spec support with a raw busctl
call before writing code for a daemon).

### OCR: engine choice, default feature, prewarm (2026-09-26)

- **Why rapidocr-core**: rusto-rs's mnn-sys build chain is
  three-strategy (vendor/prebuilt/source) + bindgen/cmake — fragile;
  paddle-ocr-rs lost on the same axis. rapidocr-core's `run_image
  (&RgbImage)` takes in-memory pixels directly; mature model-cache
  machinery (ModelCache + SHA256 verification); ort auto-downloads a
  prebuilt libonnxruntime, statically linked. PP-OCRv6 small is solid on
  mixed Chinese/English; text under ~16px on a 1080p screen struggles
  (HiDPI screens do better — more physical pixels).
- **OCR is a default feature** (`default = ["ocr"]`): product identity =
  screenshots + OCR; next to the gpui dep tree, ort+reqwest are a
  rounding error. Slim-build exit: `--no-default-features`.
- **Prewarm** (2026-09-26): one-shot process × in-process engine cache =
  full cold start on every Ctrl+O. The overlay warmups on open (own
  thread, only when models are already cached — a first-ever run must
  not surprise-download during a plain screenshot); the init hides
  inside the user's 2–5s of drawing a selection. When all model files
  exist, skip the ensure_* re-hash (it re-hashes all 31MB per call);
  corruption detection is covered by "engine init fails → clean cache".
  Effect: Ctrl+O after a real draw leaves only inference, ~300–500ms.
  Bonus: a corrupt model file is silently digested by the warmup thread
  (init fails → cache cleaned → next real OCR re-downloads; the user
  never sees it).

### OCR first use: dialog, downloader, self-healing (2026-09-26)

Ctrl+O with no models: centered confirm card (~31MB, ModelScope source,
storage path) → download with byte-accurate progress + cancel; failure
card [Retry]/[Close]; on success the selection snapshot frozen at Ctrl+O
time is fed to OCR automatically. The dialog is modal (Enter/Ctrl+S/
copy/new selections blocked), Esc = cancel. The downloader is ours (the
library's download_asset writes straight to the target — no temp+rename
on our side means an interrupted download leaves a truncated file);
writes go via **temp + atomic rename** + post-download sha256. Model
location: ~/.local/share/shotori/ocr-models/ (XDG_DATA_HOME respected);
reset for testing: `rm -rf ~/.local/share/shotori/ocr-models`.

### Save: the system file picker (2026-09-26)

`Ctrl+S` opens the desktop's native "save as" dialog
(xdg-desktop-portal FileChooser via rfd 0.17, no GTK link time; zenity
fallback if the portal is dead). Suggested name pre-filled, extension
re-appended if dropped while renaming. Flow: the overlay action crops,
stashes RGBA pixels in a static slot and tears the overlays down (portal
can't coexist — see pitfalls); after the run loop returns, the main
thread runs the dialog (blocking), writes the PNG and fires the
thumbnail notification. Headless e2e keeps working via
`SHOTORI_DEBUG_SAVE_PATH=<file>`.

### Selection chrome: two disjoint zones (2026-09-26)

An iterative lesson: a fallback chain (label above→below→inside,
toolbar below→above→inside) kept colliding as user reports rolled in;
computing both Y anchors in one six-state matrix was correct but the
label↔toolbar coupling was a bug factory. Final scheme (user-designed,
and better): **label ABOVE the box (or inside its TOP-LEFT corner if the
box hugs the screen top); toolbar BELOW the box (or inside its
BOTTOM-LEFT corner if the box reaches the screen bottom)**. No overlap
is possible by construction, the label is toolbar-independent
(drag-stable), off-screen is impossible. Inside corners carry a 12px
horizontal / 8px vertical inset; the below-fit decision keeps 12px of
breathing room at the screen edge (zero-margin looks glued on —
measured). Anchors are pure functions in `model/placement.rs` with unit
tests including a grid sweep asserting the disjoint-and-on-screen
invariant over 35 selection geometries (the clamp also carries a max()
guard against a clamp(min, max) panic on very narrow windows).

### The draggable toolbar and one geometry source (2026-09-27)

The toolbar drags via matte grip strips (a bare dot matrix, deliberately
NOT a button). Design points worth keeping:

- One geometry source: `session.toolbar_bounds()` — render, cursor
  hit-test AND drag clamp all read it, so they cannot drift.
- Reset semantics: a NEW selection or a host-window change re-anchors;
  moving/resizing the CURRENT selection keeps the user's placement; Esc
  mid-drag reverts to the pre-drag position (one Esc, one thing).
- **The grip/cursor alignment trap**: the first cut computed the cursor
  strip from layout side effects while the grip ELEMENT sat after
  padding — the rects overlapped but were not equal, so parts dragged
  with no hand cursor. Final shape: grips stay flex children of row one
  and `toolbar_grips` returns the element's literal rect from the same
  placement constants both sides share — pixel-identical by
  construction. **Rule: never compute a hit-test rect from layout side
  effects; derive both the element and the hit-test from the same
  constants.** (Same-day footnote: a rewrite dropped the outer flex gap
  and the airy matrix collapsed into tight lines — caught by the user,
  restored, pixel-verified with a thresholded crop comparison. Vision
  models misjudge textures at this scale; pixels don't lie.)
- The cursor computation hit-tests the toolbar rect FIRST and yields
  Arrow over it (matters when the toolbar parks inside the selection);
  the rect is recomputed from the same pure geometry the render side
  uses.

### Icons: self-maintained SVGs composed over Lucide (2026-09-27)

The Lucide catalog has no true mosaic/pixelate glyph (grid-2x2 etc. read
as "table"). Shotori maintains its own icons (24×24 Lucide-convention
canvas, `fill="currentColor"` so they follow the toolbar text color),
embedded via rust-embed as `OwnIcons`; a `ToolbarSource` implements
`AssetSource`: own icons first, then the `icon_assets!`-selected Lucide
set — registered app-wide via `with_assets`. When replacing theme keys,
serde's ignore-unknown-fields keeps existing user theme files loading.

### Selection editing in place (2026-09-27)

Classic screenshot-tool semantics: press an edge/corner band → resize
(pinned opposite edges, no flipping, clamped to the desktop union);
press the interior → move (`grab = press − origin`, no jumping); press
outside → fresh drag. Annotations still wipe on a NEW selection; an
edit must NOT wipe them. An in-place click inside keeps the selection
(editing semantics beat re-snapping). Esc during an edit reverts.

**The handle hit-test geometry lesson**: hit-testing each axis
independently makes the edges' EXTENSIONS into invisible grab zones (a
press 50px past the right end of the top edge grabbed "top edge").
Correct geometry: corners are square zones that may stick out past the
box, but an EDGE handle only counts along its edge's own span. Tiny
boxes (< 2× hit band per axis) resolve to the nearer edge per axis.

Cursor affordance runs through a shared `Rc<Cell<CursorStyle>>`
refreshed on every pointer move and on session changes, pushed
window-level via `set_window_cursor_style` during paint (crosshair for
tools, open/closed hand, resize arrows). Window-level is safe because
gpui resolves None-hitbox (window) requests with immediate precedence
and nothing in this UI sets an element cursor. The text editor and the
OCR setup dialog opt out (their inputs own IBeam/default via hitboxes).

**Update (2026-09-28): freehand chrome is the centerline, not capsule
rims.** The annotation selection chrome (`Shape::hilite_paths`) traced
`line::geometry`'s polygons — correct for a Line's single capsule, but a
freehand stroke is one capsule PER SEGMENT, and rim-tracing every one
rendered the selection as a chain of overlapping rings (with a dense
pile where pointer events cluster at the release point). It hid in
testing because only vector tools were eyeballed; select-on-place
(6251f71) made it fire on EVERY fresh pencil/highlighter stroke — the
"user-reported ring chain". Fix: Pencil/Highlighter/Polyline chrome
traces the recorded centerline (one open path); a single-point tap
keeps its circle rim; Line/Arrow keep the capsule rim that doubles as
the width cue. Rule: a chrome path must be O(1) per selected shape,
not O(segments).

**Update (2026-09-29): handle chips became accent dots (issue #18).**
The white-square-plus-accent-outline handles read as clutter against
the dim bands and the orange border. Restyle: one
`ui::hud::paint_handle_dot` helper (solid accent circle —
`fill(..).corner_radii(half)`; gpui quads round, no path needed, and
one quad per handle instead of two during resize drags) owns the look for BOTH
the selection chrome and the annotation shape chrome, so the future
corner loupe has exactly one place to grow on. Contract now enforced
at compile time (`const _: () = assert!(HANDLE_VIS < HANDLE_HIT)` in
model/selection.rs): the painted diameter is deliberately smaller than
the grab band — what you SEE and what you can GRAB are separate
budgets; syncing the two "for consistency" couples looks to comfort.
Test note: quad origins are floored to device pixels, so assertions on
painted geometry must carry ~1 px tolerance (same lesson as the
probe-e2e rule against hardcoded pixel coordinates).

### Window snapping: what the compositor will and won't tell you (2026-09-26)

Clients are isolated; no standard protocol exposes other clients'
geometry (ext-foreign-toplevel-list is deliberately minimal — title and
app-id only). Screen geometry is public (xdg-output), window geometry is
private. Per-compositor IPC is the only door.

- **niri (source-verified on 26.04)**: the IPC's `tile_pos_in_
  workspace_view` is populated for floating windows only; tiled windows
  also depend on the unexposed workspace scroll offset. Upstream knows:
  issue #2381, PR #4147. **Floating windows snap exactly; tiled windows
  cannot snap at all** until upstream moves. The backend interface
  already carries rects — tiled support lights up with a field-fill the
  day it merges.
- **Pixel detection was prototyped and rejected.** Frozen-frame template
  matching looked promising, but real captures showed content edges
  inside windows scoring as strongly as genuine boundaries (a ghostty
  pane border at x=232 scored 149 vs 150 for the true edge); every
  extra discriminator added new failure modes. A snap that occasionally
  grabs a wrong region is worse than no snap; silently degrading won.
- **sway / Hyprland backends**: written from their IPC docs; sway is
  fixture-tested only. Hyprland got its live session 2026-09-26 (three
  IPC traps above).

### Theme system (2026-09-26 → 2026-09-27)

- **No gpui-shell, no gpui-base Theme.** gpui-shell is a QuickJS plugin
  runtime (+13.5 MiB, not on crates.io); gpui-base's tokens serve a
  60-component design system. Shotori self-draws ~17 colors — a
  homegrown struct is the right size.
- **Install-once, read-everywhere.** `OnceLock<Theme>` set during
  startup, read via `theme::c()`. The overlay lives seconds; no hot-swap
  story to build.
- **Best-effort resolution.** Bad hex, out-of-range opacity or unknown
  fields are logged and the field falls back — a typo in a color file
  must never cost a screenshot. `--print-theme` is the exception:
  errors exit non-zero so scripts can catch them.
- Palette names stay in code (`PALETTE_NAMES`); themes carry colors
  only.
- **theme.toml replaces theme.json** (2026-09-27): configuration exposes
  base, accent, dim_opacity and the full annotation palette; toolbar and
  chip surfaces stay in coherent built-in palettes; selected tint is
  derived from the accent with contrast-aware foregrounds. Default
  `auto` follows GPUI system appearance updates; legacy JSON is not
  auto-loaded.

### CLI surface (2026-09-26)

A hand-rolled four-flag parser served exactly one session before the
real requirement showed up: screenshot-tool conventions (a `full`
subcommand with `-c/-p/-d`), where clap's derive is
cheaper than maintaining a parser. Two invariants that survive any
parser:

- The internal child-process entry points (`--notify`,
  `--clipboard-daemon`) are matched on `argv[1]` in main BEFORE flag
  parsing — they carry free-form trailing arguments and would be
  rejected as unknown flags otherwise.
- `shotori full` reuses the session machinery as-is (`select_all` builds
  the union selection, `crop_original` walks the normal cross-screen
  export), so density/gap semantics cannot drift between interactive
  and headless modes. Clipboard is the default with no `--path`; a
  directory `--path` gets the dialog-style timestamped name.

### The Windows port (2026-09-26)

The platform layer is split per-OS behind platform-neutral signatures;
the UI runs unmodified on both.

- **Capture**: GDI `BitBlt` per monitor (`CAPTUREBLT`, no cursor —
  screencopy parity). DPI awareness is set programmatically
  (per-monitor-v2) in main before anything else: an unaware process
  sees virtualized coordinates and wrong-resolution captures.
- **The coordinate-space decision** (the load-bearing one): the
  session's global "logical" space uses the monitor's **physical origin**
  with a **logical extent** (physical ÷ effective scale). Dividing every
  origin by its own scale instead would make mixed-DPI monitors
  *overlap* in logical coordinates (100% 1920px monitor then 150%
  2560px monitor: 1920 vs 2560/1.5=1707 — overlap at 1707 < 1920),
  breaking union/crop math. The hybrid space tiles exactly, and local =
  (physical - origin) ÷ scale keeps the Wayland identity the session
  already assumes. Display matching divides the physical origin by the
  scale — the same formula the Wayland backend needs for its own
  reasons, so display.rs stays shared.
- **Overlay window**: `WindowKind::PopUp` maps to
  `WS_EX_TOOLWINDOW | WS_EX_TOPMOST` + borderless — the Win32 stand-in
  for layer-shell. Window bounds must be passed as absolute
  gpui-logical coordinates (origin = physical ÷ scale) or the window
  falls back to default bounds on secondary monitors.
- **Clipboard**: Win32 owns the data after SetClipboardData — the entire
  resident-daemon machinery is Linux-only. CF_DIB + registered "PNG"
  format are offered side by side (decode round trip: callers keep one
  PNG-encoding path).
- **Notifications**: WinRT toast from the same detached child process;
  POWERSHELL_APP_ID avoids registering an AppUserModelID (the toast
  reports PowerShell as its source — cosmetic). Toast actions remain
  unsupported.
- **Window snap**: EnumWindows + DWMWA_CLOAKED filtering; every visible
  toplevel is enumerable (no tiled-window blind spot), rect converted
  physical → hybrid by the containing monitor's scale.

### Existing text must share the annotation raster path (2026-09-29)

The input widget draws caret, selection and IME marks; annotation rasterization
owns glyphs. Hiding the original only from `visible()` and removing the draft
left committed raster caches holding stale text. Re-edit now replaces the shape
at its original layer index during a model-owned transaction. Cancel restores
the snapshot; commit records one Edit (or Remove for empty text), including style
changes. This also preserves ordering relative to later erasers and filters.

Editing style comes from the active text, not toolbar presets. The blocked
session allows text settings but still rejects document gestures. Occupied text
bounds remain the hit-test region; reopening uses the available selection area
so short text can grow and wrap. Keep the editor outline inside that area:
adding an outset or forcing a minimum width after clipping leaks beyond the
selection, especially at fractional scales. Tests exercise the real editor as
well as the standalone border painter and compare live/committed crop pixels.

### Text limits must constrain edits, not just painting (2026-09-29)

Clamping `TextInput::height()` only clipped the widget while Cosmic kept accepting
rows below the selection. Validate the full shaped height before accepting text,
newlines, paste, IME preedit/commit, history restoration or a font-size change.
An overflowing operation restores the previous editor without consuming history;
a rejected IME commit restores the pre-composition text, not raw phonetic input.
Reject a whole paste rather than silently truncating it. The editor and model
must both retain the accepted font size when a slider request cannot fit.

Placement, re-edit and body movement share the same 2px inset selection area.
An empty editor needs room for a whole line at the current font size. A layout
clamp is still a defensive painting boundary, never proof that content fits.

The overlay workflow test also needs the bundled-font fixture (2026-09-29):
host fonts made a long paragraph fit at 40px locally but overflow on CI, where
the correct rollback to 32px failed the test's unconditional resize assertion.
Run the whole workflow under `with_test_font`, assert rejection for the long
paragraph, then use shorter text to assert a successful resize separately.
Do not weaken the bottom-boundary check to satisfy a font-dependent test.

### Annotation previews: caching, thresholds, background jobs (2026-09-27)

- **The composite cache**: filter/text/eraser previews retain the frozen
  crop and completed annotation layer. Pointer updates replay only the
  current draft; appends replay only new committed shapes; undo or
  replacement rebuilds; selection/display geometry changes invalidate.
  Erasers always restore the frozen capture. Keep the small-scene
  vector path for simple geometry, but switch to the shared composite at
  64 committed marks (rectangles, ellipses, lines, arrows, numbers);
  undo below the threshold returns to the vector path. This bounds
  historical rendering and retired image storage.
- **Incremental strokes**: pencil/highlighter/brush-eraser drafts retain
  union intervals at the same eight vertical samples as full export;
  new capsules update only affected pixel rows; blending always uses
  the pre-stroke layer. Blur retains a ring of horizontal sums plus one
  vertical accumulator row — at 3840×2160 with strength 16, sum storage
  drops from ~253 MiB to 2.1 MiB.
- **Background previews**: filters covering ≥262,144 physical pixels or
  strokes exceeding 1,024 points run on GPUI's background executor; one
  job per session; while it runs only the live model changes;
  completion requests the latest model snapshot rather than processing
  a backlog of pointer positions. A gesture generation prevents an old
  cancelled draft from appearing in a new gesture. The last compatible
  image remains visible while computing; copy/save independently
  rasterize current state, never exporting a stale preview.
- Regression tests compare incremental vs full pixels across fractional
  scales, crossings, retracing, undo/redo and cancellation; GPUI task
  tests cover coalescing, committing while busy, cancellation and
  geometry changes.

### Copy/save fast path (2026-09-27)

The copy path spent its whole budget on the main thread: a balanced-tier
PNG encode (the quality/size choice for files on disk) plus the
clipboard handoff, before the overlay could quit. Resolution: the disk
tier is kept for `--path` saves; the clipboard gets a fast tier, the
work moves off the UI thread, and the notification thumbnail renders in
the detached child instead of the parent.

- Two-tier PNG: `encode_png` (balanced, disk) and `encode_png_fast`
  (fdeflate, clipboard) share `encode_png_with`. Clipboard bytes go
  through the resident daemon's pipe, so speed matters more than size
  there.
- `copy_selection` crops on the main thread, then spawns encode +
  clipboard handoff on the background executor and quits once it lands.
  The daemon spawn (blocking I/O) rides the same task.
- `copy_image(w, h, rgba, png)` takes the pixels the caller already
  has: Windows builds CF_DIB directly (`dib_from_rgba`, preallocated +
  in-place swizzle) instead of decoding its own PNG.
- Capture-side u32 swizzles replace per-byte loops on the aligned fast
  path; 180°/flipped-180 rotation is a row reversal instead of a
  per-pixel copy.
- In-binary `--bench` harness (fixed-seed LCG inputs, release-only,
  `shotori --bench all`) so the numbers are reproducible.

Measured (release, 16-core; mean of ≥300 ms sampling):

| workload | before | after |
| --- | --- | --- |
| png1080-ui encode | 342.7 ms (balanced) | 11.1 ms (fast) — 31× |
| png4k-ui encode | 1383.9 ms (balanced) | 45.5 ms (fast) — 30× |
| png4k-noise encode | 1012.7 ms (balanced) | 48.5 ms (fast) — 21× |
| convert4k-xrgb | 6.3 ms | 3.2 ms — 2× |
| rotate1080p-180 | 3.1 ms | 1.2 ms — 2.6× |
| dib4k (Windows DIB build) | (PNG decode, main thread) | 6.0 ms (direct, off-thread) |

E2E wall clock (niri, eDP-1 2560×1600 @ 1.75; `SHOTORI_DEBUG_ACTION=copy`)
is dominated by the fixed 1.5 s debug-action delay (full screen 1.76 s →
1.65 s); the real change is that encode + clipboard no longer block the
overlay's main thread.

### Pin: one scene shared across processes (2026-09-27)

- Every output lays out the entire image at the same global size and
  offset, clipped by the fullscreen surface; borders follow the full
  image rectangle (no extra border or shrink at output seams).
- Placement uses the crop's actual global logical bounds, including
  native pixel rounding and mixed-DPI composition — never a division by
  the toolbar-host display's scale. Drag and zoom choose the nearest
  position with a grabbable area on a real output; desktop gaps are not
  visible screen area; release applies its final position before
  clearing the drag.
- **The Close command removes the active window directly, then closes
  its sibling surfaces** — it must not try to update the active window
  through its unavailable handle (see the window-handle pitfalls).
- Cross-process sharing: Wayland pins share one scene across processes
  and output surfaces; new/clicked pins move to the end of the paint
  order without recreating native windows; stable pin IDs keep drag,
  menu and close targets correct. A private Unix socket per Wayland
  display transfers bounded RGBA payloads; file locking elects one
  owner and allows stale socket recovery after a crash; sender overlays
  close only after scene acceptance; socket work runs outside the UI
  thread.

## Testing methodology

- **Headless e2e backdoors** (`ui/e2e.rs`):
  - `SHOTORI_DEBUG_SELECTION=x,y,w,h`: inject a ready-made selection
  - `SHOTORI_DEBUG_ACTION=copy|quit|save|ocr|ocrsetup`: fire the action
    ~1.5 s after startup, through the real dispatch_action pipeline —
    the only entry point for headless e2e (recipe:
    `SHOTORI_DEBUG_TARGET=HDMI-A-1 SHOTORI_DEBUG_SELECTION=...
    SHOTORI_DEBUG_ACTION=copy ./shotori & sleep 4; wl-paste --type
    image/png | size assertion`)
  - `SHOTORI_DEBUG_TARGET=<output name>`: restrict the backdoor to one
    overlay (multiple overlays all firing fight each other)
  - `SHOTORI_DEBUG_SAVE_PATH=<file>`: skip the save dialog
- **Don't hardcode pixel coordinates in probe assertions** — prove the
  overlay is up first, then probe relative geometry (a rounding change
  once turned every hardcoded check false-negative).
- **Capture animations with a burst of frames**; a single frame misses
  0.3 s-scale windows (grim bursts).
- **Comparison posture**: grim→screencap→grim within a one-second
  window, three-way compare (wallpaper rotation destroys cross-time
  comparisons; see the transform entry).
- **Failure paths (offline / corrupt files / retries) are mandatory
  testing** for lazy-loading designs; success-path e2e is not enough.
- **Suspicion order** when something breaks: suspect yourself first,
  then the compositor, and last remember the user is also using the
  computer (their copies replace test state).
- **The pkill in an e2e script must happen after the backdoor action
  fires** — otherwise you kill a process that hasn't done its work yet;
  wait for foreground exit before checking.
- Debug builds run the pure-Rust pixel code 10–100× slower — benchmark
  with the release install (`shotori --bench`).
- `SHOTORI_BOOT=1` prints a startup phase timing trace.

## Open & shelved

- **One-frame default-cursor flash after a cross-screen release**
  (2026-09-27, user-shelved): the compositor resets the cursor shape on
  every pointer focus switch and waits for the newly focused surface to
  re-assert; gpui can only assert during paint, so a switch landing at
  MOTION time costs at least one default frame. Two attempts were built
  and REVERTED (forced repaint on window entry; momentarily emptying the
  old window's input region at release). Next attempt must start by
  MEASURING where the gap comes from (compositor logs / cursor protocol
  tracing), not another assert-timing guess.
- **niri tiled windows cannot snap** until upstream exposes the view
  offset (issue #2381, PR #4147 unreviewed for months); the backend
  interface is ready for a field-fill.
- **sway windowsnap backend**: fixture-tested only.
- **Rotation+flip combos** (Flipped90 etc.) unimplemented.
- **Windows follow-ups**: toast actions; cross-process pin ownership;
  live display-layout changes for pins.
- **Annotation follow-ups** (condensed): endpoint/vertex editing,
  alternate arrow styles and leader arrows, brush-mode mosaic, region
  editing, smart erase (needs its own feasibility review), manual text-box resize, font selection, bold/italic,
  rotation, fill, dashed/dotted line styles, sectors/arcs,
  select/move/resize existing annotations.
- **Deferred at the user's request** (2026-09-27): spotlight, watermark,
  magnifier — not in the implementation queue.
