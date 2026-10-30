//! Overlay rendering: the frozen frame, dim backdrop, selection
//! chrome, annotation canvas, toolbar — plus the window-level pointer
//! listener plumbing (the event sink). Action handlers register on the
//! element tree here, re-attached per frame via `cx.listener`.

use super::Overlay;
use crate::actions::{
    ClearAnnotations, CopySelection, DeleteAnnotation, EscapeOverlay, FinishPolyline, Nudge,
    OcrSelection, OpenSettings, PinSelection, QuitOverlay, RedoAnnotation, SaveSelection,
    ScrollSelection, SelectScreen, ToggleArrow, ToggleEllipse, ToggleEraser, ToggleHighlighter,
    ToggleLine, ToggleMagnifier, ToggleMosaic, ToggleNumber, TogglePencil, TogglePolyline,
    ToggleRectangle, ToggleSelect, ToggleSpotlight, ToggleText, UndoAnnotation,
};
use crate::model::placement::round_px;
use crate::ui::hud::{
    annotation_chrome, annotation_hover_chrome, eraser_chrome, hover_outline, lens_draft_chrome,
    magnifier_loupe, selection_backdrop, selection_handles, selection_label,
};
use crate::ui::toolbar::selection_toolbar;
use gpui_kit::*;

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Closest observable proxy for "layer mapped & selectable": the
        // first scene paint commits right after this returns (~1 frame).
        // Per instance so the perf warm-window suite sees every overlay.
        if self.perf_first_render_pending {
            self.perf_first_render_pending = false;
            crate::boot_mark("overlay first render");
        }
        // THE cursor derivation point: every repaint stores the style the
        // handles canvas pushes during paint. Deriving here — instead of
        // at every event and action site — means any state flip lands
        // with a correct cursor on EVERY window, pointer motion or not
        // (e.g. chrome re-hosted by another window's release event).
        self.cursor.set(self.cursor_style(cx));
        // The size slider FIRST: building/rebuilding it needs &mut cx
        // (cx.new / cx.subscribe), which the `shared` read borrow below
        // would block for the rest of render.
        let size_slider = {
            let a = self.session.read(cx).annotations();
            // the edit target is the SELECTED shape when one is live
            // (select-on-place), else the active tool's preset — the
            // slider position, readout and writes all follow it
            let edit = a.edit_kind().map(|kind| {
                (
                    (kind, a.editing_filter_brush_size()),
                    a.current_size_spec(),
                    a.current_edit_size(),
                )
            });
            edit.map(|(kind, spec, current)| {
                // Clone the reusable state out first: the create branch
                // needs cx.new/cx.subscribe (mutable), so the lookup
                // must release its borrow before branching.
                let reusable = match self.slider.as_ref() {
                    Some((k, state)) if *k == kind => Some(state.clone()),
                    _ => None,
                };
                let state = if let Some(state) = reusable {
                    // sync external changes (wheel) into the thumb;
                    // skip when equal or every frame would re-notify
                    // itself into a loop
                    if (state.read(cx).value().start() - current).abs() > f32::EPSILON {
                        state.update(cx, |s, cx| s.set_value(current, window, cx));
                    }
                    state
                } else {
                    // a new tool family means a new range — the state is
                    // baked at build time, so rebuild the entity
                    let state = cx.new(|_| {
                        gpui_kit::base::slider::SliderState::new()
                            .min(spec.min)
                            .max(spec.max)
                            .step(1.)
                            .default_value(current)
                    });
                    let sub = cx.subscribe(
                        &state,
                        |this, _, event: &gpui_kit::base::slider::SliderEvent, cx| {
                            match event {
                                // value writes go to the edit target
                                // (selected shape first) and repaint the
                                // overlay so the thumb follows the pointer
                                gpui_kit::base::slider::SliderEvent::Change(v) => {
                                    this.session.update(cx, |s, cx| {
                                        s.edit_annotation_settings(|a| a.apply_size(v.start()));
                                        cx.notify();
                                    });
                                    cx.notify();
                                }
                                // one history entry per drag
                                gpui_kit::base::slider::SliderEvent::Release(_) => {
                                    this.session.update(cx, |s, _| {
                                        s.edit_annotation_settings(|a| a.end_size_drag())
                                    });
                                }
                            }
                        },
                    );
                    self.slider_sub = Some(sub);
                    self.slider = Some((kind, state.clone()));
                    state
                };
                // single-value sliders carry the thumb position in
                // percentage().END (start stays 0)
                let percentage = state.read(cx).percentage().end;
                crate::ui::toolbar::SizeSlider {
                    state,
                    percentage,
                    current,
                }
            })
        };
        // Keep display geometry stable while dragging. The backdrop paints
        // shared edges directly so fractional DPI cannot open layout seams.
        let filtered = self.session.update(cx, |session, cx| {
            session.request_filtered_preview(&self.capture.output_name, cx)
        });
        let shared = self.session.read(cx);
        let selection = shared.selection();
        let sel = shared.local_bounds(&self.capture.output_name).map(round_px);
        let backdrop = shared
            .backdrop_bounds(&self.capture.output_name)
            .map(round_px);
        let hover = shared.hover_bounds(&self.capture.output_name).map(round_px);
        let selected_shape =
            if self.text_editing.is_none() && shared.annotations().editing_text().is_none() {
                shared.selected_shape_local(&self.capture.output_name)
            } else {
                None
            };
        let lens_draft = shared.local_lens_draft_outline(&self.capture.output_name);
        let annotation_hover = shared.annotation_hover_bounds(&self.capture.output_name);
        let raster_preview = shared.uses_raster_preview();
        let shapes = if raster_preview {
            Vec::new()
        } else {
            shared.local_annotations(&self.capture.output_name)
        };
        // The live freehand stroke rides ABOVE the raster preview as a
        // vector union path: baking it into the selection image meant
        // re-uploading the entire crop to the atlas on every pointer
        // move — the stroke-stutter bottleneck. On release the stroke is
        // promoted into the image once (see session::filtered_preview).
        let draft_mark = if raster_preview {
            shared.local_draft(&self.capture.output_name)
        } else {
            None
        };
        let canvas_images = self.canvas_images.clone();
        let filtered_image = filtered.as_ref().map(|(_, image)| image.clone());
        let number_cache = self.number_cache.clone();
        let highlighter_cache = self.highlighter_cache.clone();
        let drawing_polyline = shared.annotations().is_drawing_polyline();
        let active = shared.active_on(&self.capture.output_name);
        let loupe = shared.local_loupe(&self.capture.output_name);
        let eraser = shared.eraser_chrome(&self.capture.output_name);
        let input_view = cx.entity().downgrade();
        let ws = window.bounds().size; // window logical size (= output logical size)

        // Bind the base chain, then attach feature-gated handlers via
        // shadowing — cfg attributes are illegal in the middle of a method
        // chain ("#[cfg] cannot hang mid-method-chain")
        let base = div()
            .id("shotori-overlay")
            .key_context(if self.ocr_setup.is_some() {
                "ShotoriOcrSetup"
            } else if self.text_editing.is_some() {
                "ShotoriTextEditing"
            } else if self.session.read(cx).blocked() {
                "ShotoriBlocked"
            } else if !self.focus_handle.is_focused(window) {
                // Child inputs and popup controls own their keyboard commands.
                // Plain annotation shortcuts must not intercept typed characters.
                "ShotoriControls"
            } else if drawing_polyline {
                "ShotoriOverlay PolylineDrawing"
            } else {
                "ShotoriOverlay"
            })
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ToggleSelect, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Select, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleRectangle, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Rectangle, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleEllipse, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Ellipse, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleLine, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Line, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleArrow, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Arrow, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleText, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Text, window, cx);
            }))
            .on_action(cx.listener(|this, _: &EscapeOverlay, window, cx| this.escape(window, cx)))
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::Enter, _, cx| {
                    if this.text_editing.is_some() {
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::Escape, window, cx| {
                    if let Some(editor) = &this.text_editing {
                        if editor.focus_handle(cx).is_focused(window) {
                            this.escape(window, cx);
                        } else {
                            editor.focus(window, cx);
                        }
                    } else {
                        window.focus(&this.focus_handle, cx);
                    }
                    cx.stop_propagation();
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleNumber, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Number, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePencil, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Pencil, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleHighlighter, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Highlighter, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleEraser, window, cx| {
                this.finish_text(true, window, cx);
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| {
                        let kind = if a.tool() == Some(crate::annotation::ShapeKind::EraserRect) {
                            crate::annotation::ShapeKind::EraserRect
                        } else {
                            crate::annotation::ShapeKind::Eraser
                        };
                        a.toggle(kind);
                    });
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleSpotlight, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Spotlight, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleMagnifier, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Magnifier, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleMosaic, window, cx| {
                // The toolbar's mosaic slot re-toggles BLUR when that is
                // the active filter variant (they share one button).
                let blur = this.session.read(cx).annotations().tool()
                    == Some(crate::annotation::ShapeKind::Blur);
                let kind = if blur {
                    crate::annotation::ShapeKind::Blur
                } else {
                    crate::annotation::ShapeKind::Mosaic
                };
                this.toggle_tool(kind, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePolyline, window, cx| {
                this.toggle_tool(crate::annotation::ShapeKind::Polyline, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FinishPolyline, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| a.finish_polyline());
                    cx.notify();
                });
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                if matches!(
                    event.keystroke.key.as_str(),
                    "left" | "right" | "up" | "down"
                ) {
                    this.session.update(cx, |s, _| s.finish_nudge());
                }
            }))
            .on_action(cx.listener(|this, action: &Nudge, _, cx| {
                this.session.update(cx, |s, cx| {
                    if s.nudge(point(px(action.dx as f32), px(action.dy as f32))) {
                        cx.notify();
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &UndoAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.undo();
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &DeleteAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| {
                        a.delete_selected();
                    });
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &ClearAnnotations, window, cx| {
                // An in-flight text/number edit would keep its editor
                // open over an emptied canvas; cancel it — committing
                // first is pointless when the result is about to be
                // cleared as part of the same user step.
                this.finish_text(false, window, cx);
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.edit_annotations(|a| {
                        a.clear_all();
                    });
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &RedoAnnotation, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.session.update(cx, |s, cx| {
                    s.redo();
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &CopySelection, window, cx| {
                this.finish_text(true, window, cx);
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                this.copy_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectScreen, _, cx| {
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                let name = this.capture.output_name.clone();
                this.session.update(cx, |s, cx| {
                    if s.cycle_select_all(&name) {
                        cx.notify();
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &SaveSelection, window, cx| {
                this.finish_text(true, window, cx);
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                this.save_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PinSelection, window, cx| {
                this.finish_text(true, window, cx);
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                this.pin_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ScrollSelection, window, cx| {
                this.finish_text(true, window, cx);
                if this.session.read(cx).blocked() {
                    return; // setup dialog is modal
                }
                this.scroll_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OcrSelection, window, cx| {
                this.finish_text(true, window, cx);
                this.ocr_selection(window, cx);
            }))
            .on_action(cx.listener(
                |this, _: &crate::ui::ocr_setup::OcrSetupConfirm, window, cx| {
                    this.ocr_setup_confirm(window, cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &crate::ui::ocr_setup::OcrSetupCancel, window, cx| {
                    this.ocr_setup_cancel(window, cx);
                    cx.stop_propagation();
                },
            ));

        // ⑥ First-run OCR setup dialog (confirm / progress), topmost
        let setup_el: Option<AnyElement> = self
            .ocr_setup
            .as_ref()
            .map(|s| crate::ui::ocr_setup::setup_card(s, &self.setup_focus).into_any_element());

        base
            // Close always exits; Escape owns interaction cancellation.
            .on_action(cx.listener(|_, _: &QuitOverlay, _, cx| {
                cx.quit();
                cx.stop_propagation();
            }))
            // The gear / Ctrl+, — the one "exit" that leads somewhere:
            // a regular toplevel cannot coexist with exclusive-keyboard
            // overlays (the same constraint as the save dialog), so the
            // session ends here and a detached settings process takes
            // over. It boots slowly enough (wgpu init) that the overlay
            // is gone by the time its window maps.
            .on_action(cx.listener(|_, _: &OpenSettings, _, cx| {
                crate::launch_detached("settings");
                cx.quit();
                cx.stop_propagation();
            }))
            // ── Selection interaction (events → state machine) ─────────
            // (the LEFT down is handled window-level in `pointer_event_sink`:
            // a stale pointer focus after a cross-screen release delivers
            // it with out-of-bounds local coordinates, which element
            // hit-testing would drop)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    if this.session.read(cx).blocked() && this.text_editing.is_none() {
                        return;
                    }
                    this.finish_text(true, window, cx);
                    window.focus(&this.focus_handle, cx);
                    this.session.update(cx, |s, cx| {
                        s.finish_or_exit_annotation();
                        cx.notify();
                    });
                    cx.stop_propagation();
                }),
            )
            // Wheel has three phases. (0) A FINALIZED selection with no
            // annotation edit target: zoom the crop in place, equal ratio
            // about its centre — "my box came out a little small, don't
            // make me redraw it". (1) Pre-selection with no annotation
            // tool lit: step the smart-region hierarchy under the
            // pointer (child ↔ parent ↔ whole window/screen). (2) With
            // a tool lit: step its S/M/L size preset (issue #3). Each
            // phase only consumes the event when IT is the state's
            // meaning; otherwise the event bubbles.
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                if this.text_editing.is_some() {
                    cx.propagate();
                    return;
                }
                let lines = match event.delta {
                    ScrollDelta::Lines(l) => l.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 40.,
                };
                // One discrete notch must be one step, but a notch
                // arrives (on niri) as Pixels(40 × 3): gpui's wayland
                // backend hard-codes a ×3 amplification of the axis
                // value, and the per-notch value has no cross-
                // compositor standard (continuous pixels also outrank
                // the discrete event, so Lines never reaches us here).
                // Clamping each EVENT to ±1 line lands one notch = one
                // step on any compositor whose notch value reaches the
                // /40 divisor, while touchpads (many small events)
                // still accumulate to full steps below.
                let lines = lines.clamp(-1., 1.);
                this.size_scroll_acc += lines;
                // Phase 0 — the crop owns the wheel once it exists.
                // `edit_kind()` also covers a selected shape (the wheel
                // then tunes THAT, phase 2's job), and a lit tool means
                // the user is annotating, not reframing.
                let zoomable = {
                    let session = this.session.read(cx);
                    session.annotations().edit_kind().is_none() && session.selection().is_selected()
                };
                if zoomable {
                    let mut consumed = false;
                    while this.size_scroll_acc >= 1.0 {
                        this.session.update(cx, |s, _| s.zoom_selection(1));
                        consumed = true;
                        this.size_scroll_acc -= 1.0;
                    }
                    while this.size_scroll_acc <= -1.0 {
                        this.session.update(cx, |s, _| s.zoom_selection(-1));
                        consumed = true;
                        this.size_scroll_acc += 1.0;
                    }
                    if consumed {
                        // Even a notch that hit the zoom limit belongs to
                        // the crop, not to whatever sits under the overlay.
                        cx.notify();
                    }
                    cx.stop_propagation();
                    return;
                }
                if this.session.read(cx).annotations().edit_kind().is_none() {
                    // phase 1 — hierarchy stepping (drains whole units)
                    let mut consumed = false;
                    while this.size_scroll_acc >= 1.0 {
                        consumed |= this.session.update(cx, |s, _| s.smart_step(1));
                        this.size_scroll_acc -= 1.0;
                    }
                    while this.size_scroll_acc <= -1.0 {
                        consumed |= this.session.update(cx, |s, _| s.smart_step(-1));
                        this.size_scroll_acc += 1.0;
                    }
                    if consumed {
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    cx.propagate();
                    return;
                }
                if !this.session.read(cx).annotations().edit_kind().is_some() {
                    cx.propagate();
                    return;
                }
                // phase 2 — preset stepping
                let mut changed = false;
                while this.size_scroll_acc >= 1.0 {
                    this.session.update(cx, |s, _| {
                        let position = s.to_global(&this.capture.output_name, event.position);
                        s.edit_annotations(|a| {
                            changed |=
                                if a.edit_kind() == Some(crate::annotation::ShapeKind::Spotlight) {
                                    a.step_spotlight(spotlight_adjustment(event), true)
                                } else {
                                    a.step_size_at(position, true)
                                };
                        });
                    });
                    this.size_scroll_acc -= 1.0;
                }
                while this.size_scroll_acc <= -1.0 {
                    this.session.update(cx, |s, _| {
                        let position = s.to_global(&this.capture.output_name, event.position);
                        s.edit_annotations(|a| {
                            changed |=
                                if a.edit_kind() == Some(crate::annotation::ShapeKind::Spotlight) {
                                    a.step_spotlight(spotlight_adjustment(event), false)
                                } else {
                                    a.step_size_at(position, false)
                                };
                        });
                    });
                    this.size_scroll_acc += 1.0;
                }
                if changed {
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            // ── Layer stack (bottom to top) ─────────────────────────────
            // ① The frozen screen image (opaque, filling the window)
            .child(img(self.frozen.clone()).size_full())
            .child(
                canvas(
                    move |bounds, window, _| {
                        let highlights = highlighter_cache.borrow_mut().prepare(
                            &shapes,
                            window.scale_factor(),
                            Bounds::new(point(px(0.), px(0.)), bounds.size),
                        );
                        let images = number_cache
                            .borrow_mut()
                            .prepare(&shapes, window.scale_factor());
                        let next = filtered_image
                            .into_iter()
                            .chain(images.iter().flatten().map(|image| image.image.clone()))
                            .chain(highlights.iter().flatten().map(|image| image.image.clone()))
                            .collect();
                        for image in canvas_images.borrow_mut().replace(next) {
                            // Each output has its own atlas. Do not evict another
                            // window's currently displayed shared session image.
                            if let Err(error) = window.drop_image(image) {
                                eprintln!("[shotori] preview cleanup failed: {error}");
                            }
                        }
                        shapes
                            .into_iter()
                            .zip(images)
                            .zip(highlights)
                            .collect::<Vec<_>>()
                    },
                    move |viewport, shapes, window, _| {
                        if let Some(mut clip) = sel {
                            clip.origin += viewport.origin;
                            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                            if let Some((mut bounds, image)) = filtered {
                                bounds.origin += viewport.origin;
                                if let Err(error) = window.paint_image(
                                    bounds,
                                    bounds,
                                    Corners::default(),
                                    image,
                                    0,
                                    false,
                                ) {
                                    eprintln!("[shotori] filter preview failed: {error}");
                                }
                            }
                            for ((shape, number_image), highlight) in shapes {
                                if shape.kind == crate::annotation::ShapeKind::Highlighter {
                                    if let Some(mut highlight) = highlight {
                                        highlight.bounds.origin += viewport.origin;
                                        if let Err(error) = window.paint_image(
                                            highlight.bounds,
                                            highlight.bounds,
                                            Corners::default(),
                                            highlight.image,
                                            0,
                                            false,
                                        ) {
                                            eprintln!(
                                                "[shotori] highlighter preview failed: {error}"
                                            );
                                        }
                                    }
                                    continue;
                                }
                                if let Some(mut number_image) = number_image {
                                    if let Some(arrow) = shape.number_arrow() {
                                        for path in arrow.line_paths(viewport.origin) {
                                            window.paint_path(path, rgba(shape.color));
                                        }
                                    }
                                    number_image.bounds.origin += viewport.origin;
                                    if let Err(error) = window.paint_image(
                                        number_image.bounds,
                                        number_image.bounds,
                                        Corners::default(),
                                        number_image.image,
                                        0,
                                        false,
                                    ) {
                                        eprintln!("[shotori] number preview failed: {error}");
                                    }
                                    continue;
                                }
                                if shape.has_custom_geometry() || matches!(
                                    shape.kind,
                                    crate::annotation::ShapeKind::Line
                                        | crate::annotation::ShapeKind::Arrow
                                        | crate::annotation::ShapeKind::Polyline
                                        | crate::annotation::ShapeKind::Pencil
                                ) {
                                    for path in shape.line_paths(viewport.origin) {
                                        window.paint_path(path, rgba(shape.color));
                                    }
                                    continue;
                                }
                                if shape.kind == crate::annotation::ShapeKind::Ellipse {
                                    if let Some(path) = shape.ellipse_path(viewport.origin) {
                                        window.paint_path(path, rgba(shape.color));
                                    }
                                    continue;
                                }
                                for mut stroke in shape.strokes() {
                                    stroke.origin += viewport.origin;
                                    window.paint_quad(fill(stroke, rgba(shape.color)));
                                }
                            }
                            if let Some(mark) = &draft_mark {
                                for path in mark.draft_paths(viewport.origin) {
                                    window.paint_path(path, rgba(mark.color));
                                }
                            }
                        });
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            // ①½ Annotation selection chrome (stroke + handles),
            // above the marks, below the selection border.
            .children(annotation_hover.map(annotation_hover_chrome))
            .child(annotation_chrome(selected_shape))
            .children(lens_draft.map(lens_draft_chrome))
            // ①¾ Eraser chrome (issue #14): the brush ring / the area
            // rect — pointer affordances, above the marks they erase.
            .children(eraser.map(eraser_chrome))
            // Paint the border above the export-backed preview as well as vector marks.
            .child(selection_backdrop(backdrop))
            // ②¼ Resize handles (above the border; the toolbar paints later
            // and stays on top). Drawn whenever the selection is finalized
            // or being edited — not during a fresh drag.
            .child(selection_handles(
                backdrop,
                selection.is_selected() || selection.is_editing(),
                self.cursor.clone(),
                self.text_editing.is_none() && self.ocr_setup.is_none(),
            ))
            // ②½ Window-snap hover outline (above the dim, below all
            // selection chrome: it is a hint, not a selection)
            .children(hover.map(hover_outline))
            // ②¾ Window-level pointer listeners (see `pointer_event_sink`)
            .child(pointer_event_sink(input_view))
            // ③ Size label stays independent so narrow selections cannot wrap it.
            // Size comes from the same rounded local bounds the border
            // paints (label and border cannot disagree); the old raw
            // `selection.bounds().unwrap()` here was a same-snapshot
            // unwrap with a second geometry source.
            .children(
                sel.filter(|_| active)
                    .map(|b| selection_label(b, ws, b.size)),
            )
            .children(self.text_editing.as_ref().map(|editor| editor.render()))
            // ④ Toolbar: appears only after release (no flicker while dragging).
            // Its rect comes from the session — anchored, or wherever the
            // user dragged it (see session::toolbar_bounds).
            .children(
                if selection.is_selected()
                    && active
                    && self.ocr_setup.is_none()
                    && (!self.session.read(cx).blocked() || self.text_editing.is_some())
                    && let Some(rect) = shared.toolbar_bounds(&self.capture.output_name)
                {
                    let swatch_picker = self.swatch_editor_seed(window, cx);
                    Some(selection_toolbar(
                        rect,
                        self.capture.output_name.clone().into(),
                        self.session.read(cx).annotations(),
                        self.session.clone(),
                        self.text_editing
                            .as_ref()
                            .map(|e| e.focus_handle(cx))
                            .unwrap_or_else(|| self.focus_handle.clone()),
                        size_slider,
                        swatch_picker,
                    ))
                } else {
                    None
                },
            )
            // ⑤½ Magnifier loupe (issue #19): the drag's precision
            // chrome, topmost below the setup dialog (which blocks
            // gestures anyway, so ⑥ stays above it). Renders only on
            // the output that owns the focus point.
            .children(loupe.map(|l| magnifier_loupe(self.frozen.clone(), l, ws).into_any_element()))
            .children(
                self.text_editing
                    .as_ref()
                    .filter(|editor| editor.input().read(cx).input_was_truncated())
                    .map(|_| {
                        div()
                            .absolute()
                            .top_4()
                            .left_4()
                            .right_4()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .id("text-overflow-notice")
                                    .debug_selector(|| "text-overflow-notice".to_owned())
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgba(crate::ui::theme::c().chip_bg))
                                    .text_color(rgba(crate::ui::theme::c().hint_text))
                                    .text_sm()
                                    .child(crate::i18n::t!("text_input.truncated").to_string()),
                            )
                    }),
            )
            // First-run OCR setup remains above editing feedback.
            .children(setup_el)
    }
}

/// The invisible canvas that owns the window-level pointer listeners.
///
/// `window.on_mouse_event` registrations live for one frame's event
/// dispatch, so they must be re-attached during every paint — a canvas
/// whose paint closure registers them is the sanctioned hook. Element-
/// level mouse handlers are not an alternative: under Wayland's
/// implicit grab a drag's events keep arriving at the PRESS window
/// even outside its bounds, and element hit-testing would drop exactly
/// those events.
fn pointer_event_sink(input_view: WeakEntity<Overlay>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let view = input_view.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let _ = view.update(cx, |this, cx| {
                    // Why window-level and not an element handler: after a
                    // cross-screen release the compositor may keep pointer
                    // focus on the PRESS window while the pointer sits over
                    // another output (niri re-focuses only on the next
                    // motion). The down then arrives with OUT-OF-BOUNDS
                    // local coordinates — element hit-testing drops it, and
                    // the press silently vanished. The session converts via
                    // the RECEIVING window's origin, so the position lands
                    // correctly no matter which window delivered it.
                    // Toolbar/buttons still own their presses: their element
                    // handlers stop propagation before this root listener.
                    if this.text_editing.is_some() {
                        this.finish_text(true, window, cx);
                        return;
                    }
                    if this.session.read(cx).blocked() {
                        return; // modal dialog: no new selections
                    }
                    let p = this
                        .session
                        .read(cx)
                        .to_global(&this.capture.output_name, event.position);
                    // Double-click on an existing Text shape → inline
                    // edit. Gated to the select and text tools since
                    // the select-tool flip: a draw tool's first click
                    // would ink over the shape, stranding a stray mark
                    // next to the editor. (The text tool's own press
                    // on an existing shape is a no-op — `begin`
                    // refuses Text — which is why it stays allowed.)
                    let tool = this.session.read(cx).annotations().tool();
                    if event.click_count >= 2
                        && matches!(
                            tool,
                            None | Some(
                                crate::annotation::ShapeKind::Select
                                    | crate::annotation::ShapeKind::Text
                            )
                        )
                    {
                        let hit_text =
                            this.session
                                .read(cx)
                                .annotations()
                                .hit_test(p)
                                .and_then(|ix| {
                                    (this.session.read(cx).annotations().shape_kind(ix)
                                        == Some(crate::annotation::ShapeKind::Text))
                                    .then_some(ix)
                                });
                        if let Some(ix) = hit_text {
                            this.start_edit_text(ix, event.position, window, cx);
                            return;
                        }
                    }
                    if this.session.read(cx).annotations().tool()
                        == Some(crate::annotation::ShapeKind::Text)
                        && this
                            .session
                            .read(cx)
                            .annotations()
                            .interaction_hit_test(p)
                            .is_none()
                    {
                        // The first click of a double-click must select the
                        // existing mark, not open an empty editor over it.
                        this.start_text(event.position, window, cx);
                        return;
                    }
                    // Double-click on a badge opens the value editor
                    // (issue #2); select-tool only — the gate lives in
                    // `number_at_double_click`, same reasoning as the
                    // text gate above.
                    if event.click_count == 2
                        && let Some((ix, badge_local)) = this
                            .session
                            .read(cx)
                            .number_at_double_click(&this.capture.output_name, event.position)
                    {
                        this.start_number_edit(ix, badge_local, window, cx);
                        return;
                    }
                    window.focus(&this.focus_handle, cx);
                    this.session.update(cx, |s, cx| {
                        s.pointer_down(
                            &this.capture.output_name,
                            event.position,
                            event.modifiers.alt,
                        );
                        cx.notify(); // repaint re-derives the cursor in render
                    });
                });
            });
            let view = input_view.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let _ = view.update(cx, |this, cx| {
                    // Apply the event to the session FIRST — state and the
                    // tracked pointer position both current — THEN derive
                    // the cursor. Deriving first would compute the
                    // affordance from the previous position: one event of
                    // lag on every move.
                    this.session.update(cx, |s, cx| {
                        if s.toolbar_drag_active() {
                            // the toolbar follows the pointer; hover
                            // tracking stays off under it
                            if s.toolbar_drag_move(&this.capture.output_name, event.position) {
                                cx.notify();
                            }
                            return;
                        }
                        let previous_annotation = s.hovered_annotation();
                        let changed = s.pointer_move(
                            &this.capture.output_name,
                            event.position,
                            event.modifiers.shift,
                        );
                        let hovered = s.hover_at(&this.capture.output_name, event.position);
                        // the eraser ring follows the pointer: moves
                        // repaint even when no state changed
                        if changed
                            || hovered
                            || previous_annotation != s.hovered_annotation()
                            || s.eraser_ring_follows_pointer()
                        {
                            cx.notify();
                        }
                    });
                    if this.refresh_cursor(cx) {
                        cx.notify();
                    }
                });
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                let _ = input_view.update(cx, |this, cx| {
                    this.session.update(cx, |s, cx| {
                        if s.toolbar_drag_active() {
                            s.toolbar_drag_end();
                            cx.notify();
                            return;
                        }
                        let finish = event.click_count >= 2 && s.annotations().is_pressed();
                        s.pointer_up(
                            &this.capture.output_name,
                            event.position,
                            event.modifiers.shift,
                        );
                        if finish {
                            s.edit_annotations(|a| a.finish_polyline());
                        }
                        cx.notify();
                    });
                });
            });
        },
    )
    .absolute()
    .size_full()
}

fn spotlight_adjustment(event: &ScrollEvent) -> crate::annotation::SpotlightAdjustment {
    use crate::annotation::SpotlightAdjustment;
    if event.modifiers.control {
        SpotlightAdjustment::Outline
    } else if event.modifiers.shift {
        SpotlightAdjustment::Region
    } else {
        SpotlightAdjustment::Opacity
    }
}
