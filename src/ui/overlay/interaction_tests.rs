//! End-to-end interaction transitions through real pointer and keyboard dispatch.

use super::Overlay;
use crate::{annotation::ShapeKind, model::session::ScreenshotSession, platform::capture::Capture};
use gpui_kit::{AppContext, MouseButton, TestAppContext, point, px, size};
use std::sync::Arc;

#[gpui_kit::test]
fn switching_tools_cancels_a_live_move_before_starting_the_next_stroke(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        crate::actions::bind_keys(cx, &crate::actions::default_keys());
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 1200;
    capture.height = 700;
    capture.rgba = vec![255; 1200 * 700 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(700.)));
    cx.update(|window, cx| {
        session.update(cx, |s, cx| {
            s.select_all();
            s.edit_annotations(|a| a.toggle(ShapeKind::Rectangle));
            s.pointer_down("screen", point(px(100.), px(100.)), false, false);
            s.pointer_up("screen", point(px(250.), px(200.)), false);
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    let original = cx.update(|_, cx| session.read(cx).annotations().committed()[0].clone());
    cx.simulate_mouse_down(
        point(px(170.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(
        point(px(200.), px(160.)),
        Some(MouseButton::Left),
        Default::default(),
    );
    cx.update(|_, cx| assert!(session.read(cx).is_body_moving()));
    cx.simulate_keystrokes("l");
    cx.update(|_, cx| {
        let s = session.read(cx);
        assert!(
            !s.is_body_moving(),
            "the previous move must not survive a tool switch"
        );
        assert_eq!(s.annotations().committed()[0], original);
        assert_eq!(s.annotations().tool(), Some(ShapeKind::Line));
    });
    cx.simulate_mouse_up(
        point(px(230.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_down(
        point(px(350.), px(300.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(500.), px(400.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|_, cx| {
        let a = session.read(cx).annotations();
        assert_eq!(a.committed().len(), 2);
        assert_eq!(a.committed()[0], original);
        assert_eq!(a.committed()[1].kind, ShapeKind::Line);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed(), &[original]));
}

#[gpui_kit::test]
fn badge_edit_previews_current_value_and_commits_one_undo_step(cx: &mut TestAppContext) {
    crate::annotation::text::with_test_font(|| {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
            crate::actions::init_annotation_keybindings(cx);
            crate::actions::bind_keys(cx, &crate::actions::default_keys());
        });
        let mut capture = Capture::for_test((0, 0), 1.);
        capture.output_name = "screen".into();
        capture.width = 1200;
        capture.height = 700;
        capture.rgba = vec![255; 1200 * 700 * 4];
        let capture = Arc::new(capture);
        let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
        let (view, cx) =
            cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
        cx.simulate_resize(size(px(1200.), px(700.)));
        cx.update(|window, cx| {
            session.update(cx, |s, cx| {
                s.select_all();
                s.edit_annotations(|a| a.toggle(ShapeKind::Number));
                s.pointer_down("screen", point(px(200.), px(200.)), false, false);
                s.pointer_up("screen", point(px(200.), px(200.)), false);
                s.finish_or_exit_annotation();
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let position = point(px(200.), px(200.));
        cx.simulate_event(gpui_kit::MouseDownEvent {
            position,
            button: MouseButton::Left,
            click_count: 2,
            ..Default::default()
        });
        cx.simulate_event(gpui_kit::MouseUpEvent {
            position,
            button: MouseButton::Left,
            click_count: 2,
            ..Default::default()
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert!(view.read(cx).number_edit.is_some()));
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("4");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().committed()[0].number,
                Some(4)
            );
        });
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("0");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().committed()[0].number,
                Some(4)
            );
        });
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("4");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).number_edit.is_none());
            assert_eq!(
                session.read(cx).annotations().committed()[0].number,
                Some(4)
            );
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().committed()[0].number,
                Some(1)
            );
        });
        cx.simulate_keystrokes("ctrl-y");
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().committed()[0].number,
                Some(4)
            );
        });
    });
}

#[gpui_kit::test]
fn deleting_during_a_drag_does_not_move_or_replace_the_next_mark(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        crate::actions::bind_keys(cx, &crate::actions::default_keys());
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 1200;
    capture.height = 700;
    capture.rgba = vec![255; 1200 * 700 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(700.)));
    cx.update(|window, cx| {
        session.update(cx, |s, cx| {
            s.select_all();
            s.edit_annotations(|a| a.toggle(ShapeKind::Rectangle));
            for x in [100., 400.] {
                s.pointer_down("screen", point(px(x), px(100.)), false, false);
                s.pointer_up("screen", point(px(x + 150.), px(200.)), false);
            }
            s.edit_annotations(|a| {
                a.select_index(0);
            });
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    let original = cx.update(|_, cx| session.read(cx).annotations().committed().to_vec());
    cx.simulate_mouse_down(
        point(px(170.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(
        point(px(200.), px(160.)),
        Some(MouseButton::Left),
        Default::default(),
    );
    cx.update(|_, cx| assert!(session.read(cx).is_body_moving()));
    cx.simulate_keystrokes("delete");
    cx.simulate_mouse_up(
        point(px(230.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|_, cx| {
        assert_eq!(session.read(cx).annotations().committed(), &original[1..]);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed(), original));
}
