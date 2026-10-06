//! # Overlay HUD: pure visual elements tied to the selection
//!
//! Dim strips / selection border + size label.
//! Stateless, all functional; assembled in [`crate::ui::overlay::Overlay::render`].

use gpui_kit::*;

use crate::model::placement::label_anchor;
use crate::model::selection::{HANDLE_VIS, Handle};
use crate::model::session::EraserChrome;
use crate::ui::theme;

/// Paint the dim layer and border together. Separate positioned divs snap
/// their origins and sizes independently during layout; at fractional DPI
/// their edges can differ by a device pixel. Painting shared edges bypasses
/// that layout rounding and lets GPUI snap each absolute edge consistently.
pub(crate) fn selection_backdrop(sel: Option<Bounds<Pixels>>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |viewport, (), window, _| {
            let Some(mut b) = sel else {
                window.paint_quad(fill(viewport, rgba(theme::c().dim())));
                return;
            };
            b.origin += viewport.origin;
            let border = b;
            b = b.intersect(&viewport);
            let strips = [
                Bounds::from_corners(viewport.origin, point(viewport.right(), b.top())),
                Bounds::from_corners(point(viewport.left(), b.bottom()), viewport.bottom_right()),
                Bounds::from_corners(point(viewport.left(), b.top()), point(b.left(), b.bottom())),
                Bounds::from_corners(
                    point(b.right(), b.top()),
                    point(viewport.right(), b.bottom()),
                ),
            ];
            for strip in strips {
                if strip.size.width > px(0.) && strip.size.height > px(0.) {
                    window.paint_quad(fill(strip, rgba(theme::c().dim())));
                }
            }
            window.paint_quad(outline(
                border,
                rgba(theme::c().accent),
                BorderStyle::default(),
            ));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Hover highlight for window snapping: a bare 2px accent outline on
/// the window under the cursor. Outline only — it must read as "a click
/// will select this", not as a selection (no dim change, no fill, no
/// label; those belong to a real selection).
pub(crate) fn hover_outline(b: Bounds<Pixels>) -> impl IntoElement {
    div()
        .absolute()
        .left(b.origin.x)
        .top(b.origin.y)
        .w(b.size.width)
        .h(b.size.height)
        .border_2()
        .border_color(rgba(theme::c().accent))
}

/// Cursor shape for a resize grab, by handle.
pub(crate) fn handle_cursor(h: Handle) -> CursorStyle {
    match h {
        Handle::Left | Handle::Right => CursorStyle::ResizeLeftRight,
        Handle::Top | Handle::Bottom => CursorStyle::ResizeUpDown,
        Handle::TopLeft | Handle::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        Handle::TopRight | Handle::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// Paint one resize-handle dot centered on `p` (window-local): a solid
/// accent-orange circle, [`HANDLE_VIS`] px across, no outline. The
/// single owner of the handle look — the selection chrome and the
/// annotation chrome both call it, so a restyle (or a future corner
/// loupe) changes exactly one place. A solid circle replaces the white
/// square + outline chip, which read as clutter against the dim bands
/// and the border (issue #18), and one quad per handle is also one quad
/// less per frame during resize drags. The dot is what you SEE — what
/// you can GRAB stays each context's own, larger band ([`HANDLE_HIT`]
/// here, `Shape::handle_at` for shapes).
fn paint_handle_dot(window: &mut Window, p: Point<Pixels>) {
    let half = HANDLE_VIS / 2.;
    let b = Bounds::new(
        point(p.x - px(half), p.y - px(half)),
        size(px(HANDLE_VIS), px(HANDLE_VIS)),
    );
    // Corner radius = half the edge turns the quad into a circle.
    window.paint_quad(fill(b, rgba(theme::c().accent)).corner_radii(half));
}

/// Resize handles over a finalized (or being-edited) selection, plus the
/// window cursor. The cursor lives in a shared cell that the
/// pointer-move path refreshes (see `Overlay::cursor_style`); this canvas
/// pushes it during paint. Handles sit at the selection's TRUE edges — a
/// selection spanning outputs shows them on whichever screen contains
/// the edge, never at the monitor seam (same contract as the border).
///
/// Window-level cursor push is safe: nothing else in the overlay sets a
/// cursor today (gpui-kit buttons included — verified), so it cannot
/// shadow an existing affordance. `cursor_active` false (text editor /
/// setup dialog open) leaves the cursor to whoever owns focus.
pub(crate) fn selection_handles(
    sel: Option<Bounds<Pixels>>,
    visible: bool,
    cursor: std::rc::Rc<std::cell::Cell<CursorStyle>>,
    cursor_active: bool,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |viewport, (), window, _| {
            if cursor_active {
                window.set_window_cursor_style(cursor.get());
            }
            let Some(mut b) = sel.filter(|_| visible) else {
                return;
            };
            b.origin += viewport.origin;
            let (l, r, t, bt) = (
                f32::from(b.left()),
                f32::from(b.right()),
                f32::from(b.top()),
                f32::from(b.bottom()),
            );
            let (cx, cy) = ((l + r) / 2., (t + bt) / 2.);
            for (x, y) in [
                (l, t),
                (cx, t),
                (r, t),
                (l, cy),
                (r, cy),
                (l, bt),
                (cx, bt),
                (r, bt),
            ] {
                paint_handle_dot(window, point(px(x), px(y)));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Selection size label. The label tries above the selection,
/// then below, and when neither fits (a selection spanning the screen
/// height) it is drawn INSIDE the selection box — overlaid beats
/// off-screen. It must NOT live inside the border box when avoidable: a
/// narrow selection would clamp the label's width to the selection's,
/// wrapping "W × H" into a one-character-per-line tower. As a sibling
/// anchored to the overlay root it stays content-sized.
pub(crate) fn selection_label(
    b: Bounds<Pixels>,
    ws: Size<Pixels>,
    selected_size: Size<Pixels>,
) -> AnyElement {
    let (label_x, label_y) = label_anchor(&b, ws);

    div()
        .absolute()
        .left(px(label_x))
        .top(px(label_y))
        .px_2()
        .py(px(2.))
        .rounded(px(4.))
        .bg(rgba(theme::c().accent))
        .text_size(px(12.))
        .text_color(rgba(theme::c().accent_text()))
        .child(format!(
            "{} × {}",
            f32::from(selected_size.width).round() as i32,
            f32::from(selected_size.height).round() as i32
        ))
        .into_any_element()
}

// ── OCR busy badge (spinner) ──────────────────────────────────────────

/// The busy badge: a spinner + label, centered on the selection (or the
/// window when nothing is selected), clamped on-screen.
pub(crate) fn ocr_busy_badge(sel: Option<Bounds<Pixels>>, ws: Size<Pixels>) -> AnyElement {
    const BADGE_W: f32 = 118.;
    const BADGE_H: f32 = 40.;
    let (cx, cy) = match sel {
        Some(b) => (
            f32::from(b.left()) + f32::from(b.size.width) / 2.,
            f32::from(b.top()) + f32::from(b.size.height) / 2.,
        ),
        None => (f32::from(ws.width) / 2., f32::from(ws.height) / 2.),
    };
    let x = (cx - BADGE_W / 2.).clamp(8., f32::from(ws.width) - BADGE_W - 8.);
    let y = (cy - BADGE_H / 2.).clamp(8., f32::from(ws.height) - BADGE_H - 8.);

    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(rgba(theme::c().chip_bg))
        .border_1()
        .border_color(rgba(theme::c().accent))
        .child(spinner())
        .child(
            div()
                .text_size(px(13.))
                .text_color(rgba(theme::c().hint_text))
                .child("OCR…"),
        )
        .into_any_element()
}

/// Spinner: a faint ring with one accent dot orbiting inside. Pure element
/// properties animated via `with_animation` (respects reduce_motion;
/// max_fps caps the redraw rate).
fn spinner() -> impl IntoElement {
    const R: f32 = 7.; // orbit radius
    const BOX: f32 = 2. * R + 5.; // container edge
    const CENTER: f32 = BOX / 2.;
    const DOT: f32 = 4.;

    div()
        .id("shotori-spinner")
        .relative()
        .size(px(BOX))
        // faint ring for context
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(CENTER))
                .border_1()
                .border_color(rgba(crate::ui::theme::c().pin_border)),
        )
        // the orbiting dot
        .child(
            div()
                .absolute()
                .size(px(DOT))
                .rounded(px(DOT / 2.))
                .bg(rgba(crate::ui::theme::c().accent))
                .with_animation(
                    "shotori-spin",
                    Animation::new(std::time::Duration::from_millis(900))
                        .repeat()
                        .with_max_fps(15.),
                    move |dot, delta| {
                        let a = delta * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                        let (dx, dy) = (a.cos() * R, a.sin() * R);
                        dot.left(px(CENTER + dx - DOT / 2.))
                            .top(px(CENTER + dy - DOT / 2.))
                    },
                ),
        )
}

/// The annotation selection chrome: a 1 px dashed accent frame around
/// the selected shape's ink bounding box ([`Shape::selection_box`]),
/// plus the shared handle dots ([`paint_handle_dot`]) at its anchor
/// points (endpoints for lines, vertices for polylines, corners for
/// rects/ellipses). The dashed frame replaced a 1 px accent stroke
/// that traced the shape's own outline — that stroke sat ON the ink,
/// in a hue the annotation palette all but swallowed, so a selected
/// shape read as unselected. Dashes also separate the frame from the
/// selection region's SOLID accent border (object vs region), while
/// the shared dash rhythm keeps the area eraser's white rect (a
/// gesture in flight) its sibling: hue separates their meanings.
/// Pure rendering of [`crate::annotation::Shape`] geometry, and the
/// same visual language as [`selection_handles`].
pub(crate) fn annotation_chrome(selected: Option<crate::annotation::Shape>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |viewport, (), window, _| {
            let Some(shape) = &selected else {
                return;
            };
            let mut b = shape.selection_box();
            b.origin += viewport.origin;
            let mut builder = PathBuilder::stroke(px(1.)).dash_array(&[px(4.), px(3.)]);
            builder.add_polygon(
                &[
                    point(b.left(), b.top()),
                    point(b.right(), b.top()),
                    point(b.right(), b.bottom()),
                    point(b.left(), b.bottom()),
                ],
                true,
            );
            if let Ok(path) = builder.build() {
                window.paint_path(path, rgba(theme::c().accent));
            }
            // One helper, one size: a shape's resize affordance must read
            // as the same control the selection border wears.
            for p in shape.handle_points() {
                paint_handle_dot(window, p + viewport.origin);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The eraser's pointer chrome (issue #14): a double-stroke ring
/// tracing the brush footprint — dark on the outside, white just
/// inside, readable over any capture — or the area eraser's dashed
/// rect while its gesture is in flight. Pure rendering of
/// [`crate::model::session::EraserChrome`]; the erase criterion and
/// this circle share one radius (`Annotations::erase_radius`).
pub(crate) fn eraser_chrome(chrome: EraserChrome) -> impl IntoElement {
    canvas(
        move |_, _, _| (),
        move |viewport, (), window, _| match chrome {
            EraserChrome::Ring { center, radius } => {
                let center = center + viewport.origin;
                let mut ring = |r: f32, color| {
                    let mut builder = PathBuilder::stroke(px(1.));
                    for i in 0..32 {
                        let a0 = i as f32 * std::f32::consts::TAU / 32.;
                        let (s, c) = a0.sin_cos();
                        builder.line_to(center + point(px(c * r), px(s * r)));
                    }
                    builder.close();
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }
                };
                ring(radius, rgba(0xcc202020));
                ring((radius - 1.).max(0.5), rgba(0xffffffff));
            }
            EraserChrome::Rect(b) => {
                let mut b = b;
                b.origin += viewport.origin;
                let mut builder = PathBuilder::stroke(px(1.)).dash_array(&[px(4.), px(3.)]);
                builder.add_polygon(
                    &[
                        point(b.left(), b.top()),
                        point(b.right(), b.top()),
                        point(b.right(), b.bottom()),
                        point(b.left(), b.bottom()),
                    ],
                    true,
                );
                if let Ok(path) = builder.build() {
                    window.paint_path(path, rgba(0xffffffff));
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Loupe inset side and magnification (issue #19): 160 logical px at
/// 3× shows a ~53px neighborhood of the focus — wide enough to
/// recognize the element being aligned to, narrow enough that one
/// magnified pixel still reads as one.
const LOUPE_SIDE: f32 = 160.;
const LOUPE_ZOOM: f32 = 3.;
/// Clear space between the focus point and the inset's near edge: the
/// real pixels around the point stay visible beside their magnified
/// view — the coarse pass (dragging without fine-tuning) keeps an
/// unobstructed view of the point itself.
const LOUPE_GAP: f32 = 16.;

/// Clamp that never panics on inverted bounds (a window smaller than
/// the loupe) — same posture as selection's `clamp_to`.
fn clamp_to(v: f32, lo: f32, hi: f32) -> f32 {
    if lo > hi { lo } else { v.clamp(lo, hi) }
}

/// Where the loupe inset sits: offset `GAP + side/2` out along the
/// outward diagonal (away from the resized body — never over the point
/// being placed), clamped into the window so it never leaves the
/// output being magnified. When the outward side of an axis has no
/// room (the point is near that screen edge), that axis FLIPS to hang
/// the inset on the inward side instead of letting the clamp drag it
/// back over the point — inward covers the dimmed resized body, never
/// the edge content being aligned to. The CONTENT is independent of
/// this placement: it always centers on the focus (see
/// `magnifier_loupe`). Pure geometry — one source for the element and
/// any future pointer routing.
fn loupe_frame(focus: Point<Pixels>, outward: (f32, f32), ws: Size<Pixels>) -> Bounds<Pixels> {
    let half = LOUPE_SIDE / 2.;
    let reach = half + LOUPE_GAP;
    let (fx, fy) = (f32::from(focus.x), f32::from(focus.y));
    let fits = |dir: f32, at: f32, win: f32| {
        let lo = at + dir * reach - half;
        lo >= 0. && lo + LOUPE_SIDE <= win
    };
    // flip whichever axis cannot host the outward side (per-axis, so a
    // point at the bottom edge but mid-width keeps its horizontal
    // placement and only flips vertically)
    let dx = if fits(outward.0, fx, f32::from(ws.width)) {
        outward.0
    } else {
        -outward.0
    };
    let dy = if fits(outward.1, fy, f32::from(ws.height)) {
        outward.1
    } else {
        -outward.1
    };
    let l = fx + dx * reach - half;
    let t = fy + dy * reach - half;
    Bounds::new(
        point(
            px(clamp_to(l, 0., f32::from(ws.width) - LOUPE_SIDE)),
            px(clamp_to(t, 0., f32::from(ws.height) - LOUPE_SIDE)),
        ),
        size(px(LOUPE_SIDE), px(LOUPE_SIDE)),
    )
}

/// The magnifier loupe (issue #19): a floating window onto the frozen
/// capture around the point a corner/handle drag is placing, so the
/// point lands on the exact pixel without squinting. Composition zoom
/// only — the SAME per-output texture, scaled and offset inside an
/// `overflow_hidden` frame: no buffer copies, no BGRA round-trip
/// (ui/image_util.rs stays untouched), and logical-px math that is
/// scale-factor-proof by construction, because the img already fills
/// the window 1:1. Inert by design: a drag owns the pointer through
/// the window-level listeners, and this element claims no events.
pub(crate) fn magnifier_loupe(
    image: std::sync::Arc<RenderImage>,
    loupe: crate::model::session::Loupe,
    ws: Size<Pixels>,
) -> impl IntoElement {
    let frame = loupe_frame(loupe.focus, loupe.outward, ws);
    let (fx, fy) = (f32::from(loupe.focus.x), f32::from(loupe.focus.y));
    // Frame-LOCAL placement of the zoomed image: the focus pixel lands
    // at the frame's center wherever the frame floats. Absolute
    // children position against the loupe div's own origin, NOT the
    // window — mixing the two spaces double-counts frame.origin and
    // slides the content (and the crosshair) off the point being
    // placed.
    let img_left = LOUPE_SIDE / 2. - fx * LOUPE_ZOOM;
    let img_top = LOUPE_SIDE / 2. - fy * LOUPE_ZOOM;
    // The same accent the handle dot wears: the crosshair marks the
    // dot's magnified self, so the two read as one affordance.
    let cross = rgba(theme::c().accent);
    div()
        .id("shotori-loupe")
        .absolute()
        .left(frame.origin.x)
        .top(frame.origin.y)
        .w(frame.size.width)
        .h(frame.size.height)
        .overflow_hidden()
        .rounded(px(8.))
        .border_1()
        .border_color(rgba(theme::c().accent))
        // Lift the inset off the content it magnifies. Accentless on
        // purpose: the border already carries the accent, and a black
        // shadow reads against any wallpaper.
        .shadow(vec![
            BoxShadow::new(px(0.), px(2.), rgba(0x00000059).into()).blur_radius(px(12.)),
        ])
        .child(
            img(image)
                .absolute()
                .left(px(img_left))
                .top(px(img_top))
                .w(px(f32::from(ws.width) * LOUPE_ZOOM))
                .h(px(f32::from(ws.height) * LOUPE_ZOOM))
                // Exact geometry, never letterboxed: the inset is a
                // crop of the window, not a fit.
                .object_fit(ObjectFit::Fill),
        )
        .child(
            div()
                .absolute()
                .left(px(LOUPE_SIDE / 2. - 0.5))
                .top_0()
                .w(px(1.))
                .h(px(LOUPE_SIDE))
                .bg(cross),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(LOUPE_SIDE / 2. - 0.5))
                .w(px(LOUPE_SIDE))
                .h(px(1.))
                .bg(cross),
        )
        // ~100ms fade-in, the chrome animation posture (spinner):
        // appears with the gesture, disappears with it — release is
        // instant by design, no chrome lingering over a finished edit.
        .with_animation(
            "shotori-loupe-fade",
            Animation::new(std::time::Duration::from_millis(100)).with_max_fps(30.),
            |el, delta| el.opacity(delta),
        )
}

#[cfg(test)]
mod tests {
    // Explicit imports (same reason as selection.rs: avoid gpui's test macro
    // shadowing the built-in #[test])
    use gpui_kit::{Background, Bounds, CursorStyle, Pixels, point, px, rgba, size};

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    fn ws(w: f32, h: f32) -> gpui_kit::Size<Pixels> {
        size(px(w), px(h))
    }

    // ── Magnifier loupe placement (issue #19) ─────────────────────

    #[test]
    fn loupe_floats_along_the_outward_diagonal_with_a_gap() {
        use super::{LOUPE_GAP, LOUPE_SIDE, loupe_frame};
        // mid-screen focus, dragging the bottom-right corner: outward
        // = (+1,+1) → the inset sits fully below-right, near edge GAP
        // away — the point itself stays unobstructed
        let f = loupe_frame(point(px(500.), px(400.)), (1., 1.), ws(1920., 1080.));
        assert_eq!(f32::from(f.left()), 500. + LOUPE_GAP);
        assert_eq!(f32::from(f.top()), 400. + LOUPE_GAP);
        assert_eq!(f32::from(f.size.width), LOUPE_SIDE);
        // top-left corner → up-left of the focus
        let f = loupe_frame(point(px(500.), px(400.)), (-1., -1.), ws(1920., 1080.));
        assert_eq!(f32::from(f.right()), 500. - LOUPE_GAP);
        assert_eq!(f32::from(f.bottom()), 400. - LOUPE_GAP);
    }

    #[test]
    fn loupe_flips_per_axis_near_edges_and_never_covers_the_focus() {
        use super::{LOUPE_GAP, loupe_frame};
        // focus near the bottom-right of a small window: BOTH outward
        // sides lack room → the inset flips to up-left of the point
        // instead of clamping back over it
        let f = loupe_frame(point(px(280.), px(180.)), (1., 1.), ws(300., 200.));
        assert_eq!(f32::from(f.right()), 280. - LOUPE_GAP);
        assert_eq!(f32::from(f.bottom()), 180. - LOUPE_GAP);
        assert!(!f.contains(&point(px(280.), px(180.))));
        // focus near the top-left: flips to down-right
        let f = loupe_frame(point(px(5.), px(5.)), (-1., -1.), ws(300., 200.));
        assert_eq!(f32::from(f.left()), 5. + LOUPE_GAP);
        assert_eq!(f32::from(f.top()), 5. + LOUPE_GAP);
        assert!(!f.contains(&point(px(5.), px(5.))));
        // bottom edge but mid-width: only the vertical axis flips
        let f = loupe_frame(point(px(500.), px(1060.)), (1., 1.), ws(1920., 1080.));
        assert_eq!(f32::from(f.left()), 500. + LOUPE_GAP); // horizontal kept
        assert_eq!(f32::from(f.bottom()), 1060. - LOUPE_GAP); // vertical flipped
        assert!(!f.contains(&point(px(500.), px(1060.))));
    }

    #[test]
    fn loupe_survives_a_window_smaller_than_the_inset() {
        use super::loupe_frame;
        // no panic, and the frame stays pinned at the origin
        let f = loupe_frame(point(px(20.), px(20.)), (1., 1.), ws(100., 100.));
        assert_eq!(f32::from(f.left()), 0.);
        assert_eq!(f32::from(f.top()), 0.);
    }

    struct BackdropHarness {
        selection: Option<Bounds<Pixels>>,
    }

    impl gpui_kit::Render for BackdropHarness {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{ParentElement, Styled};
            gpui_kit::div()
                .relative()
                .size_full()
                .child(super::selection_backdrop(self.selection))
        }
    }

    // Inspect GPUI's actual device-pixel quads after layout and painting.
    // Each pixel outside the border must have exactly one dim layer:
    // zero produces a bright seam, two produce a dark seam.
    #[gpui_kit::test]
    fn backdrop_has_no_gaps_or_overlaps_at_fractional_dpi(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| BackdropHarness { selection: None });
        cx.update(|window, _| window.resize(ws(400., 400.)));
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            cx.simulate_scale_factor_change(scale);
            let selections = [
                None,
                Some(bounds(50., 101., 200., 230.)),
                Some(bounds(51., 102., 201., 229.)),
                Some(bounds(50., 103., 200., 230.)),
                Some(bounds(50., 104., 200., 230.)),
                Some(bounds(0., 0., 200., 230.)),
                Some(bounds(200., 170., 200., 230.)),
                Some(bounds(0., 0., 400., 400.)),
                // A global selection can continue beyond this output. Its
                // border must stay at the real edges, not the monitor seam.
                Some(bounds(-100., 10., 600., 380.)),
                Some(bounds(10., -100., 380., 600.)),
            ];
            for selection in selections {
                view.update(cx, |view, cx| {
                    view.selection = selection;
                    cx.notify();
                });
                let quads = cx.update(|window, cx| {
                    window.draw(cx).clear(cx);
                    window.painted_quads()
                });
                let border = quads.iter().find(|q| q.border_widths.top.0 > 0.);
                assert_eq!(border.is_some(), selection.is_some());
                let contains = |b: &gpui_kit::Bounds<gpui_kit::ScaledPixels>, x: f32, y: f32| {
                    x >= b.left().0 && x < b.right().0 && y >= b.top().0 && y < b.bottom().0
                };
                for y in 0..(400. * scale) as usize {
                    for x in 0..(400. * scale) as usize {
                        let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
                        let inside = border.is_some_and(|q| contains(&q.bounds, x, y));
                        let layers = quads
                            .iter()
                            .filter(|q| q.border_widths.top.0 == 0. && contains(&q.bounds, x, y))
                            .count();
                        assert_eq!(
                            layers,
                            usize::from(!inside),
                            "scale={scale}, selection={selection:?}, pixel=({x}, {y})"
                        );
                    }
                }
            }
        }
    }

    struct HandleHarness {
        selection: Option<Bounds<Pixels>>,
        visible: bool,
    }

    impl gpui_kit::Render for HandleHarness {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{ParentElement, Styled};
            // The shared cursor cell the canvas would push through;
            // cursor_active is false so the test needs no cursor
            // plumbing.
            let cursor = std::rc::Rc::new(std::cell::Cell::new(CursorStyle::Arrow));
            gpui_kit::div()
                .relative()
                .size_full()
                .child(super::selection_handles(
                    self.selection,
                    self.visible,
                    cursor,
                    false,
                ))
        }
    }

    // The handle restyle (issue #18): eight solid accent CIRCLES, not
    // white squares with an accent outline. Encodes circle-ness (every
    // corner radius = half the edge), the accent fill, the absent
    // outline, and the dot centers landing on the selection's true
    // edges (the border's contract). See/grab separation — dot smaller
    // than HANDLE_HIT — is held in selection.rs.
    #[gpui_kit::test]
    fn selection_handles_paint_solid_accent_circles(cx: &mut gpui_kit::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| HandleHarness {
            selection: Some(bounds(100., 100., 80., 60.)),
            visible: true,
        });
        cx.update(|window, _| window.resize(ws(400., 400.)));
        // Pin scale 1: the default test scale is 2, and the exact-value
        // assertions below (dot diameter, centers) are logical px.
        cx.simulate_scale_factor_change(1.);
        let quads = cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.painted_quads()
        });
        assert_eq!(quads.len(), 8);
        let expected_bg: Background = rgba(super::theme::c().accent).into();
        let mut centers: Vec<(f32, f32)> = Vec::new();
        for q in &quads {
            let (w, h) = (q.bounds.size.width.0, q.bounds.size.height.0);
            assert_eq!((w, h), (super::HANDLE_VIS, super::HANDLE_VIS));
            // circle: radius is half the edge, on every corner
            assert_eq!(q.corner_radii.top_left.0, w / 2.);
            assert_eq!(q.corner_radii.bottom_right.0, w / 2.);
            // solid dot: accent fill, no outline
            assert_eq!(q.background, expected_bg);
            assert_eq!(q.border_widths.top.0, 0.);
            centers.push((q.bounds.origin.x.0 + w / 2., q.bounds.origin.y.0 + h / 2.));
        }
        // 4 corners + 4 edge midpoints of the selection. Quads get
        // their origins floored to device pixels, so centers may sit
        // half a pixel off the anchor — match within 1 px, not exactly
        // (dots are 40 px apart here; the tolerance is unambiguous).
        let (l, r, t, b) = (100., 180., 100., 160.);
        let (mx, my) = ((l + r) / 2., (t + b) / 2.);
        let expected: Vec<(f32, f32)> = vec![
            (l, t),
            (mx, t),
            (r, t),
            (l, my),
            (r, my),
            (l, b),
            (mx, b),
            (r, b),
        ];
        for anchor in &expected {
            let i = centers
                .iter()
                .position(|c| (c.0 - anchor.0).abs() <= 1. && (c.1 - anchor.1).abs() <= 1.);
            let Some(i) = i else {
                panic!("no dot center within 1px of anchor {anchor:?}; got {centers:?}");
            };
            centers.swap_remove(i);
        }
        assert!(
            centers.is_empty(),
            "unexpected extra dot centers: {centers:?} (expected {expected:?})"
        );

        // Hidden chrome paints nothing — no ghost dots off-state
        view.update(cx, |view, cx| {
            view.visible = false;
            cx.notify();
        });
        let quads = cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.painted_quads()
        });
        assert!(quads.is_empty());
    }
}
