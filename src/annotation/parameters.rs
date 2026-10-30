//! Typed dimensions for marks; filter intensity is independent of brush geometry.
use super::{Annotations, Shape, ShapeKind, SizeSpec};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    DashDot,
    DashDotDot,
}

/// Arrow silhouettes are independent of the shaft's dash pattern.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ArrowStyle {
    #[default]
    Open,
    DoubleOpen,
    Outline,
    Filled,
    Dimension,
    Triangle,
    DoubleTriangle,
}

impl ArrowStyle {
    pub(crate) fn supports_line_style(self) -> bool {
        self != Self::Filled
    }
}

/// Endpoint markers are independent at either end of a line or polyline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EndMarker {
    #[default]
    None,
    Open,
    Triangle,
    Bar,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LineEnds {
    pub(crate) start: EndMarker,
    pub(crate) end: EndMarker,
    pub(crate) square_caps: bool,
    pub(crate) sharp_joins: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FilterArea {
    Rectangle,
    Brush { diameter: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ShapeParameters {
    Magnifier(super::magnifier::Lens),
    Geometry {
        width: f32,
        fill: bool,
        radius: f32,
        transform: super::geometry::GeometryTransform,
        style: LineStyle,
    },
    Spotlight {
        opacity: f32,
        ellipse: bool,
        outline: super::spotlight::SpotlightOutline,
    },
    Stroke {
        width: f32,
        style: LineStyle,
        arrow: ArrowStyle,
        ends: LineEnds,
    },
    Highlighter {
        width: f32,
        options: super::highlighter::HighlighterOptions,
    },
    Text {
        font_size: f32,
    },
    Badge {
        diameter: f32,
        format: NumberFormat,
        arrow: ArrowStyle,
        tip: Option<gpui_kit::Point<gpui_kit::Pixels>>,
    },
    Filter {
        strength: f32,
        area: FilterArea,
    },
}

impl ShapeParameters {
    pub(super) fn new(kind: ShapeKind, value: f32) -> Self {
        match kind {
            ShapeKind::Rectangle | ShapeKind::Ellipse => Self::Geometry {
                width: value,
                fill: false,
                radius: 0.,
                transform: Default::default(),
                style: LineStyle::Solid,
            },
            ShapeKind::Spotlight => Self::Spotlight {
                opacity: value,
                ellipse: false,
                outline: Default::default(),
            },
            ShapeKind::Magnifier => Self::Magnifier(super::magnifier::Lens::default()),
            ShapeKind::Highlighter => Self::Highlighter {
                width: value,
                options: Default::default(),
            },
            ShapeKind::Text => Self::Text { font_size: value },
            ShapeKind::Number => Self::Badge {
                diameter: value,
                format: NumberFormat::Decimal,
                arrow: ArrowStyle::default(),
                tip: None,
            },
            ShapeKind::Mosaic | ShapeKind::Blur => Self::Filter {
                strength: value,
                area: FilterArea::Rectangle,
            },
            _ => Self::Stroke {
                width: value,
                style: LineStyle::Solid,
                arrow: ArrowStyle::default(),
                ends: LineEnds::default(),
            },
        }
    }

    pub(super) fn size(self) -> f32 {
        match self {
            Self::Magnifier(lens) => lens.zoom,
            Self::Spotlight { opacity, .. } => opacity,
            Self::Stroke { width, .. }
            | Self::Geometry { width, .. }
            | Self::Highlighter { width, .. } => width,
            Self::Text { font_size } => font_size,
            Self::Badge { diameter, .. } => diameter,
            Self::Filter { strength, .. } => strength,
        }
    }

    pub(super) fn set_size(&mut self, value: f32) {
        match self {
            Self::Magnifier(lens) => lens.zoom = value,
            Self::Spotlight { opacity, .. } => *opacity = value,
            Self::Stroke { width, .. }
            | Self::Geometry { width, .. }
            | Self::Highlighter { width, .. } => *width = value,
            Self::Text { font_size } => *font_size = value,
            Self::Badge { diameter, .. } => *diameter = value,
            Self::Filter { strength, .. } => *strength = value,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum NumberFormat {
    #[default]
    Decimal,
    Alphabetic,
    Roman,
}

impl NumberFormat {
    pub(crate) fn label(self, value: u32) -> String {
        if value == 0 {
            return "0".into();
        }
        let mut value = value;
        match self {
            Self::Decimal => value.to_string(),
            Self::Alphabetic => {
                let mut letters = Vec::new();
                while value > 0 {
                    value -= 1;
                    letters.push(char::from(b'A' + (value % 26) as u8));
                    value /= 26;
                }
                letters.into_iter().rev().collect()
            }
            Self::Roman => {
                // Classical notation has no unambiguous compact form beyond 3999.
                if value > 3999 {
                    return value.to_string();
                }
                let mut label = String::new();
                for (unit, numeral) in [
                    (1000, "M"),
                    (900, "CM"),
                    (500, "D"),
                    (400, "CD"),
                    (100, "C"),
                    (90, "XC"),
                    (50, "L"),
                    (40, "XL"),
                    (10, "X"),
                    (9, "IX"),
                    (5, "V"),
                    (4, "IV"),
                    (1, "I"),
                ] {
                    while value >= unit {
                        label.push_str(numeral);
                        value -= unit;
                    }
                }
                label
            }
        }
    }
}

impl Shape {
    pub(crate) fn number_format(&self) -> NumberFormat {
        match self.parameters {
            ShapeParameters::Badge { format, .. } => format,
            _ => NumberFormat::default(),
        }
    }

    pub(crate) fn line_ends(&self) -> LineEnds {
        match self.parameters {
            ShapeParameters::Stroke { ends, .. } => ends,
            _ => LineEnds::default(),
        }
    }

    pub(crate) fn arrow_style(&self) -> ArrowStyle {
        match self.parameters {
            ShapeParameters::Stroke { arrow, .. } | ShapeParameters::Badge { arrow, .. } => arrow,
            _ => ArrowStyle::default(),
        }
    }

    pub(crate) fn line_style(&self) -> LineStyle {
        match self.parameters {
            ShapeParameters::Stroke { style, .. } | ShapeParameters::Geometry { style, .. } => {
                style
            }
            _ => LineStyle::Solid,
        }
    }

    pub(crate) fn filter_area(&self) -> Option<FilterArea> {
        match self.parameters {
            ShapeParameters::Filter { area, .. } => Some(area),
            _ => None,
        }
    }
}

impl Annotations {
    pub(crate) fn line_ends(&self) -> LineEnds {
        self.selected()
            .map(Shape::line_ends)
            .unwrap_or(self.line_ends)
    }

    pub(crate) fn set_line_ends(&mut self, value: LineEnds) {
        self.line_ends = value;
        self.end_size_drag();
        if let Some(ix) = self.selected_index() {
            let before = self.shapes[ix].clone();
            if matches!(before.kind, ShapeKind::Line | ShapeKind::Polyline)
                && let ShapeParameters::Stroke { ends, .. } = &mut self.shapes[ix].parameters
            {
                *ends = value;
            }
            self.commit_move(ix, before);
        }
    }

    pub(crate) fn pencil_smooth(&self) -> bool {
        self.pencil_smooth
    }
    pub(crate) fn set_pencil_smooth(&mut self, value: bool) {
        self.pencil_smooth = value;
    }
    pub(crate) fn pencil_angle(&self) -> u16 {
        self.pencil_angle
    }
    pub(crate) fn set_pencil_angle(&mut self, value: u16) {
        if [0, 5, 10, 15, 30, 45].contains(&value) {
            self.pencil_angle = value;
        }
    }

    pub(crate) fn arrow_style(&self) -> ArrowStyle {
        self.selected().map(Shape::arrow_style).unwrap_or(
            if self.edit_kind() == Some(ShapeKind::Number) {
                self.number_arrow_style
            } else {
                self.arrow_style
            },
        )
    }

    pub(crate) fn set_arrow_style(&mut self, value: ArrowStyle) {
        if self.edit_kind() == Some(ShapeKind::Number) {
            self.number_arrow_style = value;
        } else {
            self.arrow_style = value;
        }
        self.end_size_drag();
        if let Some(ix) = self.selected_index()
            && matches!(self.shapes[ix].kind, ShapeKind::Arrow | ShapeKind::Number)
        {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Stroke { arrow, .. } | ShapeParameters::Badge { arrow, .. } =
                &mut self.shapes[ix].parameters
            {
                *arrow = value;
            }
            if let Some(area) = self.resize_bounds
                && let Some(tip) = self.shapes[ix].number_tip()
            {
                self.shapes[ix].drag_number_tip(tip, area.union(&before.ink_bounds()));
            }
            self.commit_move(ix, before);
        }
    }

    pub(crate) fn line_style(&self) -> LineStyle {
        self.selected()
            .map(Shape::line_style)
            .unwrap_or(self.line_style)
    }

    pub(crate) fn set_line_style(&mut self, value: LineStyle) {
        self.line_style = value;
        self.end_size_drag();
        if let Some(ix) = self.selected_index()
            && matches!(
                self.shapes[ix].kind,
                ShapeKind::Line
                    | ShapeKind::Polyline
                    | ShapeKind::Arrow
                    | ShapeKind::Rectangle
                    | ShapeKind::Ellipse
            )
        {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Stroke { style, .. } | ShapeParameters::Geometry { style, .. } =
                &mut self.shapes[ix].parameters
            {
                *style = value;
            }
            self.commit_move(ix, before);
        }
    }

    pub(crate) fn spotlight_ellipse(&self) -> bool {
        match self.selected().map(|s| s.parameters) {
            Some(ShapeParameters::Spotlight { ellipse, .. }) => ellipse,
            _ => self.spotlight_ellipse,
        }
    }

    pub(crate) fn set_spotlight_ellipse(&mut self, enabled: bool) {
        self.spotlight_ellipse = enabled;
        self.end_size_drag();
        if let Some(ix) = self.selected_index() {
            let before = self.shapes[ix].clone();
            if let ShapeParameters::Spotlight { ellipse, .. } = &mut self.shapes[ix].parameters {
                *ellipse = enabled;
            }
            self.commit_move(ix, before);
        }
    }

    /// Change the filter effect without discarding a selected region or brush path.
    pub(crate) fn set_filter_effect(&mut self, kind: ShapeKind) {
        if !matches!(kind, ShapeKind::Mosaic | ShapeKind::Blur) {
            return;
        }
        self.end_size_drag();
        if let Some(ix) = self.selected_index()
            && self.shapes[ix].filter_area().is_some()
        {
            let before = self.shapes[ix].clone();
            self.shapes[ix].kind = kind;
            self.commit_move(ix, before);
            self.tool = Some(kind);
        } else if self.tool != Some(kind) {
            self.toggle(kind);
        }
    }

    pub(crate) fn filter_area(&self) -> FilterArea {
        self.selected()
            .and_then(Shape::filter_area)
            .unwrap_or(self.filter_area)
    }

    pub(crate) fn set_filter_area(&mut self, brush: bool) {
        let Some(kind @ (ShapeKind::Mosaic | ShapeKind::Blur)) = self.edit_kind() else {
            return;
        };
        if matches!(self.filter_area(), FilterArea::Brush { .. }) == brush {
            return;
        }
        self.finish_interaction();
        self.tool = Some(kind);
        self.filter_area = if brush {
            FilterArea::Brush {
                diameter: self.filter_brush_size,
            }
        } else {
            FilterArea::Rectangle
        };
        self.filter_edit_brush_size = brush;
    }

    pub(crate) fn editing_filter_brush_size(&self) -> bool {
        matches!(self.edit_kind(), Some(ShapeKind::Mosaic | ShapeKind::Blur))
            && matches!(self.filter_area(), FilterArea::Brush { .. })
            && self.filter_edit_brush_size
    }

    pub(crate) fn edit_filter_brush_size(&mut self, enabled: bool) {
        self.end_size_drag();
        self.filter_edit_brush_size = enabled;
    }

    pub(crate) fn current_size_spec(&self) -> SizeSpec {
        if self.editing_filter_brush_size() {
            SizeSpec { min: 8., max: 96. }
        } else {
            super::size_spec(self.edit_kind().unwrap_or(ShapeKind::Pencil))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn arrow_choices_keep_selection_history_and_future_defaults() {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Arrow);
        marks.set_arrow_style(ArrowStyle::DoubleOpen);
        marks.set_line_style(LineStyle::DashDot);
        let area = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.)));
        marks.begin(point(px(20.), px(40.)), area, false);
        marks.drag_to(point(px(120.), px(40.)), area, false);
        marks.end();
        marks.select_index(0);
        assert_eq!(marks.shapes[0].arrow_style(), ArrowStyle::DoubleOpen);
        marks.set_arrow_style(ArrowStyle::Filled);
        assert_eq!(marks.selected_index(), Some(0));
        assert_eq!(marks.line_style(), LineStyle::DashDot);
        marks.undo();
        assert_eq!(marks.shapes[0].arrow_style(), ArrowStyle::DoubleOpen);
        marks.redo();
        assert_eq!(marks.shapes[0].arrow_style(), ArrowStyle::Filled);
        marks.begin(point(px(20.), px(80.)), area, false);
        marks.drag_to(point(px(120.), px(80.)), area, false);
        marks.end();
        assert_eq!(marks.shapes[1].arrow_style(), ArrowStyle::Filled);
    }

    #[test]
    fn line_style_applies_to_new_marks_and_selected_edits_are_undoable() {
        for kind in [ShapeKind::Line, ShapeKind::Polyline, ShapeKind::Arrow] {
            let mut marks = Annotations::default();
            marks.toggle(kind);
            marks.set_line_style(LineStyle::Dashed);
            let area = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.)));
            marks.begin(point(px(20.), px(40.)), area, false);
            marks.drag_to(point(px(120.), px(40.)), area, false);
            marks.end();
            if kind == ShapeKind::Polyline {
                marks.begin(point(px(120.), px(40.)), area, false);
                marks.end();
                marks.finish_polyline();
            }
            assert_eq!(marks.shapes[0].line_style(), LineStyle::Dashed);
            marks.select_index(0);
            marks.set_line_style(LineStyle::Dotted);
            assert_eq!(marks.shapes[0].line_style(), LineStyle::Dotted);
            marks.undo();
            assert_eq!(marks.shapes[0].line_style(), LineStyle::Dashed);
            marks.redo();
            assert_eq!(marks.shapes[0].line_style(), LineStyle::Dotted);
        }
    }

    #[test]
    fn filter_diameter_strength_and_selected_edits_are_independent() {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Mosaic);
        marks.set_filter_area(true);
        marks.apply_size(24.);
        marks.edit_filter_brush_size(false);
        marks.apply_size(8.);
        let selection = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.)));
        marks.begin(point(px(40.), px(40.)), selection, false);
        marks.drag_to(point(px(120.), px(40.)), selection, false);
        marks.end();
        assert_eq!(marks.shapes.len(), 1);
        assert_eq!(marks.shapes[0].size(), 8.);
        assert_eq!(
            marks.shapes[0].filter_area(),
            Some(FilterArea::Brush { diameter: 24. })
        );
        assert_eq!(marks.hit_test(point(px(80.), px(40.))), Some(0));
        assert_eq!(marks.hit_test(point(px(80.), px(80.))), None);
        marks.select_index(0);
        marks.edit_filter_brush_size(true);
        marks.apply_size(32.);
        marks.apply_size(40.);
        marks.end_size_drag();
        assert_eq!(marks.shapes[0].size(), 8.);
        marks.undo();
        assert_eq!(
            marks.shapes[0].filter_area(),
            Some(FilterArea::Brush { diameter: 24. })
        );
        marks.redo();
        assert_eq!(
            marks.shapes[0].filter_area(),
            Some(FilterArea::Brush { diameter: 40. })
        );
        marks.select_index(0);
        let path = marks.shapes[0].points.clone();
        marks.set_filter_effect(ShapeKind::Blur);
        assert_eq!(marks.shapes[0].kind, ShapeKind::Blur);
        assert_eq!(marks.shapes[0].points, path);
        assert_eq!(marks.selected_index(), Some(0));
        marks.undo();
        assert_eq!(marks.shapes[0].kind, ShapeKind::Mosaic);
    }

    #[test]
    fn filter_brush_tap_is_a_reversible_dot() {
        let mut marks = Annotations::default();
        marks.toggle(ShapeKind::Blur);
        marks.set_filter_area(true);
        let selection = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)));
        marks.begin(point(px(40.), px(40.)), selection, false);
        marks.end();
        assert_eq!(marks.shapes.len(), 1);
        assert_eq!(marks.hit_test(point(px(40.), px(40.))), Some(0));
        marks.undo();
        assert!(marks.shapes.is_empty());
        marks.redo();
        assert_eq!(marks.shapes.len(), 1);
    }
    #[test]
    fn curved_handles_keep_the_entire_selection_frame_inside_the_crop() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(160.)));
        for kind in [ShapeKind::Line, ShapeKind::Arrow] {
            let mut marks = Annotations::default();
            marks.toggle(kind);
            marks.begin(point(px(30.), px(40.)), area, false);
            marks.drag_to(point(px(150.), px(100.)), area, false);
            marks.end();
            for anchor in 0..3 {
                for destination in [
                    point(px(0.), px(0.)),
                    point(px(200.), px(160.)),
                    point(px(200.), px(0.)),
                ] {
                    let before = marks.shapes[0].clone();
                    marks.place_handle(0, anchor, &before, destination, area);
                    let frame = marks.shapes[0].selection_box();
                    assert!(
                        frame.left() >= area.left() && frame.right() <= area.right(),
                        "{frame:?}"
                    );
                    assert!(
                        frame.top() >= area.top() && frame.bottom() <= area.bottom(),
                        "{frame:?}"
                    );
                    marks.commit_move(0, before.clone());
                    marks.undo();
                    assert_eq!(marks.shapes[0], before);
                }
            }
        }
    }

    #[test]
    fn curve_midpoint_controls_render_pick_move_and_undo() {
        for kind in [ShapeKind::Line, ShapeKind::Arrow] {
            let mut marks = Annotations::default();
            marks.toggle(kind);
            let selection = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.)));
            marks.begin(point(px(20.), px(30.)), selection, false);
            marks.drag_to(point(px(160.), px(30.)), selection, false);
            marks.end();
            let before = marks.shapes[0].clone();
            assert_eq!(before.handle_points()[2], point(px(90.), px(30.)));
            marks.place_handle(0, 2, &before, point(px(90.), px(90.)), selection);
            marks.commit_move(0, before.clone());
            assert_eq!(marks.shapes[0].handle_points()[2], point(px(90.), px(90.)));
            assert_eq!(marks.hit_test(point(px(90.), px(90.))), Some(0));
            assert_eq!(marks.hit_test(point(px(90.), px(30.))), None);
            let mut pixels = vec![255; 200 * 200 * 4];
            marks.rasterize(&mut pixels, 200, 200, point(px(0.), px(0.)), 1.);
            let at = (90 * 200 + 90) * 4;
            assert_ne!(&pixels[at..at + 3], &[255; 3]);
            marks.undo();
            assert_eq!(marks.shapes[0], before);
            marks.redo();
            let curved = marks.shapes[0].clone();
            marks.place_shape(0, &curved, point(px(5.), px(8.)));
            assert_eq!(marks.shapes[0].handle_points()[2], point(px(95.), px(98.)));
        }
    }
}
