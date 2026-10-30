//! Icon-only line choices shared by annotation and magnifier parameter bars.
use super::*;
use crate::annotation::{ArrowStyle, LensConnector, LineStyle};

const SOLID: &str = "<path d='M3 12H33'/>";
const DASHED: &str = "<path d='M3 12H33' stroke-dasharray='7 5'/>";
const DOTTED: &str = "<path d='M3 12H33' stroke-dasharray='0 6'/>";
const DASH_DOT: &str = "<path d='M3 12H33' stroke-dasharray='7 4 0 4'/>";
const DASH_DOT_DOT: &str = "<path d='M3 12H33' stroke-dasharray='7 4 0 4 0 4'/>";
const CENTER: &str = "<path d='M5 12H29'/>";
const CENTER_DOT: &str =
    "<circle cx='6' cy='12' r='2.5' fill='black' stroke='none'/><path d='M8 12H29'/>";
const BORDER: &str = "<path d='M5 5V19M5 12H29'/>";
const NONE: &str = "<path d='M11 6L23 18M23 6L11 18'/>";

pub(super) fn connector_menu(
    value: LensConnector,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
) -> impl IntoElement {
    let session = session.clone();
    choice_menu(
        "lens-connector",
        "toolbar.lens_connector",
        value,
        &[
            (LensConnector::Center, "toolbar.connector_center", CENTER),
            (
                LensConnector::CenterDot,
                "toolbar.connector_center_dot",
                CENTER_DOT,
            ),
            (LensConnector::Border, "toolbar.connector_border", BORDER),
            (LensConnector::None, "toolbar.connector_none", NONE),
        ],
        focus,
        preview,
        move |value, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| a.set_lens_connector(value));
                cx.notify();
            });
        },
    )
}

pub(super) fn line_menu(
    value: LineStyle,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
) -> impl IntoElement {
    let session = session.clone();
    choice_menu(
        "line-style",
        "toolbar.line_style",
        value,
        &[
            (LineStyle::Solid, "toolbar.line_solid", SOLID),
            (LineStyle::Dashed, "toolbar.line_dashed", DASHED),
            (LineStyle::Dotted, "toolbar.line_dotted", DOTTED),
            (LineStyle::DashDot, "toolbar.line_dash_dot", DASH_DOT),
            (
                LineStyle::DashDotDot,
                "toolbar.line_dash_dot_dot",
                DASH_DOT_DOT,
            ),
        ],
        focus,
        preview,
        move |value, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| a.set_line_style(value));
                cx.notify();
            });
        },
    )
}

pub(super) fn preview(path: &str) -> Svg {
    let data = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='36' height='24' viewBox='0 0 36 24'><g fill='none' stroke='black' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>{path}</g></svg>"
    );
    svg()
        .data(data.as_bytes())
        .size(px(20.))
        .text_color(rgba(theme::c().toolbar_text))
}

pub(super) fn arrow_menu(
    value: ArrowStyle,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
) -> impl IntoElement {
    let session = session.clone();
    choice_menu(
        "arrow-style",
        "toolbar.arrow_style",
        value,
        &[
            (ArrowStyle::Open, "toolbar.arrow_open", ArrowStyle::Open),
            (
                ArrowStyle::DoubleOpen,
                "toolbar.arrow_double_open",
                ArrowStyle::DoubleOpen,
            ),
            (
                ArrowStyle::Outline,
                "toolbar.arrow_outline",
                ArrowStyle::Outline,
            ),
            (
                ArrowStyle::Filled,
                "toolbar.arrow_filled",
                ArrowStyle::Filled,
            ),
            (
                ArrowStyle::Dimension,
                "toolbar.arrow_dimension",
                ArrowStyle::Dimension,
            ),
            (
                ArrowStyle::Triangle,
                "toolbar.arrow_triangle",
                ArrowStyle::Triangle,
            ),
            (
                ArrowStyle::DoubleTriangle,
                "toolbar.arrow_double_triangle",
                ArrowStyle::DoubleTriangle,
            ),
        ],
        focus,
        arrow_preview,
        move |value, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| a.set_arrow_style(value));
                cx.notify();
            });
        },
    )
}

fn arrow_preview(style: ArrowStyle) -> Svg {
    let polygons = crate::annotation::arrow_preview_geometry(style);
    let mut data = String::from(
        "<svg xmlns='http://www.w3.org/2000/svg' width='36' height='24' viewBox='0 0 36 24'>",
    );
    for polygon in polygons {
        data.push_str("<polygon fill='black' points='");
        for p in polygon {
            data.push_str(&format!("{},{} ", f32::from(p.x), f32::from(p.y)));
        }
        data.push_str("'/>");
    }
    data.push_str("</svg>");
    svg()
        .data(data.as_bytes())
        .size(px(20.))
        .text_color(rgba(theme::c().toolbar_text))
}

/// Numeral previews use paths so they do not depend on an SVG font resolver.
pub(super) fn number_menu(
    value: crate::annotation::NumberFormat,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
) -> impl IntoElement {
    use crate::annotation::NumberFormat;
    let session = session.clone();
    choice_menu(
        "number-format",
        "toolbar.number_format",
        value,
        &[
            (
                NumberFormat::Decimal,
                "toolbar.number_decimal",
                "<path d='M14 8L18 5V19M14 19H22'/>",
            ),
            (
                NumberFormat::Alphabetic,
                "toolbar.number_alphabetic",
                "<path d='M11 19L18 5L25 19M14 14H22'/>",
            ),
            (
                NumberFormat::Roman,
                "toolbar.number_roman",
                "<path d='M10 5H16M13 5V19M10 19H16M19 5L24 19L29 5'/>",
            ),
        ],
        focus,
        preview,
        move |value, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| a.set_number_format(value));
                cx.notify();
            });
        },
    )
}

pub(super) fn pencil_angle_menu(
    value: u16,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
) -> impl IntoElement {
    let session = session.clone();
    choice_menu(
        "pencil-angle",
        "toolbar.pencil_angle",
        value,
        &[
            (0, "toolbar.angle_free", 0),
            (45, "toolbar.angle_45", 45),
            (30, "toolbar.angle_30", 30),
            (15, "toolbar.angle_15", 15),
            (10, "toolbar.angle_10", 10),
            (5, "toolbar.angle_5", 5),
        ],
        focus,
        |angle: u16| {
            let angle = if angle == 0 { 23. } else { angle as f32 };
            let (sin, cos) = angle.to_radians().sin_cos();
            preview(&format!(
                "<path d='M5 19H31M5 19L{} {}'/>",
                5. + 25. * cos,
                19. - 18. * sin
            ))
        },
        move |value, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| a.set_pencil_angle(value));
                cx.notify();
            });
        },
    )
}

/// Highlighter area and blend are independent choices in the reference toolbar.
pub(super) fn highlighter_menus(
    annotations: &crate::annotation::Annotations,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
) -> impl IntoElement {
    use crate::annotation::HighlightBlend;
    let options = annotations.highlighter_options();
    let area_session = session.clone();
    let blend_session = session.clone();
    div().flex().items_center().gap(px(2.))
        .child(choice_menu("highlighter-area", "toolbar.highlighter_area", options.rectangle,
            &[(false, "toolbar.highlighter_brush", "<path d='M5 18L30 6' stroke-width='6'/>"),
              (true, "toolbar.highlighter_rectangle", "<rect x='5' y='7' width='26' height='10' fill='black' stroke='none'/>")],
            focus, preview, move |value, cx| {
                area_session.update(cx, |s, cx| { s.edit_annotations(|a| a.set_highlighter_rectangle(value)); cx.notify(); });
            }))
        .child(choice_menu("highlighter-blend", "toolbar.highlighter_blend", options.blend,
            &[(HighlightBlend::Multiply, "toolbar.highlighter_multiply", "<path d='M9 6L21 18M9 18L21 6'/><path d='M27 5V19'/>"),
              (HighlightBlend::Translucent, "toolbar.highlighter_translucent", "<rect x='6' y='5' width='14' height='12'/><rect x='16' y='10' width='14' height='10'/>")],
            focus, preview, move |value, cx| {
                blend_session.update(cx, |s, cx| { s.edit_annotations(|a| a.set_highlighter_blend(value)); cx.notify(); });
            }))
}

pub(super) fn line_end_menus(
    ends: crate::annotation::LineEnds,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
) -> impl IntoElement {
    use crate::annotation::EndMarker;
    let mut row = div().flex().items_center().gap(px(2.));
    for start in [true, false] {
        let session = session.clone();
        row = row.child(choice_menu(
            if start { "line-start" } else { "line-end" },
            if start {
                "toolbar.line_start"
            } else {
                "toolbar.line_end"
            },
            if start { ends.start } else { ends.end },
            &[
                (
                    EndMarker::None,
                    "toolbar.endpoint_none",
                    "<path d='M4 12H32'/>",
                ),
                (
                    EndMarker::Open,
                    "toolbar.endpoint_open",
                    "<path d='M4 12H32M24 5L32 12L24 19'/>",
                ),
                (
                    EndMarker::Triangle,
                    "toolbar.endpoint_triangle",
                    "<path d='M4 12H24'/><path d='M23 6L33 12L23 18Z' fill='black'/>",
                ),
                (
                    EndMarker::Bar,
                    "toolbar.endpoint_bar",
                    "<path d='M4 12H31M31 5V19'/>",
                ),
            ],
            focus,
            if start { start_preview } else { preview },
            move |value, cx| {
                session.update(cx, |s, cx| {
                    s.edit_annotations(|a| {
                        let mut ends = a.line_ends();
                        if start {
                            ends.start = value;
                        } else {
                            ends.end = value;
                        }
                        a.set_line_ends(ends);
                    });
                    cx.notify();
                });
            },
        ));
    }
    let cap_session = session.clone();
    let join_session = session.clone();
    row.child(choice_menu(
        "line-caps",
        "toolbar.line_caps",
        ends.square_caps,
        &[
            (
                false,
                "toolbar.cap_round",
                "<path d='M7 12H29' stroke-width='8'/>",
            ),
            (
                true,
                "toolbar.cap_square",
                "<path d='M7 12H29' stroke-width='8' stroke-linecap='square'/>",
            ),
        ],
        focus,
        preview,
        move |value, cx| {
            cap_session.update(cx, |s, cx| {
                s.edit_annotations(|a| {
                    let mut ends = a.line_ends();
                    ends.square_caps = value;
                    a.set_line_ends(ends);
                });
                cx.notify();
            });
        },
    ))
    .child(choice_menu(
        "line-joins",
        "toolbar.line_joins",
        ends.sharp_joins,
        &[
            (
                false,
                "toolbar.join_round",
                "<path d='M6 18L18 6L30 18' stroke-width='5'/>",
            ),
            (
                true,
                "toolbar.join_sharp",
                "<path d='M6 18L18 6L30 18' stroke-width='5' stroke-linejoin='miter'/>",
            ),
        ],
        focus,
        preview,
        move |value, cx| {
            join_session.update(cx, |s, cx| {
                s.edit_annotations(|a| {
                    let mut ends = a.line_ends();
                    ends.sharp_joins = value;
                    a.set_line_ends(ends);
                });
                cx.notify();
            });
        },
    ))
}

fn start_preview(path: &str) -> Svg {
    preview(&format!(
        "<g transform='translate(36 0) scale(-1 1)'>{path}</g>"
    ))
}
