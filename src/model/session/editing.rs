//! Crop transactions, keyboard movement and the shared edit history.

use super::{ScreenshotSession, annotations::text_area};
use crate::annotation::{HistoryEffect, Shape};
use crate::model::selection::Selection;
use gpui_kit::{Bounds, Pixels, Point, point, px};

pub(super) struct SelectionSnapshot {
    bounds: Bounds<Pixels>,
    output: Option<String>,
    toolbar: Option<Point<Pixels>>,
}

pub(super) enum SelectionGesture {
    Pending {
        press: Point<Pixels>,
        before: SelectionSnapshot,
    },
    Reselect(SelectionSnapshot),
    Edit(SelectionSnapshot),
}

pub(super) enum NudgeGesture {
    Shape { ix: usize, before: Shape },
    Selection { before: Bounds<Pixels> },
}

impl ScreenshotSession {
    pub(super) fn selection_snapshot(&self) -> Option<SelectionSnapshot> {
        if !self.selection.is_selected() {
            return None;
        }
        Some(SelectionSnapshot {
            bounds: self.selection.bounds()?,
            output: self.active_output.clone(),
            toolbar: self.toolbar_pos,
        })
    }

    pub(super) fn commit_selection_gesture(&mut self) {
        let Some(gesture) = self.selection_gesture.take() else {
            return;
        };
        let before = match gesture {
            SelectionGesture::Pending { before, .. }
            | SelectionGesture::Reselect(before)
            | SelectionGesture::Edit(before) => before,
        };
        if let Some(after) = self.selection.bounds() {
            self.annotations.record_selection(before.bounds, after);
        }
    }

    pub(super) fn cancel_selection_gesture(&mut self) -> bool {
        let Some(gesture) = self.selection_gesture.take() else {
            return false;
        };
        let before = match gesture {
            SelectionGesture::Pending { before, .. }
            | SelectionGesture::Reselect(before)
            | SelectionGesture::Edit(before) => before,
        };
        self.selection = Selection::Selected {
            bounds: before.bounds,
        };
        self.active_output = before.output;
        self.toolbar_pos = before.toolbar;
        self.press = None;
        self.hovered = None;
        self.smart_cursor.reset();
        true
    }

    /// Move the selected mark, or the crop when no mark is selected.
    /// Repeated key presses form one edit until release or another operation.
    pub(crate) fn nudge(&mut self, delta: Point<Pixels>) -> bool {
        if self.blocked
            || delta == point(px(0.), px(0.))
            || !self.selection.is_selected()
            || self.selection_gesture.is_some()
            || self.moving.is_some()
            || self.handle_drag.is_some()
            || self.pending_click.is_some()
            || self.toolbar_drag.is_some()
            || self.annotations.is_pressed()
            || self.annotations.is_drawing_polyline()
        {
            return false;
        }
        let Some(selection) = self.selection.bounds() else {
            return false;
        };
        if let Some(ix) = self.annotations.selected_index() {
            let Some(current) = self.annotations.selected().cloned() else {
                return false;
            };
            let area = if current.kind == crate::annotation::ShapeKind::Text {
                text_area(selection).unwrap_or(selection)
            } else {
                selection
            };
            let bounds = if current.kind == crate::annotation::ShapeKind::Text {
                current.ink_bounds()
            } else {
                current.selection_box()
            };
            let delta = movement_delta(bounds, area, delta);
            if delta == point(px(0.), px(0.)) {
                return false;
            }
            self.nudge.get_or_insert_with(|| NudgeGesture::Shape {
                ix,
                before: current.clone(),
            });
            self.annotations.place_shape(ix, &current, delta);
        } else {
            let Some(desktop) = self.desktop_bounds() else {
                return false;
            };
            let delta = movement_delta(selection, desktop, delta);
            if delta == point(px(0.), px(0.)) {
                return false;
            }
            self.nudge
                .get_or_insert(NudgeGesture::Selection { before: selection });
            self.selection = Selection::Selected {
                bounds: Bounds::new(selection.origin + delta, selection.size),
            };
            self.follow_selection_host();
        }
        true
    }

    /// Wheel-zoom the finalized selection about its centre (see
    /// [`Selection::zoom`]). Consecutive notches coalesce into ONE undo
    /// entry through the nudge gesture — the same contract as arrow-key
    /// nudging, and any other action (`finish_nudge`) closes the step.
    /// Returns whether the box actually changed.
    pub(crate) fn zoom_selection(&mut self, steps: i32) -> bool {
        if self.blocked
            || !self.selection.is_selected()
            || self.selection_gesture.is_some()
            || self.toolbar_drag.is_some()
        {
            return false;
        }
        let Some(desktop) = self.desktop_bounds() else {
            return false;
        };
        let Some(before) = self.selection.bounds() else {
            return false;
        };
        if !self.selection.zoom(steps, desktop) {
            return false;
        }
        self.nudge.get_or_insert(NudgeGesture::Selection { before });
        // A box that grew across the seam must carry its chrome with
        // it, like every other geometry edit.
        self.follow_selection_host();
        true
    }

    pub(crate) fn finish_nudge(&mut self) {
        match self.nudge.take() {
            Some(NudgeGesture::Shape { ix, before }) => self.annotations.commit_move(ix, before),
            Some(NudgeGesture::Selection { before }) => {
                if let Some(after) = self.selection.bounds() {
                    self.annotations.record_selection(before, after);
                }
            }
            None => {}
        }
    }

    /// Undo the latest crop or mark edit. Active pointer gestures cancel first.
    pub(crate) fn undo(&mut self) {
        if self.blocked {
            return;
        }
        self.finish_nudge();
        if self.cancel_selection_gesture() {
            return;
        }
        if self.selection.is_dragging() {
            self.cancel_drag();
            return;
        }
        if self.moving.is_some() || self.handle_drag.is_some() {
            self.cancel_annotation();
            return;
        }
        self.pending_click = None;
        let effect = self.annotations.undo();
        self.apply_history_effect(effect);
    }

    pub(crate) fn redo(&mut self) {
        if self.blocked || !self.selection.is_selected() || self.selection_gesture.is_some() {
            return;
        }
        self.finish_nudge();
        if self.moving.is_none() && self.handle_drag.is_none() && self.pending_click.is_none() {
            let effect = self.annotations.redo();
            self.apply_history_effect(effect);
        }
    }

    fn apply_history_effect(&mut self, effect: HistoryEffect) {
        if let HistoryEffect::Selection(bounds) = effect {
            self.selection = Selection::Selected { bounds };
            self.toolbar_pos = None;
            self.follow_selection_host();
        }
    }
}

/// Clamp movement without snapping partly clipped marks to a different position.
pub(super) fn movement_delta(
    bounds: Bounds<Pixels>,
    area: Bounds<Pixels>,
    delta: Point<Pixels>,
) -> Point<Pixels> {
    fn axis(start: Pixels, end: Pixels, low: Pixels, high: Pixels, delta: Pixels) -> Pixels {
        let (min, max) = if end - start <= high - low {
            (
                f32::from(low - start).min(0.),
                f32::from(high - end).max(0.),
            )
        } else {
            (
                f32::from(high - end).min(0.),
                f32::from(low - start).max(0.),
            )
        };
        px(f32::from(delta).clamp(min, max))
    }
    point(
        axis(
            bounds.left(),
            bounds.right(),
            area.left(),
            area.right(),
            delta.x,
        ),
        axis(
            bounds.top(),
            bounds.bottom(),
            area.top(),
            area.bottom(),
            delta.y,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{annotation::ShapeKind, platform::capture::Capture};
    use gpui_kit::size;
    use std::sync::Arc;

    fn p(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    fn session() -> ScreenshotSession {
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "screen".into();
        capture.width = 400;
        capture.height = 300;
        capture.rgba = vec![255; 400 * 300 * 4];
        let mut session = ScreenshotSession::new(vec![Arc::new(capture)], Vec::new());
        session.begin("screen", p(40., 40.));
        session.end("screen", p(240., 220.));
        session.edit_annotations(|a| a.toggle(ShapeKind::Rectangle));
        session.pointer_down("screen", p(80., 80.), false);
        session.pointer_up("screen", p(140., 130.), false);
        session.edit_annotations(|a| a.toggle(ShapeKind::Select));
        session
    }

    #[test]
    fn selected_frame_interior_moves_marks_without_moving_the_crop() {
        for kind in [
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Line,
            ShapeKind::Arrow,
            ShapeKind::Pencil,
            ShapeKind::Highlighter,
        ] {
            for tool_active in [false, true] {
                let mut s = session();
                let crop = s.selection.bounds().unwrap();
                s.edit_annotations(|a| {
                    a.toggle(kind);
                    a.begin(p(80., 80.), crop, false);
                    a.drag_to(p(180., 160.), crop, false);
                    a.end();
                    if !tool_active {
                        a.finish_interaction();
                    }
                    a.select_index(1);
                });
                let original = s.annotations.committed()[1].clone();
                let press = p(100., 140.);
                assert_eq!(s.annotations.hit_test(press), None);
                s.pointer_move("screen", press, false);
                assert_eq!(
                    s.annotation_hover(),
                    Some(crate::annotation::ShapeHover::Move)
                );
                s.pointer_down("screen", press, false);
                s.pointer_move("screen", press + p(20., 10.), false);
                s.pointer_up("screen", press + p(20., 10.), false);
                assert_eq!(s.selection.bounds(), Some(crop));
                assert_eq!(s.annotations.selected_index(), Some(1));
                let moved = s.annotations.committed()[1].clone();
                assert_eq!(moved.bounds.origin, original.bounds.origin + p(20., 10.));
                s.undo();
                assert_eq!(s.annotations.committed()[1], original);
                s.redo();
                assert_eq!(s.annotations.committed()[1], moved);
                s.edit_annotations(|a| {
                    a.select_index(1);
                });
                if let Some(handle) = moved.handle_points().first() {
                    s.pointer_down("screen", *handle, false);
                    assert!(s.handle_drag_anchor().is_some());
                }
            }
        }
    }

    #[test]
    fn drawing_tools_do_not_hover_other_marks_after_auto_selection() {
        let mut s = session();
        let area = s.selection.bounds().unwrap();
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Ellipse);
            a.begin(p(160., 150.), area, false);
            a.drag_to(p(200., 190.), area, false);
            a.end();
            a.select_index(1);
        });
        s.pointer_move("screen", p(80., 105.), false);
        assert_eq!(s.hovered_annotation(), None);
        assert_eq!(s.annotations.shape_hover(p(80., 105.)), None);
        assert_eq!(s.annotations.selected_index(), Some(1));
        s.edit_annotations(|a| a.toggle(ShapeKind::Ellipse));
        assert_eq!(s.hovered_annotation(), Some(0));
    }

    #[test]
    fn right_click_finishes_polyline_before_leaving_its_tool() {
        for vertices in [1, 2] {
            let mut s = session();
            let crop = s.selection.bounds().unwrap();
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Polyline);
                for index in 0..vertices {
                    let vertex = p(90. + index as f32 * 50., 170.);
                    a.begin(vertex, crop, false);
                    a.drag_to(vertex, crop, false);
                    a.end();
                }
            });
            assert!(s.annotations.is_drawing_polyline());
            s.finish_or_exit_annotation();
            assert!(!s.annotations.is_drawing_polyline());
            assert_eq!(s.annotations.tool(), Some(ShapeKind::Polyline));
            let count = 1 + usize::from(vertices == 2);
            assert_eq!(s.annotations.committed().len(), count);
            s.finish_or_exit_annotation();
            assert_eq!(s.annotations.tool(), None);
            assert_eq!(s.annotations.committed().len(), count);
            assert_eq!(s.selection.bounds(), Some(crop));
        }
    }

    #[test]
    fn escape_cancels_gesture_then_tool_before_closing() {
        for kind in [
            ShapeKind::Text,
            ShapeKind::Select,
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Pencil,
            ShapeKind::Highlighter,
            ShapeKind::Line,
            ShapeKind::Arrow,
            ShapeKind::Polyline,
            ShapeKind::Number,
            ShapeKind::Mosaic,
            ShapeKind::Blur,
            ShapeKind::Eraser,
            ShapeKind::EraserRect,
        ] {
            let mut s = session();
            s.edit_annotations(|a| {
                a.finish_interaction();
                a.toggle(kind);
                a.select_index(0);
            });
            assert!(
                s.escape_interaction(),
                "passive {kind:?} must exit its tool before closing"
            );
            assert_eq!(s.annotations.tool(), None);
            assert_eq!(s.annotations.selected_index(), None);
            assert_eq!(s.annotations.committed().len(), 1);
            assert!(!s.escape_interaction());
            let crop = s.selection.bounds();
            s.edit_annotations(|a| a.toggle(kind));
            s.finish_or_exit_annotation();
            s.finish_or_exit_annotation();
            assert_eq!(s.annotations.tool(), None);
            assert_eq!(s.annotations.selected_index(), None);
            assert_eq!(s.annotations.committed().len(), 1);
            assert_eq!(s.selection.bounds(), crop);
        }
        for kind in [
            ShapeKind::Rectangle,
            ShapeKind::Line,
            ShapeKind::Pencil,
            ShapeKind::Polyline,
            ShapeKind::Eraser,
            ShapeKind::Select,
        ] {
            let mut s = session();
            let original = s.annotations.committed().to_vec();
            s.edit_annotations(|a| {
                a.finish_interaction();
                a.toggle(kind);
            });
            s.pointer_down("screen", p(80., 100.), false);
            s.pointer_move("screen", p(110., 110.), false);
            assert!(s.escape_interaction(), "active {kind:?}");
            assert_eq!(s.annotations.tool(), Some(kind));
            assert!(
                s.escape_interaction(),
                "second Escape leaves the tool: {kind:?}"
            );
            assert!(!s.escape_interaction(), "third Escape may close: {kind:?}");
            assert_eq!(s.annotations.tool(), None);
            assert_eq!(s.annotations.selected_index(), None);
            assert_eq!(s.annotations.committed(), original);
        }
    }

    #[test]
    fn outside_click_and_cancelled_reselection_preserve_crop_marks_and_redo() {
        let mut s = session();
        let original = s.selection.bounds().unwrap();
        let marks = s.annotations.committed().to_vec();
        s.toolbar_pos = Some(p(15., 250.));
        s.undo();
        assert!(s.annotations.committed().is_empty());
        s.pointer_down("screen", p(300., 100.), false);
        assert_eq!(s.selection.bounds(), Some(original));
        s.pointer_up("screen", p(302., 101.), false);
        assert_eq!(s.selection.bounds(), Some(original));
        s.redo();
        assert_eq!(s.annotations.committed(), marks);
        s.pointer_down("screen", p(300., 100.), false);
        s.pointer_move("screen", p(360., 160.), false);
        assert!(s.selection.is_dragging());
        assert_eq!(s.annotations.committed(), marks);
        assert!(s.cancel_annotation());
        assert_eq!(s.selection.bounds(), Some(original));
        assert_eq!(s.toolbar_pos, Some(p(15., 250.)));
        assert_eq!(s.annotations.tool(), Some(ShapeKind::Select));
        s.pointer_up("screen", p(360., 160.), false);
        assert_eq!(s.selection.bounds(), Some(original));
        s.undo();
        assert!(s.annotations.committed().is_empty());
    }

    #[test]
    fn cross_output_reselection_cancellation_restores_the_original_toolbar_host() {
        let mut s = session();
        let mut capture = Capture::for_test((400, 0), 2.);
        capture.output_name = "second".into();
        capture.width = 600;
        capture.height = 600;
        capture.rgba = vec![255; 600 * 600 * 4];
        s.screens.push(super::super::Screen {
            capture: Arc::new(capture),
            logical_size: size(px(300.), px(300.)),
            detector: None,
        });
        let original = s.selection.bounds().unwrap();
        s.toolbar_pos = Some(p(15., 250.));
        s.pointer_down("second", p(30., 40.), false);
        s.pointer_move("second", p(100., 140.), false);
        assert!(s.active_on("second"));
        assert!(s.cancel_annotation());
        assert!(s.active_on("screen"));
        assert_eq!(s.toolbar_pos, Some(p(15., 250.)));
        assert_eq!(s.selection.bounds(), Some(original));
        assert_eq!(s.annotations.committed().len(), 1);
        s.pointer_down("second", p(30., 40.), false);
        s.pointer_up("second", p(100., 140.), false);
        assert!(s.active_on("second"));
        s.undo();
        assert_eq!(s.selection.bounds(), Some(original));
        assert!(s.active_on("screen"));
    }

    #[test]
    fn crop_and_mark_changes_undo_and_redo_in_operation_order() {
        let mut s = session();
        let original = s.selection.bounds().unwrap();
        let marks = s.annotations.committed().to_vec();
        s.pointer_down("screen", p(300., 100.), false);
        s.pointer_up("screen", p(380., 180.), false);
        let reselected = s.selection.bounds().unwrap();
        assert_eq!(
            reselected,
            Bounds::new(p(300., 100.), size(px(80.), px(80.)))
        );
        assert_eq!(s.annotations.committed(), marks);
        s.edit_annotations(|a| a.toggle(ShapeKind::Number));
        s.pointer_down("screen", p(340., 140.), false);
        s.pointer_up("screen", p(340., 140.), false);
        assert_eq!(s.annotations.committed().len(), 2);
        s.undo();
        assert_eq!(s.annotations.committed(), marks);
        assert_eq!(s.selection.bounds(), Some(reselected));
        s.undo();
        assert_eq!(s.selection.bounds(), Some(original));
        assert_eq!(s.annotations.committed(), marks);
        let crop = s.crop("screen").unwrap();
        assert_eq!((crop.0, crop.1), (200, 180));
        assert!(
            crop.2
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel != &[255; 4])
        );
        s.redo();
        assert_eq!(s.selection.bounds(), Some(reselected));
        let crop = s.crop("screen").unwrap();
        assert!(
            crop.2
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel == &[255; 4])
        );
        s.redo();
        assert_eq!(s.annotations.committed().len(), 2);
    }

    #[test]
    fn moving_and_resizing_crop_keep_marks_and_record_one_step_per_drag() {
        let mut s = session();
        let original = s.selection.bounds().unwrap();
        let marks = s.annotations.committed().to_vec();
        s.pointer_down("screen", p(190., 180.), false);
        for x in [200., 210., 220.] {
            s.pointer_move("screen", p(x, 190.), false);
        }
        s.pointer_up("screen", p(230., 200.), false);
        let moved = s.selection.bounds().unwrap();
        assert_eq!(moved.origin, original.origin + p(40., 20.));
        s.pointer_down("screen", moved.bottom_right(), false);
        s.pointer_up("screen", moved.bottom_right() + p(20., 30.), false);
        let resized = s.selection.bounds().unwrap();
        assert_eq!(resized.size, moved.size + size(px(20.), px(30.)));
        assert_eq!(s.annotations.committed(), marks);
        s.undo();
        assert_eq!(s.selection.bounds(), Some(moved));
        s.undo();
        assert_eq!(s.selection.bounds(), Some(original));
        s.undo();
        assert!(s.annotations.committed().is_empty());
        s.redo();
        s.redo();
        s.redo();
        assert_eq!(s.annotations.committed(), marks);
        assert_eq!(s.selection.bounds(), Some(resized));
    }

    #[test]
    fn undo_cancels_active_crop_edit_before_touching_history() {
        let mut s = session();
        let original = s.selection.bounds().unwrap();
        s.pointer_down("screen", p(190., 180.), false);
        s.pointer_move("screen", p(210., 195.), false);
        s.undo();
        assert_eq!(s.selection.bounds(), Some(original));
        assert_eq!(s.annotations.committed().len(), 1);
        s.pointer_up("screen", p(210., 195.), false);
        s.undo();
        assert!(s.annotations.committed().is_empty());
    }

    #[test]
    fn repeated_arrow_movement_is_one_step_and_clamps_to_the_crop() {
        let mut s = session();
        s.edit_annotations(|a| {
            a.select_index(0);
        });
        let original = s.annotations.selected().unwrap().clone();
        for _ in 0..5 {
            assert!(s.nudge(p(1., 0.)));
        }
        s.finish_nudge();
        assert_eq!(
            s.annotations.selected().unwrap().bounds.origin,
            original.bounds.origin + p(5., 0.)
        );
        s.undo();
        assert_eq!(s.annotations.committed(), std::slice::from_ref(&original));
        s.redo();
        assert_eq!(
            s.annotations.committed()[0].bounds.origin,
            original.bounds.origin + p(5., 0.)
        );
        s.edit_annotations(|a| {
            a.select_index(0);
        });
        assert!(s.nudge(p(1_000., 1_000.)));
        let bounds = s.annotations.selected().unwrap().selection_box();
        assert_eq!(
            bounds.bottom_right(),
            s.selection.bounds().unwrap().bottom_right()
        );
        assert!(!s.nudge(p(1., 1.)));
        s.finish_nudge();
        s.undo();
        assert_eq!(
            s.annotations.committed()[0].bounds.origin,
            original.bounds.origin + p(5., 0.)
        );
    }

    #[test]
    fn freehand_movement_clamps_the_whole_stroke_and_undo_restores_every_point() {
        let mut s = session();
        s.edit_annotations(|a| a.toggle(ShapeKind::Pencil));
        s.pointer_down("screen", p(100., 160.), false);
        s.pointer_move("screen", p(150., 175.), false);
        s.pointer_up("screen", p(200., 190.), false);
        s.edit_annotations(|a| {
            a.toggle(ShapeKind::Select);
            a.select_index(1);
        });
        let original = s.annotations.selected().unwrap().clone();
        assert!(s.nudge(p(1_000., 1_000.)));
        let ink = s.annotations.selected().unwrap().selection_box();
        let crop = s.selection.bounds().unwrap();
        assert!((f32::from(ink.right() - crop.right())).abs() < 0.001);
        assert!((f32::from(ink.bottom() - crop.bottom())).abs() < 0.001);
        s.finish_nudge();
        s.undo();
        assert_eq!(s.annotations.committed()[1], original);
    }

    #[test]
    fn partly_clipped_marks_move_one_pixel_without_jumping_into_the_crop() {
        let mut s = session();
        s.selection = Selection::Selected {
            bounds: Bounds::new(p(100., 100.), size(px(100.), px(100.))),
        };
        s.edit_annotations(|a| {
            a.select_index(0);
        });
        let original = s.annotations.selected().unwrap().clone();
        assert!(s.nudge(p(1., 1.)));
        assert_eq!(
            s.annotations.selected().unwrap().bounds.origin,
            original.bounds.origin + p(1., 1.)
        );
        s.finish_nudge();
        s.undo();
        assert_eq!(s.annotations.committed()[0], original);
        let oversized = Bounds::new(p(80., 80.), size(px(160.), px(160.)));
        let area = Bounds::new(p(100., 100.), size(px(100.), px(100.)));
        assert_eq!(movement_delta(oversized, area, p(1., 1.)), p(1., 1.));
        assert_eq!(movement_delta(oversized, area, p(100., 100.)), p(20., 20.));
    }

    #[test]
    fn crop_nudges_use_desktop_bounds_and_break_redo_only_after_a_change() {
        let mut s = session();
        let original = s.selection.bounds().unwrap();
        assert!(s.nudge(p(10., 0.)));
        assert!(s.nudge(p(10., 0.)));
        s.finish_nudge();
        let moved = s.selection.bounds().unwrap();
        assert_eq!(moved.origin, original.origin + p(20., 0.));
        s.undo();
        assert_eq!(s.selection.bounds(), Some(original));
        s.redo();
        assert_eq!(s.selection.bounds(), Some(moved));
        assert!(s.nudge(p(-1_000., -1_000.)));
        s.finish_nudge();
        assert_eq!(s.selection.bounds().unwrap().origin, p(0., 0.));
        s.undo();
        assert!(!s.nudge(p(0., 0.)));
        s.redo();
        assert_eq!(s.selection.bounds().unwrap().origin, p(0., 0.));
        assert_eq!(s.annotations.committed().len(), 1);
    }

    #[test]
    fn hover_tracks_pickable_ink_and_disappears_during_selection_and_drawing() {
        let mut s = session();
        s.pointer_move("screen", p(110., 76.), false);
        assert_eq!(s.hovered_annotation(), Some(0));
        assert!(s.annotation_hover_bounds("screen").is_some());
        s.pointer_down("screen", p(110., 76.), false);
        s.pointer_up("screen", p(110., 76.), false);
        assert_eq!(s.annotations.selected_index(), Some(0));
        assert_eq!(s.hovered_annotation(), None);
        s.edit_annotations(|a| a.toggle(ShapeKind::Pencil));
        s.pointer_move("screen", p(110., 76.), false);
        assert_eq!(s.hovered_annotation(), None);
    }

    // ── Wheel zoom (issue: reframe an existing crop) ─────────────

    #[test]
    fn wheel_zoom_reframes_the_crop_in_place_and_undoes_as_one_step() {
        let mut s = session(); // 200×180 crop at (40,40) on a 400×300 screen
        let original = s.selection.bounds().unwrap();
        for _ in 0..3 {
            assert!(s.zoom_selection(1));
        }
        s.finish_nudge(); // one gesture = one undo entry
        let zoomed = s.selection.bounds().unwrap();
        assert!(f32::from(zoomed.size.width) > f32::from(original.size.width));
        // grew outward from the centre: it still contains the old box
        assert!(zoomed.contains(&original.origin));
        assert!(zoomed.contains(&original.bottom_right()));
        s.undo();
        assert_eq!(s.selection.bounds(), Some(original));
        s.redo();
        assert_eq!(s.selection.bounds(), Some(zoomed));
        // …and shrinking coalesces the same way, in the other direction
        for _ in 0..2 {
            assert!(s.zoom_selection(-1));
        }
        s.finish_nudge();
        let smaller = s.selection.bounds().unwrap();
        assert!(f32::from(smaller.size.width) < f32::from(zoomed.size.width));
        s.undo();
        assert_eq!(s.selection.bounds(), Some(zoomed));
    }

    #[test]
    fn wheel_zoom_never_leaves_the_desktop_nor_touches_other_states() {
        let mut s = session();
        assert!(!s.zoom_selection(0));
        // a drag in flight owns the pointer gesture: the wheel is inert
        s.pointer_down("screen", p(300., 250.), false);
        s.pointer_move("screen", p(350., 280.), false);
        assert!(!s.zoom_selection(1));
        s.pointer_up("screen", p(350., 280.), false);
        // grow until the limits: every result stays on the captured desktop
        for _ in 0..64 {
            s.zoom_selection(1);
            let b = s.selection.bounds().unwrap();
            assert!(b.left() >= px(0.) && b.top() >= px(0.), "{b:?}");
            assert!(b.right() <= px(400.) && b.bottom() <= px(300.), "{b:?}");
        }
    }
}
