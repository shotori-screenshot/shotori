//! Export: turning the session into pixels. The crop paths (plain,
//! marked-up, pin-geometry), the long-capture region query and the
//! filtered preview pipeline (rasterized annotations composited over
//! the frozen capture, cached and invalidated by session state).

use super::{
    DisplayPreview, FilterPreview, MovedReplay, RasterSelection, ScreenshotSession, ScrollRect,
};
use gpui_kit::*;
use std::sync::Arc;

/// Conservative logical bounding box enclosing every pixel a shape can
/// paint or filter, including antialiased stroke/badge fringes, blur
/// kernels and mosaic tile edges. Used both to detect higher-Z shapes
/// whose ink overlaps the drag replay plane and to bound the dirty
/// rectangle restored from `MovedReplay::base` on the next drag frame.
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

    /// One background render per session. Pointer updates remain in the model;
    /// when work finishes we snapshot only the newest state, never a frame queue.
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
        // Text geometry and its selection frame must be published together.
        // A heavy lens elsewhere must not leave the text at an older position.
        if self.annotations.editing_text().is_some()
            || self.annotations.selected_indices().iter().any(|&ix| {
                self.annotations
                    .committed()
                    .get(ix)
                    .is_some_and(|shape| shape.kind == crate::annotation::ShapeKind::Text)
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
        use crate::annotation::ShapeKind;
        self.annotations.committed().len() >= 64
            || self.annotations.visible().any(|shape| {
                matches!(
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
            let pixels = original.rgba.clone();
            let image = crate::render_image::rgba_to_render_image(
                pixels.clone(),
                original.width,
                original.height,
            );
            *cache = Some(FilterPreview {
                selection,
                committed: Vec::new(),
                draft: None,
                draft_generation: self.annotations.draft_generation(),
                original,
                pixels,
                image,
                stroke: None,
                moved: None,
            });
        }
        let cached = cache.as_mut()?;
        let local_bounds = self.local_preview_bounds(output, cached.original.bounds);
        // Live move/handle drag: replaying the whole scene per pointer
        // move (full-frame base copy + every committed shape
        // re-rasterized) cost ~20 ms/frame at 1080p and ~65 ms with a
        // magnifier on screen — the dragged shape visibly trailed the
        // pointer. Instead, composite the scene WITHOUT the replay set
        // once (split plane), then repaint only the replay set over that
        // stable base each frame.
        if let Some(editing) = self.editing_shape_indices() {
            // Single forward Z-order pass: include the edited shapes,
            // Spotlight/Magnifier passes, and every higher-Z shape whose
            // padded footprint overlaps any shape already in `replay`
            // (including transitive overlaps `0 -> 1 -> 2`). Keeping
            // `replay` in ascending `ix` order preserves committed Z-order
            // and re-runs higher-Z Blur/Mosaic filters over dragged ink,
            // while spatially disjoint higher-Z strokes remain baked in
            // `MovedReplay::base`. A Spotlight dims the unmarked capture
            // before `committed[0]`, so when one is present every shape
            // rides with `replay`.
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
            let base_stale =
                cached.moved.as_ref().is_some_and(|_| {
                    cached.committed.len() != committed.len()
                        || committed.iter().zip(&cached.committed).enumerate().any(
                            |(ix, (now, prev))| replay.binary_search(&ix).is_err() && now != prev,
                        )
                });
            // Idempotence: the renderer may call this twice for one
            // frame (element paint + the next frame before the move
            // lands). Each call would otherwise mint a fresh image
            // that never reaches the atlas — only a NEW committed
            // state (a drag frame) rebuilds.
            if !base_stale
                && cached
                    .moved
                    .as_ref()
                    .is_some_and(|m| m.indices.as_slice() == replay.as_slice())
                && cached.committed == committed
            {
                return Some((local_bounds, cached.image.clone()));
            }
            let dims = (cached.original.width, cached.original.height);
            let origin = cached.original.bounds.origin;
            let scale = cached.original.scale;
            let prev_replay = cached.moved.as_ref().map(|m| m.indices.clone());
            let rebuilt_base = base_stale
                || prev_replay
                    .as_ref()
                    .is_none_or(|indices| indices.as_slice() != replay.as_slice());
            if rebuilt_base {
                let mut base = cached
                    .moved
                    .take()
                    .map(|m| m.base)
                    .unwrap_or_else(|| Vec::with_capacity(cached.original.rgba.len()));
                base.clone_from(&cached.original.rgba);
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
                cached.moved = Some(MovedReplay {
                    base,
                    indices: replay.clone(),
                });
            }
            let moved_base = cached.moved.as_ref().map(|m| &m.base)?;
            let has_full_canvas_effect = replay.iter().any(|&ix| {
                matches!(
                    committed[ix].kind,
                    ShapeKind::Spotlight | ShapeKind::Magnifier
                )
            });
            if let Some(old_indices) = prev_replay.filter(|_| {
                !base_stale
                    && !has_full_canvas_effect
                    && cached.committed.len() == committed.len()
                    && cached.pixels.len() == moved_base.len()
            }) {
                let prev_dirty = old_indices
                    .iter()
                    .filter_map(|&ix| cached.committed.get(ix))
                    .map(padded_shape_bounds);
                let next_dirty = rebuilt_base
                    .then(|| {
                        replay
                            .iter()
                            .filter_map(|&ix| committed.get(ix))
                            .map(padded_shape_bounds)
                    })
                    .into_iter()
                    .flatten();
                if let Some(dirty) = prev_dirty.chain(next_dirty).reduce(|a, b| a.union(&b)) {
                    // Restore only the physical pixel bounding box dirtied by
                    // the previous drag frame's replay set (and any newly joined
                    // replay shapes when the split base rebuilt) instead of
                    // cloning the full capture buffer per pointer move (#32).
                    let x0 = ((f32::from(dirty.left() - origin.x) * scale).floor() - 2.)
                        .clamp(0., dims.0 as f32) as usize;
                    let y0 = ((f32::from(dirty.top() - origin.y) * scale).floor() - 2.)
                        .clamp(0., dims.1 as f32) as usize;
                    let x1 = ((f32::from(dirty.right() - origin.x) * scale).ceil() + 2.)
                        .clamp(0., dims.0 as f32) as usize;
                    let y1 = ((f32::from(dirty.bottom() - origin.y) * scale).ceil() + 2.)
                        .clamp(0., dims.1 as f32) as usize;
                    if x0 < x1 && y0 < y1 {
                        let stride = dims.0 as usize * 4;
                        let col_start = x0 * 4;
                        let col_end = x1 * 4;
                        for row in y0..y1 {
                            let row_offset = row * stride;
                            cached.pixels[row_offset + col_start..row_offset + col_end]
                                .copy_from_slice(
                                    &moved_base[row_offset + col_start..row_offset + col_end],
                                );
                        }
                    }
                }
            } else {
                cached.pixels.clone_from(moved_base);
            }
            crate::annotation::Annotations::rasterize_shapes(
                replay.iter().map(|&ix| &committed[ix]),
                &mut cached.pixels,
                dims.0,
                dims.1,
                origin,
                scale,
            );
            cached.image =
                crate::render_image::rgba_to_render_image(cached.pixels.clone(), dims.0, dims.1);
            cached.committed.clear();
            cached.committed.extend(committed.iter().cloned());
            cached.draft = None;
            cached.draft_generation = self.annotations.draft_generation();
            cached.stroke = None;
            return Some((local_bounds, cached.image.clone()));
        }
        // A drag just ended: the split composite already equals the full
        // replay (base + replay shapes at their final positions), so keep
        // the frame and just drop the split state.
        if cached.moved.take().is_some() && draft.is_none() && cached.committed == committed {
            cached.draft = None;
            cached.draft_generation = self.annotations.draft_generation();
            return Some((local_bounds, cached.image.clone()));
        }
        if cached.committed != committed
            || cached.draft.as_ref() != draft
            || cached.draft_generation != self.annotations.draft_generation()
        {
            let original = &cached.original;
            // Appending a finished stroke only replays the new suffix. Undo,
            // replacement and edits rebuild from the immutable capture.
            if cached.draft_generation != self.annotations.draft_generation() {
                cached.stroke = None;
            }
            let history_changed = cached.committed != committed;
            if history_changed {
                let appended_one = committed.len() == cached.committed.len() + 1
                    && committed.starts_with(&cached.committed);
                // Appending a finished pencil/highlighter stroke only replays the
                // new suffix. Undo, replacement and edits rebuild from the
                // immutable capture. The pair lookup degrades instead of
                // unwrapping; any miss lands in the full rebuild below, which is
                // always correct.
                let suffix =
                    if appended_one && !committed.iter().any(|s| s.kind == ShapeKind::Magnifier) {
                        cached.stroke.as_mut().zip(committed.last())
                    } else {
                        None
                    }
                    .filter(|(_, shape)| shape.is_freehand());
                if let Some((stroke, last)) = suffix {
                    // Promote the final incremental draft, including a new release
                    // point, instead of rasterizing the entire long stroke again.
                    cached.pixels = stroke
                        .render(
                            last,
                            &cached.pixels,
                            (original.width, original.height),
                            original.bounds.origin,
                            original.scale,
                        )
                        .to_vec();
                } else {
                    // Lenses can sample the unmarked capture; a suffix-only replay
                    // would incorrectly treat already painted annotations as the source.
                    // Spotlight masks also need the entire union, without repeated dimming.
                    if committed
                        .iter()
                        .any(|s| matches!(s.kind, ShapeKind::Magnifier | ShapeKind::Spotlight))
                        || !committed.starts_with(&cached.committed)
                    {
                        cached.pixels.clone_from(&original.rgba);
                        cached.committed.clear();
                    }
                    crate::annotation::Annotations::rasterize_shapes(
                        committed[cached.committed.len()..].iter(),
                        &mut cached.pixels,
                        original.width,
                        original.height,
                        original.bounds.origin,
                        original.scale,
                    );
                }
                cached.stroke = None;
                cached.committed = committed.to_vec();
            }
            if let Some(shape) =
                draft.filter(|s| s.is_freehand() && !self.annotations.composite_draft())
            {
                // Live freehand stroke: advance the incremental raster so
                // the release promotion stays O(new segments), but do NOT
                // rebuild the selection image — the stroke paints as a
                // vector union path on the overlay (see local_draft).
                // Re-swapping the whole crop into the atlas on every
                // pointer move was the stutter that broke strokes into
                // visible dots.
                cached.stroke.get_or_insert_with(Default::default).render(
                    shape,
                    &cached.pixels,
                    (original.width, original.height),
                    original.bounds.origin,
                    original.scale,
                );
            } else {
                cached.stroke = None;
                let mut pixels = cached.pixels.clone();
                // Lens and spotlight drafts re-run the whole composite (they
                // sample/dim the marked layer).
                if draft
                    .is_some_and(|s| matches!(s.kind, ShapeKind::Magnifier | ShapeKind::Spotlight))
                    || (draft.is_some() && committed.iter().any(|s| s.kind == ShapeKind::Magnifier))
                {
                    pixels.clone_from(&original.rgba);
                    crate::annotation::Annotations::rasterize_shapes(
                        committed.iter().chain(draft),
                        &mut pixels,
                        original.width,
                        original.height,
                        original.bounds.origin,
                        original.scale,
                    );
                } else {
                    crate::annotation::Annotations::rasterize_shapes(
                        draft.into_iter(),
                        &mut pixels,
                        original.width,
                        original.height,
                        original.bounds.origin,
                        original.scale,
                    );
                }
                cached.image = crate::render_image::rgba_to_render_image(
                    pixels,
                    original.width,
                    original.height,
                );
            }
            cached.draft = draft.cloned();
            cached.draft_generation = self.annotations.draft_generation();
        }
        Some((local_bounds, cached.image.clone()))
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
