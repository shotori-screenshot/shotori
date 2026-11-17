# Annotation Engine & Canvas Interaction Spec

Read alongside [`docs/annotation-parity.md`](../annotation-parity.md)
before editing `src/annotation*`, `src/model/session/annotations.rs`,
`src/ui/toolbar/*`, or `src/ui/overlay/text_edit.rs`.

## 1. Module Map & Responsibilities

`src/annotation.rs` is a private crate module tree coordinated through
`ScreenshotSession` (`src/model/session/annotations.rs`) and rendered by
`Overlay` + `model/export.rs`:

| Module | Owns |
| --- | --- |
| `src/annotation.rs` | `ShapeKind`, `Shape`, `SizeSpec` / `size_spec`, `Annotations` container, active tool state |
| `annotation/parameters.rs` | `ShapeParameters`, `ArrowStyle`, `LineStyle`, `LineEnds`, `EndMarker`, `FilterArea`, `NumberFormat` |
| `annotation/geometry.rs` | Bounds math, rotation (`ROTATION_HANDLE`), corner/sector/arc handles, square/circle & angle constraints |
| `annotation/gesture.rs` | Pointer press/drag/release state machine, move/handle drags, uncommitted-gesture cancellation |
| `annotation/select.rs` | Hit-testing (`ShapeHover`), single & `Shift` multi-select, marquee sweep, group movement & crop clamping |
| `annotation/history.rs` | Undo/redo stacks, grouped deletions, optional sequence-badge renumbering (`renumber_on_delete`) |
| `annotation/chrome.rs` | Selection frames, handle outlines, ellipse contours for overlay HUD |
| `annotation/line.rs` | Straight/curved lines, arrows (7 silhouettes), polylines, smoothed pencil, `lyon_tessellation` stroke/draft paths & coverage |
| `annotation/highlighter.rs` | Brush & rectangle highlighter, `Multiply` vs `Translucent` blending, `HighlighterCache` |
| `annotation/filter.rs` | Mosaic & Gaussian/box blur (rectangle & brush coverage), reversible intensity/diameter |
| `annotation/number.rs` | Sequence badges (`Decimal`, `Alphabetic`, `Roman`), drag-out leader arrows, `ab_glyph` rasterization & `NumberCache` |
| `annotation/text.rs` | `cosmic-text` multilingual shaping, `TextStyle` (font, bold/italic, HSLA outline & background, padding, radius, `TextWrap`) |
| `annotation/spotlight.rs` | Rectangle/ellipse cutout regions, shared dimming mask/opacity, optional outline, wheel modifiers |
| `annotation/magnifier.rs` | Rectangular loupe source + destination lens, `1..=8` zoom, 4 `LensConnector` styles, shadow, antialiasing, annotation-sampling toggle |
| `annotation/eraser.rs` | Whole-object brush (`Eraser`) and rectangle (`EraserRect`) mark erasure bound to mark visibility/history |
| `annotation/raster.rs` | Authoritative CPU RGBA8 pixel compositor shared by canvas caches and exported images |
| `ui/toolbar/*_options.rs` | Secondary toolbar row controls (`annot_options`, `geometry_options`, `line_options`, `number_options`, `spotlight_options`, `text_options`) |

## 2. Core Data & Geometry Invariants

- **Desktop-global logical coordinates**: every `Shape.bounds`,
  `Shape.points`, and sequence `number_tip` is stored in desktop-global
  logical `Pixels`. `Shape::translate` must move **all** coordinate fields
  together so multi-output projection and group drags never desynchronize
  a badge from its arrow tip or a polyline from its bounding box.
- **Region vs point-cloud vs freehand**:
  - `ShapeKind::is_region()` (`Rectangle`, `Ellipse`, `Magnifier`,
    `Spotlight`) stores canonical geometry in `bounds` (edited via corner,
    rotation, or arc handles).
  - Point-based marks (`Line`, `Arrow`, `Polyline`, `Pencil`, brush
    `Highlighter`, brush `Mosaic`/`Blur`, brush `Eraser`) store vertices
    or stroke trails in `points`.
  - `Shape::is_freehand()` (`Pencil` and brush `Highlighter`) paints live
    vector paths (`draft_paths`) during pointer drag and materializes into
    the raster layer on release.
- **Unified `SizeSpec` (`size_spec`)**: slider bounds and mouse-wheel
  stepping read the exact same `(min, max)` range per `ShapeKind`. Never
  hardcode separate min/max constants in `ui/toolbar/`.
- **Crop containment**: moving or nudging selected marks (single or
  multi-selection) clamps the group inside the active crop bounds while
  preserving relative spacing between grouped marks.

## 3. Live Preview vs Export Parity (`raster.rs`)

A control existing in the toolbar is **not** completion: live display,
async preview caches, hit-testing, undo/redo, and final PNG/JPEG/WebP
export must agree visually across `1.0×`, `1.25×` (fractional), and `2×+`
scales.

### Compositing & Sampling Order

1. **Base capture pixels** (RGBA8 physical crop).
2. **Ordinary mark & filter layer** (in creation order, skipping marks
   hidden by whole-object `Eraser` / active inline text re-edits).
3. **Spotlights**: dim outside the union of active spotlight regions using
   shared spotlight opacity, plus per-spotlight outlines.
4. **Magnifiers (`Lens`)**:
   - When annotation sampling is enabled, magnifiers sample the
     **completed ordinary annotation layer** regardless of creation order.
   - Magnifiers **never** recursively sample other magnifiers.
   - When annotation sampling is disabled, magnifiers sample the raw
     base capture.
   - Live drafts and committed magnifiers must follow the exact same
     sampling policy, and async raster jobs must never overwrite newer
     frames or flash stale spotlight/mark states.

## 4. Interaction, History & Focus Contracts

Every new tool or option control must uphold the interaction contract
tracked in [`docs/annotation-parity.md`](../annotation-parity.md):

- **Innermost-first `Escape`**: `Escape` cancels the innermost active
  operation first (open popover/menu → inline text/number editor →
  in-progress polyline/drag → active annotation tool → capture overlay).
  Never let `Escape` nuke the whole capture while a tool or editor is
  active.
- **Cancel uncommitted drags before mutating marks**: switching tools,
  deleting marks, or firing `ClearAnnotations` must cancel any active
  move/handle gesture first so releasing the mouse button later cannot
  corrupt a different mark.
- **Focus restoration after toolbar menus/popovers**: closing or
  confirming a font selector, color picker, or parameter popover must
  return keyboard focus to the overlay canvas (or active `TextEditor`)
  without clearing the selected mark, so `Ctrl+Z` and typing work
  immediately without an extra canvas click.
- **Whole-object eraser & grouped history**: erasing marks (brush or
  rectangle) and optional sequence renumbering record clean, reversible
  history entries (`UndoAnnotation` / `RedoAnnotation`).
- **Multi-selection rules**:
  - `Shift`-click toggles mark membership; `Select`-tool drag (or
    `Shift`-drag when no drawing tool is armed) sweeps a selection
    marquee, while plain blank-canvas drag moves the crop.
  - Multi-selection draws one frame per mark with **no** individual resize
    handles; style controls edit the primary mark; dragging a grouped
    magnifier translates both its source and destination rects together.
- **Inline sequence value editing**: double-clicking a `Number` badge
  reuses `text_editing` gates (blocking canvas drags), live-previews valid
  positive integers, keeps the last valid preview on invalid input, and
  restores the original value if committed empty/invalid.
