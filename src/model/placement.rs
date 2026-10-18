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

use crate::model::session::ScrollRect;

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
/// Visual stroke of the scroll-capture region frame (logical px).
pub(crate) const FRAME_STROKE: f32 = 2.;
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
/// it; single-row bars take the configured row's basis instead.
///
/// The basis must track the toolbar settings: the rendered bar hugs
/// its configured content, and render, grip hit-tests and the drag
/// clamp all derive from THIS — a stale basis shows up instantly as
/// "draggable but not a hand" (the grip/cursor alignment trap).
pub(crate) fn toolbar_size(
    annotating: bool,
    settings: &crate::model::settings::Settings,
) -> (f32, f32) {
    if annotating {
        (TB_W, TB_H)
    } else {
        (row1_basis(settings), ROW_H)
    }
}

/// One row-one button's footprint: the 30px control plus the 2px flex
/// gap that dies with it.
const BTN_PITCH: f32 = 32.;
/// A separator's footprint: 1px stroke + 2×4px margins + one gap.
const SEP_PITCH: f32 = 11.;
/// The flex gap the one spacer CHILD occupied in the row the anchor
/// was measured against (the spacer itself rendered zero-width; only
/// its gap remains as a footprint now that the variant is gone).
const GONE_SPACER_GAP: f32 = 2.;
/// The composition of the row TB_W_ROW1 was probe-measured against:
/// 19 buttons, one zero-width spacer child, one separator — the
/// pre-customization toolbar. Pinned EXPLICITLY because the factory
/// row later gained the settings gear: keying the anchor off
/// ALL.len() silently rebased it, and a 32px-short basis put the
/// right grip's hand-cursor rect over the buttons instead of the
/// rendered grip (user-reported — grips derive from this basis).
const ANCHOR_BUTTONS: f32 = 19.;
const ANCHOR_SEPARATORS: f32 = 1.;
/// Floor for the degenerate everything-hidden bar (grips + padding +
/// slack) — the basis must stay positive for the clamps.
const ROW1_W_MIN: f32 = 48.;

/// The default (factory-row) single-row width basis — what the tests
/// assert against and what a fresh install renders.
#[cfg(test)]
pub(crate) fn default_row1_basis() -> f32 {
    row1_basis(&crate::model::settings::Settings::default())
}

/// The configured row's width basis, as exact per-child deltas from
/// the probe-measured anchor (see TB_W_ROW1's comment: totals are
/// measured, never re-derived from first principles — but a delta per
/// removed/added child is exact geometry). Today's factory row: 20
/// buttons and two separators, no spacer child →
/// 660 - 2 + 32 + 11 = 701.
pub(crate) fn row1_basis(settings: &crate::model::settings::Settings) -> f32 {
    let buttons = settings.row1_button_count() as f32;
    let separators = settings.row1_separator_count() as f32;
    (TB_W_ROW1 - GONE_SPACER_GAP
        + BTN_PITCH * (buttons - ANCHOR_BUTTONS)
        + SEP_PITCH * (separators - ANCHOR_SEPARATORS))
        .max(ROW1_W_MIN)
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

/// The four visual strokes as output-local rects.
pub(crate) fn frame_strokes(rect: ScrollRect) -> [Bounds<Pixels>; 4] {
    let (x, y) = (rect.x as f32, rect.y as f32);
    let (w, h) = (rect.width as f32, rect.height as f32);
    [
        Bounds {
            origin: point(px(x - FRAME_STROKE), px(y - FRAME_STROKE)),
            size: size(px(w + 2. * FRAME_STROKE), px(FRAME_STROKE)),
        },
        Bounds {
            origin: point(px(x - FRAME_STROKE), px(y + h)),
            size: size(px(w + 2. * FRAME_STROKE), px(FRAME_STROKE)),
        },
        Bounds {
            origin: point(px(x - FRAME_STROKE), px(y)),
            size: size(px(FRAME_STROKE), px(h)),
        },
        Bounds {
            origin: point(px(x + w), px(y)),
            size: size(px(FRAME_STROKE), px(h)),
        },
    ]
}

/// Size of the move affordance that rides at the frame's top edge.
/// Small and outside the rect: it can never be captured. Sized as a
/// capsule (2:1 would be too thin to read), and close enough to the
/// stroke to belong to it — a distant floating lozenge read as random
/// decoration.
pub(crate) const FRAME_CHIP_W: f32 = 46.;
pub(crate) const FRAME_CHIP_H: f32 = 18.;
pub(crate) const FRAME_CHIP_GAP: f32 = 4.;

/// The move chip: a small grip centered above the frame (below it when
/// the frame touches the output top). `None` when neither side has room
/// — a full-height selection cannot show it without painting inside the
/// capture rect, and the strokes are not draggable anyway (only this
/// handle moves the frame, vertically).
pub(crate) fn frame_grab_chip(rect: ScrollRect, out_w: f32, out_h: f32) -> Option<Bounds<Pixels>> {
    let x = (rect.x as f32 + rect.width as f32 / 2. - FRAME_CHIP_W / 2.)
        .clamp(0., (out_w - FRAME_CHIP_W).max(0.));
    let above = rect.y as f32 - FRAME_CHIP_GAP - FRAME_CHIP_H;
    if above >= 0. {
        return Some(Bounds {
            origin: point(px(x), px(above)),
            size: size(px(FRAME_CHIP_W), px(FRAME_CHIP_H)),
        });
    }
    let below = rect.y as f32 + rect.height as f32 + FRAME_CHIP_GAP;
    (below + FRAME_CHIP_H <= out_h).then(|| Bounds {
        origin: point(px(x), px(below)),
        size: size(px(FRAME_CHIP_W), px(FRAME_CHIP_H)),
    })
}

/// The chrome-free frame toolbar's size (logical px):
/// [⇕] | [Copy][Save][✕].
pub(crate) const FRAME_TB_W: f32 = 194.;
pub(crate) const FRAME_TB_H: f32 = 38.;
/// Gap between the frame edge and its toolbar.
pub(crate) const FRAME_TB_GAP: f32 = 10.;

/// The preview panel's logical width (`ui::scroll_bar` renders it).
pub(crate) const SCROLL_PANEL_W: f32 = 264.;
/// Gap kept between the capture rect and the panel floating beside it.
/// Clears the frame's visible stroke ([`FRAME_STROKE`]) with room to
/// spare; 12 matches the codebase's other breathing-room insets.
pub(crate) const SCROLL_PANEL_GAP: f32 = 12.;
/// Breathing room between the panel and the output edges: the vertical
/// clamp of the beside-the-rect placement, and the other-output dock's
/// margins.
pub(crate) const SCROLL_PANEL_MARGIN: f32 = 10.;

/// Which side of the capture rect the preview panel floats on.
///
/// wlr-screencopy captures the fully-composited output — including our
/// own layer surfaces — so any chrome painted over the capture rect
/// burns into every folded frame. The panel therefore only floats on
/// a side that leaves [`scroll_panel_strip`] of space between the rect
/// and the output edge — hanging BESIDE the rect, not pinned to the
/// output edge (user-reported: an edge-docked preview reads as
/// disconnected from the region it previews); when no side qualifies
/// it moves to another output, or (single monitor) the session goes
/// chrome-free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScrollPanelDock {
    Left,
    Right,
    /// No side of the capture output fits; another output exists — the
    /// panel lives there (peripheral, but visible and interactive).
    OtherOutput,
    /// Single monitor, no side fits: no panel at all. The frame window
    /// hosts the session alone (strokes stay strictly OUTSIDE the rect,
    /// exits go through the keyboard).
    Hidden,
}

/// The side strip a beside-the-rect panel needs between itself and the
/// rect: the gap plus the panel's width. A side qualifies for the dock
/// when this much of it is free between the rect and the output edge
/// (any remainder stays between the panel and the edge — the panel
/// hugs the rect, not the screen).
pub(crate) fn scroll_panel_strip() -> f32 {
    SCROLL_PANEL_GAP + SCROLL_PANEL_W
}

/// Pick the panel dock for a capture rect. A side qualifies when the
/// strip between the rect and that output edge fits the panel; among
/// qualifying sides the wider one wins (ties → Right, the historical
/// default for centered rects).
pub(crate) fn plan_scroll_panel(
    rect: ScrollRect,
    out_w: f32,
    other_output: bool,
) -> ScrollPanelDock {
    let free_left = rect.x as f32;
    let free_right = out_w - (rect.x + rect.width) as f32;
    let strip = scroll_panel_strip();
    match (free_left >= strip, free_right >= strip) {
        (true, false) => ScrollPanelDock::Left,
        (false, true) => ScrollPanelDock::Right,
        (true, true) if free_left > free_right => ScrollPanelDock::Left,
        (true, true) => ScrollPanelDock::Right,
        (false, false) if other_output => ScrollPanelDock::OtherOutput,
        (false, false) => ScrollPanelDock::Hidden,
    }
}

/// The preview panel's layer-shell position as `(top, left)` margins of
/// a `TOP | LEFT` anchor (layer surfaces ignore `window_bounds`
/// origins): floating BESIDE the capture rect instead of pinned to the
/// output edge — user-reported, an edge-docked preview reads as
/// disconnected from the region it previews. Horizontally the panel
/// hugs the rect across [`SCROLL_PANEL_GAP`] on the dock's side;
/// vertically it centers on the rect, clamped into the output.
///
/// The position is frozen at launch — and the frame's x is frozen with
/// it (the ⇕ handle is the only frame motion, see
/// [`clamp_moved_rect`]) — so the region can never slide under the
/// panel. The other-output dock has no rect to hug and keeps the
/// historical edge corner.
///
/// A dock that does not fit is defensive only — the planner returns
/// [`ScrollPanelDock::Hidden`] instead; the clamps then keep the panel
/// fully on-screen (possibly over the rect — the same exposure the old
/// edge dock had).
pub(crate) fn scroll_panel_margins(
    rect: ScrollRect,
    out_w: f32,
    out_h: f32,
    panel_h: f32,
    dock: ScrollPanelDock,
) -> (f32, f32) {
    match dock {
        ScrollPanelDock::Left => (
            panel_top(rect, out_h, panel_h),
            (rect.x as f32 - SCROLL_PANEL_GAP - SCROLL_PANEL_W).max(0.),
        ),
        ScrollPanelDock::Right => (
            panel_top(rect, out_h, panel_h),
            (rect.x as f32 + rect.width as f32 + SCROLL_PANEL_GAP)
                .min((out_w - SCROLL_PANEL_W).max(0.)),
        ),
        ScrollPanelDock::OtherOutput | ScrollPanelDock::Hidden => {
            (SCROLL_PANEL_MARGIN, SCROLL_PANEL_MARGIN)
        }
    }
}

/// Vertical placement: centered on the rect, clamped into the output
/// with the strip margin (the `.max(lo)` keeps the clamp panic-free on
/// degenerate outputs — the same posture as [`clamp_moved_rect`]).
fn panel_top(rect: ScrollRect, out_h: f32, panel_h: f32) -> f32 {
    let lo = SCROLL_PANEL_MARGIN;
    let hi = (out_h - panel_h - SCROLL_PANEL_MARGIN).max(lo);
    (rect.y as f32 + rect.height as f32 / 2. - panel_h / 2.).clamp(lo, hi)
}

/// Where the frame's toolbar sits — a placement ladder that NEVER
/// paints inside the capture rect (the same screencopy-composites-our
/// -layers trap as the panel):
///
/// 1. centered below the frame's bottom edge;
/// 2. flipped above the frame's top edge;
/// 3. docked at the top of a side strip the panel does NOT occupy;
/// 4. `None` — not rendered (the panel's buttons / the keyboard carry
///    the exits; the grab bands are input-only and keep working).
///
/// The historical behavior clamped rung 2 to `y = 0`, which landed the
/// toolbar INSIDE any full-height selection — the top of every frame
/// burned a 198×32 toolbar into the long screenshot. Same geometry
/// feeds the render and the input region (one source per concept).
pub(crate) fn frame_toolbar(
    rect: ScrollRect,
    out_w: f32,
    out_h: f32,
    panel: ScrollPanelDock,
) -> Option<Bounds<Pixels>> {
    let (x, y, w, h) = (
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    let centered = |top: f32| Bounds {
        origin: point(
            px((x + w / 2. - FRAME_TB_W / 2.).clamp(0., (out_w - FRAME_TB_W).max(0.))),
            px(top),
        ),
        size: size(px(FRAME_TB_W), px(FRAME_TB_H)),
    };
    // Rung 1: below (the visual default — next to where the content
    // grows).
    if y + h + FRAME_TB_GAP + FRAME_TB_H <= out_h {
        return Some(centered(y + h + FRAME_TB_GAP));
    }
    // Rung 2: above. No `.max(0.)` clamp: a rung that does not fit
    // falls through instead of pushing the bar inside the rect.
    if y - FRAME_TB_GAP - FRAME_TB_H >= 0. {
        return Some(centered(y - FRAME_TB_GAP - FRAME_TB_H));
    }
    // Rung 3: a side strip, panel-free (the panel anchors top-to-bottom
    // of its strip — nothing else may live there). Prefer the wider
    // strip; both free (panel elsewhere/hidden) → Right, matching the
    // panel's default side.
    let free_left = x;
    let free_right = out_w - (x + w);
    let need = FRAME_TB_W + SCROLL_PANEL_MARGIN;
    let left_ok = panel != ScrollPanelDock::Left && free_left >= need;
    let right_ok = panel != ScrollPanelDock::Right && free_right >= need;
    let side = match (left_ok, right_ok) {
        (true, true) if free_left > free_right => Some(true),
        (_, true) => Some(false),
        (true, false) => Some(true),
        (false, false) => None,
    };
    side.map(|is_left| Bounds {
        origin: point(
            px(if is_left {
                SCROLL_PANEL_MARGIN
            } else {
                out_w - FRAME_TB_W - SCROLL_PANEL_MARGIN
            }),
            px(SCROLL_PANEL_MARGIN),
        ),
        size: size(px(FRAME_TB_W), px(FRAME_TB_H)),
    })
}

/// Move `rect` vertically by `dy`, clamped inside the output. Pure —
/// the unit tests drive drags through here.
///
/// Vertical is the ONLY frame motion: for the sheet, "frame moves down"
/// is indistinguishable from "content scrolls up", while a horizontal
/// jump desyncs the stitch (the folded columns stop matching) — free
/// frame movement shipped broken long screenshots. The frozen x also
/// keeps the region clear of the preview panel, which hangs beside the
/// rect at its frozen launch position (layer surfaces cannot
/// re-anchor; see [`scroll_panel_margins`]).
pub(crate) fn clamp_moved_rect(rect: ScrollRect, dy: f32, out_h: f32) -> ScrollRect {
    // Degenerate-output posture: never panic on inverted bounds.
    let clamp = |v: f32, hi: f32| if hi < 0. { 0. } else { v.clamp(0., hi) };
    ScrollRect {
        x: rect.x,
        y: clamp(rect.y as f32 + dy, out_h - rect.height as f32) as i32,
        width: rect.width,
        height: rect.height,
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports (same reason as selection.rs: avoid gpui's test
    // macro shadowing the built-in #[test])
    use super::{
        FRAME_CHIP_GAP, FRAME_CHIP_H, FRAME_CHIP_W, FRAME_TB_H, FRAME_TB_W, LABEL_H, ROW_H,
        SCROLL_PANEL_GAP, SCROLL_PANEL_MARGIN, SCROLL_PANEL_W, ScrollPanelDock, TB_H, TB_W,
        TB_W_ROW1, clamp_moved_rect, frame_grab_chip, frame_toolbar, label_anchor,
        plan_scroll_panel, scroll_panel_margins, toolbar_anchor, toolbar_size,
    };
    use crate::model::session::ScrollRect;
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
        let s = crate::model::settings::Settings::default();
        assert_eq!(toolbar_size(false, &s), (super::row1_basis(&s), ROW_H));
        assert_eq!(toolbar_size(true, &s), (TB_W, TB_H));
    }

    #[test]
    fn toolbar_size_follows_the_configured_row() {
        use crate::model::settings::{DEFAULT_ROW, Settings, ToolbarButton, ToolbarItem};
        // three buttons off: row one shrinks by exactly three pitches
        let mut s = Settings::default();
        s.set_row(
            DEFAULT_ROW
                .iter()
                .copied()
                .filter(|item| {
                    !matches!(
                        item,
                        ToolbarItem::Button(
                            ToolbarButton::Pin | ToolbarButton::Scroll | ToolbarButton::Text
                        )
                    )
                })
                .collect(),
        );
        let (w, h) = toolbar_size(false, &s);
        // 17 buttons (3 off the 20-button factory) + 2 separators,
        // against the 19-button/1-separator anchor:
        // 660 - 2 + 32*(17-19) + 11*(2-1)
        assert_eq!(
            w,
            TB_W_ROW1 - super::GONE_SPACER_GAP - 2. * super::BTN_PITCH + super::SEP_PITCH
        );
        assert_eq!(h, ROW_H);
        // annotating is the fixed-width settings row — unaffected
        assert_eq!(toolbar_size(true, &s), (TB_W, TB_H));

        // an extra separator grows the basis by its pitch
        let mut s = Settings::default();
        s.set_row(
            DEFAULT_ROW
                .iter()
                .copied()
                .chain([ToolbarItem::Separator])
                .collect(),
        );
        assert_eq!(
            toolbar_size(false, &s).0,
            TB_W_ROW1 - super::GONE_SPACER_GAP + super::BTN_PITCH + 2. * super::SEP_PITCH
        );

        // empty row (hide everything): the floor keeps the basis positive
        let mut s = Settings::default();
        s.set_row(Vec::new());
        assert_eq!(toolbar_size(false, &s).0, super::ROW1_W_MIN);
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

    fn rect(x: i32, y: i32) -> ScrollRect {
        ScrollRect {
            x,
            y,
            width: 400,
            height: 300,
        }
    }

    #[test]
    fn drag_moves_and_clamps_inside_the_output() {
        // vertical-only motion (the ⇕ handle): x is frozen at the
        // rect's position, y clamps to the output
        let moved = clamp_moved_rect(rect(100, 100), -40., 1080.);
        assert_eq!((moved.x, moved.y), (100, 60));
        // clamped at the top edge
        let clamped = clamp_moved_rect(rect(100, 100), -500., 1080.);
        assert_eq!((clamped.x, clamped.y), (100, 0));
        // clamped at the bottom edge (region fully on-screen)
        let clamped = clamp_moved_rect(rect(100, 900), 5000., 1080.);
        assert_eq!(
            (clamped.x, clamped.y),
            (100, 1080 - 300),
            "the region must stay fully inside the output"
        );
        // size never changes — drags move, they do not resize
        assert_eq!((moved.width, moved.height), (400, 300));
    }

    #[test]
    fn preview_panel_floats_beside_the_capture_rect() {
        // Right dock: the panel's left edge hangs SCROLL_PANEL_GAP to
        // the right of the rect, vertically centered on it.
        let (top, left) =
            scroll_panel_margins(rect(100, 200), 1920., 1080., 400., ScrollPanelDock::Right);
        assert_eq!(left, 100. + 400. + SCROLL_PANEL_GAP);
        assert_eq!(top, 200. + 300. / 2. - 400. / 2.);
        // Left dock: mirrored — the panel's right edge sits GAP left of
        // the rect (this rect leaves room on that side).
        let (top, left) =
            scroll_panel_margins(rect(600, 200), 1920., 1080., 400., ScrollPanelDock::Left);
        assert_eq!(left, 600. - SCROLL_PANEL_GAP - SCROLL_PANEL_W);
        assert_eq!(top, 200. + 300. / 2. - 400. / 2.);
        // A rect near the output bottom clamps the panel back on-screen.
        let (top, _) =
            scroll_panel_margins(rect(100, 900), 1920., 1080., 400., ScrollPanelDock::Right);
        assert_eq!(top, 1080. - 400. - SCROLL_PANEL_MARGIN);
        // Another output has no rect to hug: the historical edge corner.
        let (top, left) = scroll_panel_margins(
            rect(100, 200),
            1920.,
            1080.,
            400.,
            ScrollPanelDock::OtherOutput,
        );
        assert_eq!((top, left), (SCROLL_PANEL_MARGIN, SCROLL_PANEL_MARGIN));
        // A dock that does not fit (defensive — the planner would have
        // returned Hidden) clamps against the output instead of going
        // off-screen.
        let (_, left) =
            scroll_panel_margins(rect(100, 200), 1920., 1080., 400., ScrollPanelDock::Left);
        assert_eq!(left, 0.);
    }

    #[test]
    fn panel_beside_the_rect_never_overlaps_it() {
        // The session's burn-in guard: the panel is frozen at its
        // launch position and the frame moves VERTICALLY only, so
        // launch-time x-disjointness must hold for every docked
        // geometry (a horizontal drag that slid the region under the
        // panel is structurally impossible — clamp_moved_rect has no
        // dx). Swept like the selection's two-zone grid above.
        let panel_h = 432.; // 1080 * 0.4 — the launch-time height basis
        for x in [0, 50, 300, 960, 1500] {
            for w in [16, 200, 400, 800] {
                let r = ScrollRect {
                    x,
                    y: 100,
                    width: w,
                    height: 500,
                };
                let dock = plan_scroll_panel(r, 1920., false);
                let region = bounds(x as f32, 100., w as f32, 500.);
                for side in [ScrollPanelDock::Left, ScrollPanelDock::Right] {
                    if dock != side {
                        continue; // only the planned side actually docks
                    }
                    let (top, left) = scroll_panel_margins(r, 1920., 1080., panel_h, side);
                    let panel = bounds(left, top, SCROLL_PANEL_W, panel_h);
                    let inter = region.intersect(&panel);
                    assert!(
                        f32::from(inter.size.width) <= 0. || f32::from(inter.size.height) <= 0.,
                        "panel {panel:?} must not overlap the region {region:?} ({side:?})"
                    );
                    // and stays fully on the output
                    assert!(left >= 0. && left + SCROLL_PANEL_W <= 1920.);
                    assert!(top >= SCROLL_PANEL_MARGIN && top + panel_h <= 1080.);
                }
            }
        }
    }

    #[test]
    fn grab_chip_stays_outside_the_capture_rect() {
        // Above by default, centered on the frame.
        let chip = frame_grab_chip(rect(100, 100), 1920., 1080.).unwrap();
        assert_eq!(
            f32::from(chip.origin.x),
            100. + 400. / 2. - FRAME_CHIP_W / 2.
        );
        assert_eq!(
            f32::from(chip.origin.y),
            100. - FRAME_CHIP_GAP - FRAME_CHIP_H
        );
        // At the output top it flips below the frame.
        let chip = frame_grab_chip(rect(100, 0), 1920., 1080.).unwrap();
        assert_eq!(f32::from(chip.origin.y), 0. + 300. + FRAME_CHIP_GAP);
        // Full height leaves nowhere legal: no chip at all.
        let full = ScrollRect {
            x: 100,
            y: 0,
            width: 400,
            height: 1080,
        };
        assert!(frame_grab_chip(full, 1920., 1080.).is_none());
        // Never outside the output horizontally.
        let chip = frame_grab_chip(rect(1800, 100), 1920., 1080.).unwrap();
        assert!(f32::from(chip.right()) <= 1920.);
    }

    #[test]
    fn frame_toolbar_below_by_default_flips_at_the_top_clamps_x() {
        let dock = ScrollPanelDock::Right;
        // centered under the frame's bottom edge, one gap below
        let below = frame_toolbar(rect(100, 100), 1920., 1080., dock).unwrap();
        assert_eq!(
            (
                f32::from(below.origin.x),
                f32::from(below.origin.y),
                f32::from(below.size.width),
                f32::from(below.size.height)
            ),
            (
                100. + 400. / 2. - FRAME_TB_W / 2.,
                100. + 300. + 10.,
                FRAME_TB_W,
                FRAME_TB_H
            )
        );
        // a frame near the screen bottom: the toolbar flips ABOVE the
        // frame's top edge instead of leaving the output
        let r = ScrollRect {
            x: 100,
            y: 760,
            width: 400,
            height: 300,
        };
        let flipped = frame_toolbar(r, 1920., 1080., dock).unwrap();
        assert_eq!(f32::from(flipped.origin.y), 760. - 10. - FRAME_TB_H);
        // near the right edge the bar pins into the output
        let r = ScrollRect {
            x: 1800,
            y: 100,
            width: 100,
            height: 100,
        };
        let pinned = frame_toolbar(r, 1920., 1080., dock).unwrap();
        assert_eq!(f32::from(pinned.origin.x), 1920. - FRAME_TB_W);
    }

    #[test]
    fn frame_toolbar_never_sits_inside_a_full_height_rect() {
        // The historical bug: a full-height selection (the classic
        // tall-column long screenshot) flipped the toolbar "above" and
        // clamped it to y = 0 — INSIDE the capture region, so every
        // folded frame carried a 198×32 toolbar at its top. The
        // ladder must find somewhere outside or hide the bar instead.
        let full_height = ScrollRect {
            x: 760,
            y: 0,
            width: 400,
            height: 1080,
        };
        // sides fit (760 left, 760 right) — the toolbar docks into the
        // panel-FREE strip's top; with the panel right that is the left
        // strip (verified in full below), and it must never overlap the
        // capture region.
        let docked = frame_toolbar(full_height, 1920., 1080., ScrollPanelDock::Right).unwrap();
        let region = bounds(
            full_height.x as f32,
            full_height.y as f32,
            full_height.width as f32,
            full_height.height as f32,
        );
        let inter = region.intersect(&docked);
        assert!(
            f32::from(inter.size.width) <= 0. || f32::from(inter.size.height) <= 0.,
            "toolbar must not overlap the capture region: {docked:?} ∩ {region:?}"
        );

        // the panel occupies the right strip → the toolbar takes the
        // panel-free LEFT strip
        let left = frame_toolbar(full_height, 1920., 1080., ScrollPanelDock::Right).unwrap();
        assert_eq!(
            (f32::from(left.origin.x), f32::from(left.origin.y)),
            (SCROLL_PANEL_MARGIN, SCROLL_PANEL_MARGIN)
        );

        // panel LEFT → toolbar RIGHT
        let right = frame_toolbar(full_height, 1920., 1080., ScrollPanelDock::Left).unwrap();
        assert_eq!(
            (f32::from(right.origin.x), f32::from(right.origin.y)),
            (
                1920. - FRAME_TB_W - SCROLL_PANEL_MARGIN,
                SCROLL_PANEL_MARGIN
            )
        );

        // full-screen rect: no strip exists anywhere — hidden, never
        // inside (whatever the panel does)
        let full_screen = ScrollRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            frame_toolbar(full_screen, 1920., 1080., ScrollPanelDock::Right),
            None
        );
        assert_eq!(
            frame_toolbar(full_screen, 1920., 1080., ScrollPanelDock::Hidden),
            None
        );
    }

    #[test]
    fn scroll_panel_docks_where_it_fits() {
        let full = |w: i32| ScrollRect {
            x: 0,
            y: 0,
            width: w,
            height: 1080,
        };
        // partial-width rect: the wider free side wins
        assert_eq!(
            plan_scroll_panel(full(1000), 1920., false),
            ScrollPanelDock::Right
        );
        assert_eq!(
            plan_scroll_panel(
                ScrollRect {
                    x: 920,
                    y: 0,
                    width: 1000,
                    height: 1080
                },
                1920.,
                false
            ),
            ScrollPanelDock::Left
        );
        // one-sided fit docks that side (320 ≥ 276 on the right)
        assert_eq!(
            plan_scroll_panel(full(1600), 1920., false),
            ScrollPanelDock::Right
        );
        // a 220px remainder fits nothing — the strip threshold is 276
        assert_eq!(
            plan_scroll_panel(full(1700), 1920., false),
            ScrollPanelDock::Hidden
        );
        // neither side fits: another output wins over hiding
        assert_eq!(
            plan_scroll_panel(full(1920), 1920., true),
            ScrollPanelDock::OtherOutput
        );
        assert_eq!(
            plan_scroll_panel(full(1920), 1920., false),
            ScrollPanelDock::Hidden
        );
    }
}
