//! # Selection state machine: the pure-logic core of overlay interaction
//!
//! Lifecycle: `Idle` (no selection) → `Dragging` (held down) → `Selected`
//! (released and finalized). A finalized selection is **editable in place**:
//! pressing an edge/corner handle enters `Resizing`, pressing the interior
//! enters `Moving`; both release back into `Selected` (see
//! [`Selection::begin_edit`]). A press outside the selection starts a fresh
//! drag exactly as before. No rendering/UI dependencies → unit-testable
//! without a compositor; the render layer only reads [`Selection::bounds`].
//!
//! Interaction semantics (since v0.2):
//! - An in-place **click** (drag < 2px) clears the selection instead of
//!   producing a weird 0×0 selection
//! - [`Selection::cancel_drag`] while dragging (Esc) only abandons this drag
//!   (and reverts an in-flight move/resize to the pre-edit bounds)

use gpui_kit::*;

/// Threshold for "click, not drag" (logical pixels): a selection with either
/// side smaller than this is considered meaningless
const MIN_SIZE: f32 = 2.0;

/// Hit tolerance around the selection's edges/corners (logical pixels):
/// a press within this band of an edge grabs that edge instead of the
/// interior. Deliberately generous — remote mice and fractional-DPI
/// crossings cost a pixel or two.
pub(crate) const HANDLE_HIT: f32 = 8.0;

/// Painted handle-dot diameter (logical pixels) — see
/// `ui::hud::paint_handle_dot`. Deliberately smaller than
/// [`HANDLE_HIT`]: what you SEE and what you can GRAB are separate
/// budgets (a small dot stays quiet, the grab band stays generous).
/// Restyles may shrink the dot; never shrink the band to match.
pub(crate) const HANDLE_VIS: f32 = 7.0;

/// The dot must stay inside the grab band (see/grab separation, issue
/// #18): restyles may shrink the visible handle, never the grab
/// comfort. A compile-time check, so the two constants cannot drift
/// into coupling unnoticed.
const _: () = assert!(HANDLE_VIS < HANDLE_HIT);

/// One wheel notch scales a finalized selection by this factor; its
/// reciprocal shrinks it. ~7 notches double the box — fast enough to
/// reframe in one gesture, fine enough not to overshoot.
pub(crate) const ZOOM_STEP: f32 = 1.1;

/// Which edge(s) a resize grab drags. The eight zones mirror the classic
/// screenshot-tool handles (4 corners + 4 edge midpoints).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Handle {
    /// The dragged axes as (horizontal, vertical); `Some(true)` = the max
    /// edge (right/bottom), `Some(false)` = the min edge (left/top),
    /// `None` = the axis is pinned.
    pub(crate) fn axes(self) -> (Option<bool>, Option<bool>) {
        match self {
            Self::Left => (Some(false), None),
            Self::Right => (Some(true), None),
            Self::Top => (None, Some(false)),
            Self::Bottom => (None, Some(true)),
            Self::TopLeft => (Some(false), Some(false)),
            Self::TopRight => (Some(true), Some(false)),
            Self::BottomLeft => (Some(false), Some(true)),
            Self::BottomRight => (Some(true), Some(true)),
        }
    }
}

/// What a press at a point means for a finalized selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PressTarget {
    Handle(Handle),
    Interior,
    Outside,
}

/// Hit-test a press against a finalized selection: edges/corners first
/// (within [`HANDLE_HIT`]), then the interior, then outside. Geometry
/// matters: corners are square zones (they may stick out past the box),
/// but an EDGE handle only counts along its edge's own span — a press
/// 50px past the right end of the top edge is NOT a grab, it is a fresh
/// selection (an axis-independent band would turn the edges' extensions
/// into invisible grab zones — caught by the multi-output overlay test).
/// On very small selections, where both edges of an axis fall inside the
/// hit band, the NEARER edge wins — a grab is still a grab, just on one
/// side.
pub fn press_target(b: Bounds<Pixels>, p: Point<Pixels>) -> PressTarget {
    let (l, r, t, bt) = (
        f32::from(b.left()),
        f32::from(b.right()),
        f32::from(b.top()),
        f32::from(b.bottom()),
    );
    let (px, py) = (f32::from(p.x), f32::from(p.y));
    // zone per axis: Some(false) = min edge, Some(true) = max edge
    let x = axis_zone(px, l, r);
    let y = axis_zone(py, t, bt);
    let (within_x, within_y) = (px >= l && px <= r, py >= t && py <= bt);
    match (x, y) {
        (Some(false), Some(false)) => PressTarget::Handle(Handle::TopLeft),
        (Some(true), Some(false)) => PressTarget::Handle(Handle::TopRight),
        (Some(false), Some(true)) => PressTarget::Handle(Handle::BottomLeft),
        (Some(true), Some(true)) => PressTarget::Handle(Handle::BottomRight),
        (Some(false), None) if within_y => PressTarget::Handle(Handle::Left),
        (Some(true), None) if within_y => PressTarget::Handle(Handle::Right),
        (None, Some(false)) if within_x => PressTarget::Handle(Handle::Top),
        (None, Some(true)) if within_x => PressTarget::Handle(Handle::Bottom),
        _ if b.contains(&p) => PressTarget::Interior,
        _ => PressTarget::Outside,
    }
}

/// Which edge (if either) of an axis is within [`HANDLE_HIT`] of `v`.
/// `false` = min edge, `true` = max edge; ties (tiny boxes) → min edge.
fn axis_zone(v: f32, lo: f32, hi: f32) -> Option<bool> {
    let (dlo, dhi) = ((v - lo).abs(), (v - hi).abs());
    if dlo <= HANDLE_HIT && dhi <= HANDLE_HIT {
        Some(dhi < dlo) // tiny box: the nearer edge takes the grab (ties → min edge)
    } else if dlo <= HANDLE_HIT {
        Some(false)
    } else if dhi <= HANDLE_HIT {
        Some(true)
    } else {
        None
    }
}

/// Clamp that never panics: when lo > hi (a degenerate desktop vs. an
/// oversized selection), pin to lo. Same posture as placement's max()
/// guard — f32::clamp panics on inverted bounds.
fn clamp_to(v: f32, lo: f32, hi: f32) -> f32 {
    if lo > hi { lo } else { v.clamp(lo, hi) }
}

/// The pure geometry behind [`Selection::zoom`]: scale about the
/// centre, keeping the aspect ratio, clamped by the desktop and
/// [`MIN_SIZE`] through the factor.
fn zoom_bounds(b: Bounds<Pixels>, steps: i32, desktop: Bounds<Pixels>) -> Bounds<Pixels> {
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    if w <= 0. || h <= 0. {
        return b;
    }
    let cx = (f32::from(b.left()) + f32::from(b.right())) * 0.5;
    let cy = (f32::from(b.top()) + f32::from(b.bottom())) * 0.5;
    let (dl, dr) = (f32::from(desktop.left()), f32::from(desktop.right()));
    let (dt, db) = (f32::from(desktop.top()), f32::from(desktop.bottom()));
    // Headroom expressed as a FACTOR: the centre is pinned, so each
    // axis may at most double before its nearer desktop edge is hit.
    let room = ((2. * (cx - dl).min(dr - cx)) / w).min((2. * (cy - dt).min(db - cy)) / h);
    // Same for the floor: MIN_SIZE on the tighter axis governs.
    let floor = (MIN_SIZE / w).max(MIN_SIZE / h);
    let wanted = ZOOM_STEP.powi(steps);
    let factor = if steps > 0 {
        wanted.min(room).max(1.)
    } else {
        wanted.max(floor).min(1.)
    };
    if factor == 1. {
        return b;
    }
    let (nw, nh) = (w * factor, h * factor);
    // Float guard on top of `room`: a 0.1px drift at the desktop edge
    // must not re-introduce the "selection leaks past the screen" bug.
    // In-bounds inputs leave this a no-op (the centre is preserved).
    Bounds {
        origin: point(
            px(clamp_to(cx - nw * 0.5, dl, dr - nw)),
            px(clamp_to(cy - nh * 0.5, dt, db - nh)),
        ),
        size: size(px(nw), px(nh)),
    }
}

#[derive(Clone, Copy)]
pub enum Selection {
    Idle,
    Dragging {
        start: Point<Pixels>,
        current: Point<Pixels>,
    },
    Selected {
        bounds: Bounds<Pixels>,
    },
    /// Dragging a finalized selection around by its interior. `grab` is the
    /// press offset from the origin (press − origin), so the box follows the
    /// pointer without jumping. `restore` is the pre-edit bounds (Esc).
    Moving {
        bounds: Bounds<Pixels>,
        grab: Point<Pixels>,
        restore: Bounds<Pixels>,
    },
    /// Resizing via an edge/corner handle. The dragged edge(s) follow the
    /// pointer; the opposite edges stay pinned. `restore` is the pre-edit
    /// bounds (Esc).
    Resizing {
        bounds: Bounds<Pixels>,
        handle: Handle,
        restore: Bounds<Pixels>,
    },
}

impl Selection {
    /// Press: start a new selection (also used to re-select while Selected)
    pub fn begin(&mut self, p: Point<Pixels>) {
        *self = Self::Dragging {
            start: p,
            current: p,
        };
    }

    /// Drag to p; returns whether anything actually changed (callers decide
    /// on cx.notify() accordingly)
    pub fn drag_to(&mut self, p: Point<Pixels>) -> bool {
        if let Self::Dragging { current, .. } = self
            && *current != p
        {
            *current = p;
            return true;
        }
        false
    }

    /// Release: a real drag → `Selected` (corners normalized, dragging
    /// up-left works too); an in-place click (< [`MIN_SIZE`]) → `Idle`
    /// (clears the selection)
    pub fn end(&mut self, p: Point<Pixels>) {
        if let Self::Dragging { start, .. } = *self {
            let bounds = Self::normalized(start, p);
            *self = if f32::from(bounds.size.width) < MIN_SIZE
                || f32::from(bounds.size.height) < MIN_SIZE
            {
                Self::Idle
            } else {
                Self::Selected { bounds }
            };
        }
    }

    /// Cancel while dragging (first-stage Esc semantics); also reverts an
    /// in-flight edit (move/resize) to its pre-edit bounds — the selection
    /// survives, only the edit is abandoned. No-op otherwise.
    pub fn cancel_drag(&mut self) {
        if self.cancel_edit() {
            return; // edit reverted to `Selected`; the drag case below is exclusive
        }
        if matches!(self, Self::Dragging { .. }) {
            *self = Self::Idle;
        }
    }

    /// Press on a finalized selection: an edge/corner band grabs that
    /// handle, the interior starts a move. Returns false when the press is
    /// outside (or nothing is selected) — the caller then starts a fresh
    /// drag via [`Selection::begin`].
    pub fn begin_edit(&mut self, p: Point<Pixels>) -> bool {
        let Self::Selected { bounds } = *self else {
            return false;
        };
        match press_target(bounds, p) {
            PressTarget::Handle(handle) => {
                *self = Self::Resizing {
                    bounds,
                    handle,
                    restore: bounds,
                };
                true
            }
            PressTarget::Interior => {
                *self = Self::Moving {
                    bounds,
                    grab: p - bounds.origin,
                    restore: bounds,
                };
                true
            }
            PressTarget::Outside => false,
        }
    }

    /// Live-update an in-flight edit (move or resize) to the pointer
    /// position. `desktop` (the union of all screens) clamps the result —
    /// a selection cannot be dragged off the captured area, and a resize
    /// never flips past the opposite edge ([`MIN_SIZE`] holds). Returns
    /// whether anything changed.
    pub fn edit_to(&mut self, p: Point<Pixels>, desktop: Bounds<Pixels>) -> bool {
        // Match on `self` (not `*self`): bindings must be ref-mut place
        // projections, or the mutations below would hit copies — the same
        // pitfall `drag_to` avoids via match ergonomics.
        match self {
            Self::Moving { bounds, grab, .. } => {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let nx = clamp_to(
                    f32::from(p.x) - f32::from(grab.x),
                    f32::from(desktop.left()),
                    f32::from(desktop.right()) - w,
                );
                let ny = clamp_to(
                    f32::from(p.y) - f32::from(grab.y),
                    f32::from(desktop.top()),
                    f32::from(desktop.bottom()) - h,
                );
                if nx == f32::from(bounds.origin.x) && ny == f32::from(bounds.origin.y) {
                    return false;
                }
                bounds.origin = point(px(nx), px(ny));
                true
            }
            Self::Resizing { bounds, handle, .. } => {
                let (l, r, t, b) = (
                    f32::from(bounds.left()),
                    f32::from(bounds.right()),
                    f32::from(bounds.top()),
                    f32::from(bounds.bottom()),
                );
                let (dl, dr, dt, db) = (
                    f32::from(desktop.left()),
                    f32::from(desktop.right()),
                    f32::from(desktop.top()),
                    f32::from(desktop.bottom()),
                );
                let (mut nl, mut nr, mut nt, mut nb) = (l, r, t, b);
                let (hx, hy) = handle.axes();
                match hx {
                    Some(false) => nl = clamp_to(f32::from(p.x), dl, nr - MIN_SIZE),
                    Some(true) => nr = clamp_to(f32::from(p.x), nl + MIN_SIZE, dr),
                    None => {}
                }
                match hy {
                    Some(false) => nt = clamp_to(f32::from(p.y), dt, nb - MIN_SIZE),
                    Some(true) => nb = clamp_to(f32::from(p.y), nt + MIN_SIZE, db),
                    None => {}
                }
                if (nl, nr, nt, nb) == (l, r, t, b) {
                    return false;
                }
                bounds.origin = point(px(nl), px(nt));
                bounds.size = size(px(nr - nl), px(nb - nt));
                true
            }
            _ => false,
        }
    }

    /// Wheel-zoom a finalized selection in place: `steps` notches at
    /// [`ZOOM_STEP`], positive to grow, negative to shrink. Both axes
    /// share one factor (equal ratio — the crop keeps its shape) and
    /// the box's CENTRE is the anchor, so "I framed it a bit too
    /// small" grows outward evenly instead of crawling away from the
    /// cursor.
    ///
    /// The desktop and [`MIN_SIZE`] limits clamp the FACTOR, never the
    /// individual edges: clamping edges would break the ratio and slide
    /// the centre. A box already flush against the desktop simply stops
    /// growing on the tighter axis (the other axis follows, since they
    /// share the factor).
    pub fn zoom(&mut self, steps: i32, desktop: Bounds<Pixels>) -> bool {
        if steps == 0 {
            return false;
        }
        // Only a FINALIZED box zooms: an in-flight drag/edit already
        // owns the pointer gesture, and silently resizing under it would
        // fight the user's own motion.
        let Self::Selected { bounds: before } = *self else {
            return false;
        };
        let after = zoom_bounds(before, steps, desktop);
        if after == before {
            return false;
        }
        *self = Self::Selected { bounds: after };
        true
    }

    /// Release after a move/resize: the (already clamped) bounds become the
    /// finalized selection. An in-place press with no movement round-trips
    /// the original bounds — a click inside the selection keeps it.
    pub fn end_edit(&mut self) {
        if let Self::Moving { bounds, .. } | Self::Resizing { bounds, .. } = *self {
            *self = Self::Selected { bounds };
        }
    }

    /// Esc during a move/resize: revert to the pre-edit bounds. Returns
    /// whether an edit was reverted.
    pub fn cancel_edit(&mut self) -> bool {
        if let Self::Moving { restore, .. } | Self::Resizing { restore, .. } = *self {
            *self = Self::Selected { bounds: restore };
            true
        } else {
            false
        }
    }

    /// Move/resize in flight?
    pub fn is_editing(&self) -> bool {
        matches!(self, Self::Moving { .. } | Self::Resizing { .. })
    }

    /// Current selection (counts while dragging too — used by the live size label)
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        match *self {
            Self::Idle => None,
            Self::Dragging { start, current } => Some(Self::normalized(start, current)),
            Self::Selected { bounds }
            | Self::Moving { bounds, .. }
            | Self::Resizing { bounds, .. } => Some(bounds),
        }
    }

    /// Two corners → normalized Bounds (top-left = origin, positive size).
    /// `Bounds::from_corners` must not be used: it does not sort, so dragging
    /// toward the top-left produces negative width/height (a latent v0.1 bug:
    /// an "invisible selection" that made Enter report an empty selection)
    fn normalized(a: Point<Pixels>, b: Point<Pixels>) -> Bounds<Pixels> {
        let (left, right) = if a.x <= b.x { (a.x, b.x) } else { (b.x, a.x) };
        let (top, bottom) = if a.y <= b.y { (a.y, b.y) } else { (b.y, a.y) };
        Bounds {
            origin: point(left, top),
            size: size(right - left, bottom - top),
        }
    }

    pub fn is_dragging(&self) -> bool {
        matches!(self, Self::Dragging { .. })
    }

    /// Finalized? (the toolbar only appears in this state)
    pub fn is_selected(&self) -> bool {
        matches!(self, Self::Selected { .. })
    }
}

#[cfg(test)]
mod tests {
    // Deliberately not `use super::*`: the parent module's `use gpui_kit::*`
    // pulls in gpui's own `test` attribute macro which shadows the built-in
    // #[test] (macro expansion hits the recursion limit)
    use super::{Handle, PressTarget, Selection, press_target};
    use gpui_kit::{Bounds, Pixels, Point, point, px, size};

    fn pt(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    fn desk(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    fn size_of(s: &Selection) -> (f32, f32) {
        let b = s.bounds().expect("selection should exist");
        (f32::from(b.size.width), f32::from(b.size.height))
    }

    /// A 100×100 finalized selection at (100,100) — the editing test bed.
    fn selected() -> Selection {
        let mut s = Selection::Idle;
        s.begin(pt(100., 100.));
        s.drag_to(pt(200., 200.));
        s.end(pt(200., 200.));
        assert!(s.is_selected());
        s
    }

    #[test]
    fn drag_up_left_normalizes_bounds() {
        let mut s = Selection::Idle;
        s.begin(pt(100., 100.));
        s.drag_to(pt(40., 60.));
        s.end(pt(40., 60.));
        let b = s.bounds().unwrap();
        assert_eq!(f32::from(b.left()), 40.);
        assert_eq!(f32::from(b.top()), 60.);
        assert_eq!(size_of(&s), (60., 40.));
        assert!(s.is_selected());
    }

    #[test]
    fn in_place_click_clears_selection() {
        let mut s = Selection::Idle;
        s.begin(pt(50., 50.));
        s.end(pt(50., 50.));
        assert!(matches!(s, Selection::Idle));
    }

    #[test]
    fn tiny_drag_treated_as_click() {
        let mut s = Selection::Idle;
        s.begin(pt(10., 10.));
        s.end(pt(11.5, 10.)); // 1.5px < 2px
        assert!(matches!(s, Selection::Idle));
    }

    #[test]
    fn click_with_existing_selection_restarts() {
        let mut s = Selection::Idle;
        s.begin(pt(0., 0.));
        s.end(pt(100., 100.));
        assert!(s.is_selected());
        s.begin(pt(200., 200.)); // press while Selected = new selection
        assert!(s.is_dragging());
    }

    #[test]
    fn cancel_drag_returns_to_idle_without_touching_finalized() {
        let mut s = Selection::Idle;
        s.begin(pt(0., 0.));
        s.drag_to(pt(30., 30.));
        s.cancel_drag();
        assert!(matches!(s, Selection::Idle));

        let mut s = Selection::Idle;
        s.begin(pt(0., 0.));
        s.end(pt(30., 30.));
        s.cancel_drag(); // already finalized: untouched (Esc means exit here)
        assert!(s.is_selected());
    }

    #[test]
    fn drag_to_returns_false_when_unchanged() {
        let mut s = Selection::Idle;
        s.begin(pt(5., 5.));
        assert!(s.drag_to(pt(9., 5.)));
        assert!(!s.drag_to(pt(9., 5.))); // same position: no duplicate notify
        // drag while Idle is a no-op
        let mut s = Selection::Idle;
        assert!(!s.drag_to(pt(9., 5.)));
    }

    // ── In-place editing: hit zones ────────────────────────────────

    #[test]
    fn press_target_maps_all_eight_handles_interior_and_outside() {
        let b = desk(100., 100., 100., 100.);
        for (p, expected) in [
            (pt(100., 100.), Handle::TopLeft),
            (pt(150., 100.), Handle::Top),
            (pt(200., 100.), Handle::TopRight),
            (pt(200., 150.), Handle::Right),
            (pt(200., 200.), Handle::BottomRight),
            (pt(150., 200.), Handle::Bottom),
            (pt(100., 200.), Handle::BottomLeft),
            (pt(100., 150.), Handle::Left),
            // just inside the 8px band, off-center
            (pt(105., 103.), Handle::TopLeft),
            (pt(196., 150.), Handle::Right),
            (pt(150., 194.), Handle::Bottom),
        ] {
            assert_eq!(press_target(b, p), PressTarget::Handle(expected), "{p:?}");
        }
        assert_eq!(press_target(b, pt(150., 150.)), PressTarget::Interior);
        assert_eq!(press_target(b, pt(150., 111.)), PressTarget::Interior); // past the band
        assert_eq!(press_target(b, pt(50., 50.)), PressTarget::Outside);
        assert_eq!(press_target(b, pt(350., 150.)), PressTarget::Outside);
    }

    #[test]
    fn narrow_selection_grabs_the_nearer_corner_half() {
        // 10px box: both edges of each axis are inside the hit band — the
        // whole thing reads as corners, nearer edge per axis
        let b = desk(100., 100., 10., 10.);
        assert_eq!(
            press_target(b, pt(105., 105.)),
            PressTarget::Handle(Handle::TopLeft) // ties → min edges
        );
        assert_eq!(
            press_target(b, pt(107., 105.)),
            PressTarget::Handle(Handle::TopRight) // nearer to right, tie on y → top
        );
        assert_eq!(
            press_target(b, pt(103., 108.)),
            PressTarget::Handle(Handle::BottomLeft)
        );
    }

    #[test]
    fn edge_handles_do_not_extend_past_their_corners() {
        let b = desk(100., 100., 100., 100.);
        // on the top edge's line, but 50px past its right corner: a press
        // here must start a FRESH selection, not grab the edge (regression:
        // axis-independent bands made the edges' extensions invisible grab
        // zones — caught live by the multi-output overlay test)
        assert_eq!(press_target(b, pt(250., 100.)), PressTarget::Outside);
        assert_eq!(press_target(b, pt(50., 150.)), PressTarget::Outside);
        // …but a few px past the corner along the perpendicular edge is
        // still a grab (the corner square sticks out)
        assert_eq!(
            press_target(b, pt(204., 150.)),
            PressTarget::Handle(Handle::Right)
        );
        assert_eq!(
            press_target(b, pt(205., 103.)),
            PressTarget::Handle(Handle::TopRight)
        );
    }

    #[test]
    fn handle_axes_cover_corners_edges_and_pins() {
        assert_eq!(Handle::TopLeft.axes(), (Some(false), Some(false)));
        assert_eq!(Handle::BottomRight.axes(), (Some(true), Some(true)));
        assert_eq!(Handle::TopRight.axes(), (Some(true), Some(false)));
        assert_eq!(Handle::BottomLeft.axes(), (Some(false), Some(true)));
        assert_eq!(Handle::Left.axes(), (Some(false), None));
        assert_eq!(Handle::Right.axes(), (Some(true), None));
        assert_eq!(Handle::Top.axes(), (None, Some(false)));
        assert_eq!(Handle::Bottom.axes(), (None, Some(true)));
    }

    // ── In-place editing: state transitions ────────────────────────

    #[test]
    fn begin_edit_inside_moves_outside_restarts() {
        let mut s = selected();
        assert!(s.begin_edit(pt(150., 150.))); // interior → Moving
        assert!(s.is_editing());
        assert!(!s.is_selected()); // toolbar hides while editing
        let mut s = selected();
        assert!(s.begin_edit(pt(200., 200.))); // corner band → Resizing
        assert!(s.is_editing());
        let mut s = selected();
        assert!(!s.begin_edit(pt(400., 400.))); // outside → caller restarts
        assert!(s.is_selected());
        let mut s = Selection::Idle;
        assert!(!s.begin_edit(pt(0., 0.))); // nothing to edit
    }

    #[test]
    fn moving_translates_without_jump_and_clamps_to_desktop() {
        let mut s = selected();
        s.begin_edit(pt(130., 160.)); // grab at +30,+60 from origin
        assert!(s.edit_to(pt(60., 120.), desk(0., 0., 500., 500.)));
        let b = s.bounds().unwrap();
        // origin = pointer − grab offset: the box follows, it doesn't jump
        assert_eq!(b.origin, point(px(30.), px(60.)));
        assert_eq!(size_of(&s), (100., 100.));
        // pushing far past the desktop's right/bottom edge clamps flush
        assert!(s.edit_to(pt(900., 900.), desk(0., 0., 500., 500.)));
        let b = s.bounds().unwrap();
        assert_eq!(b.right(), px(500.));
        assert_eq!(b.bottom(), px(500.));
        // far past top-left clamps to the desktop origin
        assert!(s.edit_to(pt(-900., -900.), desk(0., 0., 500., 500.)));
        let b = s.bounds().unwrap();
        assert_eq!(b.origin, point(px(0.), px(0.)));
        // unchanged position reports false (no duplicate notify)
        assert!(!s.edit_to(pt(-900., -900.), desk(0., 0., 500., 500.)));
    }

    #[test]
    fn resizing_every_handle_clamps_min_size_and_desktop() {
        for handle in [
            Handle::TopLeft,
            Handle::Top,
            Handle::TopRight,
            Handle::Left,
            Handle::Right,
            Handle::BottomLeft,
            Handle::Bottom,
            Handle::BottomRight,
        ] {
            let mut s = selected();
            let press = match handle {
                Handle::TopLeft => pt(100., 100.),
                Handle::Top => pt(150., 100.),
                Handle::TopRight => pt(200., 100.),
                Handle::Left => pt(100., 150.),
                Handle::Right => pt(200., 150.),
                Handle::BottomLeft => pt(100., 200.),
                Handle::Bottom => pt(150., 200.),
                Handle::BottomRight => pt(200., 200.),
            };
            assert!(s.begin_edit(press), "{handle:?}");
            // drag wildly past the opposite edge and past the desktop:
            // no flip, ≥ MIN_SIZE, and inside the desktop
            assert!(
                s.edit_to(pt(-500., -500.), desk(0., 0., 500., 500.)),
                "{handle:?}"
            );
            let b = s.bounds().unwrap();
            assert!(f32::from(b.size.width) >= 2., "{handle:?}");
            assert!(f32::from(b.size.height) >= 2., "{handle:?}");
            assert!(b.left() >= px(0.) && b.top() >= px(0.), "{handle:?}");
            assert!(
                b.right() <= px(500.) && b.bottom() <= px(500.),
                "{handle:?}"
            );

            let mut s = selected();
            assert!(s.begin_edit(press));
            assert!(
                s.edit_to(pt(900., 900.), desk(0., 0., 500., 500.)),
                "{handle:?}"
            );
            let b = s.bounds().unwrap();
            assert!(f32::from(b.size.width) >= 2., "{handle:?}");
            assert!(f32::from(b.size.height) >= 2., "{handle:?}");
            assert!(b.left() >= px(0.) && b.top() >= px(0.), "{handle:?}");
            assert!(
                b.right() <= px(500.) && b.bottom() <= px(500.),
                "{handle:?}"
            );
        }
    }

    #[test]
    fn resize_shrinks_then_grows_without_flipping() {
        let mut s = selected();
        s.begin_edit(pt(100., 100.)); // TopLeft
        assert!(s.edit_to(pt(180., 190.), desk(0., 0., 500., 500.)));
        let b = s.bounds().unwrap();
        assert_eq!(b.origin, point(px(180.), px(190.)));
        assert_eq!(size_of(&s), (20., 10.));
        // pushing past the bottom-right clamps at MIN_SIZE, never flips
        assert!(s.edit_to(pt(350., 350.), desk(0., 0., 500., 500.)));
        let b = s.bounds().unwrap();
        assert_eq!(b.origin, point(px(198.), px(198.)));
        assert_eq!(size_of(&s), (2., 2.));
        // and back out: the handle still follows
        assert!(s.edit_to(pt(60., 60.), desk(0., 0., 500., 500.)));
        let b = s.bounds().unwrap();
        assert_eq!(b.origin, point(px(60.), px(60.)));
        assert_eq!(size_of(&s), (140., 140.));
    }

    #[test]
    fn end_edit_finalizes_and_in_place_click_keeps_bounds() {
        let mut s = selected();
        s.begin_edit(pt(150., 150.));
        s.edit_to(pt(40., 60.), desk(0., 0., 500., 500.));
        s.end_edit();
        assert!(s.is_selected());
        assert!(!s.is_editing());
        let b = s.bounds().unwrap();
        // grab was (+50,+50) from the origin → pointer (40,60) lands at
        // (-10,10), clamped to the desktop's left edge → (0,10)
        assert_eq!(b.origin, point(px(0.), px(10.)));

        // press-release without moving: the selection survives as-is
        let mut s = selected();
        s.begin_edit(pt(150., 150.));
        s.end_edit();
        assert!(s.is_selected());
        assert_eq!(s.bounds().unwrap().origin, point(px(100.), px(100.)));
    }

    #[test]
    fn cancel_edit_reverts_to_pre_edit_bounds() {
        let mut s = selected();
        s.begin_edit(pt(150., 150.));
        s.edit_to(pt(40., 60.), desk(0., 0., 500., 500.));
        s.cancel_drag(); // Esc during the edit
        assert!(s.is_selected());
        assert_eq!(s.bounds().unwrap().origin, point(px(100.), px(100.)));

        let mut s = selected();
        s.begin_edit(pt(200., 200.)); // resize grab
        s.edit_to(pt(400., 400.), desk(0., 0., 500., 500.));
        s.cancel_drag();
        assert_eq!(size_of(&s), (100., 100.));
    }

    #[test]
    fn edit_to_outside_an_edit_is_a_no_op() {
        let mut s = selected();
        assert!(!s.edit_to(pt(0., 0.), desk(0., 0., 500., 500.)));
        assert!(s.is_selected()); // untouched
    }

    // ── Wheel zoom ─────────────────────────────────────────────

    fn centre(b: gpui_kit::Bounds<Pixels>) -> (f32, f32) {
        (
            (f32::from(b.left()) + f32::from(b.right())) * 0.5,
            (f32::from(b.top()) + f32::from(b.bottom())) * 0.5,
        )
    }

    #[test]
    fn wheel_zoom_scales_both_axes_about_the_centre() {
        let mut s = selected(); // 100×100 at (100,100), centre (150,150)
        let before = s.bounds().unwrap();
        assert!(s.zoom(1, desk(0., 0., 500., 500.)));
        let b = s.bounds().unwrap();
        let fx = f32::from(b.size.width) / f32::from(before.size.width);
        let fy = f32::from(b.size.height) / f32::from(before.size.height);
        assert!((fx - 1.1).abs() < 1e-4, "one notch = one ZOOM_STEP: {fx}");
        assert!((fx - fy).abs() < 1e-4, "equal ratio: {fx} vs {fy}");
        let (cx, cy) = centre(b);
        assert!(
            (cx - 150.).abs() < 1e-3 && (cy - 150.).abs() < 1e-3,
            "{b:?}"
        );
        // the inverse notch restores the original box (factor 1.1 vs 1/1.1)
        assert!(s.zoom(-1, desk(0., 0., 500., 500.)));
        let b = s.bounds().unwrap();
        assert!((f32::from(b.size.width) - 100.).abs() < 1e-3, "{b:?}");
        assert!((f32::from(b.size.height) - 100.).abs() < 1e-3, "{b:?}");
    }

    #[test]
    fn wheel_zoom_keeps_the_ratio_and_caps_at_the_desktop() {
        let mut s = Selection::Idle;
        s.begin(pt(10., 10.));
        s.end(pt(50., 30.)); // 40×20, centre (30,20) → 30px to the left edge
        for _ in 0..40 {
            s.zoom(1, desk(0., 0., 500., 500.));
        }
        let b = s.bounds().unwrap();
        assert!((f32::from(b.size.width) / f32::from(b.size.height) - 2.).abs() < 1e-3);
        assert!(f32::from(b.left()).abs() < 1e-3, "flush left: {b:?}");
        assert!((f32::from(b.top())).abs() > 0., "the loose axis stops too");
        let (cx, cy) = centre(b);
        assert!((cx - 30.).abs() < 1e-3 && (cy - 20.).abs() < 1e-3, "{b:?}");
        assert!(!s.zoom(1, desk(0., 0., 500., 500.)), "no growth left");
    }

    #[test]
    fn wheel_zoom_out_floors_at_min_size() {
        let mut s = selected();
        for _ in 0..80 {
            s.zoom(-1, desk(0., 0., 500., 500.));
        }
        let b = s.bounds().unwrap();
        assert!((f32::from(b.size.width) - 2.).abs() < 1e-3, "{b:?}");
        assert!((f32::from(b.size.height) - 2.).abs() < 1e-3, "{b:?}");
        assert!(!s.zoom(-1, desk(0., 0., 500., 500.))); // further notches are no-ops
        let (cx, cy) = centre(b);
        assert!(
            (cx - 150.).abs() < 1e-3 && (cy - 150.).abs() < 1e-3,
            "{b:?}"
        );
    }

    #[test]
    fn wheel_zoom_only_touches_a_finalized_selection() {
        let mut s = Selection::Idle;
        assert!(!s.zoom(1, desk(0., 0., 500., 500.)));
        s.begin(pt(10., 10.));
        s.drag_to(pt(50., 50.));
        assert!(!s.zoom(1, desk(0., 0., 500., 500.)), "mid-drag");
        assert!(s.is_dragging());
    }
}
