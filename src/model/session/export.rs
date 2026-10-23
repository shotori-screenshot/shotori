//! Export: turning the session into pixels. The crop paths (plain,
//! marked-up, pin-geometry), the long-capture region query and the
//! filtered preview pipeline (rasterized annotations composited over
//! the frozen capture, cached and invalidated by session state).

use super::{DisplayPreview, FilterPreview, RasterSelection, ScreenshotSession, ScrollRect};
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
        if !self.preview_busy {
            let unchanged = self.filter_preview.borrow().as_ref().is_some_and(|cache| {
                cache.selection == self.selection.bounds()
                    && cache.committed == self.annotations.committed()
                    && cache.draft.as_ref() == self.annotations.draft_shape()
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
            let ink_only = self
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
                    && self.annotations.draft_shape().is_none()
                    && self.annotations.committed().len() == cache.committed.len() + 1
                    && self.annotations.committed().starts_with(&cache.committed)
                    && self
                        .annotations
                        .committed()
                        .last()
                        .is_some_and(|shape| shape.is_freehand())
            });
            let heavy = self.annotations.committed().iter().any(|shape| {
                shape.points.len() > 1024
                    || (matches!(
                        shape.kind,
                        crate::annotation::ShapeKind::Blur | crate::annotation::ShapeKind::Mosaic
                    ) && f32::from(shape.bounds.size.width)
                        * f32::from(shape.bounds.size.height)
                        * max_scale
                        * max_scale
                        >= 262_144.)
            }) || self.annotations.visible().any(|shape| {
                matches!(
                    shape.kind,
                    crate::annotation::ShapeKind::Blur | crate::annotation::ShapeKind::Mosaic
                ) && f32::from(shape.bounds.size.width)
                    * f32::from(shape.bounds.size.height)
                    * max_scale
                    * max_scale
                    >= 262_144.
            });
            if unchanged || !heavy || ink_only || promotable {
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
                            *session.filter_preview.get_mut() = cache;
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
        let same_gesture = display.draft_generation == self.annotations.draft_generation();
        let just_finished = same_gesture
            && self.annotations.draft_shape().is_none()
            && committed.len() == display.committed.len() + 1
            && committed.starts_with(&display.committed)
            && committed.last().map(|shape| shape.kind) == display.draft_kind;
        let valid_draft = display.draft_kind.is_none()
            || (same_gesture
                && display.draft_kind == self.annotations.draft_shape().map(|shape| shape.kind));
        if display.selection != self.selection.bounds()
            || !(just_finished || (same_history && valid_draft))
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
                        | ShapeKind::Mosaic
                        | ShapeKind::Blur
                        | ShapeKind::Text
                )
            })
    }

    /// Reuse the exported composite for freehand strokes, pixel filters and text.
    /// Captures are immutable; selection, shapes and display geometry own invalidation.
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
        let draft = self.annotations.draft_shape();
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
            });
        }
        let cached = cache.as_mut()?;
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
            let appended_one = committed.len() == cached.committed.len() + 1
                && committed.starts_with(&cached.committed);
            // Appending a finished pencil/highlighter stroke only replays the
            // new suffix. Undo, replacement and edits rebuild from the
            // immutable capture. The pair lookup degrades instead of
            // unwrapping; any miss lands in the full rebuild below, which is
            // always correct.
            let suffix = if appended_one {
                cached.stroke.as_mut().zip(committed.last())
            } else {
                None
            }
            .filter(|(_, shape)| matches!(shape.kind, ShapeKind::Pencil | ShapeKind::Highlighter));
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
                if !committed.starts_with(&cached.committed) {
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
            if history_changed {
                cached.stroke = None;
            }
            if cached.committed != committed {
                cached.committed = committed.to_vec();
            }
            if let Some(shape) = draft.filter(|s| s.is_freehand()) {
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
                crate::annotation::Annotations::rasterize_shapes(
                    draft.into_iter(),
                    &mut pixels,
                    original.width,
                    original.height,
                    original.bounds.origin,
                    original.scale,
                );
                cached.image = crate::render_image::rgba_to_render_image(
                    pixels,
                    original.width,
                    original.height,
                );
            }
            cached.draft = draft.cloned();
            cached.draft_generation = self.annotations.draft_generation();
        }
        let mut bounds = cached.original.bounds;
        bounds.origin -= self.screen(output).bounds().origin;
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
