//! CLI-driven pre-selections (`--full`, `--screen`, `--output`, `--region`,
//! `--window`) and initial annotation tool activation (`--tool`).

use gpui_kit::*;

use super::ScreenshotSession;
use crate::annotation::ShapeKind;
use crate::args::ToolArg;
use crate::model::selection::Selection;

impl ScreenshotSession {
    /// Connected output names in session registration order.
    pub fn output_names(&self) -> Vec<&str> {
        self.screens
            .iter()
            .map(|s| s.capture.output_name.as_str())
            .collect()
    }

    /// Output currently hosting the selection's chrome (toolbar / size label),
    /// or `None` while idle.
    pub fn active_output_name(&self) -> Option<&str> {
        self.active_output.as_deref()
    }

    /// Union of every screen's logical bounds.
    pub fn desktop_logical_bounds(&self) -> Option<Bounds<Pixels>> {
        self.desktop_bounds()
    }

    /// Which connected output holds the largest area of `bounds` (`None` if
    /// `bounds` does not overlap any screen).
    pub fn dominant_output_for(&self, bounds: Bounds<Pixels>) -> Option<&str> {
        self.screens
            .iter()
            .filter_map(|s| {
                let o = s.bounds().intersect(&bounds);
                let area = f32::from(o.size.width) * f32::from(o.size.height);
                (area > 0.0).then_some((s.capture.output_name.as_str(), area))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(name, _)| name)
    }

    /// Select the union of every screen's bounds, hosting the toolbar on
    /// `preferred_host` when it names a registered screen (falling back to the
    /// first screen). On multi-monitor desktops this puts the annotation
    /// toolbar on the user's focused monitor instead of whichever output the
    /// Wayland registry listed first.
    pub fn select_all_on(&mut self, preferred_host: Option<&str>) {
        let Some(bounds) = self.desktop_bounds() else {
            return;
        };
        let host = preferred_host
            .filter(|name| self.screens.iter().any(|s| s.capture.output_name == *name))
            .map(str::to_owned)
            .or_else(|| self.screens.first().map(|s| s.capture.output_name.clone()))
            .unwrap_or_default();
        self.finish_nudge();
        self.cancel_selection_gesture();
        self.hovered = None;
        self.toolbar_pos = None;
        self.set_active_output(&host);
        self.selection = Selection::Selected { bounds };
        self.apply_pending_tool();
    }

    /// Select one screen's full logical bounds by output name. Returns `false`
    /// when `name` is not among the session's screens.
    pub fn select_output(&mut self, name: &str) -> bool {
        let Some(bounds) = self
            .screens
            .iter()
            .find(|s| s.capture.output_name == name)
            .map(|s| s.bounds())
        else {
            return false;
        };
        self.finish_nudge();
        self.cancel_selection_gesture();
        self.hovered = None;
        self.toolbar_pos = None;
        self.set_active_output(name);
        self.selection = Selection::Selected { bounds };
        self.apply_pending_tool();
        true
    }

    /// Select an explicit global logical rectangle (`--region`, `--window`),
    /// clamped to the desktop union. Returns `false` if `bounds` has
    /// non-positive size or zero overlap with every connected output.
    pub fn select_bounds(&mut self, bounds: Bounds<Pixels>) -> bool {
        let Some(desk) = self.desktop_bounds() else {
            return false;
        };
        let clipped = bounds.intersect(&desk);
        if f32::from(clipped.size.width) < 1.0 || f32::from(clipped.size.height) < 1.0 {
            return false;
        }
        let Some(host) = self.dominant_output_for(clipped).map(str::to_owned) else {
            return false;
        };
        self.finish_nudge();
        self.cancel_selection_gesture();
        self.hovered = None;
        self.toolbar_pos = None;
        self.set_active_output(&host);
        self.selection = Selection::Selected { bounds: clipped };
        self.apply_pending_tool();
        true
    }

    /// Arm an annotation tool requested via `--tool`. When a selection is
    /// already finalized, activates the tool immediately; otherwise queues it
    /// so the user's first selection drag activates it on release (surviving
    /// the `annotations.reset()` in `begin`).
    pub fn arm_initial_tool(&mut self, tool: ToolArg) {
        let kind = shape_kind_for_arg(tool);
        if self.selection.is_selected() {
            self.pending_tool = None;
            self.activate_tool_kind(kind);
        } else {
            self.pending_tool = Some(kind);
        }
    }

    pub(super) fn apply_pending_tool(&mut self) {
        if self.selection.is_selected()
            && let Some(kind) = self.pending_tool.take()
        {
            self.activate_tool_kind(kind);
        }
    }

    fn activate_tool_kind(&mut self, kind: ShapeKind) {
        if self.annotations.tool() != Some(kind) {
            self.edit_annotations(|a| a.toggle(kind));
        }
    }
}

fn shape_kind_for_arg(tool: ToolArg) -> ShapeKind {
    match tool {
        ToolArg::Select => ShapeKind::Select,
        ToolArg::Rectangle => ShapeKind::Rectangle,
        ToolArg::Ellipse => ShapeKind::Ellipse,
        ToolArg::Line => ShapeKind::Line,
        ToolArg::Polyline => ShapeKind::Polyline,
        ToolArg::Arrow => ShapeKind::Arrow,
        ToolArg::Number => ShapeKind::Number,
        ToolArg::Text => ShapeKind::Text,
        ToolArg::Pencil => ShapeKind::Pencil,
        ToolArg::Highlighter => ShapeKind::Highlighter,
        ToolArg::Mosaic => ShapeKind::Mosaic,
        ToolArg::Blur => ShapeKind::Blur,
        ToolArg::Magnifier => ShapeKind::Magnifier,
        ToolArg::Spotlight => ShapeKind::Spotlight,
        ToolArg::Eraser => ShapeKind::Eraser,
    }
}

#[cfg(test)]
mod tests {
    use super::ScreenshotSession;
    use crate::annotation::ShapeKind;
    use crate::args::ToolArg;
    use crate::platform::capture::Capture;
    use gpui_kit::{Bounds, point, px, size};
    use std::sync::Arc;

    fn screen(name: &str, pos: (i32, i32), scale: f32) -> Arc<Capture> {
        let mut cap = Capture::for_test(pos, scale);
        cap.output_name = name.into();
        cap.width = (100. * scale) as u32;
        cap.height = (80. * scale) as u32;
        cap.rgba = vec![200; (cap.width * cap.height * 4) as usize];
        Arc::new(cap)
    }

    fn dual_session() -> ScreenshotSession {
        ScreenshotSession::new(
            vec![screen("left", (-100, 0), 1.), screen("right", (0, 0), 2.)],
            Vec::new(),
        )
    }

    #[test]
    fn select_all_on_prefers_focused_host_and_falls_back_cleanly() {
        let mut s = dual_session();
        s.select_all_on(Some("right"));
        assert_eq!(s.active_output_name(), Some("right"));
        assert_eq!(
            s.selection().bounds(),
            Some(Bounds::new(
                point(px(-100.), px(0.)),
                size(px(200.), px(80.))
            ))
        );

        s.select_all_on(Some("unknown"));
        assert_eq!(s.active_output_name(), Some("left"));
    }

    #[test]
    fn select_output_picks_exact_monitor_or_returns_false() {
        let mut s = dual_session();
        assert!(!s.select_output("DP-99"));
        assert!(!s.selection().is_selected());

        assert!(s.select_output("right"));
        assert_eq!(s.active_output_name(), Some("right"));
        assert_eq!(
            s.selection().bounds(),
            Some(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(80.))))
        );
    }

    #[test]
    fn select_bounds_elects_dominant_host_and_rejects_offscreen_rects() {
        let mut s = dual_session();
        // Completely off-screen
        assert!(!s.select_bounds(Bounds::new(
            point(px(500.), px(500.)),
            size(px(50.), px(50.)),
        )));
        assert!(!s.selection().is_selected());

        // Straddling the seam (-20..60): 20px on left, 60px on right → right hosts chrome
        assert!(s.select_bounds(Bounds::new(
            point(px(-20.), px(10.)),
            size(px(80.), px(40.)),
        )));
        assert_eq!(s.active_output_name(), Some("right"));
    }

    #[test]
    fn arm_initial_tool_activates_immediately_or_after_first_drag() {
        // Pre-selected session activates immediately
        let mut pre = dual_session();
        pre.select_all();
        pre.arm_initial_tool(ToolArg::Arrow);
        assert_eq!(pre.annotations().tool(), Some(ShapeKind::Arrow));

        // Idle session survives begin()'s reset and activates on end()
        let mut idle = dual_session();
        idle.arm_initial_tool(ToolArg::Blur);
        assert_eq!(idle.annotations().tool(), None);
        idle.begin("left", point(px(10.), px(10.)));
        idle.end("left", point(px(60.), px(50.)));
        assert!(idle.selection().is_selected());
        assert_eq!(idle.annotations().tool(), Some(ShapeKind::Blur));
    }
}
