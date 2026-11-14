//! Shared terminal exits and semantic status palette for scroll-capture
//! hosts (`PreviewPanel` and chrome-free `FrameView`).

use std::sync::Arc;

use gpui_kit::*;

use crate::model::scroll_sheet::Phase;
use crate::model::session::ScrollRect;
use crate::ui::theme;
use crate::ui::theme::alpha;

/// Semantic amber warning color (`#f59e0b`, RGBA) used on peripheral
/// frame strokes and status pills when overlap or alignment degrades.
pub(super) const WARNING_AMBER: u32 = 0xf59e0bff;

/// Which post-capture destination the user requested before `Finished`
/// arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum ExitTarget {
    #[default]
    Copy,
    Save,
    Pin,
}

/// Output geometry required to place a finished long capture onto the
/// floating pin board at native physical-to-logical scale.
#[derive(Clone)]
pub(super) struct PinContext {
    pub output_origin: Point<Pixels>,
    pub output_scale: f32,
    pub rect: Arc<std::sync::Mutex<ScrollRect>>,
}

/// True when `phase` represents a degraded state the user should notice
/// immediately (fast scroll outrunning overlap, unstable content, lateral
/// layout shift, stalled alignment, or length limit reached).
pub(super) fn is_warning_phase(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::NoOverlap | Phase::Unstable | Phase::LayoutShift | Phase::Stalled | Phase::MaxLength
    )
}

/// Peripheral stroke color painted strictly outside the capture rect on
/// `FrameView` so the user gets live feedback while looking at the page.
pub(super) fn frame_stroke_color(phase: Phase, paused: bool) -> u32 {
    if paused {
        alpha(theme::c().hint_text, 0xCC)
    } else if is_warning_phase(phase) {
        WARNING_AMBER
    } else {
        theme::c().accent
    }
}

/// Route a finished or partial long image to the chosen exit.
pub(super) fn dispatch_exit(
    target: ExitTarget,
    image: (u32, u32, &Arc<Vec<u8>>),
    pin_ctx: &PinContext,
    host: AnyWindowHandle,
    sibling: Option<AnyWindowHandle>,
    cx: &mut App,
) {
    let (width, height, rgba) = image;
    match target {
        ExitTarget::Copy => scroll_copy_exit(rgba, width, height, host, sibling, cx),
        ExitTarget::Save => scroll_save_exit(rgba, width, height, host, sibling, cx),
        ExitTarget::Pin => scroll_pin_exit(rgba, width, height, pin_ctx, host, sibling, cx),
    }
}

/// Clipboard exit: PNG → clipboard on the background executor → notify →
/// close host window last so the run loop stays alive until copy finishes.
pub(super) fn scroll_copy_exit(
    rgba: &Arc<Vec<u8>>,
    width: u32,
    height: u32,
    host: AnyWindowHandle,
    sibling: Option<AnyWindowHandle>,
    cx: &mut App,
) {
    let rgba = rgba.clone();
    if let Some(sibling) = sibling {
        drop(sibling.update(cx, |_, window, _| window.remove_window()));
    }
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move {
                let png = crate::model::export::encode_png_fast(width, height, &rgba)?;
                crate::clipboard::copy_image(width, height, &rgba, &png)?;
                Ok::<Vec<u8>, anyhow::Error>(png)
            })
            .await;
        match result {
            Ok(png) => {
                println!("[shotori] long screenshot {width}x{height} copied");
                crate::notify::copied(&png);
            }
            Err(e) => {
                eprintln!("[shotori] long screenshot copy failed: {e:#}");
                crate::notify::send(
                    crate::i18n::t!("scroll.notify.copy_failed.title").as_ref(),
                    crate::i18n::t!("scroll.notify.copy_failed.body").as_ref(),
                );
            }
        }
        drop(host.update(cx, |_, window, _| window.remove_window()));
    })
    .detach();
}

/// Native save dialog exit: stash pixels, unmap surfaces, flush Wayland
/// destroys over a short timer, then quit so `save_dialog::complete_pending`
/// opens the portal dialog cleanly without layer-shell occlusion.
pub(super) fn scroll_save_exit(
    rgba: &Arc<Vec<u8>>,
    width: u32,
    height: u32,
    host: AnyWindowHandle,
    sibling: Option<AnyWindowHandle>,
    cx: &mut App,
) {
    crate::save_dialog::stash(width, height, rgba.to_vec());
    cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
    if let Some(sibling) = sibling {
        drop(sibling.update(cx, |_, window, _| window.remove_window()));
    }
    drop(host.update(cx, |_, window, _| window.remove_window()));
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(150))
            .await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// Pin exit: hand the stitched RGBA image to `ui::pin` at native logical
/// scale anchored to the capture region's origin, then tear down the
/// scroll chrome. When this process owns the pin board, the new pin
/// surfaces keep the run loop alive naturally until the last pin closes;
/// when forwarded over IPC to an existing pin process, flush surface
/// unmaps before quitting.
pub(super) fn scroll_pin_exit(
    rgba: &Arc<Vec<u8>>,
    width: u32,
    height: u32,
    pin_ctx: &PinContext,
    host: AnyWindowHandle,
    sibling: Option<AnyWindowHandle>,
    cx: &mut App,
) {
    let rect_now = *pin_ctx
        .rect
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scale = pin_ctx.output_scale.max(1.0);
    let pin_bounds = Bounds::new(
        point(
            pin_ctx.output_origin.x + px(rect_now.x as f32),
            pin_ctx.output_origin.y + px(rect_now.y as f32),
        ),
        size(
            px((width as f32 / scale).max(1.0)),
            px((height as f32 / scale).max(1.0)),
        ),
    );
    let spec = crate::ui::pin::PinSpec {
        w: width,
        h: height,
        rgba: rgba.to_vec(),
        rect: pin_bounds,
    };
    let prepared = cx
        .background_executor()
        .spawn(async move { crate::ui::pin::prepare(spec) });
    cx.spawn(async move |cx| {
        let prepared = prepared.await;
        cx.update(|cx| {
            let before = cx.windows().len();
            let opened = prepared.and_then(|p| crate::ui::pin::open(p, cx));
            let local_pin_opened = opened.is_ok() && cx.windows().len() > before;
            if !local_pin_opened {
                cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
            }
            if let Some(sibling) = sibling {
                drop(sibling.update(cx, |_, window, _| window.remove_window()));
            }
            drop(host.update(cx, |_, window, _| window.remove_window()));
            match opened {
                Ok(()) => {
                    println!("[shotori] long screenshot {width}x{height} pinned");
                }
                Err(error) => {
                    eprintln!("[shotori] long screenshot pin failed: {error:#}");
                    crate::notify::send(
                        crate::i18n::t!("notify.pin.failed.title").as_ref(),
                        crate::i18n::t!("notify.pin.failed.body").as_ref(),
                    );
                }
            }
            if !local_pin_opened {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(150))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        });
    })
    .detach();
}
