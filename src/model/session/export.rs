//! Export: turning the session into pixels. The crop paths (plain,
//! marked-up, pin-geometry), the long-capture region query and the
//! filtered preview pipeline (rasterized annotations composited over
//! the frozen capture, cached and invalidated by session state).

use super::{
    DisplayPreview, DragOverlayCache, DragPreviewPlanes, FilterPreview, MovedReplay,
    RasterSelection, ScreenshotSession, ScrollRect,
};
use gpui_kit::*;
use std::sync::Arc;

/// Conservative logical bounding box enclosing every pixel a shape can
/// paint or filter, including antialiased stroke/badge fringes, rotated
/// corners, magnifier destination rects, blur kernels, and mosaic tile
/// edges. Used both to detect higher-Z shapes whose ink overlaps the drag
/// replay plane and to bound the dirty rectangle restored from
/// `MovedReplay::base` on the next frame.
fn padded_shape_bounds(shape: &crate::annotation::Shape) -> Bounds<Pixels> {
    let ink = shape.ink_bounds();
    let pad = px(shape.size().max(4.) + 4.);
    Bounds::new(
        point(ink.origin.x - pad, ink.origin.y - pad),
        size(ink.size.width + pad * 2., ink.size.height + pad * 2.),
    )
}

fn bounds_overlap(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> bool {
    a.left() <= b.right() && a.right() >= b.left() && a.top() <= b.bottom() && a.bottom() >= b.top()
}

impl ScreenshotSession {
    pub(crate) fn crop(&self, output: &str) -> Option<(u32, u32, Vec<u8>)> {
        self.crop_impl(output, true)
            .map(|r| (r.width, r.height, r.rgba))
    }

    /// The selection as a long-screenshot capture region, when it sits
    /// entirely inside ONE output (the scroll engine watches a single
    /// screen; a selection straddling outputs has no coherent scroll
    /// target). Output-local LOGICAL integer coordinates — exactly what
    /// `zwlr_screencopy_manager_v1.capture_output_region` expects (the
    /// protocol speaks logical px; the buffer it returns is physical).
    pub(crate) fn scroll_region(&self, output: &str) -> Option<ScrollRect> {
        let screen = self
            .screens
            .iter()
            .find(|s| s.capture.output_name == output)?;
        let selection = self.selection.bounds()?;
        let local = selection.origin - screen.bounds().origin;
        // Inside-on-this-output ⇔ the intersection equals the selection
        // itself (a selection reaching past this screen's edge would
        // scroll content from the neighbor — refuse instead of guessing).
        if selection.intersect(&screen.bounds()) != selection {
            return None;
        }
        let (w, h) = (
            f32::from(selection.size.width).round() as i32,
            f32::from(selection.size.height).round() as i32,
        );
        (w >= 16 && h >= 16).then_some(ScrollRect {
            x: f32::from(local.x).round() as i32,
            y: f32::from(local.y).round() as i32,
            width: w,
            height: h,
        })
    }
    /// Preserve the exact logical bounds of the rasterized crop for pin placement.
    pub(crate) fn crop_for_pin(&self, output: &str) -> Option<RasterSelection> {
        self.crop_impl(output, true)
    }

    /// Export without annotations — also the `full` subcommand's path.
    pub fn crop_original(&self, output: &str) -> Option<(u32, u32, Vec<u8>)> {
        self.crop_impl(output, false)
            .map(|r| (r.width, r.height, r.rgba))
    }

    pub(crate) fn is_preview_busy(&self) -> bool {
        self.preview_busy
    }

    pub(crate) fn clear_drag_preview_planes(&mut self) {
        self.drag_overlay.get_mut().take();
    }

    /// Zero-reupload GPU split-plane preview for a live body-move drag (#32),
    /// plus seamless holdover while a post-release heavy background composite
    /// drains (`self.preview_busy`).
    ///
    /// Why this exists alongside `MovedReplay`: even a sub-millisecond CPU
    /// drag-layer blit still paid ~5–8 ms/frame @ 4K in full-canvas `Vec<u8>`
    /// clones + RGBA→BGRA swizzling, and minting a fresh `RenderImage` forced
    /// GPUI's atlas to upload ~33 MB over PCIe every pointer move. Keeping
    /// `base`, `sprite`, and optional `above` as stable `Arc<RenderImage>`
    /// textures reduces per-move work to a `Bounds` origin add (~0 ms, 0 uploads).
    pub(crate) fn request_drag_preview_planes(
        &mut self,
        output: &str,
    ) -> Option<DragPreviewPlanes> {
        use crate::annotation::{Annotations, ShapeKind};
        if !self.uses_raster_preview()
            || self.annotations.editing_text().is_some()
            || self.annotations.draft_shape().is_some()
            || self.handle_drag.is_some()
        {
            self.drag_overlay.get_mut().take();
            return None;
        }
        let selection = self.selection.bounds()?;
        let committed = self.annotations.committed();
        let screen_origin = self.screen(output).bounds().origin;

        let project = |cache: &DragOverlayCache| -> Option<DragPreviewPlanes> {
            let anchor_shape = committed.get(cache.anchor_ix)?;
            let raw_delta = anchor_shape.bounds.origin - cache.baked_anchor_origin;
            let snapped_delta = point(
                px((f32::from(raw_delta.x) * cache.scale).round() / cache.scale),
                px((f32::from(raw_delta.y) * cache.scale).round() / cache.scale),
            );
            let mut base_bounds = cache.base_bounds;
            base_bounds.origin -= screen_origin;
            let mut drag_bounds = cache.sprite.0;
            drag_bounds.origin += snapped_delta - screen_origin;
            let above = cache.above.as_ref().map(|&(mut b, ref img)| {
                b.origin -= screen_origin;
                (b, img.clone())
            });
            Some(DragPreviewPlanes {
                base: (base_bounds, cache.base_image.clone()),
                dragged: (drag_bounds, cache.sprite.1.clone()),
                above,
            })
        };

        if !self.is_body_moving() {
            if self.preview_busy
                && let Some(cache) = self.drag_overlay.get_mut().as_ref()
                && cache.selection == Some(selection)
                && committed.len() == cache.static_shapes.len() + cache.indices.len()
                && cache
                    .static_shapes
                    .iter()
                    .all(|(ix, shape)| committed.get(*ix) == Some(shape))
                && cache.indices.iter().all(|&ix| {
                    committed
                        .get(ix)
                        .is_some_and(|s| s.is_source_over_layerable())
                })
            {
                return project(cache);
            }
            self.drag_overlay.get_mut().take();
            return None;
        }

        let mut indices: Vec<usize> = self
            .moving
            .as_ref()?
            .before
            .iter()
            .map(|(ix, _)| *ix)
            .collect();
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty()
            || !indices.windows(2).all(|w| w[1] == w[0] + 1)
            || committed
                .iter()
                .any(|s| matches!(s.kind, ShapeKind::Spotlight | ShapeKind::Magnifier))
        {
            self.drag_overlay.get_mut().take();
            return None;
        }
        let first_drag = indices[0];
        let last_drag = *indices.last()?;
        if last_drag >= committed.len()
            || !indices
                .iter()
                .all(|&ix| committed[ix].is_source_over_layerable())
            || !committed[last_drag + 1..]
                .iter()
                .all(|s| s.is_source_over_layerable())
        {
            self.drag_overlay.get_mut().take();
            return None;
        }
        // Restrict to single-output selections so desktop gaps (`alpha == 0`)
        // never need per-frame gap masking against a translated sprite, and skip
        // tiny sub-256×256 crops with trivial strokes where direct CPU replay is
        // already < 0.1 ms (avoiding 2-pass matte setup on micro selections).
        let participating: Vec<_> = self
            .screens
            .iter()
            .filter(|screen| {
                let inter = selection.intersect(&screen.bounds());
                inter.size.width > px(0.) && inter.size.height > px(0.)
            })
            .collect();
        if participating.len() != 1 {
            self.drag_overlay.get_mut().take();
            return None;
        }
        let screen = participating[0];
        let approx_scale = screen.capture.width as f32 / f32::from(screen.logical_size.width);
        let approx_pixels = f32::from(selection.size.width)
            * f32::from(selection.size.height)
            * approx_scale
            * approx_scale;
        let nontrivial_drag = approx_pixels >= 65_536.
            || indices.iter().any(|&ix| {
                let s = &committed[ix];
                s.points.len() >= 32 || matches!(s.kind, ShapeKind::Text | ShapeKind::Number)
            });
        if !nontrivial_drag {
            self.drag_overlay.get_mut().take();
            return None;
        }

        let cache_valid = self.drag_overlay.get_mut().as_ref().is_some_and(|cache| {
            cache.selection == Some(selection)
                && cache.indices == indices
                && committed.len() == cache.static_shapes.len() + cache.indices.len()
                && cache
                    .static_shapes
                    .iter()
                    .all(|(ix, shape)| committed.get(*ix) == Some(shape))
        });

        if !cache_valid {
            let original = self.crop_impl(output, false)?;
            let scale = original.scale;
            let base_bounds = original.bounds;
            let mut base_rgba = original.rgba;
            if first_drag > 0 {
                Annotations::rasterize_shapes(
                    committed[..first_drag].iter(),
                    &mut base_rgba,
                    original.width,
                    original.height,
                    base_bounds.origin,
                    scale,
                );
            }
            let base_image = crate::render_image::rgba_to_render_image(
                base_rgba,
                original.width,
                original.height,
            );
            let dragged_refs: Vec<_> = indices.iter().map(|&ix| &committed[ix]).collect();
            let baked_sprite = Annotations::bake_source_over_layer(&dragged_refs, scale)?;
            let sprite = (
                baked_sprite.bounds,
                crate::render_image::rgba_to_render_image(
                    baked_sprite.rgba,
                    baked_sprite.width,
                    baked_sprite.height,
                ),
            );
            let above = if last_drag + 1 < committed.len() {
                let above_refs: Vec<_> = committed[last_drag + 1..].iter().collect();
                let baked_above = Annotations::bake_source_over_layer(&above_refs, scale)?;
                Some((
                    baked_above.bounds,
                    crate::render_image::rgba_to_render_image(
                        baked_above.rgba,
                        baked_above.width,
                        baked_above.height,
                    ),
                ))
            } else {
                None
            };
            let static_shapes = committed
                .iter()
                .enumerate()
                .filter(|(ix, _)| !indices.contains(ix))
                .map(|(ix, s)| (ix, s.clone()))
                .collect();
            let anchor_ix = first_drag;
            *self.drag_overlay.get_mut() = Some(DragOverlayCache {
                selection: Some(selection),
                scale,
                indices,
                anchor_ix,
                baked_anchor_origin: committed[anchor_ix].bounds.origin,
                static_shapes,
                base_bounds,
                base_image,
                sprite,
                above,
            });
        }

        project(self.drag_overlay.get_mut().as_ref()?)
    }

    /// Publish edited text and lenses with their current selection geometry.
    /// Other heavy previews use one worker that snapshots the latest state on completion.
    pub(crate) fn request_filtered_preview(
        &mut self,
        output: &str,
        cx: &mut Context<Self>,
    ) -> Option<(Bounds<Pixels>, Arc<RenderImage>)> {
        if !self.uses_raster_preview() {
            self.filter_preview.get_mut().take();
            self.preview_display = None;
            return None;
        }
        // Edited geometry and its selection frame must be published together.
        // A cached lens only recomposites its changing suffix on this path.
        if self.annotations.editing_text().is_some()
            || self.annotations.selected_indices().iter().any(|&ix| {
                self.annotations.committed().get(ix).is_some_and(|shape| {
                    matches!(
                        shape.kind,
                        crate::annotation::ShapeKind::Text
                            | crate::annotation::ShapeKind::Magnifier
                    )
                })
            })
        {
            self.preview_display = None;
            return self.filtered_preview(output);
        }
        if !self.preview_busy {
            let unchanged = self.filter_preview.borrow().as_ref().is_some_and(|cache| {
                cache.selection == self.selection.bounds()
                    && cache.committed == self.annotations.committed()
                    && cache.draft.as_ref() == self.annotations.raster_draft_shape()
                    && cache.draft_generation == self.annotations.draft_generation()
            });
            let max_scale = self
                .screens
                .iter()
                .map(|s| s.capture.width as f32 / f32::from(s.logical_size.width))
                .fold(1_f32, f32::max);
            // Freehand drafts no longer bake into the image (they ride
            // as a vector overlay), so a growing stroke must never spawn
            // the worker — nor respawn it on every landing while the
            // gesture continues. Only committed/selection changes and
            // pixel-filter (blur/mosaic) marks re-render off-thread.
            let ink_only = !self.annotations.composite_draft()
                && self
                    .annotations
                    .draft_shape()
                    .is_some_and(|shape| shape.is_freehand())
                && self.filter_preview.borrow().as_ref().is_some_and(|cache| {
                    cache.selection == self.selection.bounds()
                        && cache.committed == self.annotations.committed()
                });
            // A just-finished freehand stroke promotes incrementally on
            // the UI thread (its raster state is warm, so this is O(new
            // segments)); handing the append to the worker would blank
            // the fresh ink until the worker lands.
            let promotable = self.filter_preview.borrow().as_ref().is_some_and(|cache| {
                cache.stroke.is_some()
                    && !self
                        .annotations
                        .committed()
                        .iter()
                        .any(|s| s.kind == crate::annotation::ShapeKind::Magnifier)
                    && self.annotations.draft_shape().is_none()
                    && self.annotations.committed().len() == cache.committed.len() + 1
                    && self.annotations.committed().starts_with(&cache.committed)
                    && self
                        .annotations
                        .committed()
                        .last()
                        .is_some_and(|shape| shape.is_freehand())
            });
            let heavy = self
                .annotations
                .visible()
                .any(|shape| shape.kind == crate::annotation::ShapeKind::Spotlight)
                || self.annotations.committed().iter().any(|shape| {
                    shape.points.len() > 1024
                        || (matches!(
                            shape.kind,
                            crate::annotation::ShapeKind::Blur
                                | crate::annotation::ShapeKind::Mosaic
                                | crate::annotation::ShapeKind::Magnifier
                        ) && f32::from(shape.ink_bounds().size.width)
                            * f32::from(shape.ink_bounds().size.height)
                            * max_scale
                            * max_scale
                            >= 262_144.)
                })
                || self.annotations.raster_draft_shape().is_some_and(|shape| {
                    matches!(
                        shape.kind,
                        crate::annotation::ShapeKind::Blur
                            | crate::annotation::ShapeKind::Mosaic
                            | crate::annotation::ShapeKind::Magnifier
                    ) && f32::from(shape.ink_bounds().size.width)
                        * f32::from(shape.ink_bounds().size.height)
                        * max_scale
                        * max_scale
                        >= 262_144.
                });
            // A live move/handle drag replays synchronously over the
            // split plane (see filtered_preview); spawning the worker
            // per drag frame would queue full re-renders whose results
            // land after the next move already invalidated them.
            let editing = self.editing_shape_indices().is_some();
            if unchanged || !heavy || ink_only || promotable || editing {
                self.preview_display = None;
                return self.filtered_preview(output);
            }
            let cache = self.filter_preview.get_mut().take();
            self.preview_display = cache.as_ref().map(|cache| DisplayPreview {
                selection: cache.selection,
                committed: cache.committed.clone(),
                draft_kind: cache.draft.as_ref().map(|shape| shape.kind),
                draft_generation: cache.draft_generation,
                bounds: cache.original.bounds,
                image: cache.image.clone(),
            });
            let captures = self
                .screens
                .iter()
                .map(|screen| screen.capture.clone())
                .collect();
            let sizes: Vec<_> = self
                .screens
                .iter()
                .map(|screen| screen.logical_size)
                .collect();
            let selection = self.selection;
            let geometry_revision = self.geometry_revision;
            let annotations = self.annotations.render_snapshot();
            let output = output.to_owned();
            let worker_output = output.clone();
            self.preview_busy = true;
            let task = cx.background_spawn(async move {
                let mut snapshot = Self::new(captures, Vec::new());
                for (screen, size) in snapshot.screens.iter_mut().zip(sizes) {
                    screen.logical_size = size;
                }
                snapshot.selection = selection;
                snapshot.annotations = annotations;
                snapshot.filter_preview = std::cell::RefCell::new(cache);
                snapshot.filtered_preview(&worker_output);
                snapshot.filter_preview.into_inner()
            });
            cx.spawn(async move |this, cx| {
                let cache = task.await;
                // update() fails when the preview's window closed while the
                // worker ran; the cache then has no consumer and dies with
                // the task — nothing to notify, nothing to recover.
                if this
                    .update(cx, |session, cx| {
                        session.preview_busy = false;
                        // Geometry changes cannot reuse the worker's frozen crop.
                        if session.geometry_revision == geometry_revision
                            && session.selection.bounds() == selection.bounds()
                        {
                            let current_is_fresh = session
                                .filter_preview
                                .borrow()
                                .as_ref()
                                .is_some_and(|current| {
                                    current.committed == session.annotations.committed()
                                        && current.draft.as_ref()
                                            == session.annotations.raster_draft_shape()
                                        && current.draft_generation
                                            == session.annotations.draft_generation()
                                });
                            if !current_is_fresh {
                                *session.filter_preview.get_mut() = cache;
                            }
                        }
                        session.request_filtered_preview(&output, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    // window gone mid-preview
                }
            })
            .detach();
        }
        let display = self.preview_display.as_ref()?;
        // A lagging draft may be displayed while it is being extended, but never
        // resurrect a cancelled stroke or undone history while the worker drains.
        let committed = self.annotations.committed();
        let same_history = display.committed == committed;
        // Editing selected objects must not hide every other annotation while
        // the worker replaces the composite. The old full frame remains valid
        // across single or multi-selection move/resize and release, never
        // across deletion or a new draft.
        let selected_edit = display.committed.len() == committed.len()
            && display
                .committed
                .iter()
                .zip(committed)
                .enumerate()
                .all(|(ix, (old, new))| {
                    old == new || (self.annotations.is_selected(ix) && old.kind == new.kind)
                });
        let same_gesture = display.draft_generation == self.annotations.draft_generation();
        let just_finished = same_gesture
            && self.annotations.draft_shape().is_none()
            && committed.len() == display.committed.len() + 1
            && committed.starts_with(&display.committed)
            && (committed.last().map(|shape| shape.kind) == display.draft_kind
                || (display.draft_kind.is_none()
                    && committed
                        .last()
                        .is_some_and(|s| s.kind == crate::annotation::ShapeKind::Magnifier)));
        let valid_draft = display.draft_kind.is_none()
            || (same_gesture
                && display.draft_kind == self.annotations.draft_shape().map(|shape| shape.kind));
        if display.selection != self.selection.bounds()
            || !(just_finished || ((same_history || selected_edit) && valid_draft))
        {
            return None;
        }
        Some((
            self.local_preview_bounds(output, display.bounds),
            display.image.clone(),
        ))
    }

    fn local_preview_bounds(&self, output: &str, mut bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        bounds.origin -= self.screen(output).bounds().origin;
        bounds
    }

    pub(crate) fn uses_raster_preview(&self) -> bool {
        use crate::annotation::{GeometryFill, ShapeKind};
        self.annotations.committed().len() >= 64
            || self.annotations.visible().any(|shape| {
                shape.geometry_fill_mode() == GeometryFill::Translucent
                    || matches!(
                        shape.kind,
                        ShapeKind::Pencil
                            | ShapeKind::Highlighter
                            | ShapeKind::Polyline
                            | ShapeKind::Spotlight
                            | ShapeKind::Magnifier
                            | ShapeKind::Mosaic
                            | ShapeKind::Blur
                            | ShapeKind::Text
                    )
            })
    }

    /// Reuse the exported composite for freehand strokes, pixel filters and text.
    /// Captures are immutable; selection, shapes and display geometry own invalidation.
    /// The committed indices being edited by a live drag (move or
    /// handle) or keyboard nudge, if any. `filtered_preview` uses this to
    /// repaint only the edited shapes per frame instead of the whole scene.
    fn editing_shape_indices(&self) -> Option<Vec<usize>> {
        let mut set: Vec<usize> = Vec::new();
        if let Some(drag) = &self.moving {
            set.extend(drag.before.iter().map(|(ix, _)| *ix));
        }
        if let Some(drag) = &self.handle_drag {
            set.push(drag.ix);
        }
        if let Some(super::editing::NudgeGesture::Shapes { before }) = &self.nudge {
            set.extend(before.iter().map(|(ix, _)| *ix));
        }
        if set.is_empty() {
            None
        } else {
            set.sort_unstable();
            set.dedup();
            Some(set)
        }
    }

    pub(crate) fn filtered_preview(
        &self,
        output: &str,
    ) -> Option<(Bounds<Pixels>, Arc<RenderImage>)> {
        use crate::annotation::ShapeKind;
        let mut cache = self.filter_preview.borrow_mut();
        if !self.uses_raster_preview() {
            *cache = None;
            return None;
        }
        let selection = self.selection.bounds();
        let committed = self.annotations.committed();
        let draft = self.annotations.raster_draft_shape();
        if cache.as_ref().is_none_or(|c| c.selection != selection) {
            let original = self.crop_impl(output, false)?;
            *cache = Some(FilterPreview::new(
                selection,
                original,
                self.annotations.draft_generation(),
            ));
        }
        let cached = cache.as_mut()?;
        let bounds = self.local_preview_bounds(output, cached.original.bounds);
        // Live move/handle drag: replaying the whole scene per pointer
        // move (full-frame base copy + every committed shape
        // re-rasterized) cost ~20 ms/frame at 1080p and ~65 ms with a
        // magnifier on screen — the dragged shape visibly trailed the
        // pointer. Instead, composite the scene WITHOUT the replay set
        // once (split plane), then repaint only the replay set over that
        // stable base each frame.
        if let Some(editing) = self.editing_shape_indices() {
            let single_lens = editing.as_slice() == [editing[0]]
                && committed
                    .get(editing[0])
                    .is_some_and(|s| s.kind == ShapeKind::Magnifier);
            if single_lens {
                // Single-lens drags route through MagnifierPreview instead of
                // MovedReplay so annotations=false never samples un-dragged marks
                // and later marks keep their display order over the lens.
                cached.moved = None;
            } else if let Some(image) =
                cached.update_moved_replay(&editing, committed, self.annotations.draft_generation())
            {
                return Some((bounds, image));
            }
        } else if cached.moved.take().is_some() && draft.is_none() && cached.committed == committed
        {
            // A drag or keyboard nudge just ended: the split composite already
            // equals the full replay (base + replay shapes at their final
            // positions), so keep the frame and just drop the split state.
            cached.draft = None;
            cached.draft_generation = self.annotations.draft_generation();
            return Some((bounds, cached.image.clone()));
        }
        if cached.committed != committed
            || cached.draft.as_ref() != draft
            || cached.draft_generation != self.annotations.draft_generation()
        {
            if cached.draft_generation != self.annotations.draft_generation() {
                cached.stroke = None;
            }
            let history_changed = cached.committed != committed;
            let has_lens = committed.iter().any(|s| s.kind == ShapeKind::Magnifier);
            let image_was_committed = cached.draft.as_ref().is_none_or(|shape| {
                shape.is_freehand() && !shape.needs_composited_ink() && !has_lens
            });
            if history_changed {
                if has_lens {
                    cached.update_lens_history(committed, self.annotations.selected_index())?;
                } else {
                    cached.update_standard_history(committed);
                }
                cached.stroke = None;
                cached.committed.clear();
                cached.committed.extend_from_slice(committed);
            }
            cached.update_draft_image(
                committed,
                draft,
                self.annotations.composite_draft(),
                history_changed,
                image_was_committed,
            );
            cached.draft = draft.cloned();
            cached.draft_generation = self.annotations.draft_generation();
        }
        Some((bounds, cached.image.clone()))
    }

    /// Keep the original single-output crop when possible. Spanning selections
    /// use the highest participating pixel density; desktop gaps stay transparent.
    fn crop_impl(&self, fallback_output: &str, marked: bool) -> Option<RasterSelection> {
        self.crop_composite(fallback_output, marked, true)
    }

    /// Test seam: the export-grade composite of the COMMITTED marks only —
    /// exactly what the filtered preview shows while a freehand draft is
    /// in flight, since the live stroke rides as a vector overlay
    /// (see `Overlay::render`) and never bakes into the preview image.
    #[cfg(test)]
    pub(crate) fn crop_without_freehand_draft(&self, output: &str) -> Option<(u32, u32, Vec<u8>)> {
        Self::crop_composite(self, output, true, false).map(|r| (r.width, r.height, r.rgba))
    }

    fn crop_composite(
        &self,
        fallback_output: &str,
        marked: bool,
        include_draft: bool,
    ) -> Option<RasterSelection> {
        // Export shortcut: the preview cache holds exactly this composite
        // (full-selection crop + committed annotations) whenever the
        // selection and history are unchanged since the last render.
        // Reusing it makes copy/save/pin/OCR O(memcpy) instead of a full
        // re-rasterization — decisive for long pencil strokes. The cache
        // only exists while `uses_raster_preview()` is true (it is taken
        // when that flips off), so no extra predicate is needed.
        if marked
            && let Some(cached) = self.filter_preview.borrow().as_ref()
            && let Some(selection) = self.selection.bounds()
            && cached.selection == Some(selection)
            && cached.draft.is_none()
            && self.annotations.draft_shape().is_none()
            && cached.committed == self.annotations.committed()
            && cached.draft_generation == self.annotations.draft_generation()
        {
            return Some(RasterSelection {
                scale: cached.original.scale,
                width: cached.original.width,
                height: cached.original.height,
                rgba: cached.pixels.clone(),
                bounds: cached.original.bounds,
            });
        }
        let selected = self
            .selection
            .bounds()
            .unwrap_or_else(|| self.screen(fallback_output).bounds());
        let participating: Vec<_> = self
            .screens
            .iter()
            .filter_map(|screen| {
                let intersection = selected.intersect(&screen.bounds());
                (intersection.size.width > px(0.) && intersection.size.height > px(0.))
                    .then_some((screen, intersection))
            })
            .collect();
        if participating.len() == 1 {
            let (screen, mut bounds) = participating[0];
            bounds.origin -= screen.bounds().origin;
            let cap = &screen.capture;
            let scale = cap.width as f32 / f32::from(screen.logical_size.width);
            let (w, h, mut rgba) =
                crate::model::export::crop(&cap.rgba, cap.width, cap.height, bounds, scale)?;
            // crop() rounds the source offset to native pixels. Use the
            // same rounded origin when placing logical annotation edges.
            let origin = screen.bounds().origin
                + point(
                    px((f32::from(bounds.left()) * scale).round() / scale),
                    px((f32::from(bounds.top()) * scale).round() / scale),
                );
            if marked {
                if include_draft {
                    self.annotations.rasterize(&mut rgba, w, h, origin, scale);
                } else {
                    crate::annotation::Annotations::rasterize_shapes(
                        self.annotations.committed().iter(),
                        &mut rgba,
                        w,
                        h,
                        origin,
                        scale,
                    );
                }
            }
            return Some(RasterSelection {
                scale,
                width: w,
                height: h,
                rgba,
                bounds: Bounds::new(origin, size(px(w as f32 / scale), px(h as f32 / scale))),
            });
        }
        if participating.is_empty() {
            return None;
        }
        let extent = participating
            .iter()
            .map(|(_, b)| *b)
            .reduce(|a, b| a.union(&b))?;
        let scale = participating
            .iter()
            .map(|(s, _)| s.capture.width as f32 / f32::from(s.logical_size.width))
            .fold(0., f32::max);
        let w = (f32::from(extent.size.width) * scale).round() as u32;
        let h = (f32::from(extent.size.height) * scale).round() as u32;
        if w == 0 || h == 0 {
            return None;
        }
        let mut out = image::RgbaImage::new(w, h);
        for (screen, intersection) in participating {
            let mut local = intersection;
            local.origin -= screen.bounds().origin;
            let cap = &screen.capture;
            let (cw, ch, pixels) = crate::model::export::crop(
                &cap.rgba,
                cap.width,
                cap.height,
                local,
                cap.width as f32 / f32::from(screen.logical_size.width),
            )?;
            let image = image::RgbaImage::from_raw(cw, ch, pixels)?;
            let x = (f32::from(intersection.left() - extent.left()) * scale).round() as u32;
            let y = (f32::from(intersection.top() - extent.top()) * scale).round() as u32;
            let right = (f32::from(intersection.right() - extent.left()) * scale).round() as u32;
            let bottom = (f32::from(intersection.bottom() - extent.top()) * scale).round() as u32;
            let image = image::imageops::resize(
                &image,
                right - x,
                bottom - y,
                image::imageops::FilterType::Triangle,
            );
            image::imageops::replace(&mut out, &image, x.into(), y.into());
        }
        let mut rgba = out.into_raw();
        if marked {
            if include_draft {
                self.annotations
                    .rasterize(&mut rgba, w, h, extent.origin, scale);
            } else {
                crate::annotation::Annotations::rasterize_shapes(
                    self.annotations.committed().iter(),
                    &mut rgba,
                    w,
                    h,
                    extent.origin,
                    scale,
                );
            }
        }
        Some(RasterSelection {
            scale,
            width: w,
            height: h,
            rgba,
            bounds: Bounds::new(
                extent.origin,
                size(px(w as f32 / scale), px(h as f32 / scale)),
            ),
        })
    }
}

impl FilterPreview {
    fn new(
        selection: Option<Bounds<Pixels>>,
        original: RasterSelection,
        draft_generation: u64,
    ) -> Self {
        let pixels = original.rgba.clone();
        let image = crate::render_image::rgba_to_render_image(
            pixels.clone(),
            original.width,
            original.height,
        );
        Self {
            selection,
            committed: Vec::new(),
            draft: None,
            draft_generation,
            original,
            pixels,
            image,
            stroke: None,
            moved: None,
            magnifier: None,
        }
    }

    /// Composite a non-lens drag or nudge over a cached split plane of un-dragged shapes.
    /// Returns `None` if an un-dragged mark would pollute an `annotations: false` lens.
    fn update_moved_replay(
        &mut self,
        editing: &[usize],
        committed: &[crate::annotation::Shape],
        draft_generation: u64,
    ) -> Option<Arc<RenderImage>> {
        use crate::annotation::ShapeKind;
        // Single forward Z-order pass: include the edited shapes,
        // Spotlight/Magnifier passes, and every higher-Z shape whose padded
        // footprint overlaps any shape already in `replay` (including
        // transitive overlaps `0 -> 1 -> 2`). Keeping `replay` in ascending
        // `ix` order preserves committed Z-order and re-runs higher-Z
        // Blur/Mosaic filters over dragged ink, while spatially disjoint
        // higher-Z strokes remain baked in `MovedReplay::base`. A Spotlight
        // dims the unmarked capture before `committed[0]`, so when one is
        // present every shape rides with `replay`.
        let has_spotlight = committed.iter().any(|s| s.kind == ShapeKind::Spotlight);
        let mut replay: Vec<usize> = Vec::new();
        let mut replay_bounds: Vec<Bounds<Pixels>> = Vec::new();
        for (ix, shape) in committed.iter().enumerate() {
            let shape_bounds = padded_shape_bounds(shape);
            let include = has_spotlight
                || editing.binary_search(&ix).is_ok()
                || shape.kind == ShapeKind::Magnifier
                || replay_bounds
                    .iter()
                    .any(|b| bounds_overlap(b, &shape_bounds));
            if include {
                replay.push(ix);
                replay_bounds.push(shape_bounds);
            }
        }
        if replay.len() < committed.len()
            && committed
                .iter()
                .any(|s| s.kind == ShapeKind::Magnifier && !s.lens_samples_annotations())
        {
            self.moved = None;
            return None;
        }
        let base_stale = self.moved.as_ref().is_some_and(|_| {
            self.committed.len() != committed.len()
                || committed
                    .iter()
                    .zip(&self.committed)
                    .enumerate()
                    .any(|(ix, (now, prev))| replay.binary_search(&ix).is_err() && now != prev)
        });
        // Idempotence: the renderer may call this twice for one frame
        // (element paint + the next frame before the move lands). Each call
        // would otherwise mint a fresh image that never reaches the atlas —
        // only a NEW committed state rebuilds.
        if !base_stale
            && self
                .moved
                .as_ref()
                .is_some_and(|m| m.indices.as_slice() == replay.as_slice())
            && self.committed == committed
        {
            return Some(self.image.clone());
        }
        let dims = (self.original.width, self.original.height);
        let origin = self.original.bounds.origin;
        let scale = self.original.scale;
        let rebuilt_base = base_stale
            || self
                .moved
                .as_ref()
                .is_none_or(|m| m.indices.as_slice() != replay.as_slice());
        let prior_dirty = self.moved.as_ref().and_then(|m| m.prev_dirty);
        if rebuilt_base {
            let mut base = self
                .moved
                .take()
                .map(|m| m.base)
                .unwrap_or_else(|| Vec::with_capacity(self.original.rgba.len()));
            base.clone_from(&self.original.rgba);
            crate::annotation::Annotations::rasterize_shapes(
                committed
                    .iter()
                    .enumerate()
                    .filter(|(ix, _)| replay.binary_search(ix).is_err())
                    .map(|(_, shape)| shape),
                &mut base,
                dims.0,
                dims.1,
                origin,
                scale,
            );
            self.moved = Some(MovedReplay {
                base,
                indices: replay.clone(),
                prev_dirty: prior_dirty,
            });
        }
        let curr_dirty = replay_dirty_rect(committed, &replay, dims, origin, scale);
        let has_full_canvas_effect = replay.iter().any(|&ix| {
            matches!(
                committed[ix].kind,
                ShapeKind::Spotlight | ShapeKind::Magnifier
            )
        });
        let moved = self.moved.as_mut()?;
        if !base_stale
            && !has_full_canvas_effect
            && self.committed.len() == committed.len()
            && self.pixels.len() == moved.base.len()
            && let Some(old_dirty) = moved.prev_dirty
        {
            // Restore only the physical pixel bounding box dirtied by the
            // previous frame's replay set (and any newly joined replay shapes
            // when the split base rebuilt) instead of cloning the full capture
            // buffer per step (#32).
            let dirty = if rebuilt_base {
                union_dirty_rects(old_dirty, curr_dirty)
            } else {
                old_dirty
            };
            restore_rgba_rect(&mut self.pixels, &moved.base, dims.0 as usize, dirty);
        } else {
            self.pixels.clone_from(&moved.base);
        }
        crate::annotation::Annotations::rasterize_shapes(
            replay.iter().map(|&ix| &committed[ix]),
            &mut self.pixels,
            dims.0,
            dims.1,
            origin,
            scale,
        );
        moved.prev_dirty = Some(curr_dirty);
        self.image = crate::render_image::rgba_to_render_image(self.pixels.clone(), dims.0, dims.1);
        self.committed.clear();
        self.committed.extend_from_slice(committed);
        self.draft = None;
        self.draft_generation = draft_generation;
        self.stroke = None;
        self.magnifier = None;
        Some(self.image.clone())
    }

    fn update_lens_history(
        &mut self,
        committed: &[crate::annotation::Shape],
        preferred_index: Option<usize>,
    ) -> Option<()> {
        use crate::annotation::ShapeKind;
        if self
            .magnifier
            .as_ref()
            .is_none_or(|preview| !preview.matches(committed))
        {
            let index = preferred_index
                .filter(|&index| committed[index].kind == ShapeKind::Magnifier)
                .or_else(|| {
                    committed
                        .iter()
                        .rposition(|s| s.kind == ShapeKind::Magnifier)
                })?;
            self.magnifier = Some(crate::annotation::MagnifierPreview::new(
                committed,
                index,
                &self.original.rgba,
                (self.original.width, self.original.height),
                self.original.bounds.origin,
                self.original.scale,
            ));
        }
        self.magnifier
            .as_ref()?
            .render(committed, &self.original.rgba, &mut self.pixels);
        Some(())
    }

    fn update_standard_history(&mut self, committed: &[crate::annotation::Shape]) {
        use crate::annotation::ShapeKind;
        self.magnifier = None;
        let appended_one =
            committed.len() == self.committed.len() + 1 && committed.starts_with(&self.committed);
        let suffix = if appended_one {
            self.stroke.as_mut().zip(committed.last())
        } else {
            None
        }
        .filter(|(_, shape)| shape.is_freehand());
        if let Some((stroke, last)) = suffix {
            self.pixels = stroke
                .render(
                    last,
                    &self.pixels,
                    (self.original.width, self.original.height),
                    self.original.bounds.origin,
                    self.original.scale,
                )
                .to_vec();
            return;
        }
        // Spotlight masks need the entire union without repeated dimming.
        if committed.iter().any(|s| s.kind == ShapeKind::Spotlight)
            || !committed.starts_with(&self.committed)
        {
            self.pixels.clone_from(&self.original.rgba);
            self.committed.clear();
        }
        crate::annotation::Annotations::rasterize_shapes(
            committed[self.committed.len()..].iter(),
            &mut self.pixels,
            self.original.width,
            self.original.height,
            self.original.bounds.origin,
            self.original.scale,
        );
    }

    fn update_draft_image(
        &mut self,
        committed: &[crate::annotation::Shape],
        draft: Option<&crate::annotation::Shape>,
        composite_draft: bool,
        history_changed: bool,
        image_was_committed: bool,
    ) {
        use crate::annotation::ShapeKind;
        if let Some(shape) = draft.filter(|s| s.is_freehand() && !composite_draft) {
            // Live freehand stroke: advance the incremental raster so release
            // promotion stays O(new segments), while the stroke paints as a
            // vector overlay. Refresh `image` only when switching from vector-only
            // marks or a previous raster draft so committed marks stay visible.
            self.stroke.get_or_insert_with(Default::default).render(
                shape,
                &self.pixels,
                (self.original.width, self.original.height),
                self.original.bounds.origin,
                self.original.scale,
            );
            if history_changed || !image_was_committed {
                self.image = crate::render_image::rgba_to_render_image(
                    self.pixels.clone(),
                    self.original.width,
                    self.original.height,
                );
            }
            return;
        }
        self.stroke = None;
        let mut pixels = self.pixels.clone();
        let has_lens = committed.iter().any(|s| s.kind == ShapeKind::Magnifier);
        if draft.is_some_and(|s| matches!(s.kind, ShapeKind::Magnifier | ShapeKind::Spotlight))
            || (draft.is_some() && has_lens)
        {
            pixels.clone_from(&self.original.rgba);
            crate::annotation::Annotations::rasterize_shapes(
                committed.iter().chain(draft),
                &mut pixels,
                self.original.width,
                self.original.height,
                self.original.bounds.origin,
                self.original.scale,
            );
        } else {
            crate::annotation::Annotations::rasterize_shapes(
                draft.into_iter(),
                &mut pixels,
                self.original.width,
                self.original.height,
                self.original.bounds.origin,
                self.original.scale,
            );
        }
        self.image = crate::render_image::rgba_to_render_image(
            pixels,
            self.original.width,
            self.original.height,
        );
    }
}

fn restore_rgba_rect(
    dst: &mut [u8],
    src: &[u8],
    stride_px: usize,
    (x0, y0, x1, y1): (usize, usize, usize, usize),
) {
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    if x0 == 0 && x1 == stride_px {
        let start = y0 * stride_px * 4;
        let end = y1 * stride_px * 4;
        dst[start..end].copy_from_slice(&src[start..end]);
        return;
    }
    for row in y0..y1 {
        let start = (row * stride_px + x0) * 4;
        let end = (row * stride_px + x1) * 4;
        dst[start..end].copy_from_slice(&src[start..end]);
    }
}

fn union_dirty_rects(
    a: (usize, usize, usize, usize),
    b: (usize, usize, usize, usize),
) -> (usize, usize, usize, usize) {
    if a.0 >= a.2 || a.1 >= a.3 {
        return b;
    }
    if b.0 >= b.2 || b.1 >= b.3 {
        return a;
    }
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

fn replay_dirty_rect(
    committed: &[crate::annotation::Shape],
    replay: &[usize],
    dims: (u32, u32),
    origin: Point<Pixels>,
    scale: f32,
) -> (usize, usize, usize, usize) {
    use crate::annotation::ShapeKind;
    let (w, h) = (dims.0 as usize, dims.1 as usize);
    if replay.iter().any(|&ix| {
        committed
            .get(ix)
            .is_some_and(|s| s.kind == ShapeKind::Spotlight)
    }) {
        return (0, 0, w, h);
    }
    let Some(union) = replay
        .iter()
        .filter_map(|&ix| committed.get(ix).map(padded_shape_bounds))
        .reduce(|a, b| a.union(&b))
    else {
        return (0, 0, 0, 0);
    };
    let x0 =
        ((f32::from(union.left() - origin.x) * scale).floor() - 2.).clamp(0., w as f32) as usize;
    let y0 =
        ((f32::from(union.top() - origin.y) * scale).floor() - 2.).clamp(0., h as f32) as usize;
    let x1 =
        ((f32::from(union.right() - origin.x) * scale).ceil() + 2.).clamp(0., w as f32) as usize;
    let y1 =
        ((f32::from(union.bottom() - origin.y) * scale).ceil() + 2.).clamp(0., h as f32) as usize;
    (x0, y0, x1, y1)
}

#[cfg(test)]
mod tests {
    use super::ScreenshotSession;
    use crate::annotation::{Annotations, ShapeKind};
    use crate::platform::capture::Capture;
    use gpui_kit::{AppContext, Bounds, Pixels, Point, point, px, size};
    use std::sync::Arc;

    fn patterned_capture(
        name: &str,
        logical_pos: (i32, i32),
        logical_w: u32,
        logical_h: u32,
        scale: f32,
    ) -> Arc<Capture> {
        let mut cap = Capture::for_test(logical_pos, scale);
        cap.output_name = name.into();
        cap.width = (logical_w as f32 * scale).round() as u32;
        cap.height = (logical_h as f32 * scale).round() as u32;
        let mut rgba = Vec::with_capacity((cap.width * cap.height * 4) as usize);
        for y in 0..cap.height {
            for x in 0..cap.width {
                rgba.extend_from_slice(&[
                    ((x * 13 + y * 7) % 251) as u8,
                    ((x * 5 + y * 19 + 40) % 251) as u8,
                    ((x * 23 + y * 3 + 90) % 251) as u8,
                    255,
                ]);
            }
        }
        cap.rgba = rgba;
        Arc::new(cap)
    }

    /// Reference composite built directly from the unmarked capture and
    /// `Annotations::rasterize_shapes`, bypassing `filter_preview` completely.
    fn fresh_reference_rgba(session: &ScreenshotSession, output: &str) -> Vec<u8> {
        let unmarked = session.crop_impl(output, false).unwrap();
        let mut rgba = unmarked.rgba;
        Annotations::rasterize_shapes(
            session.annotations.committed().iter(),
            &mut rgba,
            unmarked.width,
            unmarked.height,
            unmarked.bounds.origin,
            unmarked.scale,
        );
        rgba
    }

    #[test]
    fn secondary_output_move_and_handle_drags_return_output_local_bounds() {
        for (secondary_pos, secondary_scale) in [((1920, 0), 2.), ((-720, -100), 1.)] {
            let mut s = ScreenshotSession::new(
                vec![
                    patterned_capture("primary", (0, 0), 300, 200, 1.),
                    patterned_capture("secondary", secondary_pos, 300, 220, secondary_scale),
                ],
                Vec::new(),
            );
            s.begin("secondary", point(px(20.), px(30.)));
            s.end("secondary", point(px(260.), px(190.)));
            let screen_origin = point(px(secondary_pos.0 as f32), px(secondary_pos.1 as f32));
            let area = s.selection.bounds().unwrap();

            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Pencil);
                a.set_color(0);
                a.set_tool_size(8.);
                a.begin(screen_origin + point(px(50.), px(60.)), area, false);
                a.drag_to(screen_origin + point(px(110.), px(60.)), area, false);
                a.end();

                a.toggle(ShapeKind::Rectangle);
                a.set_color(1);
                a.set_tool_size(4.);
                a.begin(screen_origin + point(px(130.), px(70.)), area, false);
                a.drag_to(screen_origin + point(px(210.), px(140.)), area, false);
                a.end();

                a.toggle(ShapeKind::Select);
                a.select_index(0);
            });

            let expected_local = Bounds::new(point(px(20.), px(30.)), size(px(240.), px(160.)));
            let (idle_bounds, _) = s.filtered_preview("secondary").unwrap();
            assert_eq!(idle_bounds, expected_local, "idle {secondary_pos:?}");

            // 1. Live move drag on secondary output: initial frame, idempotent
            // repeat call, subsequent step, and release frame.
            let move_press = point(px(80.), px(60.));
            s.pointer_down("secondary", move_press, false, false);
            s.pointer_move("secondary", move_press + point(px(18.), px(12.)), false);
            assert!(s.moving.is_some());

            let (drag_bounds_1, drag_img_1) = s.filtered_preview("secondary").unwrap();
            assert_eq!(drag_bounds_1, idle_bounds, "move initial {secondary_pos:?}");

            let (repeat_bounds, repeat_img) = s.filtered_preview("secondary").unwrap();
            assert_eq!(repeat_bounds, idle_bounds, "move repeat {secondary_pos:?}");
            assert!(Arc::ptr_eq(&drag_img_1, &repeat_img));

            s.pointer_move("secondary", move_press + point(px(32.), px(24.)), false);
            let (drag_bounds_2, _) = s.filtered_preview("secondary").unwrap();
            assert_eq!(drag_bounds_2, idle_bounds, "move step {secondary_pos:?}");

            s.pointer_up("secondary", move_press + point(px(32.), px(24.)), false);
            assert!(s.moving.is_none());
            let (release_bounds, _) = s.filtered_preview("secondary").unwrap();
            assert_eq!(
                release_bounds, idle_bounds,
                "move release {secondary_pos:?}"
            );

            // 2. Live handle drag on secondary output (bottom-right corner of Rectangle).
            s.edit_annotations(|a| {
                a.select_index(1);
            });
            let br_handle_global = s.annotations.committed()[1].handle_points()[2];
            let br_local = br_handle_global - screen_origin;
            s.pointer_down("secondary", br_local, false, false);
            s.pointer_move("secondary", br_local + point(px(16.), px(14.)), false);
            assert!(s.handle_drag.is_some());

            let (h_bounds_1, h_img_1) = s.filtered_preview("secondary").unwrap();
            assert_eq!(h_bounds_1, idle_bounds, "handle initial {secondary_pos:?}");

            let (h_repeat_bounds, h_repeat_img) = s.filtered_preview("secondary").unwrap();
            assert_eq!(
                h_repeat_bounds, idle_bounds,
                "handle repeat {secondary_pos:?}"
            );
            assert!(Arc::ptr_eq(&h_img_1, &h_repeat_img));

            s.pointer_move("secondary", br_local + point(px(26.), px(22.)), false);
            let (h_bounds_2, _) = s.filtered_preview("secondary").unwrap();
            assert_eq!(h_bounds_2, idle_bounds, "handle step {secondary_pos:?}");

            s.pointer_up("secondary", br_local + point(px(26.), px(22.)), false);
            assert!(s.handle_drag.is_none());
            let (h_release_bounds, _) = s.filtered_preview("secondary").unwrap();
            assert_eq!(
                h_release_bounds, idle_bounds,
                "handle release {secondary_pos:?}"
            );
            assert_eq!(
                s.crop("secondary").unwrap().2,
                fresh_reference_rgba(&s, "secondary")
            );
        }
    }

    #[test]
    fn drag_under_higher_z_ink_and_filters_preserves_z_order_and_export_parity() {
        for filter_kind in [ShapeKind::Blur, ShapeKind::Mosaic] {
            let mut s = ScreenshotSession::new(
                vec![patterned_capture("primary", (0, 0), 340, 180, 1.)],
                Vec::new(),
            );
            s.select_all();
            let area = s.selection.bounds().unwrap();

            s.edit_annotations(|a| {
                // ix = 0: lower-Z thick Pencil stroke starting on the left.
                a.toggle(ShapeKind::Pencil);
                a.set_color(0);
                a.set_tool_size(14.);
                a.begin(point(px(15.), px(70.)), area, false);
                a.drag_to(point(px(45.), px(95.)), area, false);
                a.end();

                // ix = 1: higher-Z thick stroke in the middle.
                if a.tool() != Some(ShapeKind::Pencil) {
                    a.toggle(ShapeKind::Pencil);
                }
                a.set_color(3);
                a.set_tool_size(16.);
                a.begin(point(px(135.), px(60.)), area, false);
                a.drag_to(point(px(165.), px(115.)), area, false);
                a.end();

                // ix = 2: higher-Z Blur/Mosaic overlapping shape 1 on its right.
                a.toggle(filter_kind);
                a.set_tool_size(16.);
                a.begin(point(px(155.), px(55.)), area, false);
                a.drag_to(point(px(215.), px(125.)), area, false);
                a.end();

                // ix = 3: far-away spatially disjoint Pencil stroke.
                a.toggle(ShapeKind::Pencil);
                a.set_color(2);
                a.set_tool_size(8.);
                a.begin(point(px(285.), px(135.)), area, false);
                a.drag_to(point(px(320.), px(165.)), area, false);
                a.end();

                a.toggle(ShapeKind::Select);
                a.select_index(0);
            });

            s.filtered_preview("primary").unwrap();

            let press = point(px(25.), px(80.));
            s.pointer_down("primary", press, false, false);

            // Step 1: small move, still disjoint from 1, 2 and 3.
            s.pointer_move("primary", press + point(px(8.), px(0.)), false);
            assert!(s.moving.is_some());
            s.filtered_preview("primary").unwrap();
            assert_eq!(
                s.filter_preview
                    .borrow()
                    .as_ref()
                    .and_then(|c| c.moved.as_ref().map(|m| m.indices.clone())),
                Some(vec![0]),
                "{filter_kind:?} disjoint step"
            );
            assert_eq!(
                s.filter_preview.borrow().as_ref().unwrap().pixels,
                fresh_reference_rgba(&s, "primary"),
                "{filter_kind:?} disjoint pixels"
            );

            // Step 2 (dx=50): overlaps shape 1 only, pulling shape 2 in transitively (0 -> 1 -> 2).
            // Steps 3 & 4 (dx=135, 143): directly underneath both shape 1's ink and shape 2's
            // filter box while staying disjoint from shape 3.
            for dx in [50., 135., 143.] {
                s.pointer_move("primary", press + point(px(dx), px(4.)), false);
                s.filtered_preview("primary").unwrap();
                let indices = s
                    .filter_preview
                    .borrow()
                    .as_ref()
                    .and_then(|c| c.moved.as_ref().map(|m| m.indices.clone()))
                    .unwrap();
                assert_eq!(
                    indices,
                    vec![0, 1, 2],
                    "{filter_kind:?} at dx={dx}: must include overlapping 1,2 and exclude disjoint 3"
                );
                let expected = fresh_reference_rgba(&s, "primary");
                assert_eq!(
                    s.filter_preview.borrow().as_ref().unwrap().pixels,
                    expected,
                    "{filter_kind:?} mid-drag cached.pixels mismatch at dx={dx}"
                );
                assert_eq!(
                    s.crop("primary").unwrap().2,
                    expected,
                    "{filter_kind:?} mid-drag crop() mismatch at dx={dx}"
                );
            }

            // Release while shape 0 remains underneath shapes 1 and 2:
            // cached.moved.take() retains cached.pixels, which crop() and
            // crop_for_pin() export directly via the cache shortcut.
            s.pointer_up("primary", press + point(px(143.), px(4.)), false);
            assert!(s.moving.is_none());
            s.filtered_preview("primary").unwrap();
            assert!(s.filter_preview.borrow().as_ref().unwrap().moved.is_none());
            let expected = fresh_reference_rgba(&s, "primary");
            assert_eq!(
                s.crop("primary").unwrap().2,
                expected,
                "{filter_kind:?} post-release crop() Z-order/filter corruption"
            );
            assert_eq!(
                s.crop_for_pin("primary").unwrap().rgba,
                expected,
                "{filter_kind:?} post-release crop_for_pin() Z-order/filter corruption"
            );
        }
    }

    #[test]
    fn dirty_rect_multi_step_drag_reuses_buffer_and_leaves_no_ghost_pixels() {
        for scale in [1., 1.5] {
            let mut s = ScreenshotSession::new(
                vec![patterned_capture("primary", (0, 0), 320, 220, scale)],
                Vec::new(),
            );
            s.select_all();
            let area = s.selection.bounds().unwrap();

            s.edit_annotations(|a| {
                // Stationary lower-Z stroke baked into MovedReplay::base.
                a.toggle(ShapeKind::Pencil);
                a.set_color(1);
                a.set_tool_size(10.);
                a.begin(point(px(20.), px(180.)), area, false);
                a.drag_to(point(px(290.), px(180.)), area, false);
                a.end();

                // Dragged Pencil stroke.
                if a.tool() != Some(ShapeKind::Pencil) {
                    a.toggle(ShapeKind::Pencil);
                }
                a.set_color(0);
                a.set_tool_size(12.);
                a.begin(point(px(25.), px(30.)), area, false);
                a.drag_to(point(px(55.), px(50.)), area, false);
                a.end();

                a.toggle(ShapeKind::Select);
                a.select_index(1);
            });

            s.filtered_preview("primary").unwrap();

            let press = point(px(40.), px(40.));
            s.pointer_down("primary", press, false, false);

            // Initial drag frame builds MovedReplay; capture the pixel Vec's
            // heap pointer so subsequent dirty-rect frames prove zero reallocation.
            s.pointer_move("primary", press + point(px(10.), px(0.)), false);
            s.filtered_preview("primary").unwrap();
            let initial_ptr = s.filter_preview.borrow().as_ref().unwrap().pixels.as_ptr();
            assert_eq!(
                s.filter_preview.borrow().as_ref().unwrap().pixels,
                fresh_reference_rgba(&s, "primary")
            );

            // Six non-overlapping jump positions across the canvas (including
            // crossing over the lower-Z stroke at y=180 and leaving it again).
            let deltas: [Point<Pixels>; 6] = [
                point(px(75.), px(5.)),
                point(px(150.), px(10.)),
                point(px(225.), px(45.)),
                point(px(190.), px(135.)),
                point(px(105.), px(140.)),
                point(px(25.), px(95.)),
            ];
            for (step, delta) in deltas.into_iter().enumerate() {
                s.pointer_move("primary", press + delta, false);
                s.filtered_preview("primary").unwrap();
                let ptr = s.filter_preview.borrow().as_ref().unwrap().pixels.as_ptr();
                assert_eq!(
                    ptr, initial_ptr,
                    "scale={scale} step={step}: cached.pixels reallocated during drag"
                );
                assert_eq!(
                    s.filter_preview.borrow().as_ref().unwrap().pixels,
                    fresh_reference_rgba(&s, "primary"),
                    "scale={scale} step={step}: ghost pixels left behind"
                );
            }

            s.pointer_up("primary", press + deltas[5], false);
            s.filtered_preview("primary").unwrap();
            assert_eq!(
                s.crop("primary").unwrap().2,
                fresh_reference_rgba(&s, "primary")
            );
        }
    }

    #[gpui_kit::test]
    fn multi_selection_edit_keeps_display_preview_while_worker_drains(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let cap = patterned_capture("secondary", (1920, 0), 600, 600, 1.);
        let session = cx.new(|_| ScreenshotSession::new(vec![cap], Vec::new()));
        let screen_origin = point(px(1920.), px(0.));

        session.update(cx, |s, cx| {
            s.select_all();
            let area = s.selection.bounds().unwrap();
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Pencil);
                a.set_color(0);
                a.begin(screen_origin + point(px(40.), px(40.)), area, false);
                a.drag_to(screen_origin + point(px(90.), px(60.)), area, false);
                a.end();

                if a.tool() != Some(ShapeKind::Pencil) {
                    a.toggle(ShapeKind::Pencil);
                }
                a.set_color(1);
                a.begin(screen_origin + point(px(120.), px(40.)), area, false);
                a.drag_to(screen_origin + point(px(170.), px(60.)), area, false);
                a.end();

                // Large Blur (>= 512x512 px) so non-drag edits spawn the worker.
                a.toggle(ShapeKind::Blur);
                a.begin(screen_origin + point(px(20.), px(80.)), area, false);
                a.drag_to(screen_origin + point(px(560.), px(595.)), area, false);
                a.end();
            });
            let (idle_bounds, _) = s.filtered_preview("secondary").unwrap();
            assert_eq!(
                idle_bounds,
                Bounds::new(point(px(0.), px(0.)), size(px(600.), px(600.)))
            );

            // Trigger a background worker render on the heavy Blur while keep-history len == 3.
            s.edit_annotations(|a| {
                a.select_index(2);
                a.apply_size(24.);
                a.end_size_drag();
            });
            assert!(s.request_filtered_preview("secondary", cx).is_some());
            assert!(s.preview_busy);

            // Select BOTH Pencil strokes (multi-selection: [0, 1, 2]) and move
            // them while the worker is still in flight.
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Select);
                a.select_indices([0, 1, 2]);
            });
            let press = point(px(60.), px(50.));
            s.pointer_down("secondary", press, false, false);
            s.pointer_move("secondary", press + point(px(15.), px(10.)), false);
            s.pointer_up("secondary", press + point(px(15.), px(10.)), false);
            assert!(s.preview_busy);

            let fallback = s.request_filtered_preview("secondary", cx);
            assert!(
                fallback.is_some(),
                "multi-selection edit blanked DisplayPreview while worker drained"
            );
            assert_eq!(fallback.unwrap().0, idle_bounds);
        });
        cx.run_until_parked();
        session.update(cx, |s, cx| {
            assert!(!s.preview_busy);
            let (bounds, _) = s.request_filtered_preview("secondary", cx).unwrap();
            assert_eq!(
                bounds,
                Bounds::new(point(px(0.), px(0.)), size(px(600.), px(600.)))
            );
            assert_eq!(
                s.crop("secondary").unwrap().2,
                fresh_reference_rgba(s, "secondary")
            );
        });
    }

    #[test]
    fn arrow_key_nudge_uses_split_plane_dirty_rect_and_dynamic_replay_without_ghosts() {
        let mut s = ScreenshotSession::new(
            vec![patterned_capture("primary", (0, 0), 320, 200, 1.)],
            Vec::new(),
        );
        s.select_all();
        let area = s.selection.bounds().unwrap();

        s.edit_annotations(|a| {
            // ix = 0: lower-Z Pencil stroke to be nudged via keyboard.
            a.toggle(ShapeKind::Pencil);
            a.set_color(0);
            a.set_tool_size(12.);
            a.begin(point(px(20.), px(60.)), area, false);
            a.drag_to(point(px(50.), px(80.)), area, false);
            a.end();

            // ix = 1: higher-Z Pencil stroke in the middle (entered and exited during nudge).
            if a.tool() != Some(ShapeKind::Pencil) {
                a.toggle(ShapeKind::Pencil);
            }
            a.set_color(2);
            a.set_tool_size(14.);
            a.begin(point(px(110.), px(50.)), area, false);
            a.drag_to(point(px(130.), px(100.)), area, false);
            a.end();

            // ix = 2: stationary far-right stroke that stays in MovedReplay::base.
            if a.tool() != Some(ShapeKind::Pencil) {
                a.toggle(ShapeKind::Pencil);
            }
            a.set_color(1);
            a.set_tool_size(8.);
            a.begin(point(px(260.), px(140.)), area, false);
            a.drag_to(point(px(300.), px(170.)), area, false);
            a.end();

            a.toggle(ShapeKind::Select);
            a.select_index(0);
        });

        s.filtered_preview("primary").unwrap();

        // First nudge step initializes MovedReplay with only shape 0 in replay.
        assert!(s.nudge(point(px(10.), px(0.))));
        s.filtered_preview("primary").unwrap();
        let initial_ptr = s.filter_preview.borrow().as_ref().unwrap().pixels.as_ptr();
        assert_eq!(
            s.filter_preview
                .borrow()
                .as_ref()
                .and_then(|c| c.moved.as_ref().map(|m| m.indices.clone())),
            Some(vec![0])
        );
        assert_eq!(
            s.filter_preview.borrow().as_ref().unwrap().pixels,
            fresh_reference_rgba(&s, "primary")
        );

        // Nudge into overlap with higher-Z shape 1 (replay transitions [0] -> [0, 1]),
        // then past shape 1 so it leaves overlap again (replay transitions [0, 1] -> [0]).
        for (step, dx) in [35., 35., 45., 45.].into_iter().enumerate() {
            assert!(s.nudge(point(px(dx), px(0.))));
            s.filtered_preview("primary").unwrap();
            let ptr = s.filter_preview.borrow().as_ref().unwrap().pixels.as_ptr();
            assert_eq!(
                ptr, initial_ptr,
                "step={step}: cached.pixels reallocated during keyboard nudge"
            );
            assert_eq!(
                s.filter_preview.borrow().as_ref().unwrap().pixels,
                fresh_reference_rgba(&s, "primary"),
                "step={step}: pixel mismatch or ghost trail during keyboard nudge"
            );
        }
        assert_eq!(
            s.filter_preview
                .borrow()
                .as_ref()
                .and_then(|c| c.moved.as_ref().map(|m| m.indices.clone())),
            Some(vec![0]),
            "replay must shrink back to [0] after leaving overlap with shape 1"
        );

        // Finishing the nudge drops MovedReplay in O(1) while retaining the buffer.
        s.finish_nudge();
        s.filtered_preview("primary").unwrap();
        assert!(s.filter_preview.borrow().as_ref().unwrap().moved.is_none());
        assert_eq!(
            s.filter_preview.borrow().as_ref().unwrap().pixels.as_ptr(),
            initial_ptr
        );
        assert_eq!(
            s.crop("primary").unwrap().2,
            fresh_reference_rgba(&s, "primary")
        );
    }

    #[test]
    fn live_freehand_draft_with_committed_spotlight_or_magnifier_reuses_committed_pixels() {
        for lens_kind in [ShapeKind::Spotlight, ShapeKind::Magnifier] {
            let mut s = ScreenshotSession::new(
                vec![patterned_capture("primary", (0, 0), 260, 180, 1.)],
                Vec::new(),
            );
            s.select_all();
            let area = s.selection.bounds().unwrap();

            s.edit_annotations(|a| {
                a.toggle(lens_kind);
                a.begin(point(px(30.), px(30.)), area, false);
                a.drag_to(point(px(120.), px(110.)), area, false);
                a.end();
                a.finish_interaction();
            });
            s.filtered_preview("primary").unwrap();
            let committed_snapshot = s.filter_preview.borrow().as_ref().unwrap().pixels.clone();
            let pixels_ptr = s.filter_preview.borrow().as_ref().unwrap().pixels.as_ptr();
            let committed_vec_ptr = s
                .filter_preview
                .borrow()
                .as_ref()
                .unwrap()
                .committed
                .as_ptr();

            // Begin a live freehand Pencil stroke and move across several points.
            s.edit_annotations(|a| {
                a.toggle(ShapeKind::Pencil);
                a.begin(point(px(140.), px(40.)), area, false);
            });
            for x in [155., 175., 195., 215.] {
                s.edit_annotations(|a| {
                    a.drag_to(point(px(x), px(75.)), area, false);
                });
                s.filtered_preview("primary").unwrap();
                let cache = s.filter_preview.borrow();
                let cached = cache.as_ref().unwrap();
                assert_eq!(
                    cached.pixels.as_ptr(),
                    pixels_ptr,
                    "{lens_kind:?}: cached.pixels reallocated during freehand draft"
                );
                assert_eq!(
                    cached.committed.as_ptr(),
                    committed_vec_ptr,
                    "{lens_kind:?}: cached.committed reallocated during freehand draft where history_changed is false"
                );
                assert_eq!(cached.pixels, committed_snapshot);
            }

            s.edit_annotations(|a| a.end());
            s.filtered_preview("primary").unwrap();
            assert_eq!(
                s.crop("primary").unwrap().2,
                fresh_reference_rgba(&s, "primary"),
                "{lens_kind:?}: post-stroke export mismatch"
            );
        }
    }
}
