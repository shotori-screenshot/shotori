use super::Overlay;
use crate::{model::session::ScreenshotSession, platform::capture::Capture};
use gpui_kit::{AppContext, CursorStyle, MouseButton, TestAppContext, point, px, size};
use std::sync::Arc;

#[gpui_kit::test]
fn all_image_annotation_tools_release_replaced_previews(cx: &mut TestAppContext) {
    use crate::annotation::ShapeKind;
    cx.update(gpui_kit::base::init);
    cx.update(gpui_kit::component::init);
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 200;
    capture.height = 200;
    capture.rgba = vec![255; 200 * 200 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (view, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(200.), px(200.)));
    let mut previous: Vec<Arc<gpui_kit::RenderImage>> = Vec::new();
    for kind in [
        ShapeKind::Highlighter,
        ShapeKind::Mosaic,
        ShapeKind::Blur,
        ShapeKind::Magnifier,
        ShapeKind::Number,
        ShapeKind::Text,
        ShapeKind::Polyline,
    ] {
        cx.update(|_, cx| {
            session.update(cx, |s, cx| {
                s.begin("screen", point(px(0.), px(0.)));
                s.end("screen", point(px(190.), px(190.)));
                s.edit_annotations(|a| a.toggle(kind));
                s.pointer_down("screen", point(px(10.), px(10.)), false, false);
                cx.notify();
            })
        });
        for i in 0..30 {
            cx.update(|window, cx| {
                session.update(cx, |s, cx| {
                    if kind == ShapeKind::Text {
                        s.preview_text(
                            gpui_kit::Bounds::new(point(px(10.), px(10.)), size(px(150.), px(80.))),
                            format!("Text {i}"),
                        );
                    } else {
                        s.pointer_move("screen", point(px(30. + i as f32), px(70.)), false);
                    }
                    cx.notify();
                });
                window.draw(cx).clear(cx);
                let current = view.read(cx).canvas_images.borrow().current().to_vec();
                assert!(!current.is_empty(), "{kind:?}");
                for image in &current {
                    assert!(window.has_image_atlas_entry(image), "{kind:?}");
                }
                for old in &previous {
                    if !current.iter().any(|image| image.id == old.id) {
                        assert!(!window.has_image_atlas_entry(old), "{kind:?}");
                    }
                }
                previous = current;
            });
        }
    }
}

#[gpui_kit::test]
fn continuous_pencil_repaints_release_retired_atlas_images(cx: &mut TestAppContext) {
    cx.update(gpui_kit::base::init);
    cx.update(gpui_kit::component::init);
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 200;
    capture.height = 200;
    capture.rgba = vec![255; 200 * 200 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(200.), px(200.)));
    cx.update(|_, cx| {
        session.update(cx, |s, cx| {
            s.select_all();
            s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Pencil));
            s.pointer_down("screen", point(px(10.), px(10.)), false, false);
            cx.notify();
        })
    });
    let mut previous: Option<Arc<gpui_kit::RenderImage>> = None;
    for i in 0..250 {
        cx.update(|window, cx| {
            session.update(cx, |s, cx| {
                s.pointer_move(
                    "screen",
                    point(px(20. + (i % 150) as f32), px(30. + (i % 80) as f32)),
                    false,
                );
                if i % 25 == 24 {
                    s.pointer_up("screen", point(px(170.), px(100.)), false);
                    s.pointer_down("screen", point(px(10.), px(10.)), false, false);
                }
                cx.notify();
            });
            window.draw(cx).clear(cx);
            let (_, image) = session.read(cx).filtered_preview("screen").unwrap();
            assert!(window.has_image_atlas_entry(&image));
            if let Some(old) = previous.take()
                && old.id != image.id
            {
                assert!(!window.has_image_atlas_entry(&old));
            }
            // Mid-gesture the ink rides as a vector overlay: the image Arc
            // must stay IDENTICAL (no per-move atlas re-upload). A fresh
            // image appears only when a stroke is promoted on release,
            // and the retired one must leave the atlas immediately.
            previous = Some(image);
        });
    }
    cx.update(|window, cx| {
        session.update(cx, |s, cx| {
            s.begin("screen", point(px(1.), px(1.)));
            cx.notify();
        });
        window.draw(cx).clear(cx);
        assert!(!window.has_image_atlas_entry(&previous.unwrap()));
    });
}

#[gpui_kit::test]
fn pointer_events_share_selection_and_handle_release_outside_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::base::init);
    cx.update(gpui_kit::component::init);
    let make_capture = |name: &str, x| {
        let mut capture = Capture::for_test((x, 0), 1.);
        capture.output_name = name.into();
        capture.width = 400;
        capture.height = 400;
        capture.rgba = vec![255; 400 * 400 * 4];
        Arc::new(capture)
    };
    let left = make_capture("left", 0);
    let right = make_capture("right", 400);
    let session = cx.new(|_| ScreenshotSession::new(vec![left.clone(), right.clone()], Vec::new()));
    let mut second_context = cx.clone();
    let (_, left_cx) =
        cx.add_window_view(|window, cx| Overlay::new(left, session.clone(), window, cx));
    let (_, right_cx) = second_context
        .add_window_view(|window, cx| Overlay::new(right, session.clone(), window, cx));
    for context in [&mut *left_cx, &mut *right_cx] {
        context.simulate_resize(size(px(400.), px(400.)));
        context.update(|window, cx| window.draw(cx).clear(cx));
        context.run_until_parked();
    }
    left_cx.simulate_mouse_down(
        point(px(300.), px(80.)),
        MouseButton::Left,
        Default::default(),
    );
    left_cx.simulate_mouse_move(
        point(px(450.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    left_cx.simulate_mouse_up(
        point(px(450.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    left_cx.update(|_, cx| {
        let shared = session.read(cx);
        assert!(shared.selection().is_selected());
        assert_eq!(
            shared.local_bounds("right").unwrap().size,
            size(px(50.), px(100.))
        );
        assert_eq!(shared.crop("right").unwrap().0, 150);
    });
    right_cx.run_until_parked();
    right_cx.simulate_mouse_down(
        point(px(100.), px(80.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_move(
        point(px(200.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_up(
        point(px(200.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    left_cx.run_until_parked();
    left_cx.update(|_, cx| {
        let shared = session.read(cx);
        assert!(shared.local_bounds("left").is_none());
        assert!(shared.active_on("right"));
        assert_eq!(shared.crop("left").unwrap().0, 100);
    });
    // Some compositors transfer pointer events to the destination surface.
    // Convert its local coordinates using that output's desktop origin.
    left_cx.simulate_mouse_down(
        point(px(350.), px(80.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_move(
        point(px(50.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_up(
        point(px(50.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.update(|_, cx| {
        let shared = session.read(cx);
        assert!(shared.selection().is_selected());
        assert_eq!(
            shared.selection().bounds().unwrap().size,
            size(px(100.), px(100.))
        );
        assert_eq!(shared.crop("right").unwrap().0, 100);
    });
}

/// The user's cross-screen repro: select on one output, then MOVE the
/// selection across the seam and release. Wayland's implicit grab
/// delivers the entire gesture to the press window — without the
/// rehost on release, the size label and toolbar rendered on NEITHER
/// output until a fresh click happened to re-host the chrome.
#[gpui_kit::test]
fn chrome_follows_a_cross_output_move_through_the_event_pipeline(cx: &mut TestAppContext) {
    cx.update(gpui_kit::base::init);
    cx.update(gpui_kit::component::init);
    let make_capture = |name: &str, x| {
        let mut capture = Capture::for_test((x, 0), 1.);
        capture.output_name = name.into();
        capture.width = 400;
        capture.height = 400;
        capture.rgba = vec![255; 400 * 400 * 4];
        Arc::new(capture)
    };
    let left = make_capture("left", 0);
    let right = make_capture("right", 400);
    let session = cx.new(|_| ScreenshotSession::new(vec![left.clone(), right.clone()], Vec::new()));
    let mut second_context = cx.clone();
    let (left_view, left_cx) =
        cx.add_window_view(|window, cx| Overlay::new(left, session.clone(), window, cx));
    let (_, right_cx) = second_context
        .add_window_view(|window, cx| Overlay::new(right, session.clone(), window, cx));
    for context in [&mut *left_cx, &mut *right_cx] {
        context.simulate_resize(size(px(400.), px(400.)));
        context.update(|window, cx| window.draw(cx).clear(cx));
        context.run_until_parked();
    }

    // a selection fully on "right": global (500,80)-(700,280)
    right_cx.simulate_mouse_down(
        point(px(100.), px(80.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_move(
        point(px(300.), px(280.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_up(
        point(px(300.), px(280.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.run_until_parked();
    right_cx.update(|_, cx| {
        assert!(session.read(cx).active_on("right"));
        assert!(session.read(cx).toolbar_bounds("right").is_some());
    });

    // grab the interior on right's window and drag across the seam:
    // every event keeps arriving through the press window
    // (right-local, far negative x = pointer over "left")
    right_cx.simulate_mouse_down(
        point(px(150.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_move(
        point(px(-300.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_up(
        point(px(-300.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.run_until_parked();
    left_cx.run_until_parked();
    right_cx.update(|_, cx| {
        let shared = session.read(cx);
        assert!(shared.selection().is_selected());
        // the render inputs for left's label + toolbar, none for right
        assert!(shared.active_on("left"), "chrome re-hosts on release");
        assert!(shared.toolbar_bounds("left").is_some());
        assert!(shared.local_bounds("left").is_some());
        assert!(shared.toolbar_bounds("right").is_none());
    });
    // The cursor must be right WITHOUT a wiggle: left's window never
    // received a single pointer event (implicit grab), yet the pointer
    // physically sits inside the selection on it. The affordance is
    // derived from the session-tracked GLOBAL pointer — "can't grab
    // until I slide the mouse" was a stale per-window position.
    left_cx.update(|_, cx| {
        assert_eq!(left_view.read(cx).cursor.get(), CursorStyle::OpenHand);
    });

    // And so must the CLICK. A stale pointer focus after the release
    // (niri re-focuses only on the next motion) delivers the down to
    // the PRESS window — right — with OUT-OF-BOUNDS local
    // coordinates. Element hit-testing drops exactly that; the
    // window-level down listener must still grab.
    right_cx.simulate_mouse_down(
        point(px(-300.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.run_until_parked();
    right_cx.update(|_, cx| {
        assert!(
            session.read(cx).selection().is_editing(),
            "stale-focus press still grabs"
        );
    });
    right_cx.simulate_mouse_move(
        point(px(-280.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.simulate_mouse_up(
        point(px(-280.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    right_cx.run_until_parked();
    left_cx.run_until_parked();
    right_cx.update(|_, cx| {
        // press global (100,150) → grab (50,70); drag to (120,150)
        // → origin (70,80), and the release re-hosts on left again
        let b = session.read(cx).selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(70.), px(80.)));
        assert!(session.read(cx).active_on("left"));
    });
}

#[gpui_kit::test]
fn selection_moves_and_resizes_after_release_through_the_event_pipeline(cx: &mut TestAppContext) {
    cx.update(gpui_kit::base::init);
    cx.update(gpui_kit::component::init);
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "main".into();
    capture.width = 400;
    capture.height = 400;
    capture.rgba = vec![255; 400 * 400 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_, vcx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    vcx.simulate_resize(size(px(400.), px(400.)));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();

    // draw a selection (50,50)-(150,120)
    vcx.simulate_mouse_down(
        point(px(50.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(150.), px(120.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(150.), px(120.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();

    // press the interior and drag: the whole selection translates
    vcx.simulate_mouse_down(
        point(px(100.), px(80.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(140.), px(110.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(140.), px(110.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        let shared = session.read(cx);
        assert!(shared.selection().is_selected());
        let b = shared.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(90.), px(80.)));
        assert_eq!(b.size, size(px(100.), px(70.)));
    });

    // press exactly the bottom-right corner and drag: resize only that
    // corner; the top-left stays pinned
    vcx.simulate_mouse_down(
        point(px(190.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(230.), px(190.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(230.), px(190.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        let shared = session.read(cx);
        assert!(shared.selection().is_selected());
        let b = shared.selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(90.), px(80.)));
        assert_eq!(b.size, size(px(140.), px(110.)));
        assert_eq!(shared.crop("main").unwrap().0, 140); // export follows
    });
}

#[gpui_kit::test]
fn cursor_reflects_interior_handles_and_the_inset_toolbar(cx: &mut TestAppContext) {
    cx.update(gpui_kit::base::init);
    cx.update(gpui_kit::component::init);
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "main".into();
    capture.width = 400;
    capture.height = 400;
    capture.rgba = vec![255; 400 * 400 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (overlay, vcx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    vcx.simulate_resize(size(px(400.), px(400.)));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();

    // A selection that reaches the screen bottom parks the toolbar
    // INSIDE the box (bottom-left): anchor math puts it at (8,344) —
    // width clamped to 384 on this narrow window.
    vcx.simulate_mouse_down(
        point(px(50.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(350.), px(390.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(350.), px(390.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();

    // interior (above the toolbar): move affordance
    vcx.simulate_mouse_move(
        point(px(200.), px(200.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));

    // the toolbar's left grip — on this narrow window it even sits
    // OUTSIDE the box horizontally (toolbar pinned to x=8): the grip
    // affordance wins regardless
    vcx.simulate_mouse_move(
        point(px(14.), px(363.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));

    // over the inset toolbar: the toolbar owns the cursor, even though
    // the point is still inside the selection
    vcx.simulate_mouse_move(
        point(px(200.), px(363.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::Arrow));

    // the bottom-right corner handle: its resize arrow
    vcx.simulate_mouse_move(
        point(px(350.), px(390.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert_eq!(
            overlay.read(cx).cursor.get(),
            CursorStyle::ResizeUpLeftDownRight
        )
    });
}

#[gpui_kit::test]
fn annotation_hover_cursor_picks_then_grabs(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        crate::actions::bind_keys(cx, &crate::actions::default_keys());
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "main".into();
    capture.width = 400;
    capture.height = 400;
    capture.rgba = vec![255; 400 * 400 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (overlay, vcx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    vcx.simulate_resize(size(px(400.), px(400.)));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();

    // a selection first: (50,50)-(150,150)
    vcx.simulate_mouse_down(
        point(px(50.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(150.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(150.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();

    // a rectangle annotation inside it: (60,70)-(140,130). Placing
    // no longer selects (the select-tool flip) — while the draw
    // tool runs, hovering the ink stays a CROSSHAIR: the press
    // would draw, and no affordance may promise otherwise.
    vcx.simulate_keystrokes("r");
    vcx.run_until_parked();
    vcx.simulate_mouse_down(
        point(px(60.), px(70.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(140.), px(130.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(140.), px(130.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert_eq!(session.read(cx).annotations().selected_index(), Some(0));
    });
    vcx.simulate_mouse_move(
        point(px(100.), px(100.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));

    // switch to the select tool: the same body now advertises
    // click-to-pick — a hand before anything is grabbed would read
    // as "already holding" (issue #17)
    vcx.simulate_keystrokes("v");
    vcx.simulate_mouse_move(
        point(px(80.), px(70.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::PointingHand));

    // press-release without a drag: the shape is selected and
    // the body becomes a move affordance (open hand)
    vcx.simulate_mouse_down(
        point(px(80.), px(70.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(80.), px(70.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.simulate_mouse_move(
        point(px(100.), px(100.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert!(session.read(cx).annotations().selected().is_some());
        assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand);
    });

    // Escape clears passive state; re-enter Select to inspect picking again.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(session.read(cx).annotations().tool(), None));
    vcx.simulate_keystrokes("v");
    vcx.simulate_mouse_move(
        point(px(80.), px(70.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert_eq!(session.read(cx).annotations().selected_index(), None);
        assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::PointingHand);
    });

    // press and drag past the click slop: holding — closed hand
    vcx.simulate_mouse_down(
        point(px(80.), px(70.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(90.), px(80.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::ClosedHand));
    vcx.simulate_mouse_up(
        point(px(90.), px(80.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.simulate_mouse_move(
        point(px(110.), px(110.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    // release: back to the open hand — the pointer (110,80) sits on
    // the translated shape's top edge, still selected
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));
    let (crop, before, press) = vcx.update(|_, cx| {
        let s = session.read(cx);
        let shape = s.annotations().selected().unwrap().clone();
        let press = shape.bounds.center();
        assert_eq!(s.annotations().hit_test(press), None);
        (s.selection().bounds(), shape, press)
    });
    vcx.simulate_mouse_down(press, MouseButton::Left, Default::default());
    let release = press - point(px(5.), px(5.));
    vcx.simulate_mouse_move(release, MouseButton::Left, Default::default());
    vcx.simulate_mouse_up(release, MouseButton::Left, Default::default());
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        let s = session.read(cx);
        assert_eq!(s.selection().bounds(), crop);
        assert_eq!(
            s.annotations().selected().unwrap().bounds.origin,
            before.bounds.origin - point(px(5.), px(5.))
        );
    });
}

#[gpui_kit::test]
fn selected_swatch_becomes_the_inline_color_editor(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "main".into();
    capture.width = 800;
    capture.height = 600;
    capture.rgba = vec![255; 800 * 600 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_overlay, vcx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    vcx.simulate_resize(size(px(800.), px(600.)));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
    vcx.simulate_mouse_down(
        point(px(100.), px(200.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(600.), px(560.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(600.), px(560.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_keystrokes("r");
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));

    // The board's SELECTED cell renders as the inline picker (no
    // tb-color-{n} control there — the picker component owns that
    // slot); every other cell stays a plain one-click selector, so
    // picking and editing never fight over the same click.
    let active = vcx.update(|_, cx| session.read(cx).annotations().color_index());
    // debug_bounds wants &'static — the board is fixed at seven.
    const SWATCH_IDS: [&str; crate::model::settings::PALETTE] = [
        "tb-color-0",
        "tb-color-1",
        "tb-color-2",
        "tb-color-3",
        "tb-color-4",
        "tb-color-5",
        "tb-color-6",
    ];
    for (ix, id) in SWATCH_IDS.iter().enumerate() {
        let present = vcx.debug_bounds(id).is_some();
        assert_eq!(
            present,
            ix != active,
            "swatch {ix}: expected the selected cell to be the picker"
        );
    }
}

#[gpui_kit::test]
fn toolbar_hugs_its_content(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "main".into();
    capture.width = 800;
    capture.height = 600;
    capture.rgba = vec![255; 800 * 600 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_overlay, vcx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    vcx.simulate_resize(size(px(800.), px(600.)));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
    vcx.simulate_mouse_down(
        point(px(100.), px(200.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(600.), px(560.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(600.), px(560.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));

    // Single row (no tool active): the bar hugs the configured
    // factory row (its basis width) and the separator between the
    // tool cluster and the action cluster renders as a small
    // deliberate group break — NOT the ~70px dead hole a full-TB_W
    // bar showed (user-reported), and not a bare 2px gap either.
    vcx.update(|_, cx| {
        assert_eq!(
            session.read(cx).toolbar_bounds("main").unwrap().size.width,
            px(crate::model::placement::default_row1_basis())
        );
    });
    let gap = f32::from(
        vcx.debug_bounds("tb-ocr").unwrap().left() - vcx.debug_bounds("tb-clear").unwrap().right(),
    );
    assert!(
        (9. ..=16.).contains(&gap),
        "the tools/actions break must read as a deliberate separator break, got {gap}px"
    );
    // …and the right edge is intact: the copy button ends far enough
    // from the bar's right edge that the grip strip still fits
    // (no clipping — the failure mode that motivated measuring
    // widths in the first place).
    let copy_right = f32::from(vcx.debug_bounds("tb-copy").unwrap().right());
    let bar_right =
        vcx.update(|_, cx| f32::from(session.read(cx).toolbar_bounds("main").unwrap().right()));
    assert!(
        copy_right
            <= bar_right - crate::model::placement::BAR_PAD - crate::model::placement::GRIP_W,
        "copy clipped: {copy_right} vs bar right {bar_right}"
    );

    // The cursor's grab strips ARE the rendered grip elements,
    // exactly — same-frame, same-source (the measure canvas
    // painting inside each grip reports the very rect the debug
    // bounds record). This is the regression lock on the whole
    // basis-vs-render drift class: a stale width basis once put
    // the right strip ~70px left of the rendered grip, over the
    // buttons — draggable, but no hand cursor there (user-reported).
    let (gl, gr) = (
        vcx.debug_bounds("tb-grip-left").unwrap(),
        vcx.debug_bounds("tb-grip-right").unwrap(),
    );
    vcx.update(|_, cx| {
        let (l, r) = session.read(cx).toolbar_grips("main").unwrap();
        assert_eq!(gl, l, "left strip must be the rendered grip element");
        assert_eq!(gr, r, "right strip must be the rendered grip element");
    });

    // Two rows (a tool with the color settings row): the bar keeps
    // the full TB_W the color row needs — its last swatch must fit
    // inside the bar with room for the bar's padding.
    vcx.simulate_keystrokes("r");
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.update(|_, cx| {
        let b = session.read(cx).toolbar_bounds("main").unwrap();
        assert_eq!(b.size.height, px(crate::model::placement::TB_H));
    });
    // The alignment lock again in the TWO-ROW state — the exact
    // state the TB_W drift bit: row one renders at its configured
    // (wider) width while the basis still carried TB_W, so the
    // derived right strip sat over the buttons.
    let (gl, gr) = (
        vcx.debug_bounds("tb-grip-left").unwrap(),
        vcx.debug_bounds("tb-grip-right").unwrap(),
    );
    vcx.update(|_, cx| {
        let (l, r) = session.read(cx).toolbar_grips("main").unwrap();
        assert_eq!(gl, l, "left strip must be the rendered grip element");
        assert_eq!(gr, r, "right strip must be the rendered grip element");
    });
    let swatch = vcx.debug_bounds("tb-color-5").unwrap();
    let bar_right =
        vcx.update(|_, cx| f32::from(session.read(cx).toolbar_bounds("main").unwrap().right()));
    assert!(
        f32::from(swatch.right()) <= bar_right - 4.,
        "color row overflows the toolbar: {} > {bar_right}",
        f32::from(swatch.right())
    );

    // The mode+slider settings rows (mosaic, eraser): the readout must
    // sit INSIDE the row's own painted border. These rows once carried
    // fixed widths tuned for the old S/M/L buttons; the wider slider
    // bundle spilled past the border (user-reported, visible on HiDPI).
    for key in ["m", "d"] {
        vcx.simulate_keystrokes(key);
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let readout = vcx.debug_bounds("tb-size-readout").unwrap();
        let options = vcx.debug_bounds("tb-options").unwrap();
        assert!(
            f32::from(readout.right())
                <= f32::from(options.right()) - crate::model::placement::BAR_PAD + 0.5,
            "{key} settings row clips the size readout: readout right {} vs row right {}",
            f32::from(readout.right()),
            f32::from(options.right())
        );
    }
}

#[gpui_kit::test]
fn toolbar_drag_works_through_the_event_pipeline(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "main".into();
    capture.width = 800;
    capture.height = 600;
    capture.rgba = vec![255; 800 * 600 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (overlay, vcx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    vcx.simulate_resize(size(px(800.), px(600.)));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();

    // A selection reaching the bottom parks the toolbar INSIDE the box:
    // anchor math puts its origin at (112, 514), TB_W wide, one row tall.
    vcx.simulate_mouse_down(
        point(px(100.), px(200.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(600.), px(560.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(600.), px(560.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        let anchored = session.read(cx).toolbar_bounds("main").unwrap();
        // the inset would put it at x=112, but the factory bar
        // (~701px) does not fit an 800px window at that offset —
        // the horizontal clamp pins it into the right edge
        let basis = crate::model::placement::default_row1_basis();
        assert_eq!(
            anchored.origin,
            point(px((800. - basis - 8.).min(112.)), px(514.))
        );
    });

    // hovering either grip: the open-hand affordance
    let (left_grip, right_grip) =
        vcx.update(|_, cx| session.read(cx).toolbar_grips("main").unwrap());
    vcx.simulate_mouse_move(left_grip.center(), MouseButton::Left, Default::default());
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));
    vcx.simulate_mouse_move(right_grip.center(), MouseButton::Left, Default::default());
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));

    // press the LEFT grip, drag, release: the toolbar follows to any
    // spot on the layer; the selection beneath is untouched
    let press = left_grip.center();
    vcx.simulate_mouse_down(press, MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(
        point(px(250.), px(100.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::ClosedHand);
        let b = session.read(cx).toolbar_bounds("main").unwrap();
        // grip-center grab (11,19) held, x clamped to the window
        // (narrow single-row bar)
        assert_eq!(
            b.origin,
            point(
                px(800. - crate::model::placement::default_row1_basis() - 8.),
                px(81.),
            )
        );
    });
    vcx.simulate_mouse_up(
        point(px(250.), px(100.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert!(!session.read(cx).toolbar_drag_active());
        assert_eq!(
            session.read(cx).toolbar_bounds("main").unwrap().origin,
            point(
                px(800. - crate::model::placement::default_row1_basis() - 8.),
                px(81.),
            )
        );
        // the press on the grip never became a selection interaction
        let b = session.read(cx).selection().bounds().unwrap();
        assert_eq!(b.origin, point(px(100.), px(200.)));
        assert_eq!(b.size, size(px(500.), px(360.)));
    });

    // A tool active → row two appears. Its edges are NOT grips (by
    // design): cursor stays Arrow there and a press does not drag.
    // Regression pair for the "draggable but not a hand" mismatch:
    // the cursor strip and the element rect are now one geometry.
    vcx.simulate_keystrokes("r");
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert_eq!(
            session.read(cx).toolbar_bounds("main").unwrap().size.height,
            px(crate::model::placement::TB_H)
        );
    });
    // row TWO's left edge: plain toolbar body
    vcx.simulate_mouse_move(
        point(px(255.), px(153.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::Arrow));
    vcx.simulate_mouse_down(
        point(px(255.), px(153.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_move(
        point(px(300.), px(300.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(300.), px(300.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    // The press was on row TWO's body (not a grip): the toolbar must
    // be UNMOVED. Relative comparison — a one-row width wider than
    // the two-row TB_W (the scroll button made it so) legitimately
    // changes the persisted origin, so no hardcoded point here.
    let pre_drag_origin =
        vcx.update(|_, cx| session.read(cx).toolbar_bounds("main").unwrap().origin);
    vcx.update(|_, cx| {
        assert!(!session.read(cx).toolbar_drag_active());
        assert_eq!(
            session.read(cx).toolbar_bounds("main").unwrap().origin,
            pre_drag_origin
        );
    });

    // row ONE's strip still grabs with two rows on screen
    // (relative: the grip rides the toolbar's clamped position,
    // which the row-one width now determines)
    let row1_grip = vcx.update(|_, cx| session.read(cx).toolbar_grips("main").unwrap().0.center());
    vcx.simulate_mouse_move(row1_grip, MouseButton::Left, Default::default());
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(overlay.read(cx).cursor.get(), CursorStyle::OpenHand));
    vcx.simulate_mouse_down(row1_grip, MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(
        point(px(200.), px(300.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.simulate_mouse_up(
        point(px(200.), px(300.)),
        MouseButton::Left,
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert!(!session.read(cx).toolbar_drag_active());
        // grab was (11,19); the clamp pins the toolbar at the
        // right margin on this narrow test window
        assert_eq!(
            session.read(cx).toolbar_bounds("main").unwrap().origin,
            point(px(800. - crate::model::placement::TB_W - 8.), px(281.))
        );
    });
}

#[gpui_kit::test]
fn rectangle_toolbar_keyboard_and_export_share_the_same_state(cx: &mut TestAppContext) {
    geometry_toolbar_keyboard_and_export(cx, crate::annotation::ShapeKind::Rectangle);
}

#[gpui_kit::test]
fn ellipse_toolbar_keyboard_and_export_share_the_same_state(cx: &mut TestAppContext) {
    geometry_toolbar_keyboard_and_export(cx, crate::annotation::ShapeKind::Ellipse);
}

#[gpui_kit::test]
fn line_and_polyline_pointer_keyboard_and_toolbar_workflows(cx: &mut TestAppContext) {
    stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Line);
}

#[gpui_kit::test]
fn arrow_and_polyline_share_keyboard_focus_history_and_export(cx: &mut TestAppContext) {
    stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Arrow);
}

#[gpui_kit::test]
fn number_toolbar_keeps_focus_size_and_shared_history(cx: &mut TestAppContext) {
    stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Number);
}

#[gpui_kit::test]
fn pencil_toolbar_and_curve_share_history(cx: &mut TestAppContext) {
    stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Pencil);
}

#[gpui_kit::test]
fn highlighter_toolbar_and_curve_share_history(cx: &mut TestAppContext) {
    stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Highlighter);
}

#[gpui_kit::test]
fn mosaic_and_blur_toolbar_share_focus_history(cx: &mut TestAppContext) {
    stroke_and_polyline_workflows(cx, crate::annotation::ShapeKind::Mosaic);
}

#[gpui_kit::test]
fn text_input_commit_escape_and_history(cx: &mut TestAppContext) {
    // Host font fallback changes wrapping and can turn a valid resize
    // into a correctly rejected overflow on CI.
    crate::annotation::text::with_test_font(|| text_input_workflow(cx));
}

fn assert_inline_text_caret(
    view: &gpui_kit::Entity<Overlay>,
    cx: &mut gpui_kit::VisualTestContext,
) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let editor = view.read(cx).text_editing.as_ref().unwrap();
        assert!(
            editor.focus_handle(cx).is_focused(window),
            "inline editor owns focus"
        );
        let color: gpui_kit::Background = gpui_kit::rgba(editor.color).into();
        let scale = window.scale_factor();
        let carets = window
            .painted_quads()
            .into_iter()
            .filter(|quad| {
                quad.background == color
                    && quad.bounds.size.width.0 == scale.round().max(1.)
                    && quad.bounds.size.height.0 > scale
            })
            .collect::<Vec<_>>();
        assert_eq!(
            carets.len(),
            1,
            "the active editor paints one visible caret"
        );
        assert_eq!(
            carets[0].bounds.intersect(&carets[0].content_mask.bounds),
            carets[0].bounds
        );
    });
}

fn text_input_workflow(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        crate::actions::bind_keys(cx, &crate::actions::default_keys());
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 600;
    capture.height = 500;
    capture.rgba = vec![255; 600 * 500 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (view, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(600.), px(500.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_down(
        point(px(20.), px(20.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(550.), px(300.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let button = cx.debug_bounds("tb-text").unwrap();
    cx.simulate_click(button.center(), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(32.))));
    cx.simulate_click(point(px(60.), px(60.)), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    assert_inline_text_caret(&view, cx);
    assert!(cx.debug_bounds("text-editor").unwrap().size.width <= px(9.));
    assert!(cx.debug_bounds("tb-text").is_some());
    // A narrow output must keep trailing text controls reachable without
    // committing the active edit or changing the screenshot selection.
    let strip = cx.debug_bounds("annotation-options-scroll").unwrap();
    cx.simulate_event(gpui_kit::ScrollEvent {
        position: strip.center(),
        delta: gpui_kit::ScrollDelta::Pixels(point(px(-600.), px(0.))),
        modifiers: Default::default(),
        touch_phase: gpui_kit::TouchPhase::Moved,
        source: gpui_kit::ScrollSource::Pointer,
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let color = cx.debug_bounds("tb-color-4").unwrap();
    cx.simulate_click(color.center(), Default::default());
    cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(16.))));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(32.))));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let input = view.read(cx).text_editing.as_ref().unwrap().input().clone();
        input.update(cx, |s, cx| {
            gpui_kit::EntityInputHandler::replace_and_mark_text_in_range(
                s,
                None,
                "nihao",
                Some(5..5),
                window,
                cx,
            )
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert_eq!(
            session
                .read(cx)
                .annotations()
                .visible()
                .last()
                .unwrap()
                .text
                .as_deref(),
            Some("nihao")
        );
        let input = view.read(cx).text_editing.as_ref().unwrap().input().clone();
        input.update(cx, |s, cx| {
            gpui_kit::EntityInputHandler::replace_text_in_range(s, None, "", window, cx)
        });
    });
    cx.simulate_input("rental 中文");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("第二行很长的文字需要自动换行到下一行并立即显示");
    cx.update(|_, cx| {
        assert!(session.read(cx).blocked());
        assert_eq!(
            view.read(cx).text_editing.as_ref().unwrap().value(cx),
            "rental 中文
第二行很长的文字需要自动换行到下一行并立即显示"
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("text-editor").unwrap().size.height > px(32. * 1.35 * 2.));
    assert_inline_text_caret(&view, cx);
    let live_pixels = cx.update(|_, cx| session.read(cx).crop("screen").unwrap());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(view.read(cx).text_editing.is_none());
        let s = session.read(cx);
        assert_eq!(s.crop("screen").unwrap(), live_pixels);
        assert!(!s.blocked());
        let shapes = s.annotations().visible().collect::<Vec<_>>();
        assert_eq!(shapes.len(), 1);
        assert_eq!(
            shapes[0].text.as_deref(),
            Some(
                "rental 中文
第二行很长的文字需要自动换行到下一行并立即显示"
            )
        );
        assert_eq!(shapes[0].size(), 32.);
        assert_ne!(s.crop("screen"), s.crop_original("screen"));
    });
    // Reopening must retain the shape's style even when the next-tool
    // presets differ, and must replace the original in the raster path.
    let original = cx.update(|_, cx| session.read(cx).annotations().committed()[0].clone());
    cx.update(|_, cx| {
        session.update(cx, |s, cx| {
            s.edit_annotations(|a| {
                a.deselect();
                a.set_tool_size(16.);
                a.set_color(1);
            });
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_click(point(px(65.), px(65.)), Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(65.), px(65.)),
                click_count: 2,
                ..Default::default()
            }),
            cx,
        );
    });
    cx.simulate_mouse_up(
        point(px(65.), px(65.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.run_until_parked();
    cx.update(|_, cx| {
        let editor = view.read(cx).text_editing.as_ref().unwrap();
        assert_eq!(editor.font_size, original.size());
        assert_eq!(editor.color, original.color);
    });
    assert_inline_text_caret(&view, cx);
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("replacement text");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().committed()[0]
                .text
                .as_deref(),
            Some("replacement text")
        );
        assert_ne!(session.read(cx).crop("screen").unwrap(), live_pixels);
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().committed()[0]
                .text
                .as_deref(),
            Some("replacement text")
        );
        assert_eq!(
            session.read(cx).annotations().tool(),
            Some(crate::annotation::ShapeKind::Text)
        );
        assert!(!session.read(cx).blocked());
    });
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(&session.read(cx).annotations().committed()[0], &original));
    cx.simulate_keystrokes("v");

    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_click(point(px(65.), px(65.)), Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(65.), px(65.)),
                click_count: 2,
                ..Default::default()
            }),
            cx,
        );
    });
    cx.simulate_mouse_up(
        point(px(65.), px(65.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.run_until_parked();
    assert_inline_text_caret(&view, cx);
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("a long replacement which must grow beyond the old text width");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("another line");
    cx.update(|_, cx| {
        session.update(cx, |s, cx| {
            s.edit_annotation_settings(|a| a.apply_size(40.));
            cx.notify();
        })
    });
    cx.run_until_parked();
    // This long paragraph wraps into too many rows at 40px with the
    // bundled font. Rejection must keep editor and model in sync.
    cx.update(|_, cx| {
        let editor = view.read(cx).text_editing.as_ref().unwrap();
        assert_eq!(editor.font_size, 32.);
        assert_eq!(session.read(cx).annotations().text_size(), 32.);
        assert_eq!(
            editor.value(cx),
            "a long replacement which must grow beyond the old text width\nanother line"
        );
    });
    // Separately prove that a resize which fits is applied, rather than
    // weakening the assertion to accept either font size.
    assert_inline_text_caret(&view, cx);
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("replacement text\nanother line");
    cx.update(|_, cx| {
        session.update(cx, |s, cx| {
            s.edit_annotation_settings(|a| a.apply_size(40.));
            cx.notify();
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let color = cx.debug_bounds("tb-color-2").unwrap();
    cx.simulate_click(color.center(), Default::default());
    cx.run_until_parked();
    assert_inline_text_caret(&view, cx);
    let edited_pixels = cx.update(|_, cx| {
        let editor = view.read(cx).text_editing.as_ref().unwrap();
        assert_eq!(editor.font_size, 40.);
        assert_eq!(
            editor.color,
            crate::model::settings::current().annotation_colors()[2]
        );
        session.read(cx).crop("screen").unwrap()
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(view.read(cx).text_editing.is_none());
        assert_eq!(session.read(cx).crop("screen").unwrap(), edited_pixels);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(&session.read(cx).annotations().committed()[0], &original));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert!(session.read(cx).annotations().visible().next().is_none()));
    cx.simulate_keystrokes("ctrl-y");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
    cx.simulate_keystrokes("t");
    cx.simulate_click(point(px(60.), px(230.)), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_input("keep on escape");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().tool(),
            Some(crate::annotation::ShapeKind::Text)
        );
        assert!(view.read(cx).text_editing.is_none());
    });
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().tool(), None));
    cx.update(|_, cx| {
        assert!(view.read(cx).text_editing.is_none());
        assert!(!session.read(cx).blocked());
        assert_eq!(session.read(cx).annotations().visible().count(), 2);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.simulate_keystrokes("t");
    // Empty confirmation must not add an invisible history entry.
    cx.simulate_click(point(px(60.), px(230.)), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(view.read(cx).text_editing.is_none());
        assert_eq!(session.read(cx).annotations().visible().count(), 1);
    });
    cx.simulate_click(point(px(60.), px(230.)), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_input("click outside");
    cx.simulate_click(point(px(540.), px(290.)), Default::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(view.read(cx).text_editing.is_none());
        assert!(!session.read(cx).blocked());
        assert_eq!(session.read(cx).annotations().visible().count(), 2);
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.start_text(point(px(60.), px(150.)), window, cx)
        });
        window.draw(cx).clear(cx);
    });
    cx.simulate_input("right click keeps text");
    cx.simulate_mouse_down(
        point(px(540.), px(290.)),
        MouseButton::Right,
        Default::default(),
    );
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(view.read(cx).text_editing.is_none());
        let marks = session.read(cx).annotations();
        assert_eq!(marks.tool(), None);
        assert_eq!(marks.selected_index(), None);
        assert_eq!(
            marks.visible().last().unwrap().text.as_deref(),
            Some("right click keeps text")
        );
    });
}

#[gpui_kit::test]
fn text_overflow_notice_and_escape_keep_the_visible_prefix(cx: &mut TestAppContext) {
    use gpui_kit::EntityInputHandler;
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        crate::actions::bind_keys(cx, &crate::actions::default_keys());
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 600;
    capture.height = 400;
    capture.rgba = vec![255; 600 * 400 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (view, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(600.), px(400.)));
    cx.update(|window, cx| {
        session.update(cx, |s, cx| {
            s.begin("screen", point(px(50.), px(80.)));
            s.end("screen", point(px(300.), px(230.)));
            s.edit_annotations(|a| a.toggle(crate::annotation::ShapeKind::Text));
            cx.notify();
        });
        view.update(cx, |view, cx| {
            view.start_text(point(px(80.), px(100.)), window, cx)
        });
    });
    cx.run_until_parked();
    let input = cx.update(|_, cx| view.read(cx).text_editing.as_ref().unwrap().input().clone());
    cx.update(|window, cx| {
        input.update(cx, |s, cx| {
            s.replace_and_mark_text_in_range(None, &"pinyin".repeat(100), None, window, cx);
            s.replace_text_in_range(None, &"测试文字".repeat(100), window, cx);
        })
    });
    cx.run_until_parked();
    let accepted = cx.update(|_, cx| input.read(cx).value());
    assert!(!accepted.is_empty());
    assert!(accepted.chars().count() < 400);
    for scale in [1., 1.25, 2.] {
        cx.simulate_scale_factor_change(scale);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let notice = cx.debug_bounds("text-overflow-notice").unwrap();
        assert!(notice.size.width > px(100.) && notice.size.height > px(10.));
        assert!(notice.left() >= px(0.) && notice.right() <= px(600.));
    }
    // Leaving the editor preserves committed text, not unfinished phonetics.
    cx.update(|window, cx| {
        input.update(cx, |s, cx| {
            s.replace_and_mark_text_in_range(None, "weishuruwancheng", None, window, cx);
        })
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let s = session.read(cx);
        assert!(!s.blocked());
        assert_eq!(
            s.annotations().tool(),
            Some(crate::annotation::ShapeKind::Text)
        );

        assert_eq!(
            s.annotations().committed()[0].text.as_deref(),
            Some(accepted.as_str())
        );
        assert!(view.read(cx).text_editing.is_none());
    });
    assert!(cx.debug_bounds("text-overflow-notice").is_none());
}

#[gpui_kit::test]
fn text_style_controls_preserve_selection_validate_size_and_undo(cx: &mut TestAppContext) {
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
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_down(
            point(px(20.), px(20.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_up(
            point(px(1150.), px(500.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let text = cx.debug_bounds("tb-text").unwrap();
        cx.simulate_click(text.center(), Default::default());
        cx.simulate_click(point(px(80.), px(80.)), Default::default());
        cx.run_until_parked();
        cx.simulate_input("Editable annotation");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let original = cx.update(|_, cx| session.read(cx).annotations().committed()[0].clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bold = cx.debug_bounds("text_bold").unwrap();
        cx.simulate_click(bold.center(), Default::default());
        cx.run_until_parked();
        cx.update(|_, cx| {
            let a = session.read(cx).annotations();
            assert_eq!(a.selected_index(), Some(0));
            assert!(a.selected().unwrap().text_style.bold);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let size = cx.debug_bounds("tb-size-readout").unwrap();
        cx.simulate_click(size.center(), Default::default());
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("36");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let a = session.read(cx).annotations();
            assert_eq!(a.selected_index(), Some(0));
            assert_eq!(a.text_size(), 36.);
            assert!(a.selected().unwrap().text_style.bold);
        });
        cx.update(|window, cx| {
            assert!(
                !view.read(cx).focus_handle.is_focused(window),
                "size input lost focus after confirmation"
            )
        });
        cx.simulate_keystrokes("ctrl-a");
        cx.update(|window, cx| {
            assert!(
                !view.read(cx).focus_handle.is_focused(window),
                "size input lost focus after select all"
            )
        });
        cx.simulate_input("NaN");
        cx.update(|window, cx| {
            assert!(
                !view.read(cx).focus_handle.is_focused(window),
                "size input lost focus after input"
            )
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let annotations = session.read(cx).annotations();
            assert_eq!(annotations.text_size(), 36.);
            assert_eq!(annotations.tool(), Some(crate::annotation::ShapeKind::Text));
            assert_eq!(annotations.selected_index(), Some(0));
        });
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            assert!(view.read(cx).focus_handle.is_focused(window));
            assert_eq!(session.read(cx).annotations().selected_index(), Some(0));
            window.draw(cx).clear(cx);
        });
        let italic = cx.debug_bounds("text_italic").unwrap();
        cx.simulate_click(italic.center(), Default::default());
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(
                session
                    .read(cx)
                    .annotations()
                    .selected()
                    .unwrap()
                    .text_style
                    .italic
            )
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            assert!(
                !session.read(cx).annotations().committed()[0]
                    .text_style
                    .italic
            )
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().committed()[0].size(),
                original.size()
            )
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed()[0], original));
    });
}

#[gpui_kit::test]
fn eraser_toolbar_modes_size_and_history(cx: &mut TestAppContext) {
    use crate::annotation::ShapeKind;
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        cx.bind_keys([gpui_kit::KeyBinding::new(
            "escape",
            crate::actions::EscapeOverlay,
            Some("ShotoriOverlay"),
        )]);
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 500;
    capture.height = 500;
    capture.rgba = vec![255; 500 * 500 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (view, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(500.), px(500.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // A selection, then one pencil stroke as erase fodder.
    cx.simulate_mouse_down(
        point(px(20.), px(20.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(350.), px(300.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let pencil = cx.debug_bounds("tb-pencil").unwrap();
    cx.simulate_click(pencil.center(), Default::default());
    cx.simulate_mouse_down(
        point(px(50.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(
        point(px(180.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(180.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 1));

    // The eraser's toolbar: no color palette; the size slider
    // drives the brush ring's radius.
    let eraser = cx.debug_bounds("tb-eraser").unwrap();
    cx.simulate_click(eraser.center(), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("tb-color-0").is_none());
    cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(48.))));
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().width(), 48.));

    let marked = cx.update(|_, cx| session.read(cx).crop("screen").unwrap());
    // Brush preview removes local pixels immediately, preserving the source shape.
    cx.simulate_mouse_down(
        point(px(100.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    let erased = cx.update(|_, cx| {
        let s = session.read(cx);
        assert_eq!(s.annotations().committed().len(), 1);
        let image = s.crop("screen").unwrap();
        let at = |x: usize| (30 * image.0 as usize + x - 20) * 4;
        assert_eq!(&image.2[at(100)..at(100) + 4], &[255; 4]);
        assert_eq!(&image.2[at(60)..at(60) + 4], &marked.2[at(60)..at(60) + 4]);
        assert_ne!(image, marked);
        image
    });
    cx.simulate_mouse_up(
        point(px(100.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|_, cx| {
        assert_eq!(session.read(cx).annotations().committed().len(), 2);
        assert_eq!(session.read(cx).crop("screen").unwrap(), erased);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(session.read(cx).crop("screen").unwrap(), marked));
    cx.simulate_keystrokes("ctrl-y");
    cx.update(|_, cx| assert_eq!(session.read(cx).crop("screen").unwrap(), erased));

    // Select area erasure through its actual menu. It previews while dragging
    // and records one editable mask, independent of the source mark.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let trigger = cx.debug_bounds("eraser-mode").unwrap();
    cx.simulate_click(trigger.center(), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let rect = cx.debug_bounds("toolbar.eraser_rect").unwrap();
    cx.simulate_click(rect.center(), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().tool(),
            Some(ShapeKind::EraserRect)
        )
    });
    assert!(cx.debug_bounds("tb-size-readout").is_none());
    cx.simulate_mouse_down(
        point(px(40.), px(30.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(
        point(px(220.), px(110.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|_, cx| {
        let s = session.read(cx);
        assert_eq!(s.annotations().committed().len(), 2);
        assert_eq!(s.crop("screen"), s.crop_original("screen"));
    });
    cx.simulate_mouse_up(
        point(px(220.), px(110.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|_, cx| {
        let s = session.read(cx);
        assert_eq!(s.annotations().committed().len(), 3);
        assert_eq!(
            s.annotations().selected().unwrap().kind,
            ShapeKind::EraserRect
        );
        assert_eq!(s.crop("screen"), s.crop_original("screen"));
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(session.read(cx).crop("screen").unwrap(), erased));
    cx.simulate_keystrokes("ctrl-y");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed().len(), 3));
    // The shortcut exits the active rectangle eraser without altering history.
    cx.simulate_keystrokes("d");
    cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
    cx.simulate_keystrokes("d");
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().tool(),
            Some(ShapeKind::Eraser)
        )
    });
    let _ = view;
}

fn stroke_and_polyline_workflows(cx: &mut TestAppContext, kind: crate::annotation::ShapeKind) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        cx.bind_keys([gpui_kit::KeyBinding::new(
            "escape",
            crate::actions::EscapeOverlay,
            Some("ShotoriOverlay"),
        )]);
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 500;
    capture.height = 500;
    capture.rgba = vec![255; 500 * 500 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(500.), px(500.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_down(
        point(px(20.), px(20.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(350.), px(300.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let line_button = cx
        .debug_bounds(match kind {
            crate::annotation::ShapeKind::Arrow => "tb-arrow",
            crate::annotation::ShapeKind::Number => "tb-number",
            crate::annotation::ShapeKind::Pencil => "tb-pencil",
            crate::annotation::ShapeKind::Highlighter => "tb-highlighter",
            crate::annotation::ShapeKind::Mosaic => "tb-mosaic",
            crate::annotation::ShapeKind::Eraser => "tb-eraser",
            _ => "tb-line",
        })
        .unwrap();
    cx.simulate_click(line_button.center(), Default::default());
    if kind == crate::annotation::ShapeKind::Number {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(40.))));
        cx.simulate_keystrokes("n");
        cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
        cx.simulate_keystrokes("n");
        cx.update(|_, cx| {
            assert_eq!(session.read(cx).annotations().tool(), Some(kind));
            assert_eq!(session.read(cx).annotations().number_size(), 40.);
        });
    }

    if kind == crate::annotation::ShapeKind::Eraser {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("tb-color-0").is_none());
        cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(48.))));
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().width(), 48.));
        let trigger = cx.debug_bounds("eraser-mode").unwrap();
        cx.simulate_click(trigger.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let rect = cx.debug_bounds("toolbar.eraser_rect").unwrap();
        cx.simulate_click(rect.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // (no size UI in rectangle-eraser mode — same as before)
        cx.simulate_keystrokes("d");
        cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
        cx.simulate_keystrokes("d");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().tool(), Some(kind)));
    }
    if kind == crate::annotation::ShapeKind::Arrow {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("a");
        cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
        cx.simulate_keystrokes("a");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().tool(), Some(kind)));
    }
    cx.simulate_mouse_down(
        point(px(50.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    if matches!(
        kind,
        crate::annotation::ShapeKind::Pencil | crate::annotation::ShapeKind::Highlighter
    ) {
        cx.simulate_mouse_move(
            point(px(90.), px(90.)),
            MouseButton::Left,
            Default::default(),
        );
    }
    cx.simulate_mouse_up(
        point(
            px(180.),
            px(if kind == crate::annotation::ShapeKind::Mosaic {
                100.
            } else {
                50.
            }),
        ),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|_, cx| {
        assert_eq!(
            session
                .read(cx)
                .annotations()
                .visible()
                .next()
                .unwrap()
                .kind,
            kind
        )
    });
    if matches!(
        kind,
        crate::annotation::ShapeKind::Pencil | crate::annotation::ShapeKind::Highlighter
    ) {
        cx.update(|_, cx| {
            let marks = session.read(cx).annotations();
            let stroke = marks.visible().next().unwrap().points.clone();
            // Three samples recorded, smoothed: the first lands exactly,
            // the rest trail their raw sample within the ink radius.
            assert_eq!(stroke.len(), 3);
            assert_eq!(stroke[0], point(px(50.), px(50.)));
            for (recorded, raw) in stroke[1..].iter().zip([(90., 90.), (180., 50.)]) {
                let dx = f32::from(recorded.x - px(raw.0));
                let dy = f32::from(recorded.y - px(raw.1));
                assert!(dx.hypot(dy) <= 3., "{recorded:?} trails {raw:?} too far");
            }
        });
        let key = if kind == crate::annotation::ShapeKind::Highlighter {
            "h"
        } else {
            "b"
        };
        cx.simulate_keystrokes(key);
        cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
        cx.simulate_keystrokes(key);
    }
    if kind == crate::annotation::ShapeKind::Mosaic {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let trigger = cx.debug_bounds("filter-effect").unwrap();
        cx.simulate_click(trigger.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let blur = cx.debug_bounds("toolbar.filter_blur").unwrap();
        cx.simulate_click(blur.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(24.))));
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().tool(),
                Some(crate::annotation::ShapeKind::Blur)
            );
            assert_eq!(session.read(cx).annotations().width(), 24.);
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            assert_eq!(
                session.read(cx).annotations().committed()[0].kind,
                crate::annotation::ShapeKind::Mosaic
            )
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 0));
        cx.simulate_keystrokes("ctrl-y");
        cx.simulate_keystrokes("ctrl-y");
        cx.simulate_keystrokes("m");
        cx.update(|_, cx| assert!(!session.read(cx).annotations().enabled()));
    }
    cx.simulate_keystrokes("p");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for (x, y) in [(50., 80.), (100., 150.), (180., 80.)] {
        cx.simulate_click(point(px(x), px(y)), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let color = cx.debug_bounds("tb-color-3").unwrap();
    cx.simulate_click(color.center(), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_keystrokes("enter");
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let marks = session.read(cx).annotations();
        assert!(!marks.is_drawing_polyline());
        assert_eq!(marks.visible().count(), 2);
        assert_eq!(marks.visible().last().unwrap().points.len(), 3);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
    cx.simulate_keystrokes("ctrl-y");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 2));
    // A right click commits only confirmed vertices, dropping the preview.
    cx.simulate_click(point(px(60.), px(190.)), Default::default());
    cx.simulate_click(point(px(180.), px(190.)), Default::default());
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                position: point(px(250.), px(230.)),
                ..Default::default()
            }),
            cx,
        );
    });
    cx.simulate_mouse_down(
        point(px(250.), px(230.)),
        MouseButton::Right,
        Default::default(),
    );
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        assert_eq!(
            session
                .read(cx)
                .annotations()
                .visible()
                .last()
                .unwrap()
                .points
                .len(),
            2
        );
    });
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().tool(),
            Some(crate::annotation::ShapeKind::Polyline)
        );
        assert!(!session.read(cx).annotations().is_drawing_polyline());
    });
    cx.simulate_mouse_down(
        point(px(250.), px(230.)),
        MouseButton::Right,
        Default::default(),
    );
    cx.update(|_, cx| {
        assert_eq!(session.read(cx).annotations().tool(), None);
        assert_eq!(session.read(cx).annotations().selected_index(), None);
    });
    cx.simulate_keystrokes("p");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Double-click finishes once without adding a duplicate endpoint.
    cx.simulate_click(point(px(60.), px(240.)), Default::default());
    cx.simulate_click(point(px(160.), px(240.)), Default::default());
    cx.simulate_mouse_down(
        point(px(160.), px(240.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(160.), px(240.)),
                click_count: 2,
                ..Default::default()
            }),
            cx,
        );
        window.draw(cx).clear(cx);
        let marks = session.read(cx).annotations();
        assert!(!marks.is_drawing_polyline());
        assert_eq!(marks.visible().count(), 4);
        assert_eq!(marks.visible().last().unwrap().points.len(), 2);
    });
    cx.simulate_click(point(px(60.), px(260.)), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 4));
}

fn geometry_toolbar_keyboard_and_export(
    cx: &mut TestAppContext,
    kind: crate::annotation::ShapeKind,
) {
    let is_rectangle = kind == crate::annotation::ShapeKind::Rectangle;
    let key = if is_rectangle { "r" } else { "e" };
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        cx.bind_keys([gpui_kit::KeyBinding::new(
            "escape",
            crate::actions::EscapeOverlay,
            Some("ShotoriOverlay"),
        )]);
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 400;
    capture.height = 400;
    capture.rgba = vec![255; 400 * 400 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(400.), px(400.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_down(
        point(px(20.), px(20.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(
        point(px(250.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(250.), px(180.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let button = cx
        .debug_bounds(if is_rectangle {
            "tb-rectangle"
        } else {
            "tb-ellipse"
        })
        .expect("geometry toolbar button");
    cx.simulate_click(button.center(), Default::default());
    cx.update(|_, cx| assert!(session.read(cx).annotations().enabled()));
    let shift = gpui_kit::Modifiers {
        shift: true,
        ..Default::default()
    };
    cx.simulate_mouse_down(
        point(px(50.), px(50.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(point(px(90.), px(110.)), MouseButton::Left, shift);
    cx.simulate_mouse_up(point(px(90.), px(110.)), MouseButton::Left, shift);
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let shared = session.read(cx);
        let rectangle = shared.annotations().visible().next().unwrap();
        assert_eq!(rectangle.kind, kind);
        assert_eq!(rectangle.bounds.size, size(px(60.), px(60.)));
        assert_eq!(
            shared.selection().bounds().unwrap().size,
            size(px(230.), px(160.))
        );
        assert_ne!(
            shared.crop("screen").unwrap().2,
            shared.crop_original("screen").unwrap().2
        );
        let painted = window.painted_quads();
        let scale = window.scale_factor();
        for stroke in rectangle.strokes().into_iter().filter(|_| is_rectangle) {
            assert!(
                painted.iter().any(|quad| {
                    quad.bounds.origin.x.0 == f32::from(stroke.origin.x) * scale
                        && quad.bounds.origin.y.0 == f32::from(stroke.origin.y) * scale
                        && quad.bounds.size.width.0 == f32::from(stroke.size.width) * scale
                        && quad.bounds.size.height.0 == f32::from(stroke.size.height) * scale
                }),
                "rectangle stroke {stroke:?} missing from preview: {:?}",
                painted.iter().map(|q| q.bounds).collect::<Vec<_>>()
            );
        }
    });
    assert!(cx.debug_bounds("tb-undo").is_none());
    assert!(cx.debug_bounds("tb-redo").is_none());
    // Settings clicks must not strand keyboard focus on a transient button.
    // This test changes the next-stroke preset; editing the selected
    // shape would correctly add its own undoable style change.
    cx.update(|_, cx| session.update(cx, |s, _| s.edit_annotations(|a| a.deselect())));
    {
        cx.simulate_resize(size(px(400.), px(400.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let options = cx.debug_bounds("tb-options").unwrap();
        cx.simulate_event(gpui_kit::ScrollEvent {
            position: point(px(200.), options.center().y),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(-200.), px(0.))),
            modifiers: Default::default(),
            touch_phase: Default::default(),
            source: Default::default(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let button = cx.debug_bounds("tb-color-3").unwrap();
        assert!(
            button.left() >= px(0.) && button.right() <= px(400.),
            "color bounds: {button:?}; options: {options:?}"
        );
        cx.simulate_click(button.center(), Default::default());
        cx.simulate_resize(size(px(400.), px(400.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 0));
        cx.simulate_keystrokes("ctrl-y");
        cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
    }
    cx.update(|_, cx| {
        // (the old tb-width-2 detent click set this; the detents are
        // gone — set the stroke width through the session instead)
        session.update(cx, |s, _| s.edit_annotations(|a| a.set_tool_size(5.)));
        assert_eq!(session.read(cx).annotations().color().1, "Green");
        assert_eq!(session.read(cx).annotations().width(), 5.);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 0));
    cx.simulate_keystrokes("ctrl-y");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        assert!(!session.read(cx).annotations().enabled());
        assert_eq!(session.read(cx).annotations().visible().count(), 1);
    });
    cx.simulate_keystrokes(key);
    cx.update(|_, cx| assert!(session.read(cx).annotations().enabled()));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Even keyboard focus on a settings button must survive that row's
    // removal when the active tool is toggled off.
    cx.simulate_keystrokes("tab tab tab tab tab tab");
    cx.simulate_keystrokes(key);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 0));
    cx.simulate_keystrokes("ctrl-shift-z");
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().visible().count(), 1));
}

#[gpui_kit::test]
fn keyboard_movement_and_reselection_cancel_use_the_overlay_event_pipeline(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{KeyDownEvent, KeyUpEvent, Keystroke};
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_kit::component::init(cx);
        crate::actions::init_annotation_keybindings(cx);
        crate::actions::bind_keys(cx, &crate::actions::default_keys());
    });
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 640;
    capture.height = 480;
    capture.rgba = vec![255; 640 * 480 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(640.), px(480.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|_, cx| {
        session.update(cx, |s, cx| {
            s.begin("screen", point(px(40.), px(40.)));
            s.end("screen", point(px(440.), px(300.)));
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let key = Keystroke::parse("right").unwrap();
    for held in [false, true, true] {
        cx.simulate_event(KeyDownEvent {
            keystroke: key.clone(),
            is_held: held,
            prefer_character_input: false,
        });
    }
    cx.simulate_event(KeyUpEvent { keystroke: key });
    cx.simulate_keystrokes("shift-down");
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("shift-down").unwrap(),
    });
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).selection().bounds().unwrap().origin,
            point(px(43.), px(50.))
        )
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).selection().bounds().unwrap().origin,
            point(px(43.), px(40.))
        )
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).selection().bounds().unwrap().origin,
            point(px(40.), px(40.))
        )
    });
    cx.simulate_keystrokes("ctrl-y");
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).selection().bounds().unwrap().origin,
            point(px(43.), px(40.))
        )
    });

    cx.simulate_keystrokes("r");
    cx.simulate_mouse_down(
        point(px(90.), px(90.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(180.), px(150.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_keystrokes("v");
    cx.simulate_click(point(px(120.), px(86.)), Default::default());
    cx.simulate_keystrokes("shift-right");
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("shift-right").unwrap(),
    });
    cx.update(|_, cx| {
        let s = session.read(cx);
        assert_eq!(
            s.annotations().selected().unwrap().bounds.origin,
            point(px(100.), px(90.))
        );
        assert_eq!(
            s.selection().bounds().unwrap().origin,
            point(px(43.), px(40.))
        );
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().committed()[0].bounds.origin,
            point(px(90.), px(90.))
        )
    });
    cx.simulate_mouse_down(
        point(px(520.), px(100.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(
        point(px(600.), px(200.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_keystrokes("escape");
    cx.simulate_mouse_up(
        point(px(600.), px(200.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|_, cx| {
        let s = session.read(cx);
        assert_eq!(
            s.selection().bounds().unwrap().origin,
            point(px(43.), px(40.))
        );
        assert_eq!(s.annotations().committed().len(), 1);
        assert_eq!(
            s.annotations().tool(),
            Some(crate::annotation::ShapeKind::Select)
        );
    });
}

#[gpui_kit::test]
fn annotation_style_menus_select_without_drawing_on_the_capture(cx: &mut TestAppContext) {
    use crate::annotation::{LensConnector, LineStyle, ShapeKind};
    cx.update(gpui_kit::base::init);
    cx.update(gpui_kit::component::init);
    cx.update(|cx| crate::actions::bind_keys(cx, &crate::actions::default_keys()));
    let mut capture = Capture::for_test((0, 0), 1.);
    capture.output_name = "screen".into();
    capture.width = 900;
    capture.height = 700;
    capture.rgba = vec![255; 900 * 700 * 4];
    let capture = Arc::new(capture);
    let session = cx.new(|_| ScreenshotSession::new(vec![capture.clone()], Vec::new()));
    let (_, cx) =
        cx.add_window_view(|window, cx| Overlay::new(capture, session.clone(), window, cx));
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.update(|window, cx| {
        session.update(cx, |s, cx| {
            s.begin("screen", point(px(20.), px(20.)));
            s.end("screen", point(px(700.), px(350.)));
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Magnifier);
                let area = gpui_kit::Bounds::new(point(px(20.), px(20.)), size(px(680.), px(330.)));
                a.begin(point(px(100.), px(100.)), area, false);
                a.drag_to(point(px(180.), px(160.)), area, false);
                a.end();
                a.select_index(0);
            });
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("lens-size").is_none());
    let trigger = cx.debug_bounds("lens-connector").unwrap();
    cx.simulate_click(trigger.center(), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("toolbar.connector_none").is_some());
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        assert_eq!(
            session.read(cx).annotations().tool(),
            Some(ShapeKind::Magnifier)
        );
        assert_eq!(session.read(cx).annotations().selected_index(), Some(0));
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("toolbar.connector_none").is_none());
    for (key, value) in [
        ("toolbar.connector_none", LensConnector::None),
        ("toolbar.connector_center_dot", LensConnector::CenterDot),
        ("toolbar.connector_border", LensConnector::Border),
        ("toolbar.connector_center", LensConnector::Center),
    ] {
        let trigger = cx.debug_bounds("lens-connector").unwrap();
        assert_eq!(trigger.size.width, px(38.));
        assert_eq!(trigger.size.height, px(30.));
        cx.simulate_click(trigger.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let choice = cx.debug_bounds(key).expect("visible connector choice");
        assert_eq!(choice.size.width, px(34.));
        cx.simulate_click(choice.center(), Default::default());
        cx.update(|window, cx| {
            assert_eq!(session.read(cx).annotations().lens_options().3, value);
            assert_eq!(session.read(cx).annotations().committed().len(), 1);
            assert_eq!(session.read(cx).annotations().selected_index(), Some(0));
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds(key).is_none(), "choice closes popup");
    }
    for key in ["toolbar.lens_annotations", "toolbar.lens_smooth"] {
        let button = cx.debug_bounds(key).expect("independent effect toggle");
        cx.simulate_click(button.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    cx.update(|window, cx| {
        assert_eq!(
            session.read(cx).annotations().lens_options(),
            (false, false, false, LensConnector::Center)
        );
        session.update(cx, |s, cx| {
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Line);
                let area = gpui_kit::Bounds::new(point(px(20.), px(20.)), size(px(680.), px(330.)));
                a.begin(point(px(100.), px(220.)), area, false);
                a.drag_to(point(px(450.), px(260.)), area, false);
                a.end();
                a.select_index(1);
            });
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    for (key, value) in [
        ("toolbar.line_dashed", LineStyle::Dashed),
        ("toolbar.line_dotted", LineStyle::Dotted),
        ("toolbar.line_dash_dot", LineStyle::DashDot),
        ("toolbar.line_dash_dot_dot", LineStyle::DashDotDot),
        ("toolbar.line_solid", LineStyle::Solid),
    ] {
        let trigger = cx.debug_bounds("line-style").unwrap();
        cx.simulate_click(trigger.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let choice = cx.debug_bounds(key).expect("visible line choice");
        cx.simulate_click(choice.center(), Default::default());
        cx.update(|window, cx| {
            assert_eq!(session.read(cx).annotations().line_style(), value);
            assert_eq!(session.read(cx).annotations().committed().len(), 2);
            assert_eq!(session.read(cx).annotations().selected_index(), Some(1));
            assert_eq!(
                session.read(cx).annotations().committed()[1].line_style(),
                value
            );
            window.draw(cx).clear(cx);
        });
    }
    for (kind, trigger_id, ix) in [
        (ShapeKind::Magnifier, "lens-shape", 0),
        (ShapeKind::Spotlight, "spotlight-shape", 2),
    ] {
        cx.update(|window, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| {
                    a.toggle(kind);
                    if kind == ShapeKind::Spotlight {
                        let area = gpui_kit::Bounds::new(
                            point(px(20.), px(20.)),
                            size(px(680.), px(330.)),
                        );
                        a.begin(point(px(220.), px(80.)), area, false);
                        a.drag_to(point(px(300.), px(160.)), area, false);
                        a.end();
                    }
                    a.select_index(ix);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        for (key, ellipse) in [
            ("toolbar.lens_rectangle", false),
            ("toolbar.lens_ellipse", true),
        ] {
            let trigger = cx.debug_bounds(trigger_id).unwrap();
            cx.simulate_click(trigger.center(), Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let choice = cx.debug_bounds(key).unwrap();
            cx.simulate_click(choice.center(), Default::default());
            cx.update(|window, cx| {
                let marks = session.read(cx).annotations();
                assert_eq!(marks.selected_index(), Some(ix));
                assert_eq!(
                    if kind == ShapeKind::Spotlight {
                        marks.spotlight_ellipse()
                    } else {
                        marks.lens_options().0
                    },
                    ellipse
                );
                window.draw(cx).clear(cx);
            });
        }
    }
    cx.update(|window, cx| {
        session.update(cx, |s, cx| {
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Mosaic);
                a.set_filter_area(true);
                let area = gpui_kit::Bounds::new(point(px(20.), px(20.)), size(px(680.), px(330.)));
                a.begin(point(px(100.), px(100.)), area, false);
                a.drag_to(point(px(180.), px(160.)), area, false);
                a.end();
                a.select_index(3);
            });
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    for (trigger_id, key) in [
        ("filter-effect", "toolbar.filter_blur"),
        ("filter-effect", "toolbar.filter_mosaic"),
        ("filter-size", "toolbar.filter_strength"),
        ("filter-size", "toolbar.filter_diameter"),
    ] {
        assert!(cx.debug_bounds(key).is_none());
        let trigger = cx.debug_bounds(trigger_id).unwrap();
        cx.simulate_click(trigger.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let choice = cx.debug_bounds(key).unwrap();
        cx.simulate_click(choice.center(), Default::default());
        cx.update(|window, cx| {
            let marks = session.read(cx).annotations();
            assert_eq!(marks.selected_index(), Some(3));
            assert_eq!(marks.committed().len(), 4);
            match key {
                "toolbar.filter_blur" => assert_eq!(marks.committed()[3].kind, ShapeKind::Blur),
                "toolbar.filter_mosaic" => assert_eq!(marks.committed()[3].kind, ShapeKind::Mosaic),
                "toolbar.filter_strength" => assert!(!marks.editing_filter_brush_size()),
                _ => assert!(marks.editing_filter_brush_size()),
            }
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds(key).is_none());
    }
    // Changing the drawing method affects subsequent strokes, not existing geometry.
    for (key, brush) in [
        ("toolbar.filter_rectangle", false),
        ("toolbar.filter_brush", true),
    ] {
        let trigger = cx.debug_bounds("filter-area").unwrap();
        cx.simulate_click(trigger.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let choice = cx.debug_bounds(key).unwrap();
        cx.simulate_click(choice.center(), Default::default());
        cx.update(|window, cx| {
            let marks = session.read(cx).annotations();
            assert_eq!(
                matches!(
                    marks.filter_area(),
                    crate::annotation::FilterArea::Brush { .. }
                ),
                brush
            );
            assert_eq!(marks.committed().len(), 4);
            assert!(matches!(
                marks.committed()[3].filter_area(),
                Some(crate::annotation::FilterArea::Brush { .. })
            ));
            window.draw(cx).clear(cx);
        });
    }
    cx.update(|window, cx| {
        session.update(cx, |s, cx| {
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Arrow);
                let area = gpui_kit::Bounds::new(point(px(20.), px(20.)), size(px(680.), px(330.)));
                a.begin(point(px(100.), px(220.)), area, false);
                a.drag_to(point(px(450.), px(260.)), area, false);
                a.end();
                a.select_index(4);
            });
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    use crate::annotation::ArrowStyle;
    for (key, value) in [
        ("toolbar.arrow_double_open", ArrowStyle::DoubleOpen),
        ("toolbar.arrow_outline", ArrowStyle::Outline),
        ("toolbar.arrow_filled", ArrowStyle::Filled),
        ("toolbar.arrow_dimension", ArrowStyle::Dimension),
        ("toolbar.arrow_triangle", ArrowStyle::Triangle),
        ("toolbar.arrow_double_triangle", ArrowStyle::DoubleTriangle),
        ("toolbar.arrow_open", ArrowStyle::Open),
    ] {
        let trigger = cx.debug_bounds("arrow-style").unwrap();
        cx.simulate_click(trigger.center(), Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let choice = cx.debug_bounds(key).expect("visible arrow choice");
        cx.simulate_click(choice.center(), Default::default());
        cx.update(|window, cx| {
            let marks = session.read(cx).annotations();
            assert_eq!(marks.selected_index(), Some(4));
            assert_eq!(marks.committed().len(), 5);
            assert_eq!(marks.committed()[4].arrow_style(), value);
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("line-style").is_some(),
            value.supports_line_style()
        );
    }
    // Menu edits must retain the selected mark rather than falling through to canvas input.
    let ix = cx.update(|window, cx| {
        let ix = session.update(cx, |s, cx| {
            let ix = s.annotations().committed().len();
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Number);
                let area = gpui_kit::Bounds::new(point(px(20.), px(20.)), size(px(680.), px(330.)));
                a.begin(point(px(100.), px(80.)), area, false);
                a.drag_to(point(px(180.), px(160.)), area, false);
                a.end();
                a.select_index(ix);
            });
            cx.notify();
            ix
        });
        window.draw(cx).clear(cx);
        ix
    });
    let bounds = cx
        .debug_bounds("number-format")
        .expect("visible option trigger");
    cx.simulate_click(bounds.center(), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let bounds = cx
        .debug_bounds("toolbar.number_alphabetic")
        .expect("visible option choice");
    cx.simulate_click(bounds.center(), Default::default());
    cx.update(|window, cx| {
        let marks = session.read(cx).annotations();
        assert_eq!(marks.selected_index(), Some(ix));
        assert_eq!(marks.committed().len(), ix + 1);
        assert_eq!(
            marks.committed()[ix].number_format(),
            crate::annotation::NumberFormat::Alphabetic
        );
        window.draw(cx).clear(cx);
    });
}

#[gpui_kit::test]
fn spotlight_controls_keep_selection_and_route_modified_wheel(cx: &mut TestAppContext) {
    use crate::annotation::ShapeKind;
    use gpui_kit::{Modifiers, ScrollDelta, ScrollEvent};
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
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_down(
        point(px(20.), px(20.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(1150.), px(500.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.dispatch_action(Box::new(crate::actions::ToggleSpotlight), cx));
    cx.simulate_mouse_down(
        point(px(200.), px(100.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(
        point(px(400.), px(250.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(400.), px(250.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let original = cx.update(|_, cx| session.read(cx).annotations().selected().unwrap().clone());
    let outline_button = cx.debug_bounds("spotlight-outline").unwrap();
    cx.simulate_click(outline_button.center(), Default::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let a = session.read(cx).annotations();
        assert_eq!(a.selected_index(), Some(0));
        assert_eq!(a.spotlight_outline(), (true, 2.));
    });
    for modifiers in [
        Modifiers {
            control: true,
            ..Default::default()
        },
        Modifiers {
            shift: true,
            ..Default::default()
        },
        Modifiers::default(),
    ] {
        cx.simulate_event(ScrollEvent {
            position: original.bounds.center(),
            delta: ScrollDelta::Lines(point(0., 1.)),
            modifiers,
            touch_phase: Default::default(),
            source: Default::default(),
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|_, cx| {
            let a = session.read(cx).annotations();
            let shape = a.selected().unwrap();
            assert_eq!(shape.kind, ShapeKind::Spotlight);
            assert_eq!(a.spotlight_outline(), (true, 3.));
            if modifiers.control {
                assert_eq!(shape.bounds, original.bounds);
                assert_eq!(shape.size(), original.size());
            } else {
                assert!(shape.bounds.size.width > original.bounds.size.width);
                assert_eq!(shape.bounds.center(), original.bounds.center());
                assert_eq!(
                    shape.size(),
                    original.size() + if modifiers.shift { 0. } else { 1. }
                );
            }
        });
    }
    for _ in 0..4 {
        cx.simulate_keystrokes("ctrl-z");
    }
    cx.update(|_, cx| assert_eq!(session.read(cx).annotations().committed(), &[original]));
}

#[gpui_kit::test]
fn sequence_value_input_keeps_selection_and_restores_cursor_on_undo(cx: &mut TestAppContext) {
    use crate::annotation::ShapeKind;
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
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_down(
        point(px(20.), px(20.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_up(
        point(px(1150.), px(500.)),
        MouseButton::Left,
        Default::default(),
    );
    cx.update(|window, cx| window.dispatch_action(Box::new(crate::actions::ToggleNumber), cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let input = cx.debug_bounds("tb-number-value").unwrap();
    cx.simulate_click(input.center(), Default::default());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("12");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let a = session.read(cx).annotations();
        assert_eq!(a.next_number(), 12);
        assert_eq!(a.tool(), Some(ShapeKind::Number));
        assert!(a.committed().is_empty());
        assert!(!view.read(cx).focus_handle.is_focused(window));
    });
    cx.simulate_keystrokes("escape");
    cx.simulate_click(point(px(120.), px(100.)), Default::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().committed()[0].number,
            Some(12)
        );
        assert_eq!(session.read(cx).annotations().next_number(), 13);
    });
    let input = cx.debug_bounds("tb-number-value").unwrap();
    cx.simulate_click(input.center(), Default::default());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("9");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    for invalid in ["0", "-1", "4294967296", "n", ""] {
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input(invalid);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let a = session.read(cx).annotations();
            assert_eq!(a.selected_index(), Some(0));
            assert_eq!(a.committed()[0].number, Some(9));
            assert_eq!(a.next_number(), 13);
        });
    }
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().committed()[0].number,
            Some(12)
        )
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| {
        assert!(session.read(cx).annotations().committed().is_empty());
        assert_eq!(session.read(cx).annotations().next_number(), 12);
    });
}

#[gpui_kit::test]
fn rectangle_radius_popup_keeps_width_selection_and_undo_independent(cx: &mut TestAppContext) {
    use crate::annotation::ShapeKind;
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
            s.begin("screen", point(px(20.), px(20.)));
            s.end("screen", point(px(1100.), px(450.)));
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Rectangle);
                a.apply_size(4.);
                let area =
                    gpui_kit::Bounds::new(point(px(20.), px(20.)), size(px(1080.), px(430.)));
                a.begin(point(px(100.), px(100.)), area, false);
                a.drag_to(point(px(450.), px(300.)), area, false);
                a.end();
                a.select_index(0);
            });
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("geometry-shape").is_none());
    assert!(cx.debug_bounds("geometry-size").is_none());
    let trigger = cx.debug_bounds("corner-radius").unwrap();
    cx.simulate_click(trigger.center(), Default::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let slider = cx.debug_bounds("corner-radius-slider").unwrap();
    cx.simulate_click(
        slider.origin + point(px(35.), slider.size.height / 2.),
        Default::default(),
    );
    cx.run_until_parked();
    let radius = cx.update(|window, cx| {
        let a = session.read(cx).annotations();
        assert_eq!(a.selected_index(), Some(0));
        assert_eq!(a.committed().len(), 1);
        assert_eq!(a.current_edit_size(), 4.);
        assert!(a.geometry_radius() > 0.);
        let radius = a.geometry_radius();
        window.draw(cx).clear(cx);
        radius
    });
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("corner-radius-slider").is_none());
    let width_slider = cx.debug_bounds("tb-primary-size").unwrap();
    cx.simulate_click(
        width_slider.origin + point(px(68.), width_slider.size.height / 2.),
        Default::default(),
    );
    cx.run_until_parked();
    cx.update(|window, cx| {
        let a = session.read(cx).annotations();
        assert_eq!(a.selected_index(), Some(0));
        assert!(a.current_edit_size() > 4.);
        assert_eq!(a.geometry_radius(), radius);
        window.draw(cx).clear(cx);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| {
        let a = session.read(cx).annotations();
        assert_eq!(a.committed()[0].size(), 4.);
        assert_eq!(a.committed()[0].geometry_radius(), radius);
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().committed()[0].geometry_radius(),
            0.
        )
    });
    cx.simulate_keystrokes("ctrl-y");
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().committed()[0].geometry_radius(),
            radius
        )
    });
    cx.update(|window, cx| {
        window.dispatch_action(Box::new(crate::actions::ToggleEllipse), cx);
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("corner-radius").is_none());
    assert!(cx.debug_bounds("geometry-shape").is_none());
    cx.update(|_, cx| {
        assert_eq!(
            session.read(cx).annotations().edit_kind(),
            Some(ShapeKind::Ellipse)
        )
    });
}
