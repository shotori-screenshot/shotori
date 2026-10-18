//! # Scroll capture chrome: region frame + preview panel
//!
//! The overlays show FROZEN pixels — for the live content to scroll they
//! must unmap, and surfaces replace them for the duration of a long
//! screenshot:
//!
//! - the **region frame** — ONE full-output transparent layer surface
//!   drawing four accent strokes around the selection. Its input region
//!   covers ONLY the move handle (plus the chrome-free toolbar), so
//!   wheel events over the captured content pass through to the app —
//!   manual scrolling keeps working. The ⇕ handle slides the region
//!   VERTICALLY: the shared capture rect updates live and the engine
//!   follows the frame to content that never scrolled into view (for
//!   the sheet, "frame moves down" is indistinguishable from "content
//!   scrolls up"). Horizontal motion is deliberately NOT offered — a
//!   sideways jump desyncs the stitch (the folded columns stop
//!   matching), and free frame movement shipped broken long screenshots.
//! - the **capture HUD** — a floating action bar in the selection
//!   toolbar's own vocabulary (grip-free, 30 px icon tiles, accent
//!   primary) above a strip of the growing long image. Status and live
//!   size ride as small chips ON the strip, the same language as the
//!   selection's size label. No panel header, no footer, no slab: with
//!   the HUD up, the frame is just the selection outline plus a move
//!   chip — no duplicated toolbar.
//!
//! ## What the strip is, relative to the selection
//!
//! The strip is a **viewfinder on the sheet**, not a second copy of the
//! screen: one sheet row is always the same number of panel pixels
//! ([`crate::model::scroll_sheet::preview_fit_rows`]), the window is
//! exactly as tall as the panel's well, and the camera keeps the
//! selection's bottom edge on the window's bottom edge. So the strip
//! moves at the rate the page scrolls — new rows enter at its bottom as
//! the capture takes them, old ones leave at the top — and the tinted
//! band is the part of the sheet the selection is showing right now.
//! While the selection sits at the growing edge (the normal case) the
//! window is simply the last well-height of the sheet; scrolling back up
//! re-aims the camera and the band rises through the strip instead of
//! the whole image sliding.
//!
//! ## Chrome must never paint inside the capture rect
//!
//! wlr-screencopy captures the fully-COMPOSITED output — our layer
//! surfaces included. Anything drawn over the selection burns into every
//! captured frame (the "full-screen long screenshot contains the layer
//! chrome" bug): the panel floats only on a side with a full free strip
//! ([`crate::model::placement::plan_scroll_panel`]) and hangs BESIDE the
//! region — its launch-time position is frozen and the frame's x is
//! frozen with it, so the region can never slide under the panel. The
//! frame toolbar
//! climbs a placement ladder that never enters the region
//! ([`crate::model::placement::frame_toolbar`]) and hides when nowhere
//! fits, and on a single monitor with a full-screen selection the
//! session goes CHROME-FREE: no panel, no toolbar — the frame window
//! hosts the engine lifecycle alone (strokes stay strictly outside the
//! rect; the keyboard carries the exits) and a notification announces
//! the shortcuts. With a second monitor the panel docks there instead.
//!
//! Keyboard lives on the panel (or, chrome-free, on the frame) and
//! mirrors the selection flow exactly: Enter/Ctrl+C finish+copy,
//! Ctrl+S finish+save, Space pauses/resumes, Esc cancels.

use gpui_kit::assets::IconName;
#[cfg(target_os = "linux")]
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;

use crate::model::placement::{
    SCROLL_PANEL_MARGIN, ScrollPanelDock, plan_scroll_panel, scroll_panel_margins,
};
use crate::model::scroll_sheet::SheetLimits;
use crate::platform::scroll_capture::{self, ScrollControls, ScrollEvent, ScrollSpec};

actions!(
    scroll,
    [ScrollFinish, ScrollCancel, ScrollSave, ScrollPause]
);

use frame::FrameView;
use panel::PreviewPanel;

/// Preview panel width (logical px) — owned by `model::placement` (the
/// docking planner sizes its side strips from it); the panel's layout
/// math uses the same constant.
use crate::model::placement::SCROLL_PANEL_W as PANEL_W;
/// Preview well border (the only frame around the page strip).
const PREVIEW_BORDER: f32 = 1.;
/// Gap between the floating action bar and the preview strip.
const TOOLBAR_GAP: f32 = 6.;
/// The action bar's height — a compact version of the selection toolbar.
const TOOLBAR_H: f32 = 36.;
/// Gutter between the preview well's border and the page strip.
const PREVIEW_GUTTER: f32 = 4.;
/// The preview image's display width: window minus the well's border
/// and the inner gutter. The img element's height is derived from this
/// and the image's own aspect — NEVER from the element's intrinsic
/// sizing: gpui leaks the BITMAP's pixel height into the layout as
/// logical px, so on a 2× screen the box rendered 2× taller than
/// width × aspect, throwing the highlight's relative() fractions
/// (fractions of that box) off with it — the whole "preview doesn't
/// match my frame" saga (measured: 1164 physical px of box where 590
/// was correct).
const PREVIEW_IMG_W: f32 = PANEL_W - 2. * PREVIEW_BORDER - 2. * PREVIEW_GUTTER;

/// Everything `launch` needs about the target output, in logical px.
pub(crate) struct ScrollChrome {
    pub display_id: Option<DisplayId>,
    pub output_width: f32,
    pub output_height: f32,
}

/// Launch the scroll-capture chrome and start the engine. Must be called
/// BEFORE the overlays unmap — closing every window would end the app
/// loop with the capture half-started. Returns the shared capture rect
/// (already inside `spec`).
pub(crate) fn launch(spec: ScrollSpec, chrome: ScrollChrome, cx: &mut App) -> anyhow::Result<()> {
    let options = SheetLimits::default();
    let shared_rect = spec.rect.clone();
    let rect_now = *shared_rect
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Where may chrome live? Nothing painted over the capture rect can
    // be excluded from wlr-screencopy's composite — plan first, open
    // windows second.
    let other_output = crate::ui::pin::other_output_than(chrome.display_id);
    let dock = plan_scroll_panel(rect_now, chrome.output_width, other_output.is_some());
    let chrome_free = dock == ScrollPanelDock::Hidden;

    // The engine starts HERE (not inside the panel): whichever window
    // ends up hosting the lifecycle receives the pair. A spawn failure
    // (not a compositor failure — those arrive as Failed events) still
    // yields a live event stream carrying the error, so the lifecycle
    // code stays single-shaped.
    // The engine renders preview pixels only when a panel exists; the
    // panel writes its well height here once it knows it.
    let preview_height = spec.preview_rows.clone();
    let (controls, events) = match scroll_capture::start(spec, options) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[shotori] scroll engine failed to start: {e:#}");
            let (tx, rx) = async_channel::bounded(1);
            let _ = tx.try_send(ScrollEvent::Failed {
                reason: format!("could not start the scroll engine: {e:#}"),
                partial: None,
            });
            (ScrollControls::dead(), rx)
        }
    };

    // Region frame first: the user must never lose sight of WHAT is
    // being captured, not even for the frames before the panel maps.
    // Chrome-free sessions take the keyboard here (Exclusive + focus):
    // real wheel follows the pointer, so scrolling survives.
    let frame_options = WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: chrome_free,
        display_id: chrome.display_id,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(chrome.output_width), px(chrome.output_height)),
        })),
        #[cfg(target_os = "linux")]
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "shotori-scroll-frame".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: Some(px(-1.)),
            keyboard_interactivity: if chrome_free {
                KeyboardInteractivity::Exclusive
            } else {
                KeyboardInteractivity::None
            },
            ..Default::default()
        }),
        #[cfg(not(target_os = "linux"))]
        kind: WindowKind::PopUp,
        ..Default::default()
    };
    let frame_cell: std::rc::Rc<std::cell::RefCell<Option<gpui_kit::WeakEntity<FrameView>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let cell = frame_cell.clone();
    let frame = cx.open_window(frame_options, |window, cx| {
        let view = cx.new(|cx| {
            let view = FrameView::new(
                shared_rect.clone(),
                rect_now,
                chrome.output_width,
                chrome.output_height,
                dock,
                cx.focus_handle(),
            );
            *cell.borrow_mut() = Some(cx.entity().downgrade());
            view
        });
        // Chrome-free: this window IS the keyboard — focus its handle
        // (WindowOptions::focus only activates the window).
        if chrome_free && let Some(entity) = cell.borrow().as_ref().and_then(|w| w.upgrade()) {
            let handle = entity.read(cx).focus.clone();
            window.focus(&handle, cx);
        }
        view
    });

    // Chrome-free: no second window — the frame hosts the engine
    // lifecycle and the keyboard alone (strokes outside the region are
    // the only pixels; the notification carries the shortcuts).
    if chrome_free {
        let host_window = frame.as_ref().ok().map(|f| (*f).into());
        let host_entity = frame_cell.borrow().as_ref().and_then(|w| w.upgrade());
        if let (Some(entity), Some(host_window)) = (host_entity, host_window) {
            entity.update(cx, |f, cx| f.host_engine(controls, events, host_window, cx));
        }
        crate::notify::send(
            crate::i18n::t!("scroll.notify.running.title").as_ref(),
            crate::i18n::t!("scroll.notify.running.body").as_ref(),
        );
        return Ok(());
    }

    // The panel docks on its planned side of the capture output — or,
    // when no side fits, on the OTHER output (display + logical size
    // come from the pin registry; the preview column math needs that
    // output's height, not the capture one's).
    let (panel_display, panel_out_h) = match dock {
        ScrollPanelDock::OtherOutput => {
            // `other_output` is Some by construction of the plan; fall
            // back to the capture display rather than unwrapping.
            other_output
                .map(|(id, bounds)| (Some(id), f32::from(bounds.size.height)))
                .unwrap_or((chrome.display_id, chrome.output_height))
        }
        _ => (chrome.display_id, chrome.output_height),
    };
    // A floating card, not a full-height sidebar: the deck reads as one
    // object instead of a screen-wide slab, and keeps the output's
    // vertical rhythm. Tall enough for a useful preview on a 1080p
    // screen, capped so 4K users do not get a 1500px column.
    let panel_h = (panel_out_h * 0.40)
        .clamp(320., 500.)
        .min((panel_out_h - 2. * SCROLL_PANEL_MARGIN).max(120.));
    // The panel hangs BESIDE the capture rect (user-reported: pinned to
    // the output edge it read as disconnected from the region it
    // previews) — vertically centered on the rect, horizontally hugging
    // its planned side across SCROLL_PANEL_GAP. Layer surfaces ignore
    // `window_bounds` origins, so the position rides the TOP|LEFT
    // margins. It is frozen for the session, and the frame's x is
    // frozen with it (vertical-only drags), so the region can never
    // slide under the panel.
    let (panel_top, panel_left) = scroll_panel_margins(
        rect_now,
        chrome.output_width,
        chrome.output_height,
        panel_h,
        dock,
    );
    let panel_options = WindowOptions {
        app_id: Some(crate::APP_ID.into()),
        titlebar: None,
        // Only the action bar and the preview strip paint — the window
        // itself is a floating HUD, not a slab over the desktop.
        window_background: WindowBackgroundAppearance::Transparent,
        focus: true,
        display_id: panel_display,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(PANEL_W), px(panel_h)),
        })),
        #[cfg(target_os = "linux")]
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "shotori-scroll-preview".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::LEFT,
            exclusive_zone: Some(px(-1.)),
            // The panel owns the keyboard so the shortcuts match the
            // selection flow. NOTE: if the compositor routes wheel by
            // keyboard focus (niri does for virtual pointers), scrolling
            // degrades to frame-dragging — deliberately shipped both.
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            // CSS order: top, right, bottom, left — (top, left) carry
            // the beside-the-rect position.
            margin: Some((px(panel_top), px(0.), px(0.), px(panel_left))),
            ..Default::default()
        }),
        #[cfg(not(target_os = "linux"))]
        kind: WindowKind::PopUp,
        ..Default::default()
    };
    let panel_cell: std::rc::Rc<std::cell::RefCell<Option<gpui_kit::WeakEntity<PreviewPanel>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let pcell = panel_cell.clone();
    let panel = cx.open_window(panel_options, |window, cx| {
        cx.new(|cx| {
            let panel = PreviewPanel::new(controls, events, panel_h, preview_height, window, cx);
            *pcell.borrow_mut() = Some(cx.entity().downgrade());
            panel
        })
    });

    // The panel closes the frame on its terminal paths …
    if let Ok(panel) = &panel {
        let sibling = frame.as_ref().ok().map(|f| (*f).into());
        let _ = panel.update(cx, |p, _, _| {
            p.sibling = sibling;
        });
    }
    // … and the frame toolbar gets the engine + panel handles so its
    // buttons work without cross-window action dispatch.
    if let Some(frame) = frame_cell.borrow().as_ref().and_then(|w| w.upgrade())
        && let Some(panel) = panel_cell.borrow().as_ref().and_then(|w| w.upgrade())
    {
        let controls = panel.read(cx).controls();
        let weak_panel = panel.downgrade();
        frame.update(cx, |f, _| f.bind(controls, weak_panel));
    }
    Ok(())
}

// ── the region frame ─────────────────────────────────────────────────

/// One full-output transparent surface drawing the region frame and
/// owning its drag interaction.
mod frame;
mod panel;
