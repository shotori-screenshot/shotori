# Annotation capability and acceptance matrix

Scope: regular screenshot annotations described in the linked PixPin documentation.
Watermarks and model-based smart erasure are excluded by the requested scope. A listed control is not evidence of complete
parity: rendering, export, selection, history and pointer interaction must agree.
Native desktop visual acceptance remains required for every implemented row.

| Reference | Implemented behavior | Remaining parity work |
| --- | --- | --- |
| [Common interaction](https://pixpin.cn/docs/mark/base-use) | Color palette, custom color, width controls, selection, deletion, undo/redo, icon choice menus | Main/secondary tool areas, alpha controls, per-tool color preferences, pin annotation, complete keyboard/wheel audit |
| [Rectangle and ellipse](https://pixpin.cn/docs/mark/geo) | Dedicated rectangle/ellipse tools, fill, five stroke patterns, independent stroke-width and corner-radius controls, rectangle corner handles, rotation, ellipse sector/arc handles, square/circle constraint, moving/resizing | Native comparison of rotated geometry and handles |
| [Line and polyline](https://pixpin.cn/docs/mark/line) | Five stroke patterns, editable curve point, polyline completion, independent endpoint styles, configurable caps/joins, moving and crop containment | Complete vertex editing and angle-constraint audit |
| [Arrow](https://pixpin.cn/docs/mark/arrow) | Seven silhouettes, five applicable stroke patterns, curve editing, size/color, crop containment | Attached comments and comment preference; visual comparison of all silhouettes |
| [Sequence](https://pixpin.cn/docs/mark/serial) | Decimal, alphabetic and Roman values, direct value input, increment/decrement, size/color, undoable placement counter, optional deletion renumbering, drag-out arrows with editable tips and seven silhouettes | Badge styles, attached text, fonts, outlines, selected-badge controls |
| [Pencil](https://pixpin.cn/docs/mark/pencil) | Freehand, smoothing switch, Shift straight segment, free/5/10/15/30/45-degree constraints | Persisted pencil preferences, cursor choices and maximum-width preference |
| [Highlighter](https://pixpin.cn/docs/mark/mark-pencil) | Brush and rectangle modes, translucency and multiply blending, matching live preview and export | Native acceptance across color and fractional-scale cases |
| [Mosaic and blur](https://pixpin.cn/docs/mark/mosaic) | Rectangle and brush, separate intensity and brush diameter, reversible effect changes | Automatic matching-area mosaic; performance acceptance on large selections |
| [Text](https://pixpin.cn/docs/mark/text) | Unicode/IME input, installed fonts, bold/italic, outline and background, padding/radius, direct size/color, editing, overflow handling, moving, configured wrap modes | Rotation and scale handles, attached arrows, delete handle, native edit/IME acceptance |
| [Eraser](https://pixpin.cn/docs/mark/erase) | Local pixel brush/rectangle erasure, editable rectangular masks, clear-all, reversible edits | Native acceptance over mixed effects and overlapping masks |
| [Spotlight](https://pixpin.cn/docs/mark/highlight) | Rectangle/ellipse regions, shared dimming mask and opacity, independent outline, Shift-wheel region sizing and Ctrl-wheel outline sizing | Comparison of shadows and edge behavior on more backgrounds |
| [Magnifier](https://pixpin.cn/docs/mark/magnifier) | Rectangular default, separate source/destination, 1–8 zoom, four connector choices, annotation sampling, antialiasing, fixed border width, shadow toggle, zoom-only wheel over either lens region | Native visual comparison of shadow/antialiasing, fractional zoom behavior and frame-time acceptance |
| [Preferences](https://pixpin.cn/docs/configuration/mark) | Configurable toolbar and palette, selection-after-drawing preference, persisted text wrapping and deletion renumbering | Persist remaining new style preferences; complete common/per-tool settings and screenshot/pin consistency |

## Interaction contract

User-selected interaction requirements take precedence over reference defaults:

- Escape dismisses the innermost active interaction before leaving annotation or capture
- Right click finishes a live polyline before a subsequent click can exit drawing
- Clicking a parameter menu preserves the edited mark and applies a reversible edit
- Multi-choice controls use icon previews in dropdowns; independent flags remain toggles
- Rectangle and ellipse are chosen in the main toolbar only. Stroke width remains the
  primary slider and wheel target; rectangle radius has its own numeric icon and popover
- Dragging inside the selected mark's frame moves it; its geometry remains inside the crop
- Shift-click toggles selection; Select-tool dragging sweeps a marquee. With no drawing
  tool active, Shift-drag also selects marks while a plain blank-canvas drag moves the crop
- Group dragging and arrow-key movement preserve spacing and crop containment; grouped
  deletion and optional sequence renumbering share one undo/redo entry
- Multi-selection has one frame per mark and no resize handles. Style controls edit the
  primary mark; dragging a grouped magnifier moves both its source and enlarged image
- Drawing does not show hover selection on unrelated marks
- Magnifier border width is fixed; its slider and wheel only change zoom, and wheel
  input outside the source and enlarged image leaves the selected lens unchanged
- Magnifiers sample the completed ordinary annotation layer regardless of creation order;
  they never recursively sample other magnifiers. Disabling annotation sampling uses
  the original capture. Drafts must use the same sampling policy as committed marks; native acceptance of live freehand sampling remains required
- Selected text and its frame use the same current geometry, including while an older
  background raster job is still running
- Creating a magnifier displays an immediate source outline; asynchronous raster work
  must not erase other marks or publish stale spotlight frames

## Verification gates

Use generated scenes for geometry, rasterization and history tests. Cover fractional
output scales, empty and small bounds, transparent source pixels, opposing drag
directions, undo/redo, selected-mark edits and tool defaults. Widget tests must exercise
actual menu clicks and verify focus and selection after the menu closes. Narrow toolbar
rows must remain reachable by horizontal scrolling.

Native acceptance follows [the private compositor procedure](headless-e2e.md). Check
pointer tracking, paused drags, menu transitions and asynchronous frame replacement with
frame sequences. Compilation and unit/widget tests do not establish visual parity.

## Native scene coverage

Private Wayland compositor checks at scale 1.25 cover text style controls, sequence
badges with arrows and value controls, separate rectangle/ellipse options, grouped
selection frames without individual handles, a magnifier
drag with an unrelated arrow, and spotlight drawing with a held-button pause. The
spotlight pause retains an identical frame until pointer movement resumes. These
checks validate the generated scenes only; they do not establish whole-system parity.
