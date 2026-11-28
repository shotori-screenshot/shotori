//! Bounded single-row toolbar container with a `...` overflow popover on
//! narrow outputs. Never renders a horizontal scrollbar.

use std::rc::Rc;

use gpui_kit::{
    assets::IconName,
    component::popover::{Popover, PopoverState},
    *,
};

use crate::model::placement::{BAR_PAD, BTN_GAP, BTN_SIZE};
use crate::ui::theme;

type OverflowRenderFn =
    Rc<dyn Fn(Option<Entity<PopoverState>>, &mut Window, &mut App) -> AnyElement>;

/// One overflowable entry in a toolbar row.
///
/// `render` is invoked either inline in the bar (`popup == None`) or inside
/// the `...` overflow popover (`popup == Some(state)`). Row-one action/tool
/// buttons dismiss the popover on click; row-two option controls keep it open
/// while the user drags sliders or picks colors.
#[derive(Clone)]
pub(super) struct OverflowItem {
    pub(super) width: f32,
    pub(super) is_separator: bool,
    pub(super) active: bool,
    pub(super) render: OverflowRenderFn,
}

impl OverflowItem {
    pub(super) fn control(
        width: f32,
        active: bool,
        render: impl Fn(Option<Entity<PopoverState>>, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            width,
            is_separator: false,
            active,
            render: Rc::new(render),
        }
    }

    pub(super) fn separator() -> Self {
        Self {
            width: 11.,
            is_separator: true,
            active: false,
            render: Rc::new(|_, _, _| super::separator().into_any_element()),
        }
    }
}

#[derive(IntoElement)]
pub(super) struct ToolbarRow {
    id: &'static str,
    more_id: &'static str,
    focus: FocusHandle,
    row_bottom: Pixels,
    leading: Vec<(f32, AnyElement)>,
    trailing: Vec<(f32, AnyElement)>,
    items: Vec<OverflowItem>,
}

impl ToolbarRow {
    pub(super) fn new(
        id: &'static str,
        more_id: &'static str,
        focus: FocusHandle,
        row_bottom: Pixels,
        items: Vec<OverflowItem>,
    ) -> Self {
        Self {
            id,
            more_id,
            focus,
            row_bottom,
            leading: Vec::new(),
            trailing: Vec::new(),
            items,
        }
    }

    pub(super) fn leading(mut self, width: f32, element: impl IntoElement) -> Self {
        self.leading.push((width, element.into_any_element()));
        self
    }

    pub(super) fn trailing(mut self, width: f32, element: impl IntoElement) -> Self {
        self.trailing.push((width, element.into_any_element()));
        self
    }
}

/// Mirror `gpui-pre`'s Taffy `round_to_device_pixel` snapping so fractional
/// scales (`1.25x`, `1.5x`, `1.75x`) measure the true laid-out bar width.
fn snap(v: f32, scale: f32) -> f32 {
    if scale > 0. && scale.is_finite() {
        (v * scale).round() / scale
    } else {
        v
    }
}

fn bar_chrome_width(scale: f32) -> f32 {
    2. * snap(1., scale) + 2. * snap(BAR_PAD, scale)
}

fn entry_snapped_width(width: f32, is_separator: bool, scale: f32) -> f32 {
    if is_separator {
        // `separator()` is `w(1.)` + `mx(5.)` on both sides.
        snap(1., scale) + 2. * snap(5., scale)
    } else {
        snap(width, scale)
    }
}

/// Partition `items` into `items[..visible_end]` (inline) and
/// `items[overflow_start..overflow_end]` (inside the `...` popover).
///
/// Returns `(items.len(), items.len(), items.len())` when every item fits
/// within `max_width` at `scale`, so no `...` trigger is rendered.
pub(super) fn split_overflow(
    leading_w: &[f32],
    trailing_w: &[f32],
    items: &[(f32, bool)],
    max_width: f32,
    scale: f32,
) -> (usize, usize, usize) {
    let chrome = bar_chrome_width(scale);
    let gap = snap(BTN_GAP, scale);
    let fixed_count = leading_w.len() + trailing_w.len();
    let fixed_sum: f32 = leading_w
        .iter()
        .chain(trailing_w.iter())
        .map(|&w| snap(w, scale))
        .sum();
    let item_widths: Vec<f32> = items
        .iter()
        .map(|&(w, is_sep)| entry_snapped_width(w, is_sep, scale))
        .collect();

    let total_count = fixed_count + items.len();
    let full_width = chrome
        + fixed_sum
        + item_widths.iter().sum::<f32>()
        + (total_count.saturating_sub(1) as f32) * gap;
    if full_width <= max_width + 0.01 {
        return (items.len(), items.len(), items.len());
    }

    let more_w = snap(BTN_SIZE, scale);
    let mut overflow_end = items.len();
    while overflow_end > 0 && items[overflow_end - 1].1 {
        overflow_end -= 1;
    }

    // Prefix sums of snapped item widths for O(1) slice width checks.
    let mut prefix = Vec::with_capacity(items.len() + 1);
    prefix.push(0.);
    for &w in &item_widths {
        prefix.push(prefix.last().copied().unwrap_or(0.) + w);
    }

    for k in (0..overflow_end).rev() {
        let mut vis_end = k;
        while vis_end > 0 && items[vis_end - 1].1 {
            vis_end -= 1;
        }
        let mut overflow_start = k;
        while overflow_start < overflow_end && items[overflow_start].1 {
            overflow_start += 1;
        }
        if overflow_start >= overflow_end {
            continue;
        }
        let vis_count = fixed_count + vis_end + 1; // +1 for the `...` trigger
        let vis_width = chrome
            + fixed_sum
            + prefix[vis_end]
            + more_w
            + (vis_count.saturating_sub(1) as f32) * gap;
        if vis_width <= max_width + 0.01 {
            return (vis_end, overflow_start, overflow_end);
        }
    }

    let mut overflow_start = 0;
    while overflow_start < overflow_end && items[overflow_start].1 {
        overflow_start += 1;
    }
    (0, overflow_start, overflow_end)
}

impl RenderOnce for ToolbarRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let scale = window.scale_factor();
        let ws = window.bounds().size;
        // Budget against the display's actual available width (`ws.width - 16px`),
        // never the unsnapped analytical `row1_basis`: fractional DPI rounding
        // (`1.25x`, `1.5x`) widens `gap(3.)` by a few pixels, which must stay
        // single-row on wide displays instead of falsely overflowing into `...`.
        let avail_w = (f32::from(ws.width) - 16.).max(1.);
        let leading_w: Vec<f32> = self.leading.iter().map(|(w, _)| *w).collect();
        let trailing_w: Vec<f32> = self.trailing.iter().map(|(w, _)| *w).collect();
        let item_specs: Vec<(f32, bool)> = self
            .items
            .iter()
            .map(|item| (item.width, item.is_separator))
            .collect();
        let (vis_end, overflow_start, overflow_end) =
            split_overflow(&leading_w, &trailing_w, &item_specs, avail_w, scale);

        let id = self.id;
        let mut row = super::bar()
            .debug_selector(move || id.into())
            .shadow(super::bar_shadow())
            .flex_shrink_0();

        for (_, el) in self.leading {
            row = row.child(el);
        }
        for item in &self.items[..vis_end] {
            row = row.child((item.render)(None, window, cx));
        }

        if overflow_start < overflow_end {
            let overflow: Vec<OverflowItem> = self.items[overflow_start..overflow_end].to_vec();
            let more_active = overflow.iter().any(|item| item.active);
            let more_id = self.more_id;
            let t = theme::c();
            let trigger = super::control(
                more_id.into(),
                crate::i18n::t!("toolbar.more").into_owned(),
                more_active,
                self.focus.clone(),
                |_, _| {},
            )
            // Popover owns activation; keep focus on the popover instead of
            // returning it to the canvas before the menu opens.
            .on_click(|_, _, _| {})
            .child(super::annot_options::menu_icon_colored(
                IconName::Ellipsis,
                18.,
                super::icon_tint(more_active),
            ));

            let anchor = if self.row_bottom + px(140.) > ws.height {
                Anchor::BottomRight
            } else {
                Anchor::TopRight
            };
            let popover_max_w = (avail_w - 16.).clamp(120., 360.);
            let close_focus = self.focus.clone();
            let popover = Popover::new(more_id)
                .anchor(anchor)
                .p(px(0.))
                .rounded(px(12.))
                .bg(rgba(t.capsule_sheet_surface()))
                .border_1()
                .border_color(rgba(t.capsule_border()))
                .shadow(theme::capsule_floating_shadow())
                .on_open_change(move |open, window, cx| {
                    if !open {
                        window.focus(&close_focus, cx);
                    }
                })
                .trigger(trigger)
                .content(move |_, window, cx| {
                    let popup = cx.entity();
                    let mut panel = super::annotation_menu_panel()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(BTN_GAP))
                        .max_w(px(popover_max_w));
                    for item in &overflow {
                        panel = panel.child((item.render)(Some(popup.clone()), window, cx));
                    }
                    panel
                });
            row = row.child(popover);
        }

        for (_, el) in self.trailing {
            row = row.child(el);
        }

        row
    }
}

#[cfg(test)]
mod tests {
    use super::split_overflow;
    use crate::model::placement::{BTN_GAP, BTN_SIZE, GRIP_W};
    use crate::model::settings::{DEFAULT_ROW, ToolbarItem};

    fn default_row_specs() -> Vec<(f32, bool)> {
        DEFAULT_ROW
            .iter()
            .map(|item| match item {
                ToolbarItem::Button(_) | ToolbarItem::Custom(_) => (BTN_SIZE, false),
                ToolbarItem::Separator => (11., true),
            })
            .collect()
    }

    #[test]
    fn normal_displays_never_overflow_at_integer_or_fractional_dpi() {
        let specs = default_row_specs();
        for (logical_w, scale) in [
            (1920., 1.0),
            (1536., 1.25),
            (1280., 1.5),
            (1097., 1.75),
            (960., 2.0),
            (1000., 3.0),
        ] {
            let avail = logical_w - 16.;
            let (vis, over_start, over_end) =
                split_overflow(&[GRIP_W], &[GRIP_W], &specs, avail, scale);
            assert_eq!(
                (vis, over_start, over_end),
                (specs.len(), specs.len(), specs.len()),
                "unexpected overflow on {logical_w}px @ {scale}x"
            );
        }
    }

    #[test]
    fn narrow_display_splits_within_budget_and_trims_boundary_separators() {
        let specs = default_row_specs();
        let (vis, over_start, over_end) = split_overflow(&[GRIP_W], &[GRIP_W], &specs, 464., 1.0);
        assert!(vis > 0 && vis < specs.len());
        assert!(over_start < over_end);
        assert!(!specs[vis - 1].1, "visible row must not end on a separator");
        assert!(
            !specs[over_start].1,
            "overflow menu must not start with a separator"
        );
        // Verify total rendered width stays within 464px (including grips + `...`).
        let count = 2 + vis + 1;
        let items_sum: f32 = specs[..vis].iter().map(|(w, _)| *w).sum();
        let width = 18. + 2. * GRIP_W + items_sum + BTN_SIZE + (count - 1) as f32 * BTN_GAP;
        assert!(width <= 464., "visible width {width} exceeds 464px");
    }

    #[test]
    fn boundary_separator_is_dropped_between_visible_and_overflow() {
        // 3 buttons, separator, 3 buttons; choose width so the split falls on the separator.
        let specs = vec![
            (BTN_SIZE, false),
            (BTN_SIZE, false),
            (BTN_SIZE, false),
            (11., true),
            (BTN_SIZE, false),
            (BTN_SIZE, false),
            (BTN_SIZE, false),
        ];
        // Chrome 18 + 3 buttons (96) + `...` (32) + 3 gaps (9) = 155px;
        // adding separator (14) + 4th button (35) = 204px. Budget 175px splits right at the separator.
        let (vis, over_start, over_end) = split_overflow(&[], &[], &specs, 175., 1.0);
        assert_eq!(vis, 3);
        assert_eq!(over_start, 4);
        assert_eq!(over_end, 7);
    }

    #[test]
    fn degenerate_tiny_width_moves_all_items_to_overflow_without_panicking() {
        let specs = default_row_specs();
        let (vis, over_start, over_end) = split_overflow(&[GRIP_W], &[GRIP_W], &specs, 40., 1.0);
        assert_eq!(vis, 0);
        assert_eq!(over_start, 0);
        assert_eq!(over_end, specs.len());
    }
}
