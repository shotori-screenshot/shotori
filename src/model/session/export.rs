//! Export: turning the session into pixels. The crop paths (plain,
//! marked-up, pin-geometry), the long-capture region query and the
//! filtered preview pipeline (rasterized annotations composited over
//! the frozen capture, cached and invalidated by session state).

use super::{
    DisplayPreview, FilterPreview, MovedReplay, RasterSelection, ScreenshotSession, ScrollRect,
};
use gpui_kit::*;
use std::sync::Arc;

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
            || self.annotations.selected().is_some_and(|shape| {
                matches!(
                    shape.kind,
                    crate::annotation::ShapeKind::Text | crate::annotation::ShapeKind::Magnifier
                )
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
        // Editing one selected object must not hide every other annotation while
        // the worker replaces the composite. The old full frame remains valid
        // across move/resize and release, but never across deletion or a new draft.
        let selected_edit = display.committed.len() == committed.len()
            && display
                .committed
                .iter()
                .zip(committed)
                .enumerate()
                .all(|(ix, (old, new))| {
                    old == new
                        || (self.annotations.selected_index() == Some(ix) && old.kind == new.kind)
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
        let mut bounds = display.bounds;
        bounds.origin -= self.screen(output).bounds().origin;
        Some((bounds, display.image.clone()))
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
    /// handle), if any. `filtered_preview` uses this to repaint only
    /// the dragged shapes per frame instead of the whole scene.
    fn editing_shape_indices(&self) -> Option<Vec<usize>> {
        let mut set: Vec<usize> = Vec::new();
        if let Some(drag) = &self.moving {
            set.extend(drag.before.iter().map(|(ix, _)| *ix));
        }
        if let Some(drag) = &self.handle_drag {
            set.push(drag.ix);
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
        let mut bounds = cached.original.bounds;
        bounds.origin -= self.screen(output).bounds().origin;
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
            } else if let Some(image) = cached.update_moved_replay(editing, committed) {
                return Some((bounds, image));
            }
        } else if cached.moved.take().is_some() && cached.committed == committed {
            // A drag just ended: the split composite already equals the full
            // replay (base + dragged shapes at their final positions).
            cached.draft = draft.cloned();
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
                cached.committed = committed.to_vec();
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

    /// Composite a non-lens drag over a cached split plane of un-dragged shapes.
    /// Returns `None` if an un-dragged mark would pollute an `annotations: false` lens.
    fn update_moved_replay(
        &mut self,
        mut replay: Vec<usize>,
        committed: &[crate::annotation::Shape],
    ) -> Option<Arc<RenderImage>> {
        use crate::annotation::ShapeKind;
        for (ix, shape) in committed.iter().enumerate() {
            if matches!(shape.kind, ShapeKind::Spotlight | ShapeKind::Magnifier)
                && !replay.contains(&ix)
            {
                replay.push(ix);
            }
        }
        replay.sort_unstable();
        if replay.len() < committed.len()
            && committed
                .iter()
                .any(|s| s.kind == ShapeKind::Magnifier && !s.lens_samples_annotations())
        {
            self.moved = None;
            return None;
        }
        if self.moved.as_ref().is_some_and(|m| m.indices == replay) && self.committed == committed {
            return Some(self.image.clone());
        }
        let dims = (self.original.width, self.original.height);
        let origin = self.original.bounds.origin;
        let scale = self.original.scale;
        if self.moved.as_ref().is_none_or(|m| m.indices != replay) {
            let mut base = self.original.rgba.clone();
            crate::annotation::Annotations::rasterize_shapes(
                committed
                    .iter()
                    .enumerate()
                    .filter(|(ix, _)| !replay.contains(ix))
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
            });
        }
        let base = self.moved.as_ref().map(|m| m.base.clone())?;
        self.pixels = base;
        crate::annotation::Annotations::rasterize_shapes(
            committed
                .iter()
                .enumerate()
                .filter(|(ix, _)| replay.contains(ix))
                .map(|(_, shape)| shape),
            &mut self.pixels,
            dims.0,
            dims.1,
            origin,
            scale,
        );
        self.image = crate::render_image::rgba_to_render_image(self.pixels.clone(), dims.0, dims.1);
        self.committed = committed.to_vec();
        self.draft = None;
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
