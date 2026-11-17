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
//! - **six proposal sources** (closed contours, connected components,
//!   edge-bounded background, paired long lines, orthogonal rounded-box
//!   completion, and divider/boundary split-pane cells) so hollow rounded
//!   cards, small controls, screen-flush bars/sidebars, and T-junction
//!   tiled panes all get nominated;
//! - **corner-aware border-support scoring** — a candidate must have
//!   edges along all four sides — plus texture penalties that reject
//!   unbordered text blocks and icon grids while preserving labeled
//!   bordered controls;
//! - **sub-pixel-free refinement** on the ORIGINAL resolution: each side
//!   searches ±radius px for the crispest high-contrast boundary,
//!   undoing downscale blur without snapping onto soft drop shadows.
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
    const SPLIT: Self = Self(1 << 5);

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
    line_proposals, merge_candidate, quad_proposals, refine_rect, scale_rect, split_pane_proposals,
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
    proposals.extend(quad_proposals(&field));
    proposals.extend(split_pane_proposals(&field));

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
        let on_boundary = [
            rect.left <= 1 || rect.left + 2 >= analysis_w,
            rect.top <= 1 || rect.top + 2 >= analysis_h,
            (rect.right - 1) <= 1 || (rect.right - 1) + 2 >= analysis_w,
            (rect.bottom - 1) <= 1 || (rect.bottom - 1) + 2 >= analysis_h,
        ];
        let boundary_sides = on_boundary.into_iter().filter(|&b| b).count();
        if boundary_sides == 4 {
            continue;
        }
        let min_interior_side = sides
            .into_iter()
            .zip(on_boundary)
            .filter_map(|(s, is_bound)| (!is_bound).then_some(s))
            .fold(1.0f32, f32::min);
        let mut sorted = sides;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let support = (sides[0] + sides[1] + sides[2] + sides[3]) / 4.;
        // A real region has SOME edge on every side (sorted[0]) and a
        // decent majority on at least three (sorted[1]).
        if sorted[1] < 0.48 || sorted[0] < 0.18 || support < 0.58 {
            continue;
        }
        // Boundary-score dilution guard: `side_support` grants capture
        // boundaries a free 0.85 so screen-flush bars and tiled panes do
        // not lose points for edges outside the screenshot. When 2 or 3
        // sides sit on capture boundaries, those free 0.85 scores inflate
        // both `sorted[1]` and the 4-side mean (e.g. `[0.85, 0.85, 0.85,
        // 0.20]` yields `support = 0.69`), which would let a short floating
        // rule near the middle of the screen hallucinate a half-screen box.
        // Require every *interior* side to show continuous directional
        // support (>= 0.80), and enforce the same bar on divider/boundary
        // split-pane cells regardless of boundary count.
        if (boundary_sides >= 2 || proposal.source.has(Sources::SPLIT)) && min_interior_side < 0.80
        {
            continue;
        }
        // Line-pairing proposals are hypothesis-grade: demand more.
        if proposal.source.has(Sources::LINES) && (sorted[0] < 0.70 || support < 0.85) {
            continue;
        }
        let density = interior_density(&field.combined, rect);
        let inset = inset_support(&field, rect);
        let min_dim_px = rect.width().min(rect.height()) as f32 * scale;
        let max_dim_px = rect.width().max(rect.height()) as f32 * scale;
        // Screen- and pane-spanning bars (e.g. a 28..30 px Waybar or IDE
        // statusbar/activity rail) fall under `min_dim_px < 32` yet often
        // contain 4px-padded workspace pills whose borders land right on
        // `inset_support`'s 4px sample ring. Once `Sources::SPLIT` has
        // proven >= 0.80 continuous dividers end-to-end, score elongated
        // structural bars as panels rather than compact text words.
        let structural_bar = proposal.source.has(Sources::SPLIT)
            && min_dim_px >= 18.0
            && max_dim_px >= 180.0
            && max_dim_px >= min_dim_px * 5.0;
        let compact = min_dim_px < 32.0 && !structural_bar;
        // Distinguish an unbordered text/icon blob from a crisp 4-sided UI
        // control (e.g. a 24..30 px `[ Save ]` button or text input whose
        // label glyphs raise interior edge density without hugging all four
        // outer walls like a tight-cropped word).
        let crisp_control =
            min_dim_px >= 20.0 && support >= 0.80 && sorted[0] >= 0.65 && inset < 0.55;
        if compact {
            let text_blob = if crisp_control {
                density > 0.55
            } else {
                density > 0.30 || (density > 0.12 && inset > 0.60)
            };
            if text_blob {
                continue;
            }
        } else if support < 0.78 && (density > 0.28 || (density > 0.16 && inset > 0.45)) {
            // Multi-line prose paragraphs and unbordered icon grids exceed
            // `compact` (`>= 32 px`), yet lack a solid 4-sided frame while
            // staying textured both across the interior and 4 px inside.
            continue;
        }
        let texture_penalty = if compact {
            if crisp_control {
                density * 0.10 + inset * 0.05
            } else {
                density * 0.30 + inset * 0.18
            }
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
    let radius = ((scale.ceil() as i32) * 3).clamp(6, 16);
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

    /// Smallest parent of `current` containing `point` that is not a mere
    /// concentric border twin, or the full image when none exists before
    /// `self.bounds()`.
    fn parent_of(&self, current: IntRect, x: i32, y: i32) -> IntRect {
        let bounds = self.bounds();
        let find_parent = |skip_twins: bool| {
            self.candidates
                .iter()
                .filter(|c| {
                    c.rect.contains_point(x, y)
                        && c.rect != current
                        && c.rect.contains_rect(current)
                        && (!skip_twins || !is_border_twin(current, c.rect))
                })
                .min_by_key(|c| c.rect.area())
                .map(|c| c.rect)
        };
        if let Some(parent) = find_parent(true) {
            return parent;
        }
        if !is_border_twin(current, bounds) {
            return bounds;
        }
        find_parent(false).unwrap_or(bounds)
    }

    /// Largest child of `current` containing `point`, skipping concentric
    /// border twins whenever a genuinely distinct descendant exists.
    fn child_of(&self, current: IntRect, x: i32, y: i32) -> IntRect {
        let find_child = |skip_twins: bool| {
            self.candidates
                .iter()
                .filter(|c| {
                    c.rect.contains_point(x, y)
                        && c.rect != current
                        && current.contains_rect(c.rect)
                        && (!skip_twins || !is_border_twin(current, c.rect))
                })
                .max_by_key(|c| c.rect.area())
                .map(|c| c.rect)
        };
        find_child(true)
            .or_else(|| find_child(false))
            .unwrap_or(current)
    }
}

/// Two rects are concentric border twins when all four sides sit within
/// 4 image px (e.g. a compositor `WINDOW` rect or outer 2..4 px border
/// surviving `merge_candidate(..., 2)` around an inner client rect) and
/// their areas differ by less than 10% (preserving small controls inside
/// tight wrappers).
fn is_border_twin(a: IntRect, b: IntRect) -> bool {
    const MAX_SIDE_DIFF: i32 = 4;
    let (area_a, area_b) = (a.area(), b.area());
    let max_area = area_a.max(area_b);
    if max_area <= 0 {
        return false;
    }
    (a.left - b.left).abs() <= MAX_SIDE_DIFF
        && (a.top - b.top).abs() <= MAX_SIDE_DIFF
        && (a.right - b.right).abs() <= MAX_SIDE_DIFF
        && (a.bottom - b.bottom).abs() <= MAX_SIDE_DIFF
        && (max_area - area_a.min(area_b)) * 100 < max_area * 10
}

/// Ranking for `region_at`: reliable bounds first (quality ≥ 0.72),
/// then smaller rects at equal quality — the innermost solid frame
/// under the cursor, so labeled compact buttons/inputs beat a 20×
/// larger enclosing panel while weak hypotheses stay fallback-only.
fn rank(candidate: &RegionCandidate) -> f32 {
    let tier = if candidate.quality >= 0.72 {
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
        // Wheel-stepped parent selections get 12 image px (~6 logical px on
        // 2× HiDPI) of jitter slack so click tremors do not snap back to an
        // inner child while the pointer stays inside `current`.
        let tolerance = if self.manual_level { 12. } else { 3. };
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

    fn solid_rgba(w: i32, h: i32, rgb: [u8; 3]) -> RgbaBuf {
        RgbaBuf {
            width: w,
            height: h,
            data: [rgb[0], rgb[1], rgb[2], 255].repeat((w * h) as usize),
        }
    }

    fn set_px(buf: &mut RgbaBuf, x: i32, y: i32, rgb: [u8; 3]) {
        if x < 0 || y < 0 || x >= buf.width || y >= buf.height {
            return;
        }
        let i = ((y * buf.width + x) * 4) as usize;
        buf.data[i] = rgb[0];
        buf.data[i + 1] = rgb[1];
        buf.data[i + 2] = rgb[2];
        buf.data[i + 3] = 255;
    }

    /// Antialiased 1px rounded-rectangle stroke along the centerline of the
    /// outer pixel ring of `rect`, leaving rounded corners open while axis-
    /// aligned straight spans remain crisp 1px borders.
    fn stroke_rounded_rect(buf: &mut RgbaBuf, rect: IntRect, radius: f32, stroke: [u8; 3]) {
        let x0 = rect.left as f32 + 0.5;
        let y0 = rect.top as f32 + 0.5;
        let x1 = rect.right as f32 - 0.5;
        let y1 = rect.bottom as f32 - 0.5;
        let r = (radius - 0.5).max(0.5);
        let cx = (x0 + x1) * 0.5;
        let cy = (y0 + y1) * 0.5;
        let hx = (x1 - x0) * 0.5 - r;
        let hy = (y1 - y0) * 0.5 - r;
        for y in (rect.top - 2).max(0)..=(rect.bottom + 2).min(buf.height - 1) {
            for x in (rect.left - 2).max(0)..=(rect.right + 2).min(buf.width - 1) {
                let dx = (x as f32 + 0.5 - cx).abs() - hx;
                let dy = (y as f32 + 0.5 - cy).abs() - hy;
                let sdf = dx.max(0.).hypot(dy.max(0.)) + dx.max(dy).min(0.) - r;
                let alpha = (1.0 - sdf.abs()).clamp(0.0, 1.0);
                if alpha <= 0.0 {
                    continue;
                }
                let i = ((y * buf.width + x) * 4) as usize;
                for c in 0..3 {
                    let base = f32::from(buf.data[i + c]);
                    let fg = f32::from(stroke[c]);
                    buf.data[i + c] = (base * (1.0 - alpha) + fg * alpha).round() as u8;
                }
            }
        }
    }

    fn assert_rect_near(actual: IntRect, expected: IntRect, tol: i32) {
        assert!(
            (actual.left - expected.left).abs() <= tol
                && (actual.top - expected.top).abs() <= tol
                && (actual.right - expected.right).abs() <= tol
                && (actual.bottom - expected.bottom).abs() <= tol,
            "expected {expected:?} (±{tol} px), got {actual:?}"
        );
    }

    #[test]
    fn detects_hollow_rounded_border_card_on_same_background() {
        // #f8fafc page with a 1px #cbd5e1 rounded card (r = 12 px) and an
        // identical #f8fafc interior; also verify a compact 32 px-tall pill
        // input (r = 10 px) whose corner arcs consume >30% of vertical sides.
        let bg = [0xf8, 0xfa, 0xfc];
        let border = [0xcb, 0xd5, 0xe1];
        let card = IntRect::new(50, 40, 310, 200);
        let mut image = solid_rgba(360, 240, bg);
        stroke_rounded_rect(&mut image, card, 12.0, border);

        let detector = RegionDetector::analyze(&image.data, 360, 240);
        assert!(
            detector.candidates().iter().any(|c| {
                (c.rect.left - card.left).abs() <= 2
                    && (c.rect.top - card.top).abs() <= 2
                    && (c.rect.right - card.right).abs() <= 2
                    && (c.rect.bottom - card.bottom).abs() <= 2
            }),
            "card missing from candidates: {:?}",
            detector.candidates()
        );
        assert_rect_near(detector.region_at(180, 120), card, 2);

        let pill_input = IntRect::new(90, 96, 250, 128);
        let mut pill_scene = solid_rgba(340, 220, bg);
        stroke_rounded_rect(&mut pill_scene, pill_input, 10.0, border);
        let pill_detector = RegionDetector::analyze(&pill_scene.data, 340, 220);
        assert_rect_near(pill_detector.region_at(170, 112), pill_input, 2);
    }

    #[test]
    fn refinement_prefers_crisp_panel_edge_over_drop_shadow() {
        // Bright panel [60, 50, 260, 170] surrounded by an 8 px dark drop
        // shadow whose steepest band (3..4 px outside) has per-pixel diff ≈ 52.
        let panel = IntRect::new(60, 50, 260, 170);
        let mut image = solid_rgba(320, 220, [214, 218, 224]);
        let shadow_luma = [58u8, 68, 86, 138, 170, 190, 203, 210];
        for y in 0..220 {
            for x in 0..320 {
                if panel.contains_point(x, y) {
                    set_px(&mut image, x, y, [248, 250, 252]);
                    continue;
                }
                let dx = (panel.left - x).max(x - (panel.right - 1)).max(0);
                let dy = (panel.top - y).max(y - (panel.bottom - 1)).max(0);
                let dist = dx.max(dy);
                if (1..=8).contains(&dist) {
                    let v = shadow_luma[(dist - 1) as usize];
                    set_px(&mut image, x, y, [v, v, v.saturating_add(4)]);
                }
            }
        }

        let seeded_on_shadow = IntRect::new(57, 47, 263, 173);
        let refined = refine_rect(&image.view(), seeded_on_shadow, 6, 8);
        assert_rect_near(refined, panel, 1);

        let detector = RegionDetector::analyze(&image.data, 320, 220);
        assert_rect_near(detector.region_at(160, 110), panel, 1);
        // Stepping up from the panel must go straight to the full screen
        // rather than stopping on a leftover drop-shadow halo candidate.
        let mut cursor = SelectionCursor::default();
        assert_rect_near(cursor.hover(&detector, 160, 110), panel, 1);
        assert_eq!(
            cursor.step(&detector, 160, 110, 1),
            IntRect::new(0, 0, 320, 220)
        );
    }

    #[test]
    fn hover_prefers_labeled_button_inside_panel() {
        let full = IntRect::new(0, 0, 420, 280);
        let panel = IntRect::new(20, 20, 400, 260);
        let button = IntRect::new(60, 60, 156, 90);
        let rounded_button = IntRect::new(200, 60, 296, 90);
        let mut image = solid_rgba(420, 280, [28, 32, 40]);

        for y in panel.top..panel.bottom {
            for x in panel.left..panel.right {
                set_px(&mut image, x, y, [246, 248, 250]);
            }
        }
        for y in button.top..button.bottom {
            for x in button.left..button.right {
                let on_border = x == button.left
                    || x == button.right - 1
                    || y == button.top
                    || y == button.bottom - 1;
                let color = if on_border {
                    [100, 116, 139]
                } else {
                    [232, 236, 242]
                };
                set_px(&mut image, x, y, color);
            }
        }
        stroke_rounded_rect(&mut image, rounded_button, 8.0, [148, 163, 184]);

        // Faux glyph strokes inside y = 70..80 (six character cells with stems
        // and crossbars so interior texture matches a real `[ Submit ]` label).
        let ink = [30, 41, 59];
        for base_x in [74, 214] {
            for glyph in 0..6 {
                let gx = base_x + glyph * 11;
                for y in 70..80 {
                    set_px(&mut image, gx, y, ink);
                    set_px(&mut image, gx + 1, y, ink);
                    if glyph % 2 == 0 {
                        set_px(&mut image, gx + 6, y, ink);
                    }
                }
                for x in gx..gx + 7 {
                    set_px(&mut image, x, 70, ink);
                    set_px(&mut image, x, 75, ink);
                    set_px(&mut image, x, 79, ink);
                }
            }
        }

        // Unbordered body label directly on the panel at y = 144..156: plain
        // text words without a control frame must still be rejected so hover
        // over body copy selects the enclosing panel, not a glyph cluster.
        for glyph in 0..8 {
            let gx = 74 + glyph * 10;
            for y in 144..156 {
                set_px(&mut image, gx, y, ink);
                if glyph % 2 == 0 {
                    set_px(&mut image, gx + 5, y, ink);
                }
            }
            for x in gx..gx + 6 {
                set_px(&mut image, x, 144, ink);
                set_px(&mut image, x, 150, ink);
                set_px(&mut image, x, 155, ink);
            }
        }

        let detector = RegionDetector::analyze(&image.data, 420, 280);
        assert_rect_near(detector.region_at(100, 75), button, 2);
        assert_rect_near(detector.region_at(248, 75), rounded_button, 2);
        assert_rect_near(detector.region_at(110, 150), panel, 2);
        assert_rect_near(detector.region_at(250, 150), panel, 2);

        let mut cursor = SelectionCursor::default();
        assert_rect_near(cursor.hover(&detector, 100, 75), button, 2);
        let stepped_panel = cursor.step(&detector, 100, 75, 1);
        assert_rect_near(stepped_panel, panel, 2);
        assert!(cursor.manual_level());
        // HiDPI click/scroll tremor (~9 px > old 6 px cap, <= 12 px) keeps
        // the wheel-chosen parent instead of snapping back to the button.
        assert_eq!(cursor.hover(&detector, 108, 79), stepped_panel);
        assert!(cursor.manual_level());
        assert_eq!(cursor.step(&detector, 108, 79, 1), full);
    }

    fn fill_rect(buf: &mut RgbaBuf, rect: IntRect, rgb: [u8; 3]) {
        for y in rect.top.max(0)..rect.bottom.min(buf.height) {
            for x in rect.left.max(0)..rect.right.min(buf.width) {
                set_px(buf, x, y, rgb);
            }
        }
    }

    /// Paint dense synthetic glyph rows across `rect` (8 px tall glyph cells
    /// on an 11 px vertical pitch) so dilated `barriers` cover >55% of the
    /// pane interior and drop `background` connected-component fill well
    /// below the 0.72 threshold.
    fn paint_dense_text_block(buf: &mut RgbaBuf, rect: IntRect, ink: [u8; 3]) {
        let mut row_idx = 0i32;
        let mut y0 = rect.top + 6;
        while y0 + 8 <= rect.bottom - 4 {
            let indent = (row_idx % 3) * 8;
            let right_trim = ((row_idx * 17) % 36) + 6;
            let mut gx = rect.left + 8 + indent;
            let mut glyph = 0i32;
            while gx + 6 <= rect.right - right_trim {
                // Skip every 6th cell as a word space so lines mirror real
                // code/prose rather than continuous rules.
                if (glyph + row_idx) % 6 != 4 {
                    for y in y0..y0 + 8 {
                        set_px(buf, gx, y, ink);
                        if glyph % 2 == 0 {
                            set_px(buf, gx + 4, y, ink);
                        }
                    }
                    for x in gx..gx + 5 {
                        set_px(buf, x, y0, ink);
                        if glyph % 3 != 1 {
                            set_px(buf, x, y0 + 4, ink);
                        }
                        set_px(buf, x, y0 + 7, ink);
                    }
                }
                gx += 8;
                glyph += 1;
            }
            y0 += 11;
            row_idx += 1;
        }
    }

    #[test]
    fn detects_populated_waybar_and_dense_tiled_split_panes() {
        // 800×300 desktop: 30 px screen-flush Waybar `[0, 800) × [0, 30)`
        // with 4px-padded workspace/status pills, internal separators, and
        // clock glyphs, over two T-junction tiled panes `[0, 400) × [30, 300)`
        // and `[400, 800) × [30, 300)` packed with dense code/terminal text.
        let (w, h) = (800, 300);
        let waybar = IntRect::new(0, 0, 800, 30);
        let left_pane = IntRect::new(0, 30, 400, 300);
        let right_pane = IntRect::new(400, 30, 800, 300);

        let mut image = solid_rgba(w, h, [22, 24, 33]);
        fill_rect(&mut image, waybar, [20, 22, 30]);
        fill_rect(&mut image, left_pane, [30, 34, 46]);
        fill_rect(&mut image, right_pane, [244, 246, 250]);
        // 1px bar bottom rule + 1px vertical splitter meeting at a T-junction.
        fill_rect(&mut image, IntRect::new(0, 29, 800, 30), [72, 78, 96]);
        fill_rect(&mut image, IntRect::new(400, 30, 401, 300), [84, 92, 112]);

        // Waybar pills at y = 4..26 (landing directly on `inset_support`'s
        // 4 px ring) plus internal separators and unbordered title/clock text.
        for &(x0, x1, fill) in &[
            (6, 94, [48, 56, 78]),
            (102, 186, [38, 44, 62]),
            (690, 794, [44, 50, 70]),
        ] {
            let pill = IntRect::new(x0, 4, x1, 26);
            fill_rect(&mut image, pill, fill);
            for gx in (x0 + 8..x1 - 8).step_by(9) {
                for y in 9..21 {
                    set_px(&mut image, gx, y, [226, 232, 240]);
                }
                for x in gx..gx + 5 {
                    set_px(&mut image, x, 9, [226, 232, 240]);
                    set_px(&mut image, x, 20, [226, 232, 240]);
                }
            }
        }
        // Internal bar separators and dense center window-title glyphs.
        fill_rect(&mut image, IntRect::new(200, 5, 201, 25), [90, 98, 120]);
        fill_rect(&mut image, IntRect::new(676, 5, 677, 25), [90, 98, 120]);
        for gx in (220..650).step_by(8) {
            if gx % 48 == 4 {
                continue;
            }
            for y in 9..21 {
                set_px(&mut image, gx, y, [210, 216, 228]);
            }
            for x in gx..gx + 5 {
                set_px(&mut image, x, 9, [210, 216, 228]);
                set_px(&mut image, x, 15, [210, 216, 228]);
            }
        }

        paint_dense_text_block(&mut image, left_pane, [214, 222, 236]);
        paint_dense_text_block(&mut image, right_pane, [36, 42, 56]);

        // Verify the test scene actually breaks `background` fill (< 0.72) so
        // `component_proposals(&background, true)` alone cannot rescue the panes.
        let field = build_edge_field(image.view());
        let barriers = super::image::dilate_linf1(&field.combined);
        for pane in [left_pane, right_pane] {
            let mut clear = 0i64;
            for y in pane.top..pane.bottom {
                for x in pane.left..pane.right {
                    clear += i64::from(barriers.at(x, y) == 0);
                }
            }
            let fill_ratio = clear as f64 / pane.area() as f64;
            assert!(
                fill_ratio < 0.55,
                "expected dense text to break background fill (< 0.55), got {fill_ratio:.3}"
            );
        }

        let detector = RegionDetector::analyze(&image.data, w as u32, h as u32);
        assert_rect_near(detector.region_at(400, 15), waybar, 2);
        assert_rect_near(detector.region_at(200, 160), left_pane, 2);
        assert_rect_near(detector.region_at(600, 160), right_pane, 2);

        let mut cursor = SelectionCursor::default();
        assert_rect_near(cursor.hover(&detector, 200, 160), left_pane, 2);
        let parent = cursor.step(&detector, 200, 160, 1);
        assert!(
            parent == IntRect::new(0, 30, 800, 300)
                || (parent.left == 0
                    && (parent.top - 30).abs() <= 2
                    && parent.right == 800
                    && parent.bottom == 300)
                || parent == IntRect::new(0, 0, w, h),
            "unexpected parent step from left tiled pane: {parent:?}"
        );
    }

    #[test]
    fn detects_ide_t_junction_sidebar_and_split_editors() {
        // 840×520 IDE layout:
        // - top toolbar:       [0, 840) × [0, 34)
        // - left file sidebar: [0, 220) × [34, 494)
        // - top-left editor:   [220, 530) × [34, 340)
        // - top-right editor:  [530, 840) × [34, 340)
        // - bottom terminal:   [220, 840) × [340, 494)
        // - bottom statusbar:  [0, 840) × [494, 520)
        let (w, h) = (840, 520);
        let toolbar = IntRect::new(0, 0, 840, 34);
        let sidebar = IntRect::new(0, 34, 220, 494);
        let editor_left = IntRect::new(220, 34, 530, 340);
        let editor_right = IntRect::new(530, 34, 840, 340);
        let terminal = IntRect::new(220, 340, 840, 494);
        let statusbar = IntRect::new(0, 494, 840, 520);

        let mut image = solid_rgba(w, h, [30, 33, 42]);
        fill_rect(&mut image, toolbar, [24, 26, 34]);
        fill_rect(&mut image, sidebar, [27, 30, 39]);
        fill_rect(&mut image, editor_left, [33, 37, 48]);
        fill_rect(&mut image, editor_right, [33, 37, 48]);
        fill_rect(&mut image, terminal, [22, 25, 33]);
        fill_rect(&mut image, statusbar, [38, 74, 132]);

        // Structural 1px splitters forming nested T-junctions on both axes.
        let rule = [68, 74, 92];
        fill_rect(&mut image, IntRect::new(0, 34, 840, 35), rule);
        fill_rect(&mut image, IntRect::new(0, 493, 840, 494), rule);
        fill_rect(&mut image, IntRect::new(220, 34, 221, 494), rule);
        fill_rect(&mut image, IntRect::new(220, 340, 840, 341), rule);
        fill_rect(&mut image, IntRect::new(530, 34, 531, 340), rule);

        paint_dense_text_block(&mut image, sidebar, [178, 186, 204]);
        paint_dense_text_block(&mut image, editor_left, [212, 220, 234]);
        paint_dense_text_block(&mut image, editor_right, [212, 220, 234]);
        paint_dense_text_block(&mut image, terminal, [168, 218, 186]);

        let detector = RegionDetector::analyze(&image.data, w as u32, h as u32);
        assert_rect_near(detector.region_at(420, 17), toolbar, 2);
        assert_rect_near(detector.region_at(110, 240), sidebar, 2);
        assert_rect_near(detector.region_at(375, 180), editor_left, 2);
        assert_rect_near(detector.region_at(685, 180), editor_right, 2);
        assert_rect_near(detector.region_at(530, 420), terminal, 2);
        assert_rect_near(detector.region_at(420, 507), statusbar, 2);
    }

    #[test]
    fn isolated_floating_dividers_do_not_hallucinate_screen_panels() {
        let (w, h) = (800, 600);
        let full = IntRect::new(0, 0, w, h);

        // 1. Floating horizontal & vertical rules in open space plus a half-
        //    width rule anchored to ONLY the left screen edge.
        let mut rules_scene = solid_rgba(w, h, [246, 248, 250]);
        fill_rect(
            &mut rules_scene,
            IntRect::new(180, 150, 620, 152),
            [80, 90, 110],
        );
        fill_rect(
            &mut rules_scene,
            IntRect::new(520, 260, 522, 510),
            [80, 90, 110],
        );
        fill_rect(
            &mut rules_scene,
            IntRect::new(0, 420, 360, 422),
            [80, 90, 110],
        );

        let detector = RegionDetector::analyze(&rules_scene.data, w as u32, h as u32);
        assert_eq!(detector.region_at(400, 80), full);
        assert_eq!(detector.region_at(400, 220), full);
        assert_eq!(detector.region_at(200, 360), full);
        assert_eq!(detector.region_at(200, 500), full);
        assert_eq!(detector.region_at(660, 380), full);

        // 2. Unbordered prose paragraphs and an unbordered icon grid on open
        //    background must not produce half-screen or grid-frame boxes.
        let mut prose_scene = solid_rgba(w, h, [250, 250, 252]);
        paint_dense_text_block(
            &mut prose_scene,
            IntRect::new(80, 60, 720, 240),
            [32, 38, 48],
        );
        for row in 0..3 {
            for col in 0..5 {
                let ix = 140 + col * 110;
                let iy = 320 + row * 72;
                for dy in 0..16 {
                    set_px(&mut prose_scene, ix + dy, iy + dy, [45, 55, 72]);
                    set_px(&mut prose_scene, ix + 15 - dy, iy + dy, [45, 55, 72]);
                }
            }
        }
        let prose_detector = RegionDetector::analyze(&prose_scene.data, w as u32, h as u32);
        assert_eq!(prose_detector.region_at(400, 150), full);
        assert_eq!(prose_detector.region_at(310, 360), full);
    }

    #[test]
    fn detects_downscaled_three_column_t_junction_panes() {
        // 1600×900 (> 1280 long-edge cap, exercising downscale + scale_rect +
        // refine_rect) with a 32 px top bar and three T-junction columns
        // `[0, 260) × [32, 900)`, `[260, 1100) × [32, 900)`, and
        // `[1100, 1600) × [32, 900)`.
        let (w, h) = (1600, 900);
        let top_bar = IntRect::new(0, 0, 1600, 32);
        let col_left = IntRect::new(0, 32, 260, 900);
        let col_mid = IntRect::new(260, 32, 1100, 900);
        let col_right = IntRect::new(1100, 32, 1600, 900);

        let mut image = solid_rgba(w, h, [24, 27, 36]);
        fill_rect(&mut image, top_bar, [18, 20, 28]);
        fill_rect(&mut image, col_left, [28, 32, 43]);
        fill_rect(&mut image, col_mid, [34, 39, 52]);
        fill_rect(&mut image, col_right, [26, 30, 40]);
        fill_rect(&mut image, IntRect::new(0, 32, 1600, 34), [76, 84, 104]);
        fill_rect(&mut image, IntRect::new(260, 32, 262, 900), [76, 84, 104]);
        fill_rect(&mut image, IntRect::new(1100, 32, 1102, 900), [76, 84, 104]);

        paint_dense_text_block(&mut image, col_left, [196, 206, 224]);
        paint_dense_text_block(&mut image, col_mid, [214, 222, 236]);
        paint_dense_text_block(&mut image, col_right, [188, 214, 198]);

        let detector = RegionDetector::analyze(&image.data, w as u32, h as u32);
        assert_rect_near(detector.region_at(800, 16), top_bar, 2);
        assert_rect_near(detector.region_at(130, 450), col_left, 2);
        assert_rect_near(detector.region_at(680, 450), col_mid, 2);
        assert_rect_near(detector.region_at(1350, 450), col_right, 2);
    }

    #[test]
    fn hierarchy_step_skips_concentric_border_twins() {
        let full = IntRect::new(0, 0, 800, 600);
        let button = candidate(IntRect::new(140, 140, 220, 172), 0.88, Sources::CONTOUR);
        let client = candidate(IntRect::new(100, 100, 500, 400), 0.90, Sources::CONTOUR);
        let window_twin = candidate(IntRect::new(98, 98, 502, 402), 0.92, Sources::WINDOW);
        let outer_border_twin = candidate(IntRect::new(96, 96, 504, 404), 0.86, Sources::CONTOUR);

        let detector = RegionDetector::from_candidates(
            800,
            600,
            vec![
                button.clone(),
                client.clone(),
                window_twin,
                outer_border_twin.clone(),
            ],
        );
        let mut cursor = SelectionCursor::default();

        assert_eq!(cursor.hover(&detector, 180, 156), button.rect);
        // First notch up lands on the panel; second notch skips both 2 px and
        // 4 px concentric twins straight to full-screen bounds.
        assert_eq!(cursor.step(&detector, 180, 156, 1), client.rect);
        assert_eq!(cursor.step(&detector, 180, 156, 1), full);

        // Stepping back down from full bounds lands on the outermost window
        // frame, then skips the inner border twins straight to the button.
        assert_eq!(cursor.step(&detector, 180, 156, -1), outer_border_twin.rect);
        assert_eq!(cursor.step(&detector, 180, 156, -1), button.rect);

        // Also verify when a distinct enclosing workspace container exists
        // before `full`: the twin is skipped in favor of that container.
        let workspace = candidate(IntRect::new(40, 32, 760, 568), 0.91, Sources::SPLIT);
        let nested_detector = RegionDetector::from_candidates(
            800,
            600,
            vec![
                button.clone(),
                client.clone(),
                candidate(IntRect::new(97, 97, 503, 403), 0.92, Sources::WINDOW),
                workspace.clone(),
            ],
        );
        cursor.reset();
        assert_eq!(cursor.hover(&nested_detector, 180, 156), button.rect);
        assert_eq!(cursor.step(&nested_detector, 180, 156, 1), client.rect);
        assert_eq!(cursor.step(&nested_detector, 180, 156, 1), workspace.rect);
        assert_eq!(cursor.step(&nested_detector, 180, 156, 1), full);
    }
}
