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

#[gpui_kit::test]
fn text_colors_are_direct_controls_and_preserve_editing_and_history(cx: &mut TestAppContext) {
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
                s.pointer_down("screen", point(px(20.), px(20.)), false, false);
                s.pointer_up("screen", point(px(1150.), px(500.)), false);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        let text = cx.debug_bounds("tb-text").unwrap();
        cx.simulate_click(text.center(), Default::default());
        cx.simulate_click(point(px(80.), px(80.)), Default::default());
        cx.run_until_parked();
        cx.simulate_input("Editable colors");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        for (toggle, picker, background) in [
            ("text_background", "text-background-color", true),
            ("text_outline", "text-outline-color", false),
        ] {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let target = cx.debug_bounds(toggle).unwrap().center();
            cx.simulate_click(target, Default::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                assert_eq!(
                    session.read(cx).annotations().selected_index(),
                    Some(0),
                    "after {toggle}"
                );
                window.draw(cx).clear(cx);
            });
            let target = cx.debug_bounds(picker).unwrap().center();
            cx.simulate_click(target, Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|_, cx| {
                assert_eq!(
                    session.read(cx).annotations().selected_index(),
                    Some(0),
                    "after opening {picker}"
                )
            });
            let target = cx.debug_bounds("palette-1-5").unwrap().center();
            cx.simulate_click(target, Default::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let a = session.read(cx).annotations();
                let style = &a.selected().unwrap().text_style;
                let color = if background {
                    style.background
                } else {
                    style.outline
                };
                assert_eq!(
                    color,
                    Some(super::hsla_to_u32(gpui_kit::component::theme::red(500)))
                );
                assert_eq!(a.selected_index(), Some(0));
                assert!(view.read(cx).focus_handle.is_focused(window));
            });
            cx.simulate_keystrokes("ctrl-z");
            cx.update(|_, cx| {
                let style = &session.read(cx).annotations().committed()[0].text_style;
                assert_eq!(
                    if background {
                        style.background
                    } else {
                        style.outline
                    },
                    Some(0xffffffff)
                );
            });
            cx.simulate_keystrokes("ctrl-y");
            cx.update(|_, cx| {
                session.update(cx, |s, cx| {
                    s.edit_annotations(|a| {
                        a.select_index(0);
                    });
                    cx.notify();
                })
            });
        }
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let options = cx.debug_bounds("text-background-options").unwrap().center();
        cx.simulate_click(options, Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("toolbar.corner_radius").is_some());
        let color = cx.debug_bounds("text-background-color").unwrap().center();
        cx.simulate_click(color, Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("palette-1-5").is_some(),
            "one click switches from parameters to color"
        );
        assert!(cx.debug_bounds("toolbar.corner_radius").is_none());
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            assert_eq!(session.read(cx).annotations().selected_index(), Some(0));
            assert!(view.read(cx).focus_handle.is_focused(window));
        });
        let position = cx.update(|_, cx| {
            session.read(cx).annotations().committed()[0]
                .bounds
                .center()
        });
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
        cx.update(|window, cx| {
            assert!(view.read(cx).text_editing.is_some());
            window.draw(cx).clear(cx);
        });
        let target = cx.debug_bounds("text-background-color").unwrap().center();
        cx.simulate_click(target, Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let target = cx.debug_bounds("palette-4-5").unwrap().center();
        cx.simulate_click(target, Default::default());
        cx.run_until_parked();
        cx.simulate_input(" continued");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let shape = &session.read(cx).annotations().committed()[0];
            assert!(shape.text.as_ref().unwrap().contains(" continued"));
            assert_eq!(
                shape.text_style.background,
                Some(super::hsla_to_u32(gpui_kit::component::theme::green(500)))
            );
        });
    });
}

#[gpui_kit::test]
fn custom_palette_applies_to_selected_marks_and_live_text(cx: &mut TestAppContext) {
    // Color edits persist global palette slots. Exercise that production path in
    // a separate process so neither the user's config nor other tests are changed.
    const CHILD: &str = "SHOTORI_TEST_PALETTE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let config = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ui::overlay::interaction_tests::custom_palette_applies_to_selected_marks_and_live_text",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("XDG_CONFIG_HOME", config.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
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
        for kind in [
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Line,
            ShapeKind::Arrow,
            ShapeKind::Pencil,
            ShapeKind::Highlighter,
            ShapeKind::Magnifier,
            ShapeKind::Spotlight,
            ShapeKind::Number,
            ShapeKind::Text,
        ] {
            cx.update(|window, cx| {
                session.update(cx, |s, cx| {
                    s.clear_annotations();
                    s.begin("screen", point(px(20.), px(20.)));
                    s.end("screen", point(px(1150.), px(500.)));
                    s.edit_annotations(|a| {
                        a.toggle(kind);
                        let bounds =
                            gpui_kit::Bounds::new(point(px(80.), px(80.)), size(px(180.), px(90.)));
                        if kind == ShapeKind::Text {
                            a.add_text(bounds, "Editable color".into());
                        } else {
                            a.begin(
                                bounds.origin,
                                gpui_kit::Bounds::new(
                                    point(px(20.), px(20.)),
                                    size(px(1130.), px(480.)),
                                ),
                                false,
                            );
                            a.drag_to(
                                bounds.bottom_right(),
                                gpui_kit::Bounds::new(
                                    point(px(20.), px(20.)),
                                    size(px(1130.), px(480.)),
                                ),
                                false,
                            );
                            a.end();
                        }
                        a.select_index(0);
                        if kind == ShapeKind::Spotlight {
                            a.set_spotlight_outline(true);
                        }
                    });
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let trigger = cx
                .debug_bounds("color-picker-trigger")
                .expect("active swatch")
                .center();
            let before = cx.update(|_, cx| session.read(cx).annotations().committed()[0].color);
            let expected = super::hsla_to_u32(
                if before == super::hsla_to_u32(gpui_kit::component::theme::red(500)) {
                    gpui_kit::component::theme::green(500)
                } else {
                    gpui_kit::component::theme::red(500)
                },
            );
            cx.simulate_click(trigger, Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let selector = if expected == super::hsla_to_u32(gpui_kit::component::theme::red(500)) {
                "palette-1-5"
            } else {
                "palette-4-5"
            };
            let target = cx.debug_bounds(selector).unwrap().center();
            cx.simulate_click(target, Default::default());
            cx.run_until_parked();
            cx.update(|_, cx| {
                let a = session.read(cx).annotations();
                assert_eq!(a.selected_index(), Some(0), "{kind:?}");
                assert_eq!(a.committed()[0].color, expected, "{kind:?}");
            });
            cx.simulate_keystrokes("ctrl-z");
            cx.update(|_, cx| {
                assert_eq!(
                    session.read(cx).annotations().committed()[0].color,
                    before,
                    "undo {kind:?}"
                )
            });
            cx.simulate_keystrokes("ctrl-y");
            cx.update(|_, cx| {
                assert_eq!(
                    session.read(cx).annotations().committed()[0].color,
                    expected,
                    "redo {kind:?}"
                )
            });
        }
        // The same subscription must update the live editor while canvas edits
        // are blocked, without prematurely committing its text transaction.
        cx.update(|window, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| {
                    a.begin_text_edit(0);
                });
                cx.notify();
            });
            window.draw(cx).clear(cx);
            let picker = view.read(cx).swatch_picker.clone().unwrap();
            picker.update(cx, |state, cx| {
                state.update_color(gpui_kit::hsla(0.6, 0.8, 0.5, 0.25), window, cx)
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let expected = super::hsla_to_u32(gpui_kit::hsla(0.6, 0.8, 0.5, 1.));
            assert_eq!(
                session.read(cx).annotations().editing_text().unwrap().color,
                expected
            );
            assert_eq!(
                view.read(cx)
                    .swatch_picker
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .value()
                    .unwrap()
                    .a,
                1.
            );
        });
    });
}

#[gpui_kit::test]
fn secondary_sliders_move_the_thumb_and_keep_one_undo_step(cx: &mut TestAppContext) {
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
        for (kind, menu) in [
            (ShapeKind::Rectangle, "corner-radius"),
            (ShapeKind::Spotlight, "spotlight-outline-options"),
            (ShapeKind::Text, "text-background-options"),
        ] {
            cx.update(|window, cx| {
                session.update(cx, |s, cx| {
                    s.clear_annotations();
                    s.begin("screen", point(px(20.), px(20.)));
                    s.end("screen", point(px(1150.), px(500.)));
                    s.edit_annotations(|a| {
                        a.toggle(kind);
                        let bounds =
                            gpui_kit::Bounds::new(point(px(80.), px(80.)), size(px(180.), px(90.)));
                        if kind == ShapeKind::Text {
                            a.add_text(bounds, "Rounded background".into());
                        } else {
                            let area = gpui_kit::Bounds::new(
                                point(px(20.), px(20.)),
                                size(px(1130.), px(480.)),
                            );
                            a.begin(bounds.origin, area, false);
                            a.drag_to(bounds.bottom_right(), area, false);
                            a.end();
                        }
                        a.select_index(0);
                        if kind == ShapeKind::Text {
                            let mut style = a.text_style().clone();
                            style.background = Some(0xffffffff);
                            a.set_text_style(style);
                        } else if kind == ShapeKind::Spotlight {
                            a.set_spotlight_outline(true);
                        }
                    });
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let before = cx.update(|_, cx| session.read(cx).annotations().committed()[0].clone());
            let trigger = cx.debug_bounds(menu).unwrap().center();
            cx.simulate_click(trigger, Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let track = cx.debug_bounds("annotation-slider-track").unwrap();
            let thumb_before = cx.debug_bounds("annotation-slider-thumb").unwrap();
            cx.simulate_mouse_down(track.center(), MouseButton::Left, Default::default());
            cx.simulate_mouse_move(
                point(track.origin.x + track.size.width * 0.8, track.center().y),
                Some(MouseButton::Left),
                Default::default(),
            );
            cx.simulate_mouse_up(
                point(track.origin.x + track.size.width * 0.8, track.center().y),
                MouseButton::Left,
                Default::default(),
            );
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let thumb_after = cx.debug_bounds("annotation-slider-thumb").unwrap();
            assert!(thumb_after.center().x > thumb_before.center().x, "{kind:?}");
            let after = cx.update(|_, cx| {
                let a = session.read(cx).annotations();
                assert_eq!(a.selected_index(), Some(0));
                a.committed()[0].clone()
            });
            assert_ne!(before, after, "{kind:?}");
            cx.simulate_keystrokes("escape");
            cx.update(|window, cx| {
                assert_eq!(session.read(cx).annotations().selected_index(), Some(0));
                assert!(
                    view.read(cx).focus_handle.is_focused(window),
                    "focus after {menu}"
                );
            });
            cx.simulate_keystrokes("ctrl-z");
            cx.update(|_, cx| {
                assert_eq!(
                    session.read(cx).annotations().committed()[0],
                    before,
                    "{kind:?}"
                )
            });
            cx.simulate_keystrokes("ctrl-y");
            cx.update(|_, cx| {
                assert_eq!(
                    session.read(cx).annotations().committed()[0],
                    after,
                    "{kind:?}"
                )
            });
        }
    });
}

#[gpui_kit::test]
fn toolbar_rows_fit_and_narrow_outputs_keep_trailing_controls_reachable(cx: &mut TestAppContext) {
    crate::annotation::text::with_test_font(|| {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
            crate::actions::init_annotation_keybindings(cx);
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
        cx.update(|_, cx| {
            session.update(cx, |s, cx| {
                s.set_size("screen", size(px(1200.), px(700.)));
                s.begin("screen", point(px(1000.), px(80.)));
                s.end("screen", point(px(1180.), px(450.)));
                s.edit_annotations(|a| {
                    let mut style = a.text_style().clone();
                    style.outline = Some(0x3388ffff);
                    style.background = Some(0xffeeccff);
                    a.set_text_style(style);
                });
                cx.notify();
            });
        });
        for kind in [
            ShapeKind::Line,
            ShapeKind::Polyline,
            ShapeKind::Arrow,
            ShapeKind::Number,
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Pencil,
            ShapeKind::Highlighter,
            ShapeKind::Mosaic,
            ShapeKind::Blur,
            ShapeKind::Eraser,
            ShapeKind::EraserRect,
            ShapeKind::Magnifier,
            ShapeKind::Spotlight,
            ShapeKind::Text,
        ] {
            cx.update(|_, cx| {
                session.update(cx, |s, cx| {
                    s.edit_annotations(|a| a.toggle(kind));
                    cx.notify();
                })
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let options = cx.debug_bounds("tb-options").unwrap();
            let viewport = cx.debug_bounds("annotation-options-scroll").unwrap();
            assert!(
                options.right() <= viewport.right(),
                "{kind:?}: {options:?} exceeds {viewport:?}"
            );
            assert!(
                options.size.width <= px(crate::model::placement::TB_W),
                "{kind:?}: options budget {options:?}"
            );
            let grip = cx.debug_bounds("tb-grip-right").unwrap();
            cx.update(|_, cx| {
                let bounds = session.read(cx).toolbar_bounds("screen").unwrap();
                assert!(bounds.right() <= px(1192.));
                assert!(grip.right() <= bounds.right());
                assert!(options.right() <= bounds.right());
            });
        }
        // Resize with the widest text options active, then use the actual
        // scrollbar track to expose and click the final palette entry.
        cx.simulate_resize(size(px(480.), px(700.)));
        cx.update(|_, cx| {
            session.update(cx, |s, cx| {
                s.select_all();
                s.edit_annotations(|a| {
                    a.add_text(
                        gpui_kit::Bounds::new(point(px(40.), px(60.)), size(px(160.), px(40.))),
                        "Selection survives scrolling".into(),
                    );
                    a.select_index(0);
                });
                cx.notify();
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let strip = cx.debug_bounds("annotation-options-scroll").unwrap();
        assert!(strip.right() <= px(472.));
        let scroll_end = point(strip.right() - px(2.), strip.bottom() - px(2.));
        cx.simulate_click(scroll_end, Default::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let color = cx.debug_bounds("tb-color-6").unwrap();
        assert!(
            color.right() <= strip.right(),
            "last color not exposed by scrollbar: {color:?} vs {strip:?}"
        );
        cx.simulate_click(color.center(), Default::default());
        cx.update(|_, cx| {
            let a = session.read(cx).annotations();
            assert_eq!(a.selected_index(), Some(0));
            assert_eq!(a.color_index(), 6);
        });
        // The main row obeys the same viewport and can expose its right grip.
        let strip = cx.debug_bounds("toolbar-actions-scroll").unwrap();
        cx.simulate_click(
            point(strip.right() - px(2.), strip.bottom() - px(2.)),
            Default::default(),
        );
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let grip = cx.debug_bounds("tb-grip-right").unwrap();
        assert!(grip.right() <= strip.right());
        assert!(grip.left() >= strip.left());
    });
}
