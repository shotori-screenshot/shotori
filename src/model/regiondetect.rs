//! # Smart region detection: pixel-based selection candidates
//!
//! Hover-highlight + click-to-select for UI regions (panels, controls,
//! text blocks) straight from the frozen pixels — no compositor IPC
//! involved. Ported from mark-shot's `region_detection` module; the
//! algorithm is identical, the comments preserve its hard-won rationale.
//!
//! Why this exists next to [`crate::platform::windowsnap`]: the IPC
//! backends cover floating windows only (niri's tiled geometry is not
//! exposed at all), and naive pixel detection was prototyped and
//! rejected once before — content edges inside windows scored as
//! strongly as real boundaries, so snapping silently grabbed wrong
//! regions. This pipeline fixes exactly that with:
//!
//! - a **directional color-difference edge field** (not just Canny):
//!   per-axis gradients with non-maximum suppression, a self-calibrating
//!   threshold (35th percentile of the gradient histogram, clamped), and
//!   long-run "weak line" promotion so faint 1px panel borders survive
//!   while short text strokes do not;
//! - **four proposal sources** (closed contours, connected components,
//!   edge-bounded background, paired long lines) so both small controls
//!   and large panels get nominated;
//! - **border-support scoring** — a candidate must have edges along all
//!   four sides — plus texture penalties that reject text blocks and
//!   icon grids;
//! - **sub-pixel-free refinement** on the ORIGINAL resolution: each side
//!   searches ±radius px for the strongest color boundary, undoing the
//!   downscale blur.
//!
//! Analysis runs at most 1280 px on the long edge, once per capture, on
//! a background thread (see `main.rs`); the result is a candidate list
//! cheap to query per pointer move. Compositor window rects are injected
//! afterwards as high-quality candidates (see
//! [`RegionDetector::add_window_rects`]) so hover prefers UI controls
//! and the wheel hierarchy can step out to whole windows.
//!
//! Debug builds run the pixel loops without optimization (~10× slower);
//! the feature simply lights up a moment later. Release analysis of a
//! 2560×1440 frame is on the order of a hundred milliseconds.

// ── Geometry ─────────────────────────────────────────────────────────

/// Half-open pixel rect `[left, right) × [top, bottom)` — the pipeline's
/// native space (analysis and full-resolution pixels alike). Unlike a
/// width/height rect it cannot express a negative size, and edge tests
/// are unambiguous when mapping between resolutions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IntRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl IntRect {
    pub(crate) fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
    fn width(self) -> i32 {
        self.right - self.left
    }
    fn height(self) -> i32 {
        self.bottom - self.top
    }
    fn area(self) -> i64 {
        i64::from(self.width()) * i64::from(self.height())
    }
    /// Point containment, half-open on right/bottom (±1 px against Qt's
    /// inclusive QRect — immaterial for hit-tests).
    fn contains_point(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    /// Full containment (borders may touch).
    fn contains_rect(self, other: IntRect) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

/// Where a candidate came from. Detection sources merge (a rect proposed
/// by both contours and lines is more trustworthy); `WINDOW` marks
/// compositor-rect injection and changes hover priority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sources(u8);

impl Sources {
    pub(crate) const CONTOUR: Self = Self(1 << 0);
    const COMPONENT: Self = Self(1 << 1);
    const BACKGROUND: Self = Self(1 << 2);
    const LINES: Self = Self(1 << 3);
    pub(crate) const WINDOW: Self = Self(1 << 4);

    fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    fn has(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// A selection candidate: the rect plus why we believe in it. Quality
/// distinguishes solid frames from hypothesis-grade pairings; sources
/// mark provenance (window injection changes hover priority).
#[derive(Clone, Debug)]
pub(crate) struct RegionCandidate {
    pub(crate) rect: IntRect,
    quality: f32,
    sources: Sources,
}

impl RegionCandidate {
    /// Hand-built candidate for integration tests in other modules.
    #[cfg(test)]
    pub(crate) fn synthetic(rect: IntRect, quality: f32, sources: Sources) -> Self {
        Self {
            rect,
            quality,
            sources,
        }
    }
}

mod edges;
mod image;
mod proposals;

use edges::build_edge_field;
use image::{GrayImage, RgbaView, close_linf1, dilate_linf1, downscale};
use proposals::{
    border_sides, component_proposals, contour_proposals, inset_support, interior_density,
    line_proposals, merge_candidate, refine_rect, scale_rect,
};
// ── Pipeline ─────────────────────────────────────────────────────────

/// Analysis resolution: the long edge capped at 1280 px (never below
/// 3 px, or Canny's 3×3 kernels would degenerate).
fn analysis_size(width: i32, height: i32) -> (i32, i32) {
    const MAX_DIMENSION: i32 = 1280;
    let longest = width.max(height);
    if longest <= MAX_DIMENSION {
        return (width, height);
    }
    let scale = f64::from(MAX_DIMENSION) / f64::from(longest);
    let w = ((f64::from(width) * scale).round() as i32).max(3);
    let h = ((f64::from(height) * scale).round() as i32).max(3);
    (w, h)
}

/// Budgets: proposals can explode to tens of thousands on busy screens;
/// candidates are what survives scoring.
const MAX_PROPOSALS: usize = 4096;
const MAX_REFINED: usize = 512;
const MAX_CANDIDATES: usize = 512;

/// One-shot region detection over a capture. Returns scored candidates
/// in original-image pixels (refined), sorted by area ascending.
fn detect_regions(view: RgbaView<'_>) -> Vec<RegionCandidate> {
    let (width, height) = (view.width, view.height);
    if width < 3 || height < 3 {
        return Vec::new();
    }

    let (analysis_w, analysis_h) = analysis_size(width, height);
    // Conditional borrow: at native resolution the pipeline reads the
    // caller's buffer directly; only a real downscale allocates.
    let downscaled;
    let analysis = if analysis_w != width || analysis_h != height {
        downscaled = downscale(&view, analysis_w, analysis_h);
        downscaled.view()
    } else {
        view
    };

    let field = build_edge_field(analysis);
    let closed = close_linf1(&field.combined);
    let mut proposals = contour_proposals(&closed);
    proposals.extend(component_proposals(&closed, false));
    proposals.extend(line_proposals(&field));

    // Background regions: everything NOT behind a dilated edge — panels
    // and split layouts bounded by edges nominate their interiors.
    let barriers = dilate_linf1(&field.combined);
    let mut background = GrayImage::new(analysis_w, analysis_h);
    for y in 0..analysis_h {
        for x in 0..analysis_w {
            background.set(x, y, if barriers.at(x, y) == 0 { 255 } else { 0 });
        }
    }
    proposals.extend(component_proposals(&background, true));

    let scale = width as f32 / analysis_w as f32;
    let min_width = ((12.0f32 / scale).ceil() as i32).max(5);
    let min_height = ((12.0f32 / scale).ceil() as i32).max(5);
    let full_area = i64::from(analysis_w) * i64::from(analysis_h);
    proposals.retain(|proposal| {
        proposal.rect.width() >= min_width
            && proposal.rect.height() >= min_height
            && proposal.rect.area() < full_area * 995 / 1000
    });

    // Keep both ends of the size spectrum: dropping only the middle
    // preserves small controls AND large panels within the budget.
    proposals.sort_by_key(|p| p.rect.area());
    if proposals.len() > MAX_PROPOSALS {
        let keep = MAX_PROPOSALS / 2;
        let mut trimmed = Vec::with_capacity(MAX_PROPOSALS);
        let split = proposals.len() - keep;
        trimmed.extend_from_slice(&proposals[..keep]);
        trimmed.extend_from_slice(&proposals[split..]);
        proposals = trimmed;
    }

    let mut candidates: Vec<RegionCandidate> = Vec::new();
    let merge_tolerance = (scale.ceil() as i32) + 1;
    for proposal in proposals {
        let rect = proposal.rect;
        let sides = border_sides(&field, rect);
        let mut sorted = sides;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let support = (sides[0] + sides[1] + sides[2] + sides[3]) / 4.;
        // A real region has SOME edge on every side (sorted[0]) and a
        // decent majority on at least three (sorted[1]).
        if sorted[1] < 0.48 || sorted[0] < 0.18 || support < 0.58 {
            continue;
        }
        // Line-pairing proposals are hypothesis-grade: demand more.
        if proposal.source.has(Sources::LINES) && (sorted[0] < 0.70 || support < 0.85) {
            continue;
        }
        let density = interior_density(&field.combined, rect);
        let inset = inset_support(&field, rect);
        let compact = rect.width().min(rect.height()) as f32 * scale < 32.0;
        // Compact + busy inside = text block or icon grid, not a panel.
        if compact && (density > 0.30 || (density > 0.12 && inset > 0.60)) {
            continue;
        }
        let texture_penalty = if compact {
            density * 0.30 + inset * 0.18
        } else {
            density * 0.08
        };
        merge_candidate(
            &mut candidates,
            RegionCandidate {
                rect: scale_rect(rect, analysis_w, analysis_h, width, height),
                quality: (support * 0.65 + sorted[0] * 0.25 + 0.10 - texture_penalty).clamp(0., 1.),
                sources: proposal.source,
            },
            merge_tolerance,
        );
    }

    // Refinement touches original pixels: afford it for the best only.
    candidates.sort_by(|a, b| {
        b.quality
            .partial_cmp(&a.quality)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(MAX_REFINED);
    let radius = ((scale.ceil() as i32) * 3).clamp(3, 16);
    for candidate in candidates.iter_mut() {
        candidate.rect = refine_rect(&view, candidate.rect, radius, field.threshold);
    }
    let mut refined: Vec<RegionCandidate> = Vec::new();
    for candidate in candidates {
        merge_candidate(&mut refined, candidate, 2);
    }
    refined
}

// ── Detector & cursor ────────────────────────────────────────────────

/// Frozen per-capture candidate index. Detection runs ONCE; hover and
/// hierarchy switching only query this. `pub` with private fields: the
/// binary passes it through opaquely from the background analysis into
/// the session.
pub struct RegionDetector {
    image_width: i32,
    image_height: i32,
    candidates: Vec<RegionCandidate>,
}

impl RegionDetector {
    /// Analyze a capture (RGBA8, `width`×`height`).
    pub fn analyze(rgba: &[u8], width: u32, height: u32) -> Self {
        let (w, h) = (width as i32, height as i32);
        let mut candidates = RgbaView::new(rgba, w, h)
            .map(detect_regions)
            .unwrap_or_default();
        // Best-first for the cap, then stable area order for hierarchy
        // queries (children before parents).
        candidates.sort_by(|a, b| {
            b.quality
                .partial_cmp(&a.quality)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.rect.area().cmp(&b.rect.area()))
        });
        candidates.truncate(MAX_CANDIDATES);
        candidates.sort_by_key(|c| c.rect.area());
        Self {
            image_width: w,
            image_height: h,
            candidates,
        }
    }

    /// Build from fixed candidates — the test injection path.
    #[cfg(test)]
    pub(crate) fn from_candidates(
        width: i32,
        height: i32,
        candidates: Vec<RegionCandidate>,
    ) -> Self {
        Self {
            image_width: width,
            image_height: height,
            candidates,
        }
    }

    fn bounds(&self) -> IntRect {
        IntRect::new(0, 0, self.image_width, self.image_height)
    }

    /// The full-image rect (hover treats it as "no hit").
    pub(crate) fn image_bounds(&self) -> IntRect {
        self.bounds()
    }

    /// How many candidates survived scoring (diagnostics logging).
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    #[cfg(test)]
    fn candidates(&self) -> &[RegionCandidate] {
        &self.candidates
    }

    /// Inject compositor window rects (already in this image's pixel
    /// space, clipped to its bounds). They join the hierarchy as
    /// high-quality parents; hover still prefers UI candidates.
    pub(crate) fn add_window_rects(&mut self, windows: &[IntRect]) {
        for window in windows {
            let clipped = IntRect::new(
                window.left.max(0),
                window.top.max(0),
                window.right.min(self.image_width),
                window.bottom.min(self.image_height),
            );
            if clipped.width() < 8 || clipped.height() < 8 {
                continue;
            }
            merge_candidate(
                &mut self.candidates,
                RegionCandidate {
                    rect: clipped,
                    quality: 0.92,
                    sources: Sources::WINDOW,
                },
                2,
            );
        }
        self.candidates.sort_by_key(|c| c.rect.area());
    }

    /// Best candidate covering a point (image px). UI candidates win
    /// over injected window rects — hovering a control inside a window
    /// should pick the control; the window remains reachable via the
    /// wheel. Falls back to the full image.
    fn region_at(&self, x: i32, y: i32) -> IntRect {
        let mut best_ui: Option<&RegionCandidate> = None;
        let mut best_window: Option<&RegionCandidate> = None;
        for candidate in &self.candidates {
            if !candidate.rect.contains_point(x, y) {
                continue;
            }
            let window = candidate.sources.has(Sources::WINDOW);
            let best = if window {
                &mut best_window
            } else {
                &mut best_ui
            };
            if best.is_none_or(|current| rank(candidate) < rank(current)) {
                *best = Some(candidate);
            }
        }
        best_ui
            .or(best_window)
            .map(|c| c.rect)
            .unwrap_or_else(|| self.bounds())
    }

    /// Smallest parent of `current` containing `point`, or the full
    /// image when there is none.
    fn parent_of(&self, current: IntRect, x: i32, y: i32) -> IntRect {
        self.candidates
            .iter()
            .filter(|c| {
                c.rect.contains_point(x, y) && c.rect != current && c.rect.contains_rect(current)
            })
            .min_by_key(|c| c.rect.area())
            .map(|c| c.rect)
            .unwrap_or_else(|| self.bounds())
    }

    /// Largest child of `current` containing `point`, or `current`.
    fn child_of(&self, current: IntRect, x: i32, y: i32) -> IntRect {
        self.candidates
            .iter()
            .filter(|c| {
                c.rect.contains_point(x, y) && c.rect != current && current.contains_rect(c.rect)
            })
            .max_by_key(|c| c.rect.area())
            .map(|c| c.rect)
            .unwrap_or(current)
    }
}

/// Ranking for `region_at`: reliable bounds first (quality ≥ 0.80),
/// then smaller rects at equal quality — the innermost solid frame
/// under the cursor.
fn rank(candidate: &RegionCandidate) -> f32 {
    let tier = if candidate.quality >= 0.80 {
        0.
    } else {
        10000.
    };
    let size = (candidate.rect.area().max(0) as f32).sqrt().sqrt();
    tier + size / candidate.quality.max(0.1).powi(2)
}

/// Hover state for the smart-selection phase: small pointer moves do
/// not jump rects, and a wheel-selected level survives clicks until the
/// pointer actually leaves.
pub(crate) struct SelectionCursor {
    current: Option<IntRect>,
    anchor: (f32, f32),
    manual_level: bool,
}

impl Default for SelectionCursor {
    fn default() -> Self {
        Self {
            current: None,
            anchor: (0., 0.),
            manual_level: false,
        }
    }
}

impl SelectionCursor {
    /// Update the hovered candidate for a pointer position (image px).
    /// Returns the current rect.
    pub(crate) fn hover(&mut self, detector: &RegionDetector, x: i32, y: i32) -> IntRect {
        let tolerance = if self.manual_level { 6. } else { 3. };
        if let Some(current) = self.current
            && current.contains_point(x, y)
        {
            let distance =
                ((x as f32 - self.anchor.0).powi(2) + (y as f32 - self.anchor.1).powi(2)).sqrt();
            if distance <= tolerance {
                return current;
            }
        }
        let rect = detector.region_at(x, y);
        self.current = Some(rect);
        self.anchor = (x as f32, y as f32);
        self.manual_level = false;
        rect
    }

    /// Wheel step: positive grows to the parent region, negative
    /// shrinks to the child. The chosen level sticks until the pointer
    /// leaves the rect (or moves beyond the manual tolerance).
    pub(crate) fn step(
        &mut self,
        detector: &RegionDetector,
        x: i32,
        y: i32,
        direction: i32,
    ) -> IntRect {
        let current = self.hover(detector, x, y);
        let rect = if direction > 0 {
            detector.parent_of(current, x, y)
        } else if direction < 0 {
            detector.child_of(current, x, y)
        } else {
            current
        };
        self.current = Some(rect);
        self.anchor = (x as f32, y as f32);
        self.manual_level = true;
        rect
    }

    /// Is the current rect a wheel-chosen level (not plain hover)?
    pub(crate) fn manual_level(&self) -> bool {
        self.manual_level
    }

    pub(crate) fn reset(&mut self) {
        self.current = None;
        self.manual_level = false;
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: keep test helpers explicit; also avoids
    // pulling gpui's test attribute shadowing (see selection.rs).
    use super::edges::{ComponentBounds, EdgeField, build_edge_field, connected_components};
    use super::image::{GrayImage, RgbaBuf, RgbaView, canny};
    use super::proposals::refine_rect;
    use super::{
        IntRect, RegionCandidate, RegionDetector, SelectionCursor, Sources, analysis_size,
        detect_regions, scale_rect,
    };

    /// A W×H RGBA image: dark background with a light panel.
    fn panel_image(w: i32, h: i32, panel: IntRect) -> RgbaBuf {
        let mut data = vec![32u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                if panel.contains_point(x, y) {
                    data[i] = 220;
                    data[i + 1] = 220;
                    data[i + 2] = 225;
                } else {
                    data[i] = 32;
                    data[i + 1] = 32;
                    data[i + 2] = 36;
                }
                data[i + 3] = 255;
            }
        }
        RgbaBuf {
            width: w,
            height: h,
            data,
        }
    }

    fn candidate(rect: IntRect, quality: f32, sources: Sources) -> RegionCandidate {
        RegionCandidate {
            rect,
            quality,
            sources,
        }
    }

    #[test]
    fn analysis_size_caps_the_long_edge() {
        assert_eq!(analysis_size(800, 600), (800, 600));
        let (w, h) = analysis_size(2560, 1440);
        assert_eq!(w.max(h), 1280);
        assert_eq!((w, h), (1280, 720));
        // portrait
        let (w, h) = analysis_size(600, 3000);
        assert_eq!((w, h), (256, 1280));
    }

    #[test]
    fn scale_rect_rounds_half_up_per_edge() {
        let rect = IntRect::new(10, 20, 300, 200);
        let scaled = scale_rect(rect, 1280, 720, 2560, 1440);
        assert_eq!(scaled, IntRect::new(20, 40, 600, 400));
    }

    #[test]
    fn connected_components_bounds_and_count() {
        let mut image = GrayImage::new(100, 100);
        for y in 10..20 {
            for x in 10..30 {
                image.set(x, y, 255);
            }
        }
        for y in 50..60 {
            for x in 70..90 {
                image.set(x, y, 128);
            }
        }
        let components: Vec<ComponentBounds> = connected_components(&image, true)
            .into_iter()
            .filter(|c| c.pixels > 0)
            .collect();
        assert_eq!(components.len(), 2);
        assert!(components.iter().any(|c| c.left == 10
            && c.top == 10
            && c.right == 30
            && c.bottom == 20
            && c.pixels == 200));
        assert!(
            components
                .iter()
                .any(|c| c.left == 70 && c.top == 50 && c.right == 90 && c.bottom == 60)
        );
    }

    #[test]
    fn canny_finds_a_step_edge() {
        let mut image = GrayImage::new(80, 60);
        for y in 0..60 {
            for x in 40..80 {
                image.set(x, y, 200);
            }
        }
        let edges = canny(&image, 12., 30.);
        let hits: Vec<(i32, i32)> = (0..60)
            .flat_map(|y| (0..80).map(move |x| (x, y)))
            .filter(|&(x, y)| edges.at(x, y) != 0)
            .collect();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|&(x, _)| (38..=42).contains(&x)));
    }

    #[test]
    fn refinement_snaps_to_the_true_edge() {
        let image = panel_image(200, 150, IntRect::new(40, 30, 160, 90));
        let estimate = IntRect::new(37, 27, 164, 93);
        let refined = refine_rect(&image.view(), estimate, 6, 8);
        assert_eq!(refined, IntRect::new(40, 30, 160, 90));
    }

    #[test]
    fn detect_regions_finds_a_solid_panel() {
        let panel = IntRect::new(40, 30, 160, 90);
        let image = panel_image(240, 160, panel);
        let candidates = detect_regions(image.view());
        assert!(!candidates.is_empty());
        assert!(
            candidates.iter().any(|c| (c.rect.left - 40).abs() <= 2
                && (c.rect.top - 30).abs() <= 2
                && (c.rect.right - 160).abs() <= 2
                && (c.rect.bottom - 90).abs() <= 2),
            "no candidate matches the panel: {candidates:?}"
        );
    }

    #[test]
    fn edge_field_threshold_stays_inside_its_clamps() {
        let image = panel_image(240, 160, IntRect::new(40, 30, 160, 90));
        let EdgeField { threshold, .. } = build_edge_field(image.view());
        assert!((5..=18).contains(&threshold), "threshold {threshold}");
    }

    #[test]
    fn region_at_prefers_ui_over_window_candidates() {
        let window = candidate(IntRect::new(0, 0, 380, 280), 0.92, Sources::WINDOW);
        let control = candidate(IntRect::new(100, 100, 200, 180), 0.85, Sources::CONTOUR);
        let detector =
            RegionDetector::from_candidates(400, 300, vec![window.clone(), control.clone()]);
        assert_eq!(detector.region_at(150, 140), control.rect);
        // outside the control: the window candidate takes over
        assert_eq!(detector.region_at(50, 50), window.rect);
        // and a total miss falls back to the full image
        assert_eq!(detector.region_at(390, 290), IntRect::new(0, 0, 400, 300));
    }

    #[test]
    fn cursor_hovers_debounces_and_steps_hierarchy() {
        let panel = candidate(IntRect::new(40, 30, 160, 90), 0.9, Sources::CONTOUR);
        let window = candidate(IntRect::new(0, 0, 240, 100), 0.92, Sources::WINDOW);
        let full = IntRect::new(0, 0, 240, 160);
        let detector = RegionDetector::from_candidates(240, 160, vec![panel.clone(), window]);
        let mut cursor = SelectionCursor::default();

        // hover inside the panel
        assert_eq!(cursor.hover(&detector, 100, 60), panel.rect);
        // small move: no jump
        assert_eq!(cursor.hover(&detector, 101, 61), panel.rect);
        // move out of the panel: the window rect
        assert_eq!(
            cursor.hover(&detector, 10, 50),
            IntRect::new(0, 0, 240, 100)
        );
        // step down into the panel again (manual level)
        assert_eq!(cursor.step(&detector, 100, 60, -1), panel.rect);
        assert!(cursor.manual_level());
        // hover within tolerance keeps the manual level
        assert_eq!(cursor.hover(&detector, 101, 61), panel.rect);
        assert!(cursor.manual_level());
        // step up: panel → window → full bounds
        assert_eq!(
            cursor.step(&detector, 100, 60, 1),
            IntRect::new(0, 0, 240, 100)
        );
        assert_eq!(cursor.step(&detector, 100, 60, 1), full);
        // reset
        cursor.reset();
        assert!(!cursor.manual_level());
    }

    #[test]
    fn window_rects_merge_into_candidates() {
        let mut detector = RegionDetector::from_candidates(
            240,
            160,
            vec![candidate(
                IntRect::new(10, 10, 100, 100),
                0.9,
                Sources::CONTOUR,
            )],
        );
        detector.add_window_rects(&[IntRect::new(0, 0, 240, 160), IntRect::new(-5, -5, 4, 4)]);
        // the tiny clipped-out rect is dropped, the window kept
        assert_eq!(detector.candidates().len(), 2);
        assert!(
            detector
                .candidates()
                .iter()
                .any(|c| c.sources.has(Sources::WINDOW))
        );
    }

    #[test]
    fn analyze_end_to_end_on_synthetic_screen() {
        // two panels on one dark screen
        let mut image = panel_image(300, 200, IntRect::new(20, 20, 140, 90));
        for y in 120..170 {
            for x in 160..280 {
                let i = (y * 300 + x) * 4;
                image.data[i] = 200;
                image.data[i + 1] = 210;
                image.data[i + 2] = 200;
            }
        }
        let detector = RegionDetector::analyze(&image.data, 300, 200);
        assert!(!detector.candidates().is_empty());
        // hovering inside the second panel must return it (not bounds)
        let rect = detector.region_at(200, 140);
        assert!(rect.left >= 155 && rect.right <= 285, "got {rect:?}");
        assert!(rect.top >= 115 && rect.bottom <= 175, "got {rect:?}");
    }

    #[test]
    fn analyze_rejects_truncated_input() {
        let detector = RegionDetector::analyze(&[0u8; 16], 100, 100);
        assert!(detector.candidates().is_empty());
        let view = RgbaView::new(&[0u8; 16], 100, 100);
        assert!(view.is_none());
    }
}
