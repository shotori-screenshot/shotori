//! # Placement: the two-zone chrome layout contract
//!
//! The user-designed scheme (2026-09-26): the **label owns the top zone**
//! (above the selection, or inside its top-left corner when the selection
//! hugs the screen top) and the **toolbar owns the bottom zone** (below
//! the selection, or inside its bottom-left corner when it reaches the
//! screen bottom). The zones cannot intersect by construction, the label
//! never depends on the toolbar's existence (no "reserving room" jumps
//! while dragging), and nothing renders off-screen.
//!
//! Both anchors are pure functions; the invariant is enforced by a grid
//! sweep test over 35 selection geometries. `ui::hud` and `ui::toolbar`
//! consume these — this module is the single source of truth.

use gpui_kit::*;

/// Rough label width covering the widest "3072 × 1920" + padding; the
/// height matches the rendered chip.
const LABEL_W: f32 = 110.;
pub(crate) const LABEL_H: f32 = 24.;

/// Inset kept between the box border and elements drawn INSIDE it —
/// flush against the border line looks glued-on (user-reported).
const INSET: f32 = 12.;

/// Toolbar: [Copy][Save][OCR][Cancel] on row one; annotation tools,
/// colors and widths on row two (only while a tool is active).
/// Includes the two edge drag-grips (see [`GRIP_W`]).
/// Toolbar width — MEASURED live (grim + per-icon ink-slot scan of the
/// final layout): the full row demands a ~631px window; 632 keeps a
/// hair of headroom (narrow screens clamp it down; see
/// `toolbar_bounds`). Do not "fix" this back to a first-principles
/// 16×30+gaps estimate — the real pitch is 33px/button and the naive
/// sum runs ~70px short, which silently clips copy + the right grip
/// (bit us at 544, 576 AND 608).
pub(crate) const TB_W: f32 = 632.;
/// Toolbar width for the SINGLE-ROW state (no tool active): row one's
/// natural content width, measured via test probe
/// (`toolbar_hugs_its_content`): the full TB_W leaves a ~70px dead
/// hole between the tool cluster and the action cluster, which the
/// flex_1 spacer widens into a visible gap (user-reported). Re-based
/// 562 → 595 when the clear-all button joined the tool cluster
/// (issue #15), 595 → 627 when the select button took the first slot,
/// and 627 → 660 for the scroll button (2026-10-06): one button pitch
/// per addition (probe-measured).
pub(crate) const TB_W_ROW1: f32 = 660.;
/// Width of one drag-grip strip at the toolbar's left/right edge.
pub(crate) const GRIP_W: f32 = 12.;
/// The bar rows' horizontal padding. The grip elements sit INSIDE that
/// padding on row one — `session::toolbar_grips` uses the same constant
/// so the cursor strip and the element rect stay identical.
pub(crate) const BAR_PAD: f32 = 5.;
/// Single-row height (tools inactive)
pub(crate) const ROW_H: f32 = 38.;
/// Two-row height (the tall case used for placement decisions)
pub(crate) const TB_H: f32 = ROW_H * 2. + 6.;

/// Breathing room kept between the lowest element and the screen edge —
/// "fits at exactly zero margin" still looks glued on (measured).
const EDGE_B: f32 = 12.;

/// Label placement: ABOVE the selection, or — when the selection hugs
/// the top of the screen — INSIDE the box at its top-left corner. Never
/// below (see the module doc).
pub(crate) fn label_anchor(b: &Bounds<Pixels>, ws: Size<Pixels>) -> (f32, f32) {
    let inside = f32::from(b.top()) < LABEL_H + 10.;
    let x = (f32::from(b.left()) + if inside { INSET } else { 0. })
        .clamp(4., (f32::from(ws.width) - LABEL_W - 4.).max(4.));
    let y = if inside {
        // inside, top-left corner
        f32::from(b.top()) + 8.
    } else {
        f32::from(b.top()) - LABEL_H - 6.
    };
    (x, y)
}

/// Toolbar placement: BELOW the selection, or — when the selection
/// reaches the bottom of the screen — INSIDE the box at its bottom-left
/// corner. Horizontally clamped; `width`/`height` are the toolbar's
/// current size basis (see [`toolbar_size`]).
pub(crate) fn toolbar_anchor(
    b: &Bounds<Pixels>,
    ws: Size<Pixels>,
    width: f32,
    height: f32,
) -> (f32, f32) {
    let inside = f32::from(b.bottom()) + height + 8. + EDGE_B > f32::from(ws.height);
    let x = (f32::from(b.left()) + if inside { INSET } else { 0. })
        .clamp(8., (f32::from(ws.width) - width - 8.).max(8.));
    let y = if inside {
        // inside, bottom-left corner (inset from the border)
        f32::from(b.bottom()) - height - 8.
    } else {
        f32::from(b.bottom()) + 8.
    };
    (x, y.clamp(8., (f32::from(ws.height) - height - 8.).max(8.)))
}

/// The toolbar's full rect: [`toolbar_anchor`] plus the width clamp the
/// render side applies (the state's base width, or the window minus
/// breathing room on narrow screens). One source of truth for render,
/// cursor hit-tests and the drag clamp — they cannot drift apart.
pub(crate) fn toolbar_bounds(
    b: &Bounds<Pixels>,
    ws: Size<Pixels>,
    width: f32,
    height: f32,
) -> Bounds<Pixels> {
    let (x, y) = toolbar_anchor(b, ws, width, height);
    let w = width.min((f32::from(ws.width) - 16.).max(1.));
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(w), px(height)),
    }
}

/// The toolbar's (width, height) basis for its two row-count states.
/// Two-row bars keep the measured TB_W — the color settings row needs
/// it; single-row bars hug row one's natural width instead.
pub(crate) fn toolbar_size(annotating: bool) -> (f32, f32) {
    if annotating {
        (TB_W, TB_H)
    } else {
        (TB_W_ROW1, ROW_H)
    }
}

/// Snap a bounds to whole pixels for DISPLAY (dim strips, chrome,
/// toolbar). Edges are rounded independently (round(origin)+round(size)
/// can drift by 1px from round(origin+size)). Cropping keeps its own
/// physical-pixel rounding — this is purely a rendering concern.
pub(crate) fn round_px(b: Bounds<Pixels>) -> Bounds<Pixels> {
    let l = f32::from(b.left()).round();
    let t = f32::from(b.top()).round();
    let r = f32::from(b.right()).round();
    let btm = f32::from(b.bottom()).round();
    Bounds {
        origin: point(px(l), px(t)),
        size: size(px(r - l), px(btm - t)),
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports (same reason as selection.rs: avoid gpui's test
    // macro shadowing the built-in #[test])
    use super::{
        LABEL_H, ROW_H, TB_H, TB_W, TB_W_ROW1, label_anchor, toolbar_anchor, toolbar_size,
    };
    use gpui_kit::{Bounds, Pixels, point, px, size};

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    fn screen() -> gpui_kit::Size<Pixels> {
        size(px(1920.), px(1080.))
    }

    #[test]
    fn label_sits_above_by_default() {
        let (x, y) = label_anchor(&bounds(50., 100., 300., 200.), screen());
        assert_eq!((x, y), (50., 70.));
    }

    #[test]
    fn label_goes_inside_top_left_when_hugging_the_top() {
        let (x, y) = label_anchor(&bounds(50., 0., 300., 200.), screen());
        assert_eq!((x, y), (50. + 12., 8.));
    }

    #[test]
    fn label_stays_put_while_dragging_near_the_bottom() {
        // The label must not depend on the toolbar (which only exists after
        // release): a drag reaching the screen bottom keeps the label above
        let (_, y) = label_anchor(&bounds(50., 300., 300., 779.), screen());
        assert_eq!(y, 270.);
    }

    #[test]
    fn label_clamps_near_the_right_edge() {
        let (x, _) = label_anchor(&bounds(1900., 100., 20., 200.), screen());
        assert_eq!(x, 1920. - 110. - 4.);
    }

    #[test]
    fn toolbar_sits_below_by_default() {
        let (x, y) = toolbar_anchor(&bounds(50., 100., 300., 200.), screen(), TB_W_ROW1, ROW_H);
        assert_eq!((x, y), (50., 308.));
    }

    #[test]
    fn toolbar_goes_inside_bottom_left_when_reaching_the_bottom() {
        // two-row toolbar (annotating): the tall case
        let (x, y) = toolbar_anchor(&bounds(50., 300., 300., 780.), screen(), TB_W, TB_H);
        assert_eq!((x, y), (50. + 12., 1080. - TB_H - 8.));
    }

    #[test]
    fn toolbar_clamps_horizontally() {
        // a selection hugging the right edge: the toolbar pins into the screen
        let b = bounds(1800., 500., 100., 200.);
        let (x, _) = toolbar_anchor(&b, screen(), TB_W, TB_H);
        assert_eq!(x, 1920. - super::TB_W - 8.);
    }

    #[test]
    fn toolbar_size_switches_with_state() {
        assert_eq!(toolbar_size(false), (TB_W_ROW1, ROW_H));
        assert_eq!(toolbar_size(true), (TB_W, TB_H));
    }

    #[test]
    fn zones_stay_disjoint_across_a_grid_of_selections() {
        // The scheme's core invariant: the label zone (top) and the toolbar
        // zone (bottom) never overlap and never leave the screen — swept
        // over a representative grid of selection geometries, with the
        // toolbar at its tallest (two rows while annotating).
        for top in [0., 4., 34., 50., 78., 200., 800.] {
            for bottom in [top + 40., 1000., 1040., 1072., 1080.] {
                if bottom <= top || bottom > 1080. {
                    continue;
                }
                let b = bounds(50., top, 300., bottom - top);
                let (_, ly) = label_anchor(&b, screen());
                let (_, ty) = toolbar_anchor(&b, screen(), TB_W, TB_H);
                assert!(ly >= 0., "label off-screen for {b:?}");
                assert!(
                    ty >= 0. && ty + TB_H <= 1080.,
                    "toolbar off-screen for {b:?}"
                );
                assert!(
                    ly + LABEL_H <= ty,
                    "zones overlap for {b:?}: label {}..{}, toolbar {ty}..{}",
                    ly,
                    ly + LABEL_H,
                    ty + TB_H
                );
            }
        }
    }
}
