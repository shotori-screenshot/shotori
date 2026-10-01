//! Geometry annotations in desktop logical coordinates, shared by all outputs.
mod chrome;
mod filter;
mod highlighter;
mod line;
pub(crate) use line::StrokePreview;
pub(crate) mod text;
pub(crate) use highlighter::HighlighterCache;
mod number;
mod select;
pub(crate) use number::NumberCache;
pub(crate) use select::ShapeHover;

use chrome::ellipse_contour;
use gpui_kit::{Bounds, Path, PathBuilder, Pixels, Point, point, px, size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShapeKind {
    Eraser,
    EraserRect,
    Text,
    Number,
    Pencil,
    Highlighter,
    Mosaic,
    Blur,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Polyline,
}

impl ShapeKind {
    /// Region kinds carry their geometry as a single bounds box
    /// (corner-handle editing); point-cloud kinds as `points`
    /// (endpoint/vertex editing).
    pub(crate) fn is_region(self) -> bool {
        matches!(self, ShapeKind::Rectangle | ShapeKind::Ellipse)
    }
}

/// A tool family's continuous size model (issue #3, phase 2): a
/// clamped min/max range. One spec per family — the wheel and the
/// slider both read the same numbers.
pub(crate) struct SizeSpec {
    pub(crate) min: f32,
    pub(crate) max: f32,
}

/// The size semantics of a shape kind: stroke widths, brush widths,
/// filter strengths, badge diameters or font sizes, each with its own
/// range.
pub(crate) fn size_spec(kind: ShapeKind) -> SizeSpec {
    match kind {
        ShapeKind::Highlighter => SizeSpec { min: 8., max: 60. },
        ShapeKind::Mosaic | ShapeKind::Blur => SizeSpec { min: 4., max: 48. },
        ShapeKind::Eraser | ShapeKind::EraserRect => SizeSpec { min: 8., max: 96. },
        ShapeKind::Number => SizeSpec { min: 16., max: 64. },
        ShapeKind::Text => SizeSpec { min: 12., max: 72. },
        _ => SizeSpec { min: 1., max: 20. },
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shape {
    pub(crate) kind: ShapeKind,
    pub(crate) number: Option<u32>,
    pub(crate) text: Option<String>,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) color: u32,
    pub(crate) width: f32,
    pub(crate) points: Vec<Point<Pixels>>,
}

impl Shape {
    pub(crate) fn line_paths(&self, offset: Point<Pixels>) -> Vec<Path<Pixels>> {
        line::paths(self, offset)
    }

    /// Match export's inward ellipse ring; reverse the inner contour to cut a hole.
    pub(crate) fn ellipse_path(&self, offset: Point<Pixels>) -> Option<Path<Pixels>> {
        let rx = f32::from(self.bounds.size.width) / 2.;
        let ry = f32::from(self.bounds.size.height) / 2.;
        if rx <= 0. || ry <= 0. {
            return None;
        }
        let center = self.bounds.origin + offset + point(px(rx), px(ry));
        let mut path = PathBuilder::fill();
        ellipse_contour(&mut path, center, rx, ry, 1.);
        if rx > self.width && ry > self.width {
            ellipse_contour(&mut path, center, rx - self.width, ry - self.width, -1.);
        }
        path.build().ok()
    }

    fn rasterize_ellipse(
        &self,
        rgba: &mut [u8],
        w: u32,
        h: u32,
        origin: Point<Pixels>,
        scale: f32,
    ) {
        let rx = f32::from(self.bounds.size.width) * scale / 2.;
        let ry = f32::from(self.bounds.size.height) * scale / 2.;
        if rx <= 0. || ry <= 0. {
            return;
        }
        let cx = f32::from(self.bounds.left() - origin.x) * scale + rx;
        let cy = f32::from(self.bounds.top() - origin.y) * scale + ry;
        let inner_rx = rx - self.width * scale;
        let inner_ry = ry - self.width * scale;
        let left = (cx - rx).floor().clamp(0., w as f32) as usize;
        let right = (cx + rx).ceil().clamp(0., w as f32) as usize;
        let top = (cy - ry).floor().clamp(0., h as f32) as usize;
        let bottom = (cy + ry).ceil().clamp(0., h as f32) as usize;
        let color = self.color.to_be_bytes();
        let mut coverage = vec![0_f32; right - left];
        for row in top..bottom {
            coverage.fill(0.);
            // Analytic horizontal coverage and eight vertical samples smooth the edge
            // without testing every subpixel of the entire bounding rectangle.
            for sample in 0..8 {
                let dy = row as f32 + (sample as f32 + 0.5) / 8. - cy;
                if dy.abs() >= ry {
                    continue;
                }
                let outer = rx * (1. - (dy / ry).powi(2)).sqrt();
                let inner = if inner_rx > 0. && inner_ry > 0. && dy.abs() < inner_ry {
                    inner_rx * (1. - (dy / inner_ry).powi(2)).sqrt()
                } else {
                    0.
                };
                for (start, end) in [(cx - outer, cx - inner), (cx + inner, cx + outer)] {
                    let start_col = (start.floor().max(left as f32) as usize).min(right);
                    let end_col = (end.ceil().max(left as f32) as usize).min(right);
                    for col in start_col..end_col {
                        coverage[col - left] +=
                            (end.min(col as f32 + 1.) - start.max(col as f32)).max(0.) / 8.;
                    }
                }
            }
            for (index, coverage) in coverage.iter().copied().enumerate() {
                let offset = (row * w as usize + left + index) * 4;
                if rgba[offset + 3] == 0 || coverage == 0. {
                    continue;
                }
                let coverage = coverage.min(1.);
                for channel in 0..3 {
                    rgba[offset + channel] = (rgba[offset + channel] as f32 * (1. - coverage)
                        + color[channel] as f32 * coverage)
                        .round() as u8;
                }
            }
        }
    }

    /// Inward strokes keep both preview and export within the rectangle.
    pub(crate) fn strokes(&self) -> [Bounds<Pixels>; 4] {
        let b = self.bounds;
        let width = px(self.width)
            .min(b.size.width / 2.)
            .min(b.size.height / 2.);
        [
            Bounds::new(b.origin, size(b.size.width, width)),
            Bounds::new(
                point(b.left(), b.bottom() - width),
                size(b.size.width, width),
            ),
            Bounds::new(
                point(b.left(), b.top() + width),
                size(width, b.size.height - width * 2.),
            ),
            Bounds::new(
                point(b.right() - width, b.top() + width),
                size(width, b.size.height - width * 2.),
            ),
        ]
    }
}

#[derive(Clone)]
struct Draft {
    start: Point<Pixels>,
    shape: Shape,
}

/// An eraser gesture in flight (issue #14). The eraser deletes whole
/// shapes, so unlike every other tool it carries NO draft shape — the
/// gesture state is the sweep itself, and its only output is
/// removals from `shapes`.
#[derive(Clone, Copy)]
enum EraserGesture {
    /// Brush sweep. `last` is the previous sample; the segment to the
    /// next one is interpolated so a fast flick cannot jump over thin
    /// ink between two pointer events.
    Stroke { last: Point<Pixels> },
    /// Area sweep: the dragged rectangle, committed on release (the
    /// bounds are only known at the end).
    Rect {
        start: Point<Pixels>,
        end: Point<Pixels>,
    },
}

/// One reversible step. Placement history was a plain shape stack, but
/// in-place edits need both directions recorded: `before` to undo,
/// `after` to redo. Shape indices are stable because committed shapes
/// only ever leave from the tail via undo.
#[derive(Clone)]
enum HistoryEntry {
    Add(Shape),
    Edit {
        ix: usize,
        before: Shape,
        after: Shape,
    },
    /// A mid-sequence removal (the Delete key): undo re-inserts at the
    /// same index. Indices in older entries stay valid because the
    /// stack unwinds in reverse order — each entry's index matches the
    /// moment it was recorded.
    Remove {
        ix: usize,
        shape: Shape,
    },
    /// An eraser gesture's whole-shape deletions (issue #14), one
    /// entry per gesture so a single undo restores everything the
    /// sweep took (and redo re-takes it). Recorded LIVE — removals
    /// hit `shapes` as the brush touches them — so each pair's index
    /// is the moment it left; undo re-inserts in reverse, redo
    /// removes in forward order.
    RemoveMany {
        removals: Vec<(usize, Shape)>,
    },
    /// Clear-all (issue #15): every placed shape leaves as ONE entry,
    /// so a single undo restores the whole sequence in order. A
    /// whole-list replacement rather than N per-shape `Remove`s: the
    /// saved sequence is self-describing, so undo puts the exact list
    /// back no matter what interleaves after the clear — no index
    /// arithmetic over a list that empties and refills.
    RemoveAll {
        shapes: Vec<Shape>,
    },
}

/// Per-tool size memory: every ShapeKind remembers its own last-used
/// size, so adjusting one tool never bleeds into another. Editing a
/// SELECTED shape writes back to its own kind's slot too — the next
/// stroke of that kind continues from the adjusted value. Ranges
/// still come from `size_spec` per family.
struct ToolSizes {
    rectangle: f32,
    ellipse: f32,
    line: f32,
    arrow: f32,
    polyline: f32,
    pencil: f32,
    highlighter: f32,
    mosaic: f32,
    blur: f32,
    eraser: f32,
    eraser_rect: f32,
    number: f32,
    text: f32,
}

pub(crate) struct Annotations {
    tool: Option<ShapeKind>,
    color_ix: usize,
    preset: ToolSizes,
    highlighter_color_ix: usize,
    shapes: Vec<Shape>,
    history: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    draft: Option<Draft>,
    draft_generation: u64,
    pressed: bool,
    /// The eraser gesture in flight (no draft exists while Some).
    eraser: Option<EraserGesture>,
    /// The history's trailing RemoveMany entry belongs to the live
    /// gesture and extends with each removal; cleared when the
    /// gesture ends (see `commit_removals`).
    eraser_entry_open: bool,
    /// Index into `shapes` of the currently selected annotation, if any.
    /// Editing actions (wheel size stepping, later drags) target it.
    selected: Option<usize>,
    text_original: Option<(usize, Shape)>,
    /// A slider size-drag is in flight: consecutive `apply_size` calls
    /// merge into one history entry (see `apply_size`).
    size_drag_active: bool,
}

impl Default for Annotations {
    fn default() -> Self {
        Self {
            tool: None,
            color_ix: 0,
            preset: ToolSizes {
                rectangle: 3.,
                ellipse: 3.,
                line: 3.,
                arrow: 3.,
                polyline: 3.,
                pencil: 3.,
                highlighter: 20.,
                mosaic: 16.,
                blur: 16.,
                eraser: 32.,
                eraser_rect: 32.,
                number: 32.,
                text: 24.,
            },
            highlighter_color_ix: 2,
            shapes: Vec::new(),
            history: Vec::new(),
            redo: Vec::new(),
            draft: None,
            draft_generation: 0,
            pressed: false,
            eraser: None,
            eraser_entry_open: false,
            selected: None,
            text_original: None,
            size_drag_active: false,
        }
    }
}

impl Annotations {
    /// Only render state crosses to the worker; undo stacks and tool state stay on the UI thread.
    pub(crate) fn render_snapshot(&self) -> Self {
        Self {
            shapes: self.shapes.clone(),
            draft: self.draft.clone(),
            draft_generation: self.draft_generation,
            ..Self::default()
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.tool.is_some()
    }
    pub(crate) fn color(&self) -> (u32, &'static str) {
        if let Some(shape) = self.editing_text() {
            return (shape.color, "Text");
        }
        let ix = if self.tool == Some(ShapeKind::Highlighter) {
            self.highlighter_color_ix
        } else {
            self.color_ix
        };
        (
            crate::ui::theme::c().annotation_colors[ix],
            crate::ui::theme::PALETTE_NAMES[ix],
        )
    }
    /// A kind's remembered size (width, strength, diameter or font —
    /// whatever that kind's slider means).
    pub(crate) fn size_of(&self, kind: ShapeKind) -> f32 {
        match kind {
            ShapeKind::Rectangle => self.preset.rectangle,
            ShapeKind::Ellipse => self.preset.ellipse,
            ShapeKind::Line => self.preset.line,
            ShapeKind::Arrow => self.preset.arrow,
            ShapeKind::Polyline => self.preset.polyline,
            ShapeKind::Pencil => self.preset.pencil,
            ShapeKind::Highlighter => self.preset.highlighter,
            ShapeKind::Mosaic => self.preset.mosaic,
            ShapeKind::Blur => self.preset.blur,
            ShapeKind::Eraser => self.preset.eraser,
            ShapeKind::EraserRect => self.preset.eraser_rect,
            ShapeKind::Number => self.preset.number,
            ShapeKind::Text => self.preset.text,
        }
    }
    /// Remember a kind's size, clamped to its family's [`SizeSpec`].
    pub(crate) fn set_size_of(&mut self, kind: ShapeKind, v: f32) {
        let spec = size_spec(kind);
        let v = v.clamp(spec.min, spec.max);
        let slot = match kind {
            ShapeKind::Rectangle => &mut self.preset.rectangle,
            ShapeKind::Ellipse => &mut self.preset.ellipse,
            ShapeKind::Line => &mut self.preset.line,
            ShapeKind::Arrow => &mut self.preset.arrow,
            ShapeKind::Polyline => &mut self.preset.polyline,
            ShapeKind::Pencil => &mut self.preset.pencil,
            ShapeKind::Highlighter => &mut self.preset.highlighter,
            ShapeKind::Mosaic => &mut self.preset.mosaic,
            ShapeKind::Blur => &mut self.preset.blur,
            ShapeKind::Eraser => &mut self.preset.eraser,
            ShapeKind::EraserRect => &mut self.preset.eraser_rect,
            ShapeKind::Number => &mut self.preset.number,
            ShapeKind::Text => &mut self.preset.text,
        };
        *slot = v;
    }
    /// The active tool's drawing width (what a fresh stroke takes).
    pub(crate) fn width(&self) -> f32 {
        self.tool.map_or(3., |t| self.size_of(t))
    }
    pub(crate) fn text_size(&self) -> f32 {
        self.editing_text()
            .map_or(self.preset.text, |shape| shape.width)
    }
    /// The active tool's current size — whatever that means for the
    /// tool; one continuous value the slider shows.
    pub(crate) fn tool_size(&self) -> f32 {
        self.tool.map_or(3., |t| self.size_of(t))
    }
    /// Set the active tool's size, clamped to its [`SizeSpec`].
    pub(crate) fn set_tool_size(&mut self, v: f32) {
        let Some(tool) = self.tool else { return };
        self.set_size_of(tool, v);
    }
    pub(crate) fn draft_generation(&self) -> u64 {
        self.draft_generation
    }

    pub(crate) fn preview_text(&mut self, bounds: Bounds<Pixels>, value: String) {
        if let Some((ix, _)) = &self.text_original {
            if let Some(shape) = self.shapes.get_mut(*ix) {
                shape.bounds = bounds;
                shape.text = Some(value);
            }
            return;
        }
        let color = self.color().0;
        let width = self.text_size();
        self.preview_text_with(bounds, value, color, width);
    }

    fn preview_text_with(&mut self, bounds: Bounds<Pixels>, value: String, color: u32, width: f32) {
        if !self.has_text_preview() {
            self.draft_generation += 1;
        }
        self.draft = Some(Draft {
            start: bounds.origin,
            shape: Shape {
                kind: ShapeKind::Text,
                number: None,
                text: Some(value),
                bounds,
                color,
                width,
                points: Vec::new(),
            },
        });
    }
    pub(crate) fn has_text_preview(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|d| d.shape.kind == ShapeKind::Text)
    }
    #[cfg(test)]
    pub(crate) fn add_text(&mut self, bounds: Bounds<Pixels>, value: String) {
        if value.trim().is_empty() {
            return;
        }
        self.record_add(Shape {
            kind: ShapeKind::Text,
            number: None,
            text: Some(value),
            bounds,
            color: self.color().0,
            width: self.text_size(),
            points: Vec::new(),
        });
    }
    pub(crate) fn number_size(&self) -> f32 {
        self.preset.number
    }
    pub(crate) fn next_number(&self) -> u32 {
        self.shapes
            .iter()
            .filter_map(|s| s.number)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }
    /// The number a new badge takes. `same_number` (Alt held) reuses
    /// the largest number already on canvas — several badges marking
    /// the same step — instead of advancing; an empty canvas starts
    /// at 1 either way.
    fn placement_number(&self, same_number: bool) -> u32 {
        if same_number {
            self.shapes
                .iter()
                .filter_map(|s| s.number)
                .max()
                .unwrap_or(1)
        } else {
            self.next_number()
        }
    }
    pub(crate) fn tool(&self) -> Option<ShapeKind> {
        self.tool
    }
    pub(crate) fn toggle(&mut self, kind: ShapeKind) {
        self.draft = None;
        self.pressed = false;
        self.selected = None;
        // a keyboard shortcut can switch tools mid-gesture; the sweep
        // must stop with it (a stale gesture would keep erasing under
        // the next tool's pointer moves)
        self.eraser = None;
        self.eraser_entry_open = false;
        self.tool = if self.tool == Some(kind) {
            None
        } else {
            Some(kind)
        };
    }

    pub(crate) fn set_color(&mut self, ix: usize) {
        if ix >= crate::ui::theme::c().annotation_colors.len() {
            return;
        }
        if self.editing_text().is_some() {
            self.color_ix = ix;
        }
        if let Some(shape) = self.editing_text_mut() {
            shape.color = crate::ui::theme::c().annotation_colors[ix];
            return;
        }
        if let Some(selected_ix) = self.selected_index() {
            let kind = self.shapes[selected_ix].kind;
            if kind == ShapeKind::Highlighter {
                self.highlighter_color_ix = ix;
            } else {
                self.color_ix = ix;
            }
            if !matches!(kind, ShapeKind::Mosaic | ShapeKind::Blur) {
                let raw_color = crate::ui::theme::c().annotation_colors[ix];
                let color = if kind == ShapeKind::Highlighter {
                    (raw_color & 0xffffff00) | 96
                } else {
                    raw_color
                };
                if self.shapes[selected_ix].color != color {
                    let before = self.shapes[selected_ix].clone();
                    self.shapes[selected_ix].color = color;
                    let after = self.shapes[selected_ix].clone();
                    self.history.push(HistoryEntry::Edit {
                        ix: selected_ix,
                        before,
                        after,
                    });
                    self.redo.clear();
                }
            }
            return;
        }
        if self.tool == Some(ShapeKind::Highlighter) {
            self.highlighter_color_ix = ix;
        } else {
            self.color_ix = ix;
        }
    }
    /// Nudge a size by one unit (the wheel). With a live selection this
    /// targets the SELECTED shape's size (recorded as a reversible
    /// edit); otherwise it steps the active tool's size — the value the
    /// NEXT stroke will take. Continuous within the tool's [`SizeSpec`].
    pub(crate) fn step_size(&mut self, up: bool) -> bool {
        // invariant: `selected` is a valid index or None — every path
        // that mutates `shapes` (undo/redo/reset/record_add) or leaves
        // edit mode (begin/toggle) clears it
        debug_assert!(self.selected.is_none_or(|ix| self.shapes.get(ix).is_some()));
        if let Some(ix) = self.selected {
            // The wheel over a selected NUMBER tunes its value (issue
            // #2's quick nudge — diameter stays with the slider);
            // every other kind steps its size as before.
            if self
                .shapes
                .get(ix)
                .is_some_and(|s| s.kind == ShapeKind::Number)
            {
                return self.step_selected_number(ix, up);
            }
            return self.step_selected_size(ix, up);
        }
        let Some(tool) = self.tool else {
            return false;
        };
        let spec = size_spec(tool);
        let cur = self.tool_size();
        let next = if up { cur + 1. } else { cur - 1. };
        if next == cur || next < spec.min || next > spec.max {
            return false;
        }
        self.set_tool_size(next);
        true
    }

    pub(crate) fn reset(&mut self) {
        self.shapes.clear();
        self.text_original = None;
        self.history.clear();
        self.redo.clear();
        self.draft = None;
        self.pressed = false;
        self.eraser = None;
        self.eraser_entry_open = false;
        self.tool = None;
        self.selected = None;
    }

    pub(crate) fn begin(&mut self, p: Point<Pixels>, selection: Bounds<Pixels>, same_number: bool) {
        if !self.enabled() || self.tool == Some(ShapeKind::Text) || !selection.contains(&p) {
            return;
        }
        // starting a new stroke gives up the selection
        self.selected = None;
        if self.tool == Some(ShapeKind::Number)
            && (selection.size.width < px(16.) || selection.size.height < px(16.))
        {
            return;
        }
        self.pressed = true;
        if matches!(self.tool, Some(ShapeKind::Eraser | ShapeKind::EraserRect)) {
            // Object eraser (issue #14): no draft, no pixel math —
            // the press itself already deletes what it lands on
            self.eraser_entry_open = false; // a new gesture never
            // merges into the previous one's history entry
            if self.tool == Some(ShapeKind::EraserRect) {
                self.eraser = Some(EraserGesture::Rect { start: p, end: p });
            } else {
                self.eraser = Some(EraserGesture::Stroke { last: p });
                self.erase_sample(p);
            }
            return;
        }
        if self.tool == Some(ShapeKind::Polyline) && self.draft.is_some() {
            return;
        }
        self.draft_generation += 1;
        self.draft = Some(Draft {
            start: p,
            shape: Shape {
                kind: self.tool.expect("active annotation tool"),
                text: None,
                number: (self.tool == Some(ShapeKind::Number))
                    .then(|| self.placement_number(same_number)),
                bounds: if self.tool == Some(ShapeKind::Number) {
                    number_bounds(p, selection, self.number_size())
                } else {
                    Bounds::new(p, size(px(0.), px(0.)))
                },
                color: if self.tool == Some(ShapeKind::Highlighter) {
                    (self.color().0 & 0xffffff00) | 96
                } else {
                    self.color().0
                },
                width: self.width(),
                points: if matches!(
                    self.tool,
                    Some(ShapeKind::Line | ShapeKind::Arrow | ShapeKind::Polyline)
                ) {
                    vec![p, p]
                } else if matches!(self.tool, Some(ShapeKind::Pencil | ShapeKind::Highlighter)) {
                    vec![p]
                } else {
                    Vec::new()
                },
            },
        });
    }

    pub(crate) fn drag_to(
        &mut self,
        p: Point<Pixels>,
        selection: Bounds<Pixels>,
        square: bool,
    ) -> bool {
        // Copy the gesture out (it is `Copy`) — the sampling below
        // needs `&mut self` for the removals themselves.
        if let Some(EraserGesture::Stroke { last }) = self.eraser {
            // Brush sweep: interpolate along the segment since the
            // last sample — two pointer events far apart must still
            // erase every piece of ink between them.
            let end = line_endpoint(last, p, selection, false);
            let radius = self.erase_radius();
            let step = radius.max(2.);
            let (dx, dy) = (f32::from(end.x - last.x), f32::from(end.y - last.y));
            let span = dx.hypot(dy);
            let steps = (span / step).ceil().max(1.) as usize;
            let mut changed = false;
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                let sample = point(
                    px(f32::from(last.x) + dx * t),
                    px(f32::from(last.y) + dy * t),
                );
                changed |= self.erase_sample(sample);
            }
            self.eraser = Some(EraserGesture::Stroke { last: end });
            return changed;
        }
        if let Some(EraserGesture::Rect { start, end: cur }) = self.eraser {
            // Area sweep: track the rect; deletion happens on release.
            let end = point(
                p.x.clamp(selection.left(), selection.right()),
                p.y.clamp(selection.top(), selection.bottom()),
            );
            let changed = cur != end;
            self.eraser = Some(EraserGesture::Rect { start, end });
            return changed;
        }
        let Some(draft) = self.draft.as_mut() else {
            return false;
        };
        if matches!(draft.shape.kind, ShapeKind::Pencil | ShapeKind::Highlighter) {
            let end = line_endpoint(draft.start, p, selection, false);
            if draft.shape.points.last() == Some(&end) {
                return false;
            }
            draft.shape.points.push(end);
            return true;
        }
        if draft.shape.kind == ShapeKind::Number {
            let bounds = number_bounds(p, selection, f32::from(draft.shape.bounds.size.width));
            let changed = bounds != draft.shape.bounds;
            draft.shape.bounds = bounds;
            return changed;
        }
        if matches!(
            draft.shape.kind,
            ShapeKind::Line | ShapeKind::Arrow | ShapeKind::Polyline
        ) {
            let last = draft.shape.points.len() - 1;
            let start = draft.shape.points[last - 1];
            let end = line_endpoint(start, p, selection, square);
            let changed = draft.shape.points[last] != end;
            draft.shape.points[last] = end;
            return changed;
        }
        let start = draft.start;
        let mut end = point(
            p.x.clamp(selection.left(), selection.right()),
            p.y.clamp(selection.top(), selection.bottom()),
        );
        if square {
            let dx = f32::from(end.x - start.x);
            let dy = f32::from(end.y - start.y);
            let available_x = if dx < 0. {
                start.x - selection.left()
            } else {
                selection.right() - start.x
            };
            let available_y = if dy < 0. {
                start.y - selection.top()
            } else {
                selection.bottom() - start.y
            };
            let side = dx
                .abs()
                .max(dy.abs())
                .min(f32::from(available_x))
                .min(f32::from(available_y));
            end = point(
                start.x + px(if dx < 0. { -side } else { side }),
                start.y + px(if dy < 0. { -side } else { side }),
            );
        }
        let bounds = Bounds::from_corners(
            point(start.x.min(end.x), start.y.min(end.y)),
            point(start.x.max(end.x), start.y.max(end.y)),
        );
        let changed = draft.shape.bounds != bounds;
        draft.shape.bounds = bounds;
        changed
    }

    pub(crate) fn end(&mut self) {
        let pressed = std::mem::take(&mut self.pressed);
        // The eraser gesture closes regardless of `pressed` — an
        // undo mid-sweep already cleared the flag, and a stale
        // gesture must never leak into the next tool's drags.
        if let Some(gesture) = self.eraser.take() {
            self.eraser_entry_open = false;
            if pressed && let EraserGesture::Rect { start, end } = gesture {
                // The area eraser deletes on release — the bounds are
                // only final now
                let area = Bounds::from_corners(
                    point(start.x.min(end.x), start.y.min(end.y)),
                    point(start.x.max(end.x), start.y.max(end.y)),
                );
                self.erase_rect(area);
            }
            return;
        }
        if !pressed {
            return;
        }
        if let Some(draft) = self.draft.as_mut()
            && draft.shape.kind == ShapeKind::Polyline
        {
            let last = draft.shape.points.len() - 1;
            let end = draft.shape.points[last];
            if distance(draft.shape.points[last - 1], end) >= 2. {
                draft.shape.points.push(end);
            } else {
                draft.shape.points[last] = draft.shape.points[last - 1];
            }
            return;
        }
        if let Some(draft) = self.draft.take() {
            let valid = if matches!(draft.shape.kind, ShapeKind::Line | ShapeKind::Arrow) {
                distance(draft.shape.points[0], draft.shape.points[1]) >= 2.
            } else if matches!(draft.shape.kind, ShapeKind::Pencil | ShapeKind::Highlighter) {
                true
            } else {
                draft.shape.bounds.size.width >= px(2.) && draft.shape.bounds.size.height >= px(2.)
            };
            if valid {
                self.record_add(draft.shape);
            }
        }
    }

    pub(crate) fn is_pressed(&self) -> bool {
        self.pressed
    }

    pub(crate) fn is_drawing_polyline(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| draft.shape.kind == ShapeKind::Polyline)
    }

    /// Commit confirmed vertices, never the floating cursor preview.
    pub(crate) fn finish_polyline(&mut self) {
        if !self.is_drawing_polyline() {
            return;
        }
        self.pressed = false;
        let mut shape = self.draft.take().unwrap().shape;
        shape.points.pop();
        if shape.points.len() >= 2 {
            self.record_add(shape);
        }
    }

    /// Escape cancels a stroke first, then drops the selection, then
    /// leaves the tool while keeping marks. An interrupted eraser
    /// gesture merely stops — its deletions were live and history-
    /// recorded as they happened; one Ctrl+Z reverts the whole sweep.
    pub(crate) fn cancel(&mut self) -> bool {
        self.pressed = false;
        self.eraser = None;
        self.eraser_entry_open = false;
        if self.draft.take().is_some() {
            return true;
        }
        if self.selected.take().is_some() {
            return true;
        }
        if self.enabled() {
            self.tool = None;
            return true;
        }
        false
    }
    /// Record a committed shape: the Add entry enables undo, and any
    /// diverging action invalidates the redo stack. Also drops the
    /// selection — indices above the tail may have shifted.
    fn record_add(&mut self, shape: Shape) {
        self.shapes.push(shape.clone());
        self.history.push(HistoryEntry::Add(shape));
        self.redo.clear();
        // freshly placed marks select themselves: wheel-resize or
        // drag-tune right after release without a second click
        self.selected = Some(self.shapes.len() - 1);
    }

    /// The stroke eraser's brush radius — half the eraser tool's
    /// remembered size; the overlay's ring chrome renders the same
    /// circle, so what you see is what erases.
    pub(crate) fn erase_radius(&self) -> f32 {
        self.size_of(ShapeKind::Eraser) / 2.
    }

    /// The area eraser's in-flight rect in selection-global
    /// coordinates (the dashed chrome outline). None outside the
    /// gesture.
    pub(crate) fn eraser_rect_bounds(&self) -> Option<Bounds<Pixels>> {
        match self.eraser {
            Some(EraserGesture::Rect { start, end }) => Some(Bounds::from_corners(
                point(start.x.min(end.x), start.y.min(end.y)),
                point(start.x.max(end.x), start.y.max(end.y)),
            )),
            _ => None,
        }
    }

    /// One brush sample: delete every shape whose ink the circle at
    /// `p` touches, topmost first. Removals are live (the canvas
    /// updates as the brush sweeps) and accumulate into ONE history
    /// entry per gesture — created on the first removal, extended
    /// afterwards (the `apply_size` merging pattern), so history
    /// never diverges from `shapes` even if undo lands mid-gesture.
    fn erase_sample(&mut self, p: Point<Pixels>) -> bool {
        let radius = self.erase_radius();
        let mut removed = Vec::new();
        // Descending indices stay valid as the list shrinks; each
        // pair records the index valid at the moment its shape left.
        let mut ix = self.shapes.len();
        while ix > 0 {
            ix -= 1;
            if select::shape_erased(&self.shapes[ix], p, radius) {
                removed.push((ix, self.shapes.remove(ix)));
            }
        }
        self.commit_removals(removed)
    }

    /// The area eraser's release: delete every shape whose bounds
    /// intersect the dragged rect — deliberately coarse ("clear this
    /// area"), in contrast with the brush's ink-touching precision.
    fn erase_rect(&mut self, area: Bounds<Pixels>) -> bool {
        let hits = |b: &Bounds<Pixels>| {
            let i = b.intersect(&area);
            f32::from(i.size.width) > 0. && f32::from(i.size.height) > 0.
        };
        let mut removed = Vec::new();
        let mut ix = self.shapes.len();
        while ix > 0 {
            ix -= 1;
            if hits(&self.shapes[ix].bounds) {
                removed.push((ix, self.shapes.remove(ix)));
            }
        }
        self.commit_removals(removed)
    }

    /// Fold a batch of removals into the gesture's trailing history
    /// entry (or open one). One undo then restores the whole sweep.
    fn commit_removals(&mut self, removed: Vec<(usize, Shape)>) -> bool {
        if removed.is_empty() {
            return false;
        }
        self.selected = None; // indices shift below
        match self.history.last_mut() {
            Some(HistoryEntry::RemoveMany { removals }) if self.eraser_entry_open => {
                removals.extend(removed);
            }
            _ => {
                self.history
                    .push(HistoryEntry::RemoveMany { removals: removed });
                self.redo.clear();
                self.eraser_entry_open = true;
            }
        }
        true
    }

    pub(crate) fn undo(&mut self) {
        self.pressed = false;
        self.selected = None;
        if self.draft.take().is_some() {
            return;
        }
        if let Some(entry) = self.history.pop() {
            match &entry {
                HistoryEntry::Add(_) => {
                    self.shapes.pop();
                }
                HistoryEntry::Edit { ix, before, .. } => {
                    if let Some(shape) = self.shapes.get_mut(*ix) {
                        *shape = before.clone();
                    }
                }
                HistoryEntry::Remove { ix, shape } => {
                    self.shapes.insert(*ix, shape.clone());
                }
                HistoryEntry::RemoveMany { removals } => {
                    // reverse gesture order: each index matches the
                    // list state at the moment that shape left
                    for (ix, shape) in removals.iter().rev() {
                        self.shapes.insert(*ix, shape.clone());
                    }
                }
                HistoryEntry::RemoveAll { shapes } => {
                    self.shapes = shapes.clone();
                }
            }
            self.redo.push(entry);
        }
    }
    pub(crate) fn redo(&mut self) {
        if self.draft.is_none()
            && let Some(entry) = self.redo.pop()
        {
            match &entry {
                HistoryEntry::Add(shape) => {
                    self.shapes.push(shape.clone());
                }
                HistoryEntry::Edit { ix, after, .. } => {
                    if let Some(shape) = self.shapes.get_mut(*ix) {
                        *shape = after.clone();
                    }
                }
                HistoryEntry::Remove { ix, .. } => {
                    self.shapes.remove(*ix);
                }
                HistoryEntry::RemoveMany { removals } => {
                    // forward gesture order reproduces the states the
                    // indices were recorded against
                    for (ix, _) in removals {
                        self.shapes.remove(*ix);
                    }
                    self.selected = None;
                }
                HistoryEntry::RemoveAll { .. } => {
                    self.shapes.clear();
                }
            }
            self.history.push(entry);
        }
    }
    pub(crate) fn visible(&self) -> impl Iterator<Item = &Shape> + '_ {
        self.shapes
            .iter()
            .chain(self.draft.as_ref().map(|draft| &draft.shape))
    }

    /// The live replacement stays at its original layer index, so raster caches,
    /// exports and later erasers all see the same image while typing.
    pub(crate) fn begin_text_edit(&mut self, ix: usize) -> bool {
        if self.text_original.is_some() {
            return false;
        }
        let Some(shape) = self.shapes.get(ix).filter(|s| s.kind == ShapeKind::Text) else {
            return false;
        };
        self.text_original = Some((ix, shape.clone()));
        self.end_size_drag();
        self.selected = Some(ix);
        true
    }

    pub(crate) fn editing_text(&self) -> Option<&Shape> {
        if let Some((ix, _)) = &self.text_original {
            return self.shapes.get(*ix);
        }
        self.draft
            .as_ref()
            .map(|d| &d.shape)
            .filter(|s| s.kind == ShapeKind::Text)
    }

    fn editing_text_mut(&mut self) -> Option<&mut Shape> {
        if let Some((ix, _)) = &self.text_original {
            return self.shapes.get_mut(*ix);
        }
        self.draft
            .as_mut()
            .map(|d| &mut d.shape)
            .filter(|s| s.kind == ShapeKind::Text)
    }

    pub(crate) fn finish_text_edit(&mut self, commit: bool) {
        if let Some((ix, original)) = self.text_original.take() {
            let Some(slot) = self.shapes.get_mut(ix) else {
                return;
            };
            let edited = std::mem::replace(slot, original);
            if commit {
                if edited.text.as_deref().is_none_or(|s| s.trim().is_empty()) {
                    self.delete_shape(ix);
                } else {
                    self.update_text(
                        ix,
                        edited.bounds,
                        edited.text.unwrap_or_default(),
                        edited.width,
                        edited.color,
                    );
                }
            }
        } else if self.has_text_preview() {
            let draft = self.draft.take();
            if let Some(draft) = draft.filter(|d| {
                commit
                    && d.shape
                        .text
                        .as_deref()
                        .is_some_and(|s| !s.trim().is_empty())
            }) {
                self.record_add(draft.shape);
            }
        }
    }

    pub(crate) fn shape(&self, ix: usize) -> Option<&Shape> {
        self.shapes.get(ix)
    }

    pub(crate) fn shape_kind(&self, ix: usize) -> Option<ShapeKind> {
        self.shapes.get(ix).map(|s| s.kind)
    }

    pub(crate) fn update_text(
        &mut self,
        ix: usize,
        bounds: Bounds<Pixels>,
        text: String,
        font_size: f32,
        color: u32,
    ) {
        if let Some(shape) = self.shapes.get_mut(ix) {
            let before = shape.clone();
            shape.text = Some(text);
            shape.bounds = bounds;
            shape.width = font_size;
            shape.color = color;
            let after = shape.clone();
            if after != before {
                self.history.push(HistoryEntry::Edit { ix, before, after });
                self.redo.clear();
            }
            self.selected = Some(ix);
        }
    }

    pub(crate) fn delete_shape(&mut self, ix: usize) {
        if ix < self.shapes.len() {
            let shape = self.shapes.remove(ix);
            self.history.push(HistoryEntry::Remove { ix, shape });
            self.redo.clear();
            self.selected = None;
        }
    }

    /// Remove every placed shape as ONE history entry (issue #15), so
    /// a single Ctrl+Z restores the whole sequence in order and redo
    /// re-clears. Selection/editing state that references shape
    /// indices goes first — the list is about to empty — and a live
    /// draft is canceled: the cleared canvas shows exactly the frozen
    /// capture. The active tool, colors and per-tool sizes stay; only
    /// the marks leave. No entry when nothing is placed (an empty
    /// canvas must not pollute undo, matching `delete_selected`).
    pub(crate) fn clear_all(&mut self) -> bool {
        if self.shapes.is_empty() {
            return false;
        }
        self.draft = None;
        self.pressed = false;
        self.selected = None;
        self.text_original = None;
        self.size_drag_active = false;
        self.eraser = None;
        self.eraser_entry_open = false;
        self.history.push(HistoryEntry::RemoveAll {
            shapes: std::mem::take(&mut self.shapes),
        });
        self.redo.clear();
        true
    }

    pub(crate) fn committed(&self) -> &[Shape] {
        &self.shapes
    }

    pub(crate) fn draft_shape(&self) -> Option<&Shape> {
        self.draft.as_ref().map(|draft| &draft.shape)
    }

    pub(crate) fn rasterize(
        &self,
        rgba: &mut [u8],
        w: u32,
        h: u32,
        origin: Point<Pixels>,
        scale: f32,
    ) {
        Self::rasterize_shapes(self.visible(), rgba, w, h, origin, scale);
    }

    pub(crate) fn rasterize_shapes<'a>(
        shapes: impl Iterator<Item = &'a Shape>,
        rgba: &mut [u8],
        w: u32,
        h: u32,
        origin: Point<Pixels>,
        scale: f32,
    ) {
        for shape in shapes {
            if shape.kind == ShapeKind::Text {
                text::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if matches!(shape.kind, ShapeKind::Mosaic | ShapeKind::Blur) {
                filter::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if shape.kind == ShapeKind::Number {
                number::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if matches!(
                shape.kind,
                ShapeKind::Line
                    | ShapeKind::Arrow
                    | ShapeKind::Polyline
                    | ShapeKind::Pencil
                    | ShapeKind::Highlighter
            ) {
                line::rasterize(shape, rgba, w, h, origin, scale);
                continue;
            }
            if shape.kind == ShapeKind::Ellipse {
                shape.rasterize_ellipse(rgba, w, h, origin, scale);
                continue;
            }
            let color = shape.color.to_be_bytes();
            for stroke in shape.strokes() {
                let x = |v: Pixels| {
                    (f32::from(v - origin.x) * scale)
                        .round()
                        .clamp(0., w as f32) as usize
                };
                let y = |v: Pixels| {
                    (f32::from(v - origin.y) * scale)
                        .round()
                        .clamp(0., h as f32) as usize
                };
                for row in y(stroke.top())..y(stroke.bottom()) {
                    for col in x(stroke.left())..x(stroke.right()) {
                        let offset = (row * w as usize + col) * 4;
                        // Preserve transparent gaps between desktop outputs.
                        if rgba[offset + 3] != 0 {
                            rgba[offset..offset + 4].copy_from_slice(&color);
                        }
                    }
                }
            }
        }
    }
}

fn number_bounds(p: Point<Pixels>, selection: Bounds<Pixels>, diameter: f32) -> Bounds<Pixels> {
    let diameter = px(diameter)
        .min(selection.size.width)
        .min(selection.size.height);
    let radius = diameter / 2.;
    let center = point(
        p.x.clamp(selection.left() + radius, selection.right() - radius),
        p.y.clamp(selection.top() + radius, selection.bottom() - radius),
    );
    Bounds::new(center - point(radius, radius), size(diameter, diameter))
}

fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    f32::from(b.x - a.x).hypot(f32::from(b.y - a.y))
}

fn line_endpoint(
    start: Point<Pixels>,
    p: Point<Pixels>,
    bounds: Bounds<Pixels>,
    constrain: bool,
) -> Point<Pixels> {
    let end = point(
        p.x.clamp(bounds.left(), bounds.right()),
        p.y.clamp(bounds.top(), bounds.bottom()),
    );
    if !constrain {
        return end;
    }
    let dx = f32::from(end.x - start.x);
    let dy = f32::from(end.y - start.y);
    let direction =
        ((dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() as i32).rem_euclid(8) as usize;
    let (x, y): (f32, f32) = [
        (1., 0.),
        (1., 1.),
        (0., 1.),
        (-1., 1.),
        (-1., 0.),
        (-1., -1.),
        (0., -1.),
        (1., -1.),
    ][direction];
    let mut length = dx.hypot(dy) / x.hypot(y);
    for (direction, available) in [
        (
            x,
            if x < 0. {
                start.x - bounds.left()
            } else {
                bounds.right() - start.x
            },
        ),
        (
            y,
            if y < 0. {
                start.y - bounds.top()
            } else {
                bounds.bottom() - start.y
            },
        ),
    ] {
        if direction != 0. {
            length = length.min(f32::from(available));
        }
    }
    point(
        (start.x + px(x * length)).clamp(bounds.left(), bounds.right()),
        (start.y + px(y * length)).clamp(bounds.top(), bounds.bottom()),
    )
}

#[cfg(test)]
mod tests {
    use super::{Annotations, ShapeHover, ShapeKind};
    use gpui_kit::{Bounds, point, px, size};

    fn selection() -> Bounds<gpui_kit::Pixels> {
        Bounds::new(point(px(-20.), px(0.)), size(px(100.), px(100.)))
    }
    /// The click-select path: probe + select, as pointer_up drives it.
    fn click(a: &mut Annotations, p: gpui_kit::Point<gpui_kit::Pixels>) -> bool {
        let hit = a.hit_test(p);
        hit.is_some_and(|ix| a.select_index(ix))
    }
    fn rectangle(a: &mut Annotations) {
        a.begin(point(px(10.), px(10.)), selection(), false);
        a.drag_to(point(px(30.), px(40.)), selection(), false);
        a.end();
    }

    #[test]
    fn text_edit_transaction_preserves_layers_cancel_and_history() {
        let mut a = super::Annotations::default();
        a.toggle(super::ShapeKind::Text);
        let bounds = gpui_kit::Bounds::new(point(px(10.), px(10.)), size(px(200.), px(80.)));
        a.add_text(bounds, "original".into());
        let original = a.committed()[0].clone();
        a.add_text(bounds, "later layer".into());
        let later = a.committed()[1].clone();
        let history_len = a.history.len();
        assert!(a.begin_text_edit(0));
        a.preview_text(bounds, "replacement".into());
        a.apply_size(40.);
        a.set_color(2);
        assert_eq!(
            a.visible().next().unwrap().text.as_deref(),
            Some("replacement")
        );
        assert_eq!(a.committed()[1], later);
        assert_eq!(a.history.len(), history_len);
        a.finish_text_edit(false);
        assert_eq!(a.committed()[0], original);
        assert_eq!(a.history.len(), history_len);
        assert!(a.begin_text_edit(0));
        a.preview_text(bounds, "committed".into());
        a.apply_size(36.);
        a.set_color(3);
        let edited = a.committed()[0].clone();
        a.finish_text_edit(true);
        assert_eq!(a.history.len(), history_len + 1);
        a.undo();
        assert_eq!(a.committed()[0], original);
        // A canceled edit must not consume the redo branch.
        assert!(a.begin_text_edit(0));
        a.preview_text(bounds, "canceled".into());
        a.finish_text_edit(false);
        a.redo();
        assert_eq!(a.committed()[0], edited);
        assert_eq!(a.committed()[1], later);
        assert!(a.begin_text_edit(0));
        a.preview_text(bounds, "  ".into());
        a.finish_text_edit(true);
        assert_eq!(a.committed(), std::slice::from_ref(&later));
        a.undo();
        assert_eq!(a.committed(), &[edited, later]);
    }

    #[test]
    fn vector_shapes_select_by_hit_with_topmost_priority() {
        let mut a = Annotations::default();
        // three marks laid out with clear water between them — the 8 px
        // hit tolerance reaches surprisingly far, so each probe point
        // must be checked against every shape's inflated band
        a.toggle(super::ShapeKind::Rectangle);
        a.begin(point(px(0.), px(10.)), selection(), false);
        a.drag_to(point(px(40.), px(50.)), selection(), false);
        a.end();
        a.toggle(super::ShapeKind::Line);
        a.begin(point(px(50.), px(10.)), selection(), false);
        a.drag_to(point(px(90.), px(50.)), selection(), false);
        a.end();
        a.toggle(super::ShapeKind::Ellipse);
        a.begin(point(px(50.), px(65.)), selection(), false);
        a.drag_to(point(px(78.), px(95.)), selection(), false);
        a.end();

        // rectangle: edge band hits, interior is click-transparent
        assert!(click(&mut a, point(px(0.), px(30.))));
        assert_eq!(
            a.selected().map(|s| s.kind),
            Some(super::ShapeKind::Rectangle)
        );
        assert!(a.hit_test(point(px(20.), px(30.))).is_none());

        // line: on-segment hits (drag_to snapped the end to (80,50) —
        // 45° snapping; probe a point ON the actual segment), far
        // off-segment misses
        assert!(click(&mut a, point(px(56.), px(18.))));
        assert_eq!(a.selected().map(|s| s.kind), Some(super::ShapeKind::Line));
        assert!(a.hit_test(point(px(75.), px(5.))).is_none());

        // ellipse (center 64,80, rx 14, ry 15): ring band hits, the
        // empty middle does not
        assert!(click(&mut a, point(px(78.), px(80.))));
        assert_eq!(
            a.selected().map(|s| s.kind),
            Some(super::ShapeKind::Ellipse)
        );
        assert!(a.hit_test(point(px(64.), px(80.))).is_none());

        // handle anchors follow the shape's own editing semantics:
        // two endpoints for a line, four corners for regions, none for
        // freehand strokes
        a.toggle(super::ShapeKind::Pencil);
        a.begin(point(px(5.), px(5.)), selection(), false);
        a.drag_to(point(px(60.), px(50.)), selection(), false);
        a.end();
        let handles = |a: &Annotations| -> Vec<(ShapeKind, usize)> {
            a.committed()
                .iter()
                .map(|s| (s.kind, s.handle_points().len()))
                .collect()
        };
        assert_eq!(
            handles(&a),
            vec![
                (ShapeKind::Rectangle, 4),
                (ShapeKind::Line, 2),
                (ShapeKind::Ellipse, 4),
                (ShapeKind::Pencil, 0),
            ]
        );
    }

    #[test]
    fn hover_splits_pick_from_move_by_selection() {
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Rectangle);
        // two rectangles, the later one topmost; their top edges both
        // run through (30,10)
        a.begin(point(px(0.), px(10.)), selection(), false);
        a.drag_to(point(px(40.), px(50.)), selection(), false);
        a.end();
        a.begin(point(px(20.), px(10.)), selection(), false);
        a.drag_to(point(px(60.), px(50.)), selection(), false);
        a.end();

        // the freshly placed top shape is selected (record_add picks
        // it), so the bottom one is the unselected case: a press over
        // it would pick it. Blank canvas offers nothing.
        assert_eq!(
            a.shape_hover(point(px(0.), px(30.))),
            Some(ShapeHover::Pick)
        );
        assert_eq!(a.shape_hover(point(px(70.), px(30.))), None);

        // selected: the same body becomes a move affordance
        assert!(a.select_index(0));
        assert_eq!(
            a.shape_hover(point(px(0.), px(30.))),
            Some(ShapeHover::Move)
        );

        // at the overlap the TOPMOST shape decides even though the
        // selected one is hit too — a press parks its click on the
        // topmost, so the affordance must promise the same
        assert_eq!(
            a.shape_hover(point(px(30.), px(10.))),
            Some(ShapeHover::Pick)
        );
    }

    #[test]
    fn wheel_edits_the_selection_and_adopts_it_as_the_preset() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a); // committed at width 3 (M)
        assert!(click(&mut a, point(px(10.), px(25.))));

        assert!(a.step_size(true)); // 3 → 4 on the SHAPE
        assert_eq!(a.selected().map(|s| s.width), Some(4.));
        assert_eq!(a.width(), 4.); // the edit became the preset too

        // deselected, the same wheel steps the preset directly
        let _ = a.cancel(); // consumed by dropping the selection
        assert!(a.enabled());
        assert!(a.step_size(false));
        assert_eq!(a.width(), 3.);
    }

    #[test]
    fn selected_width_edits_undo_and_redo_through_the_command_stack() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a); // committed at width 3 (M)
        assert!(click(&mut a, point(px(10.), px(25.))));
        assert!(a.step_size(true)); // 3 → 4
        assert_eq!(a.visible().next().unwrap().width, 4.);

        a.undo(); // Edit reversed
        assert_eq!(a.visible().next().unwrap().width, 3.);
        a.undo(); // Add reversed — shape leaves
        assert_eq!(a.visible().count(), 0);
        a.redo(); // Add replayed
        assert_eq!(a.visible().next().unwrap().width, 3.);
        a.redo(); // Edit replayed
        assert_eq!(a.visible().next().unwrap().width, 4.);
    }

    #[test]
    fn selection_lifecycle_clears_on_draw_tool_switch_and_escape() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);

        assert!(click(&mut a, point(px(10.), px(25.))));
        a.begin(point(px(50.), px(50.)), selection(), false); // new stroke wins
        assert_eq!(a.selected(), None);
        a.end();

        assert!(click(&mut a, point(px(10.), px(25.))));
        a.toggle(super::ShapeKind::Arrow); // tool switch wins
        assert_eq!(a.selected(), None);

        assert!(click(&mut a, point(px(10.), px(25.))));
        assert!(a.cancel()); // Escape consumed by the deselection
        assert!(a.enabled()); // tool still active…
        assert!(a.cancel()); // …the next Escape leaves it
        assert!(!a.enabled());
    }

    #[test]
    fn freehand_number_text_and_filter_shapes_select_and_step() {
        let mut a = Annotations::default();

        // pencil: visual-polygon hit on the stroke, +1 per notch
        a.toggle(super::ShapeKind::Pencil);
        a.begin(point(px(5.), px(5.)), selection(), false);
        for p in [(20., 20.), (40., 30.), (60., 50.)] {
            a.drag_to(point(px(p.0), px(p.1)), selection(), false);
        }
        a.end();
        assert!(click(&mut a, point(px(40.), px(30.))));
        assert_eq!(a.selected().map(|s| s.kind), Some(super::ShapeKind::Pencil));
        assert!(a.step_size(true));
        assert_eq!(a.selected().map(|s| s.width), Some(4.));

        // number badge: circle hit, miss outside; the wheel tunes the
        // VALUE now (issue #2) — the diameter stays with the slider;
        // undo restores. (This badge is #2 in the sequence: the pencil
        // stroke placed first is not numbered, `next_number` starts at 1.)
        a.toggle(super::ShapeKind::Number);
        a.begin(point(px(30.), px(50.)), selection(), false);
        a.end();
        assert!(click(&mut a, point(px(30.), px(50.))));
        assert_eq!(a.selected().map(|s| s.kind), Some(super::ShapeKind::Number));
        assert!(a.hit_test(point(px(30.), px(70.))).is_none());
        let diameter = f32::from(a.selected().unwrap().bounds.size.width);
        assert!(a.step_size(true)); // value 1 → 2
        assert_eq!(a.selected().and_then(|s| s.number), Some(2));
        assert_eq!(
            f32::from(a.selected().unwrap().bounds.size.width),
            diameter,
            "the wheel no longer resizes a badge"
        );
        assert!(a.step_size(false)); // back to 1
        a.undo(); // undoes the LAST notch (1 → 2 again); undo drops the selection
        macro_rules! badge {
            () => {
                a.committed()
                    .iter()
                    .find(|s| s.kind == super::ShapeKind::Number)
                    .unwrap()
                    .number
            };
        }
        assert_eq!(badge!(), Some(2), "undo reverts the -1 notch");
        a.undo();
        assert_eq!(badge!(), Some(1), "undo reverts the +1 notch too");

        // text: solid-bounds hit; wheel steps the font size (24 → 25)
        a.toggle(super::ShapeKind::Text);
        a.add_text(
            Bounds::new(point(px(0.), px(60.)), size(px(50.), px(20.))),
            "hi".into(),
        );
        assert!(click(&mut a, point(px(25.), px(70.))));
        assert_eq!(a.selected().map(|s| s.kind), Some(super::ShapeKind::Text));
        assert!(a.step_size(true));
        assert_eq!(a.selected().map(|s| s.width), Some(25.));

        // mosaic: solid-bounds hit; wheel steps filter strength (16 → 17)
        a.toggle(super::ShapeKind::Mosaic);
        a.begin(point(px(-10.), px(10.)), selection(), false);
        a.drag_to(point(px(20.), px(40.)), selection(), false);
        a.end();
        assert!(click(&mut a, point(px(5.), px(25.))));
        assert_eq!(a.selected().map(|s| s.kind), Some(super::ShapeKind::Mosaic));
        assert!(a.step_size(true));
        assert_eq!(a.selected().map(|s| s.width), Some(17.));
    }

    #[test]
    fn delete_removes_the_selection_and_undo_reinserts_in_place() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a); // A: (10,10) → (30,40)
        a.begin(point(px(40.), px(10.)), selection(), false);
        a.drag_to(point(px(60.), px(40.)), selection(), false);
        a.end(); // B beside it

        assert!(click(&mut a, point(px(10.), px(25.)))); // A (ix 0)
        assert!(a.delete_selected());
        assert_eq!(a.visible().count(), 1);
        assert_eq!(a.committed()[0].bounds.origin, point(px(40.), px(10.))); // B shifted down to ix 0

        a.undo(); // re-insert at the same index
        assert_eq!(a.visible().count(), 2);
        assert_eq!(a.committed()[0].bounds.origin, point(px(10.), px(10.)));
        assert_eq!(a.committed()[1].bounds.origin, point(px(40.), px(10.)));

        a.redo(); // remove again
        assert_eq!(a.visible().count(), 1);

        // deleting with no selection is a no-op
        assert!(!a.delete_selected());
    }

    #[test]
    fn clear_all_is_one_history_entry_whose_undo_restores_every_shape() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a); // A: (10,10) → (30,40)
        a.begin(point(px(40.), px(10.)), selection(), false);
        a.drag_to(point(px(60.), px(40.)), selection(), false);
        a.end(); // B beside it
        a.toggle(super::ShapeKind::Pencil);
        a.begin(point(px(5.), px(5.)), selection(), false);
        a.drag_to(point(px(60.), px(50.)), selection(), false);
        a.end(); // freehand C
        let before: Vec<super::Shape> = a.committed().to_vec();
        assert_eq!(before.len(), 3);
        let history_len = a.history.len();

        assert!(a.clear_all());
        // ONE entry for the whole wipe — not one per shape
        assert_eq!(a.history.len(), history_len + 1);
        assert_eq!(a.visible().count(), 0);

        a.undo(); // a single Ctrl+Z brings all three back…
        assert_eq!(a.committed(), before.as_slice());
        a.redo(); // …and redo re-clears them in one step
        assert_eq!(a.visible().count(), 0);
        a.undo();
        assert_eq!(a.committed(), before.as_slice());
    }

    #[test]
    fn clear_all_on_an_empty_canvas_records_no_history() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        assert!(!a.clear_all());
        assert!(a.history.is_empty());

        rectangle(&mut a);
        assert!(a.clear_all());
        assert!(!a.clear_all()); // a second press on the emptied canvas
        assert_eq!(a.history.len(), 2); // Add + exactly ONE Clear
        a.undo();
        assert_eq!(a.visible().count(), 1);
        // the no-op press must not have clobbered the redo stack
        a.redo();
        assert_eq!(a.visible().count(), 0);
    }

    #[test]
    fn clear_all_survives_history_interleaved_with_new_strokes() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a); // A
        a.begin(point(px(40.), px(10.)), selection(), false);
        a.drag_to(point(px(60.), px(40.)), selection(), false);
        a.end(); // B
        let before: Vec<super::Shape> = a.committed().to_vec();

        assert!(a.clear_all());
        rectangle(&mut a); // C on the emptied canvas
        assert_eq!(a.committed().len(), 1);
        a.undo(); // C leaves
        assert_eq!(a.visible().count(), 0);
        a.undo(); // the clear unwinds: A and B return, in order
        assert_eq!(a.committed(), before.as_slice());
        a.redo(); // re-clear
        assert_eq!(a.visible().count(), 0);
        a.redo(); // re-add C
        assert_eq!(a.committed().len(), 1);
    }

    #[test]
    fn clear_all_cancels_selection_draft_and_a_pending_text_edit() {
        // a live selection referencing a shape index
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        assert!(click(&mut a, point(px(10.), px(25.))));
        assert!(a.selected().is_some());
        assert!(a.clear_all());
        assert_eq!(a.visible().count(), 0);
        assert!(a.selected().is_none());

        // a stroke mid-gesture: the draft goes with the wipe (a
        // committed shape must exist too, else clear records nothing)
        a.toggle(super::ShapeKind::Pencil);
        a.begin(point(px(5.), px(5.)), selection(), false);
        a.drag_to(point(px(40.), px(30.)), selection(), false);
        a.end(); // committed pencil stroke
        a.begin(point(px(5.), px(5.)), selection(), false); // next in flight
        a.drag_to(point(px(20.), px(20.)), selection(), false);
        assert!(a.is_pressed());
        assert!(a.draft_shape().is_some());
        assert!(a.clear_all());
        assert!(!a.is_pressed());
        assert!(a.draft_shape().is_none());

        // an in-place text edit whose (ix, original) snapshot would
        // dangle: dropping it must leave a consistent state, and the
        // overlay's later finish must be a harmless no-op
        a.toggle(super::ShapeKind::Text);
        a.add_text(
            Bounds::new(point(px(0.), px(60.)), size(px(50.), px(20.))),
            "hi".into(),
        );
        assert!(a.begin_text_edit(0));
        a.preview_text(
            Bounds::new(point(px(0.), px(60.)), size(px(50.), px(20.))),
            "edited".into(),
        );
        let history_len = a.history.len();
        assert!(a.clear_all());
        a.finish_text_edit(true); // the overlay path after clear
        assert_eq!(a.visible().count(), 0);
        assert_eq!(a.history.len(), history_len + 1); // just the Clear
    }

    #[test]
    fn brush_eraser_deletes_whole_shapes_as_one_history_entry() {
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Rectangle);
        rectangle(&mut a); // (10,10) → (30,40)
        a.toggle(ShapeKind::Pencil);
        a.begin(point(px(40.), px(10.)), selection(), false);
        a.drag_to(point(px(60.), px(40.)), selection(), false);
        a.end(); // diagonal stroke beside it
        let before = a.committed().to_vec();
        assert_eq!(before.len(), 2);
        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(24.); // radius 12
        let history_len = a.history.len();
        // One sweep crossing BOTH: the press lands on the rectangle's
        // right band (a drawing tool would park a click-select there),
        // the drag reaches the diagonal.
        assert!(!a.parks_click_select());
        a.begin(point(px(28.), px(25.)), selection(), false);
        assert_eq!(a.committed().len(), 1, "the press itself erases");
        a.drag_to(point(px(50.), px(25.)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 0);
        // ONE entry for the whole gesture — not one per shape
        assert_eq!(a.history.len(), history_len + 1);
        a.undo(); // a single Ctrl+Z brings both back, in layer order
        assert_eq!(a.committed(), before.as_slice());
        a.redo(); // …and redo re-takes them in one step
        assert_eq!(a.committed().len(), 0);
    }

    #[test]
    fn brush_radius_decides_touching_and_interpolation_covers_gaps() {
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Line);
        a.set_tool_size(2.); // hairline: 1 px half-width
        a.begin(point(px(10.), px(50.)), selection(), false);
        a.drag_to(point(px(70.), px(50.)), selection(), false);
        a.end();
        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(16.); // radius 8 → reaches 9 px off the ink
        // near-miss: beyond radius + half-width
        a.begin(point(px(40.), px(60.5)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 1, "beyond the brush: untouched");
        // a fast flick whose ENDPOINTS both miss but whose path
        // crosses the ink — sampling only the endpoints would leave
        // the line alive
        a.begin(point(px(40.), px(30.)), selection(), false);
        a.drag_to(point(px(40.), px(70.)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 0, "the sweep crossed the line");
    }

    #[test]
    fn closed_polygon_interior_is_not_erasable_without_touching_ink() {
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Polyline);
        // vertices must sit INSIDE the selection: begin() guards on
        // it, and an outside click places no vertex
        for (x, y) in [(10., 10.), (70., 10.), (70., 60.), (10., 60.), (10., 10.)] {
            let p = point(px(x), px(y));
            a.begin(p, selection(), false);
            a.drag_to(p, selection(), false);
            a.end();
        }
        a.finish_polyline();
        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(16.); // radius 8, well inside the ring
        // Scrub the middle: the interior SELECTS (issue #16) but holds
        // no ink — a brush there must leave the ring alone, or nothing
        // inside it could ever be erased individually.
        a.begin(point(px(40.), px(35.)), selection(), false);
        a.drag_to(point(px(30.), px(35.)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 1);
        // touching the outline erases the whole polygon as one object
        a.begin(point(px(40.), px(10.)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 0);
    }

    #[test]
    fn rect_eraser_deletes_intersecting_bounds_on_release_only() {
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Rectangle);
        rectangle(&mut a); // (10,10) → (30,40)
        a.toggle(ShapeKind::Text);
        a.add_text(
            Bounds::new(point(px(50.), px(50.)), size(px(100.), px(20.))),
            "far".into(),
        );
        let history_len = a.history.len();
        a.toggle(ShapeKind::EraserRect);
        // drag an area covering the rectangle but not the far text
        a.begin(point(px(5.), px(5.)), selection(), false);
        a.drag_to(point(px(35.), px(45.)), selection(), false);
        // in-flight: nothing is deleted yet — the bounds are only
        // final on release, so the dashed rect is honest feedback
        assert_eq!(a.committed().len(), 2);
        a.end();
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.committed()[0].text.as_deref(), Some("far"));
        assert_eq!(a.history.len(), history_len + 1); // one entry
        // a stray click (zero-area rect) deletes nothing and records
        // nothing — even ON a shape's bounds
        let h = a.history.len();
        a.begin(point(px(50.), px(50.)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.history.len(), h);
        a.undo();
        assert_eq!(a.committed().len(), 2);
        a.redo();
        assert_eq!(a.committed().len(), 1);
    }

    #[test]
    fn mid_gesture_undo_reverts_removal_so_far_and_diverges_redo() {
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Pencil);
        a.begin(point(px(10.), px(10.)), selection(), false);
        a.drag_to(point(px(10.), px(40.)), selection(), false);
        a.end();
        a.toggle(ShapeKind::Rectangle);
        rectangle(&mut a);
        let before = a.committed().to_vec();
        a.toggle(ShapeKind::Eraser);
        a.set_tool_size(24.);
        // the press erases the rectangle; the gesture stays open
        a.begin(point(px(28.), px(25.)), selection(), false);
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.history.len(), 3); // pencil, rect, RemoveMany
        // Ctrl+Z mid-sweep: removals are history-recorded as they
        // happen, so the undo cleanly reverts the sweep so far
        a.undo();
        assert_eq!(a.committed(), before.as_slice());
        // the sweep continues: a NEW entry; the redo stack is gone
        a.drag_to(point(px(10.), px(25.)), selection(), false);
        a.end();
        assert_eq!(a.committed().len(), 0);
        assert_eq!(a.history.len(), 3);
    }

    #[test]
    fn slider_drag_edits_the_selection_as_one_history_entry() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a); // width 3, auto-selected on commit
        assert!(a.selected().is_some());

        // a whole drag: many Change values, one merged Edit
        for v in [4., 6., 9., 12.] {
            a.apply_size(v);
        }
        assert_eq!(a.selected().map(|s| s.width), Some(12.));
        a.end_size_drag();
        a.undo(); // ONE undo restores the pre-drag width
        assert_eq!(a.committed()[0].width, 3.);

        // without a selection, the same write targets the tool preset
        let _ = a.cancel();
        let _ = a.cancel(); // deselect, then leave the tool
        a.toggle(super::ShapeKind::Rectangle);
        a.apply_size(7.);
        a.end_size_drag();
        assert_eq!(a.width(), 7.);
        assert_eq!(a.committed()[0].width, 3.); // shape untouched
    }

    #[test]
    fn size_edits_carry_to_the_next_stroke_and_stay_per_tool() {
        let mut a = Annotations::default();

        // thicken the pencil's next stroke by editing its placed mark
        a.toggle(super::ShapeKind::Pencil);
        a.begin(point(px(5.), px(5.)), selection(), false);
        a.drag_to(point(px(60.), px(50.)), selection(), false);
        a.end(); // auto-selected
        a.apply_size(9.);
        a.end_size_drag();
        a.deselect();
        a.begin(point(px(5.), px(5.)), selection(), false); // next pencil stroke
        assert_eq!(a.draft_shape().unwrap().width, 9.);

        // the rectangle tool keeps its OWN memory
        a.end();
        a.toggle(super::ShapeKind::Rectangle);
        assert_eq!(a.width(), 3.);
    }

    #[test]
    fn wheel_steps_sizes_continuously_and_clamps_at_spec_ends() {
        let mut a = Annotations::default();

        // no active tool → nothing to step
        assert!(!a.step_size(true));

        // pencil: stroke range 1..20, ±1 per wheel notch
        a.toggle(super::ShapeKind::Pencil);
        assert_eq!(a.width(), 3.);
        assert!(a.step_size(true));
        assert_eq!(a.width(), 4.);
        assert!(a.step_size(false));
        assert_eq!(a.width(), 3.);
        // clamp at both spec ends
        a.set_tool_size(1.);
        assert!(!a.step_size(false)); // already at min
        a.set_tool_size(20.);
        assert!(!a.step_size(true)); // already at max

        // mosaic: filter strength keeps its own slot; pencil unchanged
        a.toggle(super::ShapeKind::Mosaic);
        assert_eq!(a.width(), 16.);
        assert!(a.step_size(true));
        assert_eq!(a.width(), 17.);
        a.toggle(super::ShapeKind::Pencil);
        assert_eq!(a.width(), 20.);

        // text drives the font size (range 12..72)
        a.toggle(super::ShapeKind::Text);
        assert_eq!(a.text_size(), 24.);
        assert!(a.step_size(false));
        assert_eq!(a.text_size(), 23.);

        // number drives the badge diameter (range 16..64)
        a.toggle(super::ShapeKind::Number);
        assert_eq!(a.number_size(), 32.);
        assert!(a.step_size(true));
        assert_eq!(a.number_size(), 33.);
    }

    #[test]
    fn highlighter_has_independent_style_and_whole_stroke_history() {
        let mut a = Annotations::default();
        let original_color = a.color();
        a.toggle(super::ShapeKind::Highlighter);
        assert_eq!(a.color().1, "Yellow");
        assert_eq!(a.width(), 20.);
        a.set_color(4);
        a.set_tool_size(32.);
        a.begin(point(px(10.), px(30.)), selection(), false);
        a.drag_to(point(px(60.), px(30.)), selection(), false);
        a.end();
        let mark = a.visible().next().unwrap().clone();
        assert_eq!(mark.color & 255, 96);
        assert_eq!(mark.width, 32.);
        a.toggle(super::ShapeKind::Pencil);
        assert_eq!(a.color(), original_color);
        assert_eq!(a.width(), 3.);
        a.undo();
        assert_eq!(a.visible().count(), 0);
        a.redo();
        assert_eq!(a.visible().next().unwrap(), &mark);
        a.toggle(super::ShapeKind::Highlighter);
        assert_eq!(a.width(), 32.);
        assert_eq!(a.color().1, "Blue");
    }

    #[test]
    fn pencil_records_curve_and_release_as_one_history_entry() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Pencil);
        let points =
            [(0., 10.), (15., 25.), (30., 10.), (40., 35.)].map(|(x, y)| point(px(x), px(y)));
        a.begin(points[0], selection(), false);
        for p in &points[1..] {
            assert!(a.drag_to(*p, selection(), false));
            assert!(!a.drag_to(*p, selection(), false));
        }
        a.end();
        assert!(!a.drag_to(point(px(70.), px(70.)), selection(), false));
        assert_eq!(a.visible().next().unwrap().points, points);
        a.undo();
        assert_eq!(a.visible().count(), 0);
        a.redo();
        assert_eq!(a.visible().next().unwrap().points, points);
        a.begin(points[0], selection(), false);
        a.drag_to(point(px(200.), px(-40.)), selection(), false);
        assert_eq!(
            *a.visible().last().unwrap().points.last().unwrap(),
            point(px(80.), px(0.))
        );
        a.cancel();
        assert_eq!(a.visible().count(), 1);
    }

    #[test]
    fn pencil_click_exports_round_dot_at_fractional_scales() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Pencil);
        a.set_tool_size(5.);
        a.begin(point(px(20.), px(20.)), selection(), false);
        a.end();
        assert_eq!(a.visible().count(), 1);
        assert_eq!(
            a.visible()
                .next()
                .unwrap()
                .line_paths(point(px(0.), px(0.)))
                .len(),
            1
        );
        for scale in [1., 1.25, 1.7, 2.] {
            let mut pixels = vec![255; 100 * 100 * 4];
            let center = (20. * scale) as usize;
            // Transparent gaps between screens must remain transparent.
            pixels[(center * 100 + center + 1) * 4 + 3] = 0;
            a.rasterize(&mut pixels, 100, 100, point(px(0.), px(0.)), scale);
            let at = |x, y| &pixels[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4];
            assert_eq!(at(center, center), a.color().0.to_be_bytes());
            assert_eq!(at(center + 1, center)[3], 0);
            assert_eq!(at(center + 5, center + 5), [255; 4]);
        }
    }

    #[test]
    fn rectangle_history_preserves_style_and_new_strokes_clear_redo() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        let first_color = a.visible().next().unwrap().color;
        a.deselect(); // Set the next stroke preset, not the selected rectangle.
        a.set_color(1);
        a.set_tool_size(5.);
        rectangle(&mut a);
        assert_eq!(a.visible().count(), 2);
        a.undo();
        assert!(!a.redo.is_empty());
        assert_eq!(a.visible().next().unwrap().color, first_color);
        a.redo();
        assert_eq!(a.visible().count(), 2);
        a.undo();
        rectangle(&mut a);
        assert!(a.redo.is_empty());
        a.reset();
        assert_eq!(a.visible().count(), 0);
        assert!(!a.enabled());
    }

    #[test]
    fn square_stays_square_at_boundary_and_reverse_drag_normalizes() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        a.begin(point(px(20.), px(20.)), selection(), false);
        a.drag_to(point(px(-100.), px(-200.)), selection(), true);
        let b = a.visible().next().unwrap().bounds;
        assert_eq!(b.origin, point(px(0.), px(0.)));
        assert_eq!(b.size, size(px(20.), px(20.)));
        a.end();
        assert_eq!(a.visible().count(), 1);
    }

    #[test]
    fn outside_click_and_tiny_stroke_do_not_destroy_redo() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        a.undo();
        a.begin(point(px(200.), px(20.)), selection(), false);
        a.end();
        a.begin(point(px(10.), px(10.)), selection(), false);
        a.drag_to(point(px(11.), px(11.)), selection(), false);
        a.end();
        assert!(!a.redo.is_empty());
        assert_eq!(a.visible().count(), 0);
    }

    #[test]
    fn escape_cancels_draft_then_exits_tool_without_losing_marks() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        a.begin(point(px(10.), px(10.)), selection(), false);
        assert!(a.cancel());
        assert!(a.enabled());
        assert_eq!(a.visible().count(), 1);
        assert!(a.cancel());
        assert!(!a.enabled());
        assert_eq!(a.visible().count(), 1);
        assert!(!a.cancel());
    }

    #[test]
    fn raster_strokes_preserve_interior_and_transparent_gaps() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        let mut pixels = [10, 20, 30, 255].repeat(100 * 100);
        let gap = (10 * 100 + 15) * 4;
        pixels[gap..gap + 4].fill(0);
        a.rasterize(&mut pixels, 100, 100, point(px(0.), px(0.)), 1.);
        assert_eq!(
            &pixels[(10 * 100 + 10) * 4..(10 * 100 + 10) * 4 + 4],
            &a.color().0.to_be_bytes()
        );
        assert_eq!(
            &pixels[(20 * 100 + 20) * 4..(20 * 100 + 20) * 4 + 4],
            &[10, 20, 30, 255]
        );
        assert_eq!(&pixels[gap..gap + 4], &[0; 4]);
    }
    #[test]
    fn mixed_shapes_share_history_and_switching_cancels_only_the_draft() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        a.begin(point(px(10.), px(10.)), selection(), false);
        a.toggle(super::ShapeKind::Ellipse);
        assert_eq!(a.visible().count(), 1);
        a.set_color(3);
        rectangle(&mut a);
        a.undo();
        assert_eq!(
            a.visible().next().unwrap().kind,
            super::ShapeKind::Rectangle
        );
        a.redo();
        let ellipse = a.visible().last().unwrap();
        assert_eq!(ellipse.kind, super::ShapeKind::Ellipse);
        assert_eq!(ellipse.color, a.color().0);
        a.toggle(super::ShapeKind::Ellipse);
        assert!(!a.enabled());
        assert_eq!(a.visible().count(), 2);
    }

    #[test]
    fn shift_ellipse_is_a_circle_even_at_selection_boundary() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Ellipse);
        a.begin(point(px(20.), px(20.)), selection(), false);
        a.drag_to(point(px(-100.), px(-200.)), selection(), true);
        a.end();
        let ellipse = a.visible().next().unwrap();
        assert_eq!(ellipse.kind, super::ShapeKind::Ellipse);
        assert_eq!(ellipse.bounds.origin, point(px(0.), px(0.)));
        assert_eq!(ellipse.bounds.size, size(px(20.), px(20.)));
    }

    #[test]
    fn ellipse_raster_has_smooth_edges_and_preserves_hole_corners_and_gaps() {
        for scale in [1., 1.25, 1.5, 1.73, 2.] {
            let ellipse = super::Shape {
                number: None,
                text: None,
                kind: super::ShapeKind::Ellipse,
                bounds: Bounds::new(point(px(-10.), px(15.)), size(px(60.), px(40.))),
                color: 0xff0000ff,
                points: Vec::new(),
                width: 3.,
            };
            let w = (100. * scale) as u32;
            let mut pixels = [0, 0, 0, 255].repeat((w * w) as usize);
            let gap_x = (40. * scale) as usize;
            let gap_y = (16. * scale) as usize;
            let gap = (gap_y * w as usize + gap_x) * 4;
            pixels[gap..gap + 4].fill(0);
            ellipse.rasterize_ellipse(&mut pixels, w, w, point(px(-20.), px(0.)), scale);
            let pixel = |x: f32, y: f32| {
                let offset = (((y * scale) as usize) * w as usize + (x * scale) as usize) * 4;
                &pixels[offset..offset + 4]
            };
            assert_eq!(pixel(40., 35.), &[0, 0, 0, 255], "center at {scale}");
            assert_eq!(pixel(11., 16.), &[0, 0, 0, 255], "corner at {scale}");
            assert_eq!(pixel(11., 35.), &[255, 0, 0, 255], "left edge at {scale}");
            assert_eq!(pixel(68., 35.), &[255, 0, 0, 255], "right edge at {scale}");
            assert_eq!(&pixels[gap..gap + 4], &[0; 4]);
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[0] > 0 && p[0] < 255)
            );
            assert!(ellipse.ellipse_path(point(px(0.), px(0.))).is_some());
        }
    }

    #[test]
    fn tiny_and_clipped_ellipses_render_without_invalid_geometry() {
        for (width, height) in [(0., 0.), (2., 2.), (2., 60.), (60., 2.), (60., 40.)] {
            let ellipse = super::Shape {
                number: None,
                text: None,
                kind: super::ShapeKind::Ellipse,
                bounds: Bounds::new(point(px(-10.), px(-10.)), size(px(width), px(height))),
                color: 0xff0000ff,
                points: Vec::new(),
                width: 5.,
            };
            let mut pixels = [0, 0, 0, 255].repeat(400);
            ellipse.rasterize_ellipse(&mut pixels, 20, 20, point(px(0.), px(0.)), 1.25);
            assert_eq!(
                ellipse.ellipse_path(point(px(0.), px(0.))).is_some(),
                width > 0.
            );
        }
    }
    #[test]
    fn horizontal_vertical_and_reverse_lines_are_valid_but_tiny_clicks_preserve_redo() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Line);
        for (x, y) in [(50., 20.), (20., 50.), (-10., 20.)] {
            a.begin(point(px(20.), px(20.)), selection(), false);
            a.drag_to(point(px(x), px(y)), selection(), false);
            a.end();
        }
        assert_eq!(a.visible().count(), 3);
        a.undo();
        a.begin(point(px(20.), px(20.)), selection(), false);
        a.end();
        a.redo();
        assert_eq!(a.visible().count(), 3);
    }

    #[test]
    fn line_snapping_preserves_45_degree_angles_at_every_boundary() {
        let start = point(px(30.), px(30.));
        for (x, y) in [
            (-200., -80.),
            (-80., 10.),
            (40., -300.),
            (200., 200.),
            (200., 50.),
            (45., 200.),
        ] {
            let end = super::line_endpoint(start, point(px(x), px(y)), selection(), true);
            // Stroke endpoints may lie exactly on the crop's exclusive right/bottom edge.
            assert!(end.x >= selection().left() && end.x <= selection().right());
            assert!(end.y >= selection().top() && end.y <= selection().bottom());
            let dx = f32::from(end.x - start.x).abs();
            let dy = f32::from(end.y - start.y).abs();
            assert!(dx < 0.001 || dy < 0.001 || (dx - dy).abs() < 0.001);
        }
    }

    #[test]
    fn polyline_commits_clicked_nodes_only_and_undoes_as_one_annotation() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Polyline);
        for (x, y) in [(10., 10.), (40., 40.), (10., 70.)] {
            let p = point(px(x), px(y));
            a.begin(p, selection(), false);
            a.drag_to(p, selection(), false);
            a.end();
        }
        a.drag_to(point(px(60.), px(80.)), selection(), false);
        // A toolbar mouse-up has no corresponding canvas mouse-down.
        a.end();
        assert!(a.is_drawing_polyline());
        a.finish_polyline();
        let shape = a.visible().next().unwrap();
        assert_eq!(shape.points.len(), 3);
        assert_eq!(shape.points[2], point(px(10.), px(70.)));
        a.undo();
        assert_eq!(a.visible().count(), 0);
        a.redo();
        assert_eq!(a.visible().next().unwrap().points.len(), 3);
        a.begin(point(px(10.), px(10.)), selection(), false);
        assert!(a.cancel());
        assert_eq!(a.visible().count(), 1);
        a.begin(point(px(10.), px(10.)), selection(), false);
        a.end();
        a.finish_polyline();
        assert_eq!(a.visible().count(), 1);
    }

    #[test]
    fn line_raster_preserves_gaps_and_has_round_caps_at_fractional_scales() {
        for scale in [1., 1.25, 1.73, 2.] {
            let shape = super::Shape {
                number: None,
                text: None,
                kind: super::ShapeKind::Polyline,
                bounds: selection(),
                points: vec![
                    point(px(10.), px(30.)),
                    point(px(60.), px(30.)),
                    point(px(60.), px(60.)),
                ],
                width: 5.,
                color: 0xff0000ff,
            };
            let w = (100. * scale) as u32;
            let mut rgba = [0, 0, 0, 255].repeat((w * w) as usize);
            let at =
                |x: f32, y: f32| (((y * scale) as usize) * w as usize + (x * scale) as usize) * 4;
            let gap = at(40., 30.);
            rgba[gap..gap + 4].fill(0);
            super::line::rasterize(&shape, &mut rgba, w, w, point(px(0.), px(0.)), scale);
            for (x, y) in [(30., 30.), (60., 30.), (60., 50.)] {
                assert_eq!(&rgba[at(x, y)..at(x, y) + 4], &[255, 0, 0, 255]);
            }
            assert!(rgba[at(8., 30.)] > 200, "round cap at scale {scale}");
            assert_eq!(&rgba[gap..gap + 4], &[0; 4]);
            assert_eq!(&rgba[at(30., 40.)..at(30., 40.) + 4], &[0, 0, 0, 255]);
            assert!(
                rgba.as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[0] > 0 && p[0] < 255)
            );
            assert_eq!(shape.line_paths(point(px(0.), px(0.))).len(), 2);
        }
    }
    #[test]
    fn sequence_history_reuses_undone_numbers_and_reset_starts_at_one() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Number);
        for expected in 1..=12 {
            a.begin(point(px(30.), px(30.)), selection(), false);
            a.end();
            assert_eq!(a.visible().last().unwrap().number, Some(expected));
        }
        a.undo();
        assert_eq!(a.next_number(), 12);
        a.redo();
        assert_eq!(a.next_number(), 13);
        a.undo();
        a.begin(point(px(30.), px(30.)), selection(), false);
        a.end();
        a.redo();
        assert_eq!(a.visible().count(), 12);
        a.begin(point(px(30.), px(30.)), selection(), false);
        a.cancel();
        assert_eq!(a.next_number(), 13);
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        assert_eq!(a.next_number(), 13);
        a.reset();
        assert_eq!(a.next_number(), 1);
    }
    #[test]
    fn same_number_placement_repeats_the_largest_badge() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Number);
        // empty canvas: an Alt placement starts the sequence at 1
        a.begin(point(px(30.), px(30.)), selection(), true);
        a.end();
        assert_eq!(a.visible().last().unwrap().number, Some(1));
        // plain placement advances
        a.begin(point(px(30.), px(30.)), selection(), false);
        a.end();
        assert_eq!(a.visible().last().unwrap().number, Some(2));
        // Alt re-placements repeat the largest number on canvas
        a.begin(point(px(30.), px(30.)), selection(), true);
        a.end();
        a.begin(point(px(30.), px(30.)), selection(), true);
        a.end();
        assert_eq!(a.visible().count(), 4);
        assert_eq!(a.visible().last().unwrap().number, Some(2));
        // after the run of 2s the sequence resumes from max + 1
        assert_eq!(a.next_number(), 3);
        // undo drops the repeated badge; the next Alt placement re-reads
        // the CURRENT max, not a remembered "last placed" value
        a.undo();
        a.begin(point(px(30.), px(30.)), selection(), true);
        a.end();
        assert_eq!(a.visible().last().unwrap().number, Some(2));
    }
    #[test]
    fn number_editor_previews_commits_and_cancels() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Number);
        a.begin(point(px(30.), px(30.)), selection(), false);
        a.end(); // badge 1
        let ix = a.committed().len() - 1;
        let before = a.committed()[ix].clone();

        // live preview writes the value with NO history entry — a
        // half-typed buffer must stay outside undo
        a.preview_number(ix, 7);
        assert_eq!(a.committed()[ix].number, Some(7));
        assert_eq!(a.history.len(), 1, "only the Add entry while typing");

        // commit (the overlay's finish path): one Edit entry, undo and
        // redo both restore the exact values
        a.commit_move(ix, before.clone());
        a.undo();
        assert_eq!(a.committed()[ix].number, Some(1));
        a.redo();
        assert_eq!(a.committed()[ix].number, Some(7));

        // cancel: re-previewing the original leaves nothing new to undo
        a.preview_number(ix, 3);
        a.preview_number(ix, 1);
        a.undo(); // undoes the committed Edit, not the cancelled buffer
        assert_eq!(a.committed()[ix].number, Some(1));

        // the wheel floors at 1 (0 is not a badge)
        a.select_index(ix);
        assert!(!a.step_size(false), "1 is the floor");
    }
    #[test]
    fn number_drag_and_size_stay_inside_selection_and_tiny_regions_do_not_count() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Number);
        a.set_tool_size(40.);
        a.begin(point(px(-19.), px(1.)), selection(), false);
        a.drag_to(point(px(300.), px(300.)), selection(), false);
        a.end();
        let mark = a.visible().next().unwrap();
        assert_eq!(mark.bounds.size, size(px(40.), px(40.)));
        assert_eq!(mark.bounds.right(), selection().right());
        assert_eq!(mark.bounds.bottom(), selection().bottom());
        // the number slot is its own — other tools' presets untouched
        assert_eq!(a.size_of(super::ShapeKind::Rectangle), 3.);
        let tiny = Bounds::new(point(px(0.), px(0.)), size(px(10.), px(10.)));
        a.begin(point(px(5.), px(5.)), tiny, false);
        a.end();
        assert_eq!(a.next_number(), 2);
    }
    #[test]
    fn set_color_updates_selected_shape_and_records_undo() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        let orig_color = a.visible().next().unwrap().color;
        assert_eq!(a.selected_index(), Some(0));
        a.set_color(1);
        let new_color = a.visible().next().unwrap().color;
        assert_ne!(new_color, orig_color);
        assert_eq!(new_color, crate::ui::theme::c().annotation_colors[1]);
        a.undo();
        assert_eq!(a.visible().next().unwrap().color, orig_color);
        a.redo();
        assert_eq!(a.visible().next().unwrap().color, new_color);
    }
}
