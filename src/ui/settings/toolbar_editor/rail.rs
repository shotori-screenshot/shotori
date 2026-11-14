//! Hand-drawn horizontal scrollbar rail and edge auto-scroll helpers for
//! the toolbar layout editor's preview strip.
//!
//! Kept separate from the drag-and-drop item palette so the sign-domain
//! boundary (`strip_scroll_pos` / `set_strip_scroll_pos`), thumb travel
//! clamps and grab-point scrub math live directly next to their unit tests.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui::theme;

use super::super::SettingsWindow;

/// The drag payload marking a scrollbar-rail scrub in flight.
pub(super) struct RailScrub;

/// An empty drag ghost for the scrollbar scrub — the thumb itself is
/// the feedback (it re-renders from the handle on every scrub event),
/// so nothing follows the pointer.
pub(super) struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The scrub's ghost constructor (fn item, matching the higher-ranked
/// `impl Fn` bound `on_drag` demands).
fn rail_scrub_ghost(
    _: &RailScrub,
    _: Point<Pixels>,
    _: &mut Window,
    cx: &mut App,
) -> Entity<NoGhost> {
    cx.new(|_| NoGhost)
}

impl SettingsWindow {
    /// The preview strip's scrollbar: HAND-DRAWN, below and outside the
    /// bar's box, with its geometry linked straight to the scroll
    /// handle — thumb width = viewport/content, thumb x = offset/max.
    ///
    /// Both pieces are PILLS: the track is 2px with a 1px radius, the
    /// thumb 7px with a 3.5px radius.
    pub(super) fn scrollbar_rail(&self, cx: &Context<Self>) -> Stateful<Div> {
        let handle = &self.preview_scroll;
        let overflow = self
            .rail_has_overflow
            .unwrap_or_else(|| handle.max_offset().x > px(0.));
        let t = theme::c();
        let track_color = rgba((t.toolbar_border & 0xFFFFFF00) | 0x59);
        let thumb_color = if self.rail_hovered {
            rgba(t.btn_text)
        } else {
            rgba((t.btn_text & 0xFFFFFF00) | 0x99)
        };
        // Geometry is resolved at PAINT time from the live scroll handle,
        // not baked into laid-out children: a window resize re-lays-out
        // and repaints the cached element tree without calling `render`
        // again, so anything computed there kept the previous width until
        // the next scroll event.
        let state = self.preview_scroll.clone();
        div()
            .id("preview-scrollbar")
            .when(overflow, |rail| rail.mt(px(4.)).h(px(9.)))
            .when(!overflow, |rail| rail.h(px(0.)))
            .w_full()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_rail_press))
            .on_drag(RailScrub, rail_scrub_ghost)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.rail_hovered == *hovered {
                    return;
                }
                this.rail_hovered = *hovered;
                cx.notify();
            }))
            .when(overflow, |rail| {
                rail.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let max_x = state.max_offset().x;
                            if max_x <= px(0.) {
                                return;
                            }
                            let (thumb_w, usable) = rail_thumb_geometry(bounds.size.width, max_x);
                            let pos = strip_scroll_pos(&state).clamp(px(0.), max_x.max(px(0.)));
                            let thumb_x = if max_x > px(0.) {
                                usable * (pos / max_x)
                            } else {
                                px(0.)
                            };
                            window.paint_quad(
                                fill(
                                    Bounds::new(
                                        point(bounds.origin.x, bounds.origin.y + px(3.)),
                                        size(bounds.size.width, px(2.)),
                                    ),
                                    track_color,
                                )
                                .corner_radii(px(1.)),
                            );
                            window.paint_quad(
                                fill(
                                    Bounds::new(
                                        point(bounds.origin.x + thumb_x, bounds.origin.y),
                                        size(thumb_w, px(7.)),
                                    ),
                                    thumb_color,
                                )
                                .corner_radii(px(3.5)),
                            );
                        },
                    )
                    .size_full(),
                )
            })
    }

    /// The rail's drag body. Registered on the PAGE, not on the rail:
    /// `on_drag_move` follows the pointer over whatever is under it, and
    /// a 9px-tall rail loses the gesture the moment the pointer moves a
    /// few pixels vertically.
    pub(super) fn on_rail_scrub(
        &mut self,
        event: &DragMoveEvent<RailScrub>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let strip = self.preview_scroll.bounds();
        self.scrub_rail_to(strip, event.event.position, cx);
    }

    /// Capture the contact point inside the thumb on the arming press so
    /// grabbing the thumb by its edge keeps that edge under the cursor.
    fn on_rail_press(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let strip = self.preview_scroll.bounds();
        let handle = self.preview_scroll.clone();
        let max_x = handle.max_offset().x;
        if max_x <= px(0.) || strip.size.width <= px(0.) {
            return;
        }
        let (thumb_w, usable) = rail_thumb_geometry(strip.size.width, max_x);
        let pos = strip_scroll_pos(&handle).clamp(px(0.), max_x);
        let thumb_x = usable * (pos / max_x);
        self.rail_grab = Some(rail_grip(
            event.position.x - strip.origin.x,
            thumb_x,
            thumb_w,
        ));
    }

    /// Pointer → scroll offset with grab semantics (preserves the
    /// contact point recorded by [`Self::on_rail_press`]).
    fn scrub_rail_to(
        &mut self,
        strip: Bounds<Pixels>,
        pointer: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let handle = self.preview_scroll.clone();
        let max_x = handle.max_offset().x;
        if max_x <= px(0.) || strip.size.width <= px(0.) {
            return;
        }
        let (thumb_w, usable) = rail_thumb_geometry(strip.size.width, max_x);
        if usable <= px(0.) {
            return;
        }
        let grip = self
            .rail_grab
            .unwrap_or(thumb_w / 2.)
            .clamp(px(0.), thumb_w);
        let local_x = pointer.x - strip.origin.x;
        let fraction = scrub_fraction(local_x, grip, usable);
        set_strip_scroll_pos(&handle, px(fraction * max_x.as_f32()));
        cx.notify();
    }
}

fn scrub_fraction(local_x: Pixels, grip: Pixels, usable: Pixels) -> f32 {
    ((local_x - grip) / usable).clamp(0., 1.)
}

fn rail_grip(local_x: Pixels, thumb_x: Pixels, thumb_w: Pixels) -> Pixels {
    (local_x - thumb_x).clamp(px(0.), thumb_w)
}

fn rail_thumb_geometry(viewport_w: Pixels, max_x: Pixels) -> (Pixels, Pixels) {
    let content_w = (viewport_w + max_x.max(px(0.))).max(px(1.));
    let ratio = (viewport_w / content_w).clamp(0., 1.);
    // Keep at least MIN_THUMB grabbable width and MIN_TRAVEL free track
    // even when a row overflows by only a few pixels.
    const MIN_THUMB: f32 = 24.;
    const MIN_TRAVEL: f32 = 32.;
    let thumb = px(ratio * viewport_w.as_f32())
        .max(px(MIN_THUMB))
        .min((viewport_w - px(MIN_TRAVEL)).max(px(MIN_THUMB)));
    let usable = (viewport_w - thumb).max(px(0.));
    (thumb, usable)
}

/// gpui stores a scroll container's offset NEGATED (`0` at start,
/// `-max_offset()` at end). Every read/write of `preview_scroll` goes
/// through these two helpers so the sign flip lives in one place.
pub(super) fn strip_scroll_pos(handle: &ScrollHandle) -> Pixels {
    -handle.offset().x
}

pub(super) fn set_strip_scroll_pos(handle: &ScrollHandle, pos: Pixels) {
    handle.set_offset(point(-pos, handle.offset().y));
}

pub(super) fn edge_scroll_delta(local_x: Pixels, width: Pixels) -> Option<Pixels> {
    const ZONE: f32 = 48.;
    const MIN_SPEED: f32 = 6.;
    const MAX_SPEED: f32 = 36.;
    let speed = |distance: Pixels| {
        let t = 1. - (distance / px(ZONE)).clamp(0., 1.);
        px(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED))
    };
    let from_left = local_x;
    let from_right = width - local_x;
    if from_left < px(ZONE) {
        Some(-speed(from_left))
    } else if from_right < px(ZONE) {
        Some(speed(from_right))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: globbing `gpui_kit::*` inside `mod tests` shadows
    // Rust's built-in `#[test]` attribute.
    use super::{
        rail_grip, rail_thumb_geometry, scrub_fraction, set_strip_scroll_pos, strip_scroll_pos,
    };
    use gpui_kit::{ScrollHandle, point, px};

    #[test]
    fn strip_scroll_helpers_flip_the_sign_domain() {
        let handle = ScrollHandle::new();
        handle.set_offset(point(px(-120.), px(0.)));
        assert_eq!(strip_scroll_pos(&handle), px(120.));
        set_strip_scroll_pos(&handle, px(60.));
        assert_eq!(handle.offset().x, px(-60.));
        assert_eq!(handle.offset().y, px(0.));
    }

    #[test]
    fn rail_keeps_a_grabbable_thumb_and_real_travel() {
        const RAIL: f32 = 797.;
        let (thumb, usable) = rail_thumb_geometry(px(RAIL), px(3.));
        assert!(thumb <= px(RAIL - 32.), "thumb {thumb:?} ate the travel");
        assert!(usable >= px(32.), "travel {usable:?} too small to drag");

        let (thumb, usable) = rail_thumb_geometry(px(400.), px(300.));
        let expected = 400. * 400. / 700.;
        assert!((f32::from(thumb) - expected).abs() < 0.5, "thumb {thumb:?}");
        assert!((f32::from(usable) - (400. - expected)).abs() < 0.5);

        let (thumb, usable) = rail_thumb_geometry(px(RAIL), px(0.));
        assert!(thumb > px(0.) && thumb <= px(RAIL));
        assert!(usable >= px(0.));
        let (thumb, usable) = rail_thumb_geometry(px(0.), px(0.));
        assert!(thumb > px(0.));
        assert_eq!(usable, px(0.));
    }

    #[test]
    fn a_grab_does_not_move_the_thumb() {
        let usable = px(400.);
        let thumb = px(120.);
        for at in [0., 0.25, 0.5, 0.87, 1.] {
            let thumb_x = usable * at;
            for offset in [px(0.), px(60.), thumb] {
                let pointer = thumb_x + offset;
                let grip = rail_grip(pointer, thumb_x, thumb);
                assert_eq!(grip, offset, "grip at {at} from {offset:?}");
                let back = scrub_fraction(pointer, grip, usable);
                assert!(
                    (back - at).abs() < 1e-4,
                    "grab {offset:?} at {at} moved the thumb to {back}"
                );
            }
            assert_eq!(rail_grip(thumb_x - px(50.), thumb_x, thumb), px(0.));
            assert_eq!(rail_grip(thumb_x + thumb + px(50.), thumb_x, thumb), thumb);
        }
        assert_eq!(scrub_fraction(px(-500.), px(12.), usable), 0.);
        assert_eq!(scrub_fraction(px(5000.), px(12.), usable), 1.);
    }
}
