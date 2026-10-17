//! Smart region detection: the pixel-based candidate layer. The
//! background analysis lands per output as a [`RegionDetector`];
//! these methods answer "what is under the pointer" for the hover
//! outline, the click snap and the wheel hierarchy — debounced by the
//! smart cursor, gated by the settings switch.

use std::sync::Arc;

use super::{Screen, ScreenshotSession};
use gpui_kit::*;

impl ScreenshotSession {
    // ── Smart regions (pixel-based candidates) ─────────────────────

    /// The screen whose bounds contain a global logical point.
    fn screen_index_at(&self, global: Point<Pixels>) -> Option<usize> {
        self.screens
            .iter()
            .position(|s| s.bounds().contains(&global))
    }

    /// A screen's physical-pixel coordinate for a global logical point
    /// (the detector's native space).
    fn to_physical(screen: &Screen, global: Point<Pixels>) -> (i32, i32) {
        let bounds = screen.bounds();
        let sx = screen.capture.width as f32 / f32::from(bounds.size.width).max(1.);
        let sy = screen.capture.height as f32 / f32::from(bounds.size.height).max(1.);
        (
            ((f32::from(global.x - bounds.origin.x)) * sx).round() as i32,
            ((f32::from(global.y - bounds.origin.y)) * sy).round() as i32,
        )
    }

    /// A detector rect (screen physical px) back into global logical
    /// coordinates — the session's selection space.
    fn to_logical_bounds(
        screen: &Screen,
        rect: crate::model::regiondetect::IntRect,
    ) -> Bounds<Pixels> {
        let bounds = screen.bounds();
        let sx = f32::from(bounds.size.width) / (screen.capture.width as f32).max(1.);
        let sy = f32::from(bounds.size.height) / (screen.capture.height as f32).max(1.);
        let origin = bounds.origin;
        Bounds {
            origin: point(
                px(rect.left as f32 * sx) + origin.x,
                px(rect.top as f32 * sy) + origin.y,
            ),
            size: size(
                px((rect.right - rect.left) as f32 * sx),
                px((rect.bottom - rect.top) as f32 * sy),
            ),
        }
    }

    /// Install a screen's detector (delivered by the background
    /// analysis in `main`). The compositor snap rects are injected as
    /// window candidates so the wheel hierarchy can step out to whole
    /// windows even where pixel detection found nothing. Refreshes the
    /// hover under the (possibly stationary) pointer.
    pub fn set_region_detector(
        &mut self,
        name: &str,
        mut detector: crate::model::regiondetect::RegionDetector,
    ) {
        let Some(ix) = self
            .screens
            .iter()
            .position(|s| s.capture.output_name == name)
        else {
            return;
        };
        let screen_bounds = self.screens[ix].bounds();
        let capture = self.screens[ix].capture.clone();
        // Snap rects live in global logical space; the detector wants
        // this screen's PHYSICAL pixels. Clip to the screen first — a
        // window straddling two outputs must not import the other
        // output's pixels into this detector's coordinate space.
        let sx = capture.width as f32 / f32::from(screen_bounds.size.width).max(1.);
        let sy = capture.height as f32 / f32::from(screen_bounds.size.height).max(1.);
        let windows: Vec<crate::model::regiondetect::IntRect> = self
            .snaps
            .iter()
            .map(|snap| snap.bounds.intersect(&screen_bounds))
            .filter(|b| b.size.width > px(0.) && b.size.height > px(0.))
            .map(|b| {
                let local = b.origin - screen_bounds.origin;
                crate::model::regiondetect::IntRect {
                    left: (f32::from(local.x) * sx).round() as i32,
                    top: (f32::from(local.y) * sy).round() as i32,
                    right: ((f32::from(local.x) + f32::from(b.size.width)) * sx).round() as i32,
                    bottom: ((f32::from(local.y) + f32::from(b.size.height)) * sy).round() as i32,
                }
            })
            .collect();
        detector.add_window_rects(&windows);
        self.screens[ix].detector = Some(Arc::new(detector));
        self.cursor_screen = None; // re-pin on the next query
        // The outline may light up under a pointer that has not moved
        // since the analysis landed (mark-shot refreshes on arrival
        // too — without this the first hover waits for motion). Only
        // while the session is still in its selection phase.
        if let Some(global) = self.pointer_global
            && !self.blocked
            && !self.selection.is_dragging()
            && !self.selection.is_selected()
            && !self.selection.is_editing()
            && !self.annotations.enabled()
            && let Some(bounds) = self.smart_region_at(global)
            && Some(bounds) != self.hovered
        {
            self.hovered = Some(bounds);
        }
    }

    /// Query the smart layer at a global logical point: the hovered
    /// region (debounced, wheel-level respected), as global logical
    /// bounds. None when detection has not landed, the point has no
    /// candidate (or only the full screen, which reads as "no hit" —
    /// unless the user wheeled up to it explicitly).
    pub(super) fn smart_region_at(&mut self, global: Point<Pixels>) -> Option<Bounds<Pixels>> {
        // The settings switch is the feature's single off switch: one
        // check here silences hover, click-snap and the mark-shot
        // refresh together (the wheel path has its own gate in
        // `smart_step`). The compositor snap fallback below stays live.
        if !crate::model::settings::current().smart_region() {
            return None;
        }
        let ix = self.screen_index_at(global)?;
        let detector = self.screens[ix].detector.clone()?;
        if self.cursor_screen != Some(ix) {
            self.smart_cursor.reset();
            self.cursor_screen = Some(ix);
        }
        let (x, y) = Self::to_physical(&self.screens[ix], global);
        let rect = self.smart_cursor.hover(&detector, x, y);
        if rect == detector.image_bounds() && !self.smart_cursor.manual_level() {
            return None; // full-screen = miss → fall back to snaps
        }
        Some(Self::to_logical_bounds(&self.screens[ix], rect))
    }

    /// Wheel step through the region hierarchy under the pointer:
    /// positive grows to the parent, negative shrinks to the child.
    /// Only meaningful in the pre-selection phase; returns whether the
    /// event was consumed (a detector must have landed).
    pub(crate) fn smart_step(&mut self, direction: i32) -> bool {
        if self.blocked
            // same off switch as `smart_region_at` — the wheel must not
            // resurrect a hierarchy the user turned off
            || !crate::model::settings::current().smart_region()
            || self.annotations.enabled()
            || self.annotations.tool().is_some()
            || self.selection.bounds().is_some()
            || self.selection.is_dragging()
            || self.selection.is_editing()
        {
            return false;
        }
        let Some(global) = self.pointer_global else {
            return false;
        };
        let Some(ix) = self.screen_index_at(global) else {
            return false;
        };
        let Some(detector) = self.screens[ix].detector.clone() else {
            return false;
        };
        if self.cursor_screen != Some(ix) {
            self.smart_cursor.reset();
            self.cursor_screen = Some(ix);
        }
        let (x, y) = Self::to_physical(&self.screens[ix], global);
        let rect = self.smart_cursor.step(&detector, x, y, direction);
        self.hovered = Some(Self::to_logical_bounds(&self.screens[ix], rect));
        true
    }
}
