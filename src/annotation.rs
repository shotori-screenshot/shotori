//! Geometry annotations in desktop logical coordinates, shared by all outputs.
//!
//! The core owns shape vocabulary, tool presets and annotation state. Gesture,
//! selection and history modules own editing; per-tool modules share geometry
//! between live previews, hit testing and export. The raster module composites
//! capture pixels, ordinary marks, erasure, spotlights and magnifier sampling.
mod chrome;
mod filter;
mod magnifier;
mod raster;
mod spotlight;
pub(crate) use magnifier::{LensConnector, LensOption};
pub(crate) use spotlight::SpotlightAdjustment;
mod geometry;
pub(crate) const ROTATION_HANDLE: usize = usize::MAX;
mod parameters;
use parameters::ShapeParameters;
pub(crate) use parameters::{ArrowStyle, EndMarker, FilterArea, LineEnds, LineStyle, NumberFormat};
mod highlighter;
mod line;
pub(crate) use line::StrokePreview;
pub(crate) mod text;
pub(crate) use highlighter::{HighlightBlend, HighlighterCache};
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
    Magnifier,
    Spotlight,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Polyline,
    /// Explicit selection mode retained for custom toolbars and shortcuts.
    /// Selection is also available when no drawing tool is active.
    Select,
}

impl ShapeKind {
    /// Region kinds carry their geometry as a single bounds box
    /// (corner-handle editing); point-cloud kinds as `points`
    /// (endpoint/vertex editing).
    pub(crate) fn is_region(self) -> bool {
        matches!(
            self,
            ShapeKind::Rectangle | ShapeKind::Ellipse | ShapeKind::Magnifier | ShapeKind::Spotlight
        )
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
        ShapeKind::Spotlight => SizeSpec { min: 0., max: 100. },
        ShapeKind::Magnifier => SizeSpec { min: 1., max: 8. },
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
    pub(crate) text_style: text::TextStyle,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) color: u32,
    parameters: ShapeParameters,
    pub(crate) points: Vec<Point<Pixels>>,
}

impl Shape {
    /// Translate every absolute coordinate together for movement and output projection.
    pub(crate) fn translate(&mut self, delta: Point<Pixels>) {
        self.bounds.origin += delta;
        for point in &mut self.points {
            *point += delta;
        }
        self.set_number_tip(self.number_tip().map(|tip| tip + delta));
    }

    pub(crate) fn size(&self) -> f32 {
        self.parameters.size()
    }

    pub(crate) fn line_paths(&self, offset: Point<Pixels>) -> Vec<Path<Pixels>> {
        line::paths(self, offset)
    }

    /// The live freehand draft as union-filled vector paths (see
    /// `line::draft_paths`): painted per frame while drawing, while the
    /// rasterized form only materializes in the selection image on
    /// release.
    pub(crate) fn draft_paths(&self, offset: Point<Pixels>) -> Vec<Path<Pixels>> {
        line::draft_paths(self, offset)
    }

    /// Freehand tools record a point trail instead of bounds; their
    /// preview and rasterization paths are unique (see `StrokePreview`,
    /// `line::coverage`).
    pub(crate) fn is_freehand(&self) -> bool {
        self.kind == ShapeKind::Pencil
            || (self.kind == ShapeKind::Highlighter && !self.highlighter_options().rectangle)
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
        if rx > self.size() && ry > self.size() {
            ellipse_contour(&mut path, center, rx - self.size(), ry - self.size(), -1.);
        }
        path.build().ok()
    }

    /// Inward strokes keep both preview and export within the rectangle.
    pub(crate) fn strokes(&self) -> [Bounds<Pixels>; 4] {
        let b = self.bounds;
        let width = px(self.size())
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

/// Freehand input conditioning: the smoothed ink cursor trails the raw
/// pointer by ≈ this many logical px in steady state, at any speed.
/// Larger = smoother curves but more visible lag.
const INK_SMOOTHING_RADIUS: f32 = 2.;
/// Minimum travel between two recorded freehand points. Pointer events
/// arrive far denser than ink needs; the step also caps the point count
/// of slow scribbles and keeps the capsule union cheap.
const INK_RESAMPLE_STEP: f32 = 1.;

#[derive(Clone)]
struct Draft {
    pub(super) start: Point<Pixels>,
    pub(super) shape: Shape,
    /// Smoothed ink cursor for freehand tools (Pencil/Highlighter): a
    /// distance-adaptive exponential average of the raw pointer trail.
    /// Raw event positions quantize to the event grid and chain into
    /// visible corners and stair-steps; the EMA rounds them into a
    /// continuous curve. Committed points ARE the smoothed cursor, so
    /// preview, export and hit-testing all share one geometry.
    pub(super) ink: Point<Pixels>,
    pub(super) straight_anchor: Option<usize>,
    pub(super) repeat_number: bool,
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
    spotlight_opacity: f32,
    spotlight_ellipse: bool,
    spotlight_outline: spotlight::SpotlightOutline,
    lens: magnifier::Lens,
    line_style: LineStyle,
    arrow_style: ArrowStyle,
    line_ends: LineEnds,
    highlighter: highlighter::HighlighterOptions,
    text_style: text::TextStyle,
    pencil_smooth: bool,
    pencil_angle: u16,
    geometry_fill: bool,
    geometry_radius: f32,
    number_format: NumberFormat,
    number_arrow_style: ArrowStyle,
    number_override: Option<u32>,
    renumber_on_delete: bool,
    filter_area: FilterArea,
    filter_brush_size: f32,
    filter_edit_brush_size: bool,
    highlighter_color_ix: usize,
    shapes: Vec<Shape>,
    history: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    draft: Option<Draft>,
    draft_generation: u64,
    pressed: bool,
    /// Pointer state accompanying the editable erase draft.
    eraser: Option<EraserGesture>,
    /// Selected indices in selection order; the last mark owns parameter edits.
    /// Handles are available only for a single selection.
    selected: Vec<usize>,
    /// Select newly placed marks for immediate editing when enabled in settings.
    auto_select: bool,
    text_original: Option<(usize, Shape)>,
    /// A slider size-drag is in flight: consecutive `apply_size` calls
    /// merge into one history entry (see `apply_size`).
    size_drag_active: bool,
    resize_bounds: Option<Bounds<Pixels>>,
}

impl Default for Annotations {
    fn default() -> Self {
        Self {
            tool: None,
            color_ix: 0,
            spotlight_opacity: 55.,
            spotlight_ellipse: false,
            spotlight_outline: Default::default(),
            lens: magnifier::Lens::default(),
            line_style: LineStyle::Solid,
            arrow_style: ArrowStyle::default(),
            line_ends: LineEnds::default(),
            highlighter: Default::default(),
            text_style: Default::default(),
            pencil_smooth: true,
            pencil_angle: 0,
            geometry_fill: false,
            geometry_radius: 0.,
            number_format: NumberFormat::default(),
            number_arrow_style: ArrowStyle::default(),
            number_override: None,
            renumber_on_delete: false,
            filter_area: FilterArea::Rectangle,
            filter_brush_size: 32.,
            filter_edit_brush_size: false,
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
            selected: Vec::new(),
            auto_select: false,
            text_original: None,
            size_drag_active: false,
            resize_bounds: None,
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

    /// A tool that acts on canvas gestures (draw, erase) is active.
    /// The select tool is deliberately NOT "enabled": it has no
    /// gesture of its own — its pointer routing must fall through to
    /// region editing exactly like having no tool at all.
    pub(crate) fn enabled(&self) -> bool {
        self.tool.is_some_and(|k| k != ShapeKind::Select)
    }
    /// The swatch-board index the ACTIVE tool would paint with —
    /// highlighter keeps its own slot. The toolbar's inline color
    /// editor (the picker replacing the selected swatch) binds to
    /// this index.
    pub(crate) fn color_index(&self) -> usize {
        if self.tool == Some(ShapeKind::Highlighter) {
            self.highlighter_color_ix
        } else {
            self.color_ix
        }
    }

    pub(crate) fn color(&self) -> (u32, &'static str) {
        if let Some(shape) = self.editing_text() {
            return (shape.color, "Text");
        }
        let ix = self.color_index();
        (
            crate::model::settings::current().annotation_colors()[ix],
            crate::model::settings::PALETTE_NAMES[ix],
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
            ShapeKind::Magnifier => self.lens.zoom,
            ShapeKind::Spotlight => self.spotlight_opacity,
            ShapeKind::Eraser => self.preset.eraser,
            ShapeKind::EraserRect => self.preset.eraser_rect,
            ShapeKind::Number => self.preset.number,
            ShapeKind::Text => self.preset.text,
            // unreachable in practice (edit_kind filters it): a mode
            // has no size to remember
            ShapeKind::Select => 0.,
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
            ShapeKind::Magnifier => &mut self.lens.zoom,
            ShapeKind::Spotlight => &mut self.spotlight_opacity,
            ShapeKind::Eraser => &mut self.preset.eraser,
            ShapeKind::EraserRect => &mut self.preset.eraser_rect,
            ShapeKind::Number => &mut self.preset.number,
            ShapeKind::Text => &mut self.preset.text,
            // unreachable in practice (edit_kind filters it): nothing
            // to remember for a mode
            ShapeKind::Select => return,
        };
        *slot = v;
    }
    /// The active tool's drawing width (what a fresh stroke takes).
    pub(crate) fn width(&self) -> f32 {
        self.tool.map_or(3., |t| self.size_of(t))
    }
    pub(crate) fn text_size(&self) -> f32 {
        self.editing_text()
            .or_else(|| {
                self.selected()
                    .filter(|shape| shape.kind == ShapeKind::Text)
            })
            .map_or(self.preset.text, |shape| shape.size())
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
    /// A lens or multiply blend needs captured pixels, not a transparent vector overlay.
    pub(crate) fn composite_draft(&self) -> bool {
        self.draft_shape().is_some_and(|draft| {
            draft.needs_composited_ink()
                || self.shapes.iter().any(|s| s.kind == ShapeKind::Magnifier)
        })
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
            straight_anchor: None,
            repeat_number: false,
            start: bounds.origin,
            ink: bounds.origin,
            shape: Shape {
                kind: ShapeKind::Text,
                number: None,
                text: Some(value),
                text_style: self.text_style.clone(),
                bounds,
                color,
                parameters: crate::annotation::ShapeParameters::new(ShapeKind::Text, width),
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
            text_style: self.text_style.clone(),
            bounds,
            color: self.color().0,
            parameters: crate::annotation::ShapeParameters::new(ShapeKind::Text, self.text_size()),
            points: Vec::new(),
        });
    }
    pub(crate) fn number_size(&self) -> f32 {
        self.preset.number
    }
    pub(crate) fn next_number(&self) -> u32 {
        if let Some(value) = self.number_override {
            return value;
        }
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

    /// Production constructor: placement preferences come from the
    /// persisted settings, read once per capture (the overlay session
    /// is rebuilt for every screenshot; there is no live reload).
    /// Tests build `default()` instead — factory behavior, no global
    /// state, no cross-test races.
    pub(crate) fn from_settings() -> Self {
        Self {
            auto_select: crate::model::settings::current().annotation_auto_select(),
            renumber_on_delete: crate::model::settings::current().annotation_renumber_on_delete(),
            text_style: text::TextStyle {
                wrap: crate::model::settings::current().annotation_text_wrap(),
                ..Default::default()
            },
            ..Self::default()
        }
    }

    pub(crate) fn toggle(&mut self, kind: ShapeKind) {
        self.draft = None;
        self.pressed = false;
        self.selected.clear();
        // a keyboard shortcut can switch tools mid-gesture; the sweep
        // must stop with it (a stale gesture would keep erasing under
        // the next tool's pointer moves)
        self.eraser = None;
        self.tool = if self.tool == Some(kind) {
            None
        } else {
            Some(kind)
        };
    }

    pub(crate) fn set_color(&mut self, ix: usize) {
        if ix >= crate::model::settings::PALETTE {
            return;
        }
        if self.editing_text().is_some() {
            self.color_ix = ix;
        }
        if let Some(shape) = self.editing_text_mut() {
            shape.color = crate::model::settings::current().annotation_colors()[ix];
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
                let color = crate::model::settings::current().annotation_colors()[ix];
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
        if self.editing_filter_brush_size() {
            let current = self.current_edit_size();
            let spec = self.current_size_spec();
            let next = (current + if up { 1. } else { -1. }).clamp(spec.min, spec.max);
            if next == current {
                return false;
            }
            self.end_size_drag();
            self.apply_size(next);
            self.end_size_drag();
            return true;
        }
        // invariant: every index in `selected` is valid — every path
        // that mutates `shapes` (undo/redo/reset/record_add) or leaves
        // edit mode (begin/toggle) clears the list
        debug_assert!(
            self.selected
                .iter()
                .all(|ix| self.shapes.get(*ix).is_some())
        );
        if let Some(ix) = self.selected_index() {
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
        self.number_override = None;
        self.text_original = None;
        self.history.clear();
        self.redo.clear();
        self.draft = None;
        self.pressed = false;
        self.eraser = None;
        self.tool = None;
        self.selected.clear();
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
        self.selected = vec![ix];
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
            let edited = std::mem::replace(slot, original.clone());
            if commit {
                if edited.text.as_deref().is_none_or(|s| s.trim().is_empty()) {
                    self.delete_shape(ix);
                } else {
                    self.shapes[ix] = edited;
                    self.commit_move(ix, original);
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
        self.selected.clear();
        self.text_original = None;
        self.size_drag_active = false;
        self.eraser = None;
        self.history.push(HistoryEntry::RemoveAll {
            shapes: std::mem::take(&mut self.shapes),
        });
        self.redo.clear();
        true
    }

    pub(crate) fn committed(&self) -> &[Shape] {
        &self.shapes
    }

    /// Lens selection paints a cheap outline until release, without resampling the crop.
    pub(crate) fn raster_draft_shape(&self) -> Option<&Shape> {
        self.draft_shape()
            .filter(|s| s.kind != ShapeKind::Magnifier)
    }

    pub(crate) fn draft_shape(&self) -> Option<&Shape> {
        self.draft.as_ref().map(|draft| &draft.shape)
    }
}

mod eraser;
mod gesture;
mod history;

use eraser::EraserGesture;
use gesture::distance;
pub(crate) use history::HistoryEffect;
use history::HistoryEntry;
/// Use the same silhouette for toolbar previews and captured marks.
pub(crate) fn arrow_preview_geometry(style: ArrowStyle) -> Vec<Vec<Point<Pixels>>> {
    line::arrow_geometry(
        &[point(px(4.), px(12.)), point(px(32.), px(12.))],
        1.6,
        style,
        LineStyle::Solid,
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
                (ShapeKind::Rectangle, 12),
                (ShapeKind::Line, 3),
                (ShapeKind::Ellipse, 9),
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

        // Since the select-tool flip a placed shape is NOT selected
        // and the rectangle tool advertises nothing; enter the select
        // tool and the unselected bottom shape reads as a pick.
        // Blank canvas offers nothing.
        a.toggle(ShapeKind::Rectangle); // off
        a.toggle(ShapeKind::Select);
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

        // The selected frame owns overlapping content until it is deselected.
        assert_eq!(
            a.shape_hover(point(px(30.), px(10.))),
            Some(ShapeHover::Move)
        );
    }

    #[test]
    fn auto_select_option_selects_the_fresh_mark() {
        // The pure model can disable auto-selection independently of settings.
        let mut a = Annotations::default();
        a.toggle(ShapeKind::Rectangle);
        rectangle(&mut a);
        assert_eq!(a.selected_index(), None);

        // Enabled selection makes the fresh mark ready for wheel/drag tuning.
        let mut a = Annotations {
            auto_select: true,
            ..Annotations::default()
        };
        a.toggle(ShapeKind::Rectangle);
        rectangle(&mut a);
        assert_eq!(a.selected().map(|s| s.kind), Some(ShapeKind::Rectangle));

        // The next stroke takes the selection with it (begin drops it)
        // and hands it back on release.
        a.begin(point(px(50.), px(50.)), selection(), false);
        assert_eq!(a.selected_index(), None);
        a.drag_to(point(px(70.), px(70.)), selection(), false);
        a.end();
        assert_eq!(a.selected_index(), Some(1));
    }

    #[test]
    fn wheel_edits_the_selection_and_adopts_it_as_the_preset() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a); // committed at width 3 (M)
        assert!(click(&mut a, point(px(10.), px(25.))));

        assert!(a.step_size(true)); // 3 → 4 on the SHAPE
        assert_eq!(a.selected().map(|s| s.size()), Some(4.));
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
        assert_eq!(a.visible().next().unwrap().size(), 4.);

        a.undo(); // Edit reversed
        assert_eq!(a.visible().next().unwrap().size(), 3.);
        a.undo(); // Add reversed — shape leaves
        assert_eq!(a.visible().count(), 0);
        a.redo(); // Add replayed
        assert_eq!(a.visible().next().unwrap().size(), 3.);
        a.redo(); // Edit replayed
        assert_eq!(a.visible().next().unwrap().size(), 4.);
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
        assert_eq!(a.selected().map(|s| s.size()), Some(4.));

        // Number value buttons edit independently of wheel/slider diameter;
        // undo restores. (This badge is #2 in the sequence: the pencil
        // stroke placed first is not numbered, `next_number` starts at 1.)
        a.toggle(super::ShapeKind::Number);
        a.begin(point(px(30.), px(50.)), selection(), false);
        a.end();
        assert!(click(&mut a, point(px(30.), px(50.))));
        assert_eq!(a.selected().map(|s| s.kind), Some(super::ShapeKind::Number));
        assert!(a.hit_test(point(px(30.), px(70.))).is_none());
        let diameter = f32::from(a.selected().unwrap().bounds.size.width);
        assert!(a.step_selected_number(a.selected_index().unwrap(), true)); // value 1 → 2
        assert_eq!(a.selected().and_then(|s| s.number), Some(2));
        assert_eq!(
            f32::from(a.selected().unwrap().bounds.size.width),
            diameter,
            "value edits preserve badge diameter"
        );
        assert!(a.step_selected_number(a.selected_index().unwrap(), false)); // back to 1
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
        assert_eq!(a.selected().map(|s| s.size()), Some(25.));

        // mosaic: solid-bounds hit; wheel steps filter strength (16 → 17)
        a.toggle(super::ShapeKind::Mosaic);
        a.begin(point(px(-10.), px(10.)), selection(), false);
        a.drag_to(point(px(20.), px(40.)), selection(), false);
        a.end();
        assert!(click(&mut a, point(px(5.), px(25.))));
        assert_eq!(a.selected().map(|s| s.kind), Some(super::ShapeKind::Mosaic));
        assert!(a.step_size(true));
        assert_eq!(a.selected().map(|s| s.size()), Some(17.));
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
    fn slider_drag_edits_the_selection_as_one_history_entry() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a); // width 3
        a.toggle(super::ShapeKind::Rectangle); // off
        a.toggle(super::ShapeKind::Select);
        assert!(a.select_index(0)); // pick it with the select tool
        assert!(a.selected().is_some());

        // a whole drag: many Change values, one merged Edit
        for v in [4., 6., 9., 12.] {
            a.apply_size(v);
        }
        assert_eq!(a.selected().map(|s| s.size()), Some(12.));
        a.end_size_drag();
        a.undo(); // ONE undo restores the pre-drag width
        assert_eq!(a.committed()[0].size(), 3.);

        // without a selection, the same write targets the tool preset
        let _ = a.cancel();
        let _ = a.cancel(); // deselect, then leave the tool
        a.toggle(super::ShapeKind::Rectangle);
        a.apply_size(7.);
        a.end_size_drag();
        assert_eq!(a.width(), 7.);
        assert_eq!(a.committed()[0].size(), 3.); // shape untouched
    }

    #[test]
    fn size_edits_carry_to_the_next_stroke_and_stay_per_tool() {
        let mut a = Annotations::default();

        // thicken the pencil's next stroke by editing its placed mark
        a.toggle(super::ShapeKind::Pencil);
        a.begin(point(px(5.), px(5.)), selection(), false);
        a.drag_to(point(px(60.), px(50.)), selection(), false);
        a.end(); // placed (no auto-select since the flip); the tool
        // preset is the slider's target while it stays active
        a.apply_size(9.);
        a.end_size_drag();
        a.deselect();
        a.begin(point(px(5.), px(5.)), selection(), false); // next pencil stroke
        assert_eq!(a.draft_shape().unwrap().size(), 9.);

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
        assert_eq!(mark.color & 255, 255);
        assert_eq!(mark.size(), 32.);
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
            // Repeating a sample converges: the smoothed cursor is
            // already within a resample step of the recorded tail.
            assert!(!a.drag_to(*p, selection(), false));
        }
        a.end();
        assert!(!a.drag_to(point(px(70.), px(70.)), selection(), false));
        // The stroke is one history entry whose smoothed points hug the
        // raw polyline — within the smoothing trail of every sample.
        let stroke_points = a.visible().next().unwrap().points.clone();
        assert_eq!(stroke_points.len(), points.len());
        for (recorded, raw) in stroke_points.iter().zip(points) {
            let dx = f32::from(recorded.x - raw.x);
            let dy = f32::from(recorded.y - raw.y);
            assert!(dx.hypot(dy) <= super::INK_SMOOTHING_RADIUS + super::INK_RESAMPLE_STEP);
        }
        a.undo();
        assert_eq!(a.visible().count(), 0);
        a.redo();
        assert_eq!(a.visible().next().unwrap().points, stroke_points);
        // A flick into the selection edge keeps the tip at the release
        // position, within the smoothing trail.
        a.begin(points[0], selection(), false);
        a.drag_to(point(px(200.), px(-40.)), selection(), false);
        let tip = *a.visible().last().unwrap().points.last().unwrap();
        let dx = f32::from(tip.x - px(80.));
        let dy = f32::from(tip.y - px(0.));
        assert!(dx.hypot(dy) <= super::INK_SMOOTHING_RADIUS + super::INK_RESAMPLE_STEP);
        a.cancel();
        assert_eq!(a.visible().count(), 1);
    }

    #[test]
    fn freehand_smoothing_damps_tremor_and_trails_fast_flicks() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Pencil);
        a.begin(point(px(50.), px(50.)), selection(), false);
        // Sub-pixel tremor around one spot never becomes ink.
        for i in 0..50 {
            let jitter = if i % 2 == 0 { 0.4 } else { -0.4 };
            assert!(!a.drag_to(
                point(px(50. + jitter), px(50. + jitter)),
                selection(),
                false
            ));
        }
        assert_eq!(a.visible().last().unwrap().points.len(), 1);
        // A fast flick records points that approach the pointer but
        // never overshoot it, and repeated samples converge quickly.
        assert!(a.drag_to(point(px(75.), px(50.)), selection(), false));
        let mut repeats = 0;
        while a.drag_to(point(px(75.), px(50.)), selection(), false) {
            repeats += 1;
            assert!(repeats < 100, "smoothing cursor must converge");
        }
        let tail = a.visible().last().unwrap().points.clone();
        for p in &tail {
            assert!(f32::from(p.x) >= 50. && f32::from(p.x) <= 75.);
            assert!((f32::from(p.y) - 50.).abs() <= super::INK_SMOOTHING_RADIUS);
        }
        let tip = tail.last().unwrap();
        assert!(f32::from(px(75.) - tip.x) <= super::INK_SMOOTHING_RADIUS);
    }

    #[test]
    fn freehand_smoothing_keeps_ink_inside_the_selection() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Highlighter);
        a.begin(point(px(5.), px(5.)), selection(), false);
        for i in 0..20 {
            let x = 5. + i as f32 * 12.;
            let y = if i % 3 == 0 { 200. } else { -30. };
            a.drag_to(point(px(x), px(y)), selection(), false);
        }
        let selection = selection();
        for p in &a.visible().last().unwrap().points {
            // The clamp edge itself is fair game (f32 saturation can
            // touch it); the ink must never step PAST the selection.
            assert!(f32::from(p.x) >= f32::from(selection.left()));
            assert!(f32::from(p.x) <= f32::from(selection.right()));
            assert!(f32::from(p.y) >= f32::from(selection.top()));
            assert!(f32::from(p.y) <= f32::from(selection.bottom()));
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
            let end = super::gesture::line_endpoint(start, point(px(x), px(y)), selection(), true);
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
        assert!(!a.step_selected_number(ix, false), "1 is the floor");
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
        assert_eq!(mark.bounds.left(), selection().left());
        assert_eq!(mark.bounds.top(), selection().top());
        assert!(mark.number_arrow().is_some());
        assert!(mark.ink_bounds().right() <= selection().right());
        assert!(mark.ink_bounds().bottom() <= selection().bottom());
        // the number slot is its own — other tools' presets untouched
        assert_eq!(a.size_of(super::ShapeKind::Rectangle), 3.);
        let tiny = Bounds::new(point(px(0.), px(0.)), size(px(10.), px(10.)));
        a.begin(point(px(5.), px(5.)), tiny, false);
        a.end();
        assert_eq!(a.next_number(), 2);
    }
    #[test]
    fn placed_marks_do_not_select_themselves() {
        // The 2026-10-06 select-tool flip: placing a mark leaves the
        // selection empty (record_add drops it — indices shift), and
        // picking it back up is the select tool's job.
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        assert!(a.selected_index().is_none());
        a.toggle(super::ShapeKind::Rectangle);
        a.toggle(super::ShapeKind::Select);
        assert!(click(&mut a, point(px(12.), px(25.)))); // on the left edge band
    }

    #[test]
    fn escape_exits_the_select_tool_like_any_other() {
        // The select tool is not `enabled()` (no canvas gesture), but
        // Esc must still leave it — cancel() drops ANY lit button.
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Select);
        assert!(!a.enabled());
        assert_eq!(a.tool(), Some(super::ShapeKind::Select));
        assert!(a.cancel());
        assert_eq!(a.tool(), None);
    }

    #[test]
    fn set_color_updates_selected_shape_and_records_undo() {
        let mut a = Annotations::default();
        a.toggle(super::ShapeKind::Rectangle);
        rectangle(&mut a);
        // placing no longer selects — pick it with the select tool
        a.toggle(super::ShapeKind::Rectangle);
        a.toggle(super::ShapeKind::Select);
        assert!(a.select_index(0));
        let orig_color = a.visible().next().unwrap().color;
        assert_eq!(a.selected_index(), Some(0));
        a.set_color(1);
        let new_color = a.visible().next().unwrap().color;
        assert_ne!(new_color, orig_color);
        assert_eq!(
            new_color,
            crate::model::settings::current().annotation_colors()[1]
        );
        a.undo();
        assert_eq!(a.visible().next().unwrap().color, orig_color);
        a.redo();
        assert_eq!(a.visible().next().unwrap().color, new_color);
    }
}
