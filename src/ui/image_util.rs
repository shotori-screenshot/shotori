//! # In-memory pixels → gpui display plumbing for ui
//!
//! The RGBA→BGRA contract and its one constructor live in
//! [`crate::render_image`] (a leaf: model/ and annotation/ render caches
//! need the same swap and must not depend on ui/). This module keeps the
//! ui-side frame cache and re-exports the constructor so existing call
//! sites stay stable.

use std::sync::Arc;

use gpui_kit::RenderImage;

pub use crate::render_image::rgba_to_render_image;

/// Images painted directly on a canvas bypass GPUI's managed image element.
/// Retain only this window's current frame and explicitly evict retired atlas tiles.
#[derive(Default)]
pub(crate) struct CanvasImages {
    current: Vec<Arc<RenderImage>>,
}

impl CanvasImages {
    #[cfg(test)]
    pub(crate) fn current(&self) -> &[Arc<RenderImage>] {
        &self.current
    }

    pub(crate) fn replace(&mut self, next: Vec<Arc<RenderImage>>) -> Vec<Arc<RenderImage>> {
        let retired = self
            .current
            .drain(..)
            .filter(|old| !next.iter().any(|new| old.id == new.id))
            .collect();
        self.current = next;
        retired
    }
}

#[cfg(test)]
mod tests {
    use super::{CanvasImages, rgba_to_render_image};

    #[test]
    fn changing_canvas_images_retire_old_frames_but_keep_shared_images() {
        let fixed = rgba_to_render_image(vec![255; 16], 2, 2);
        let mut images = CanvasImages::default();
        let mut previous = None;
        for _ in 0..2000 {
            let fresh = rgba_to_render_image(vec![255; 16], 2, 2);
            let retired = images.replace(vec![fixed.clone(), fresh.clone()]);
            assert_eq!(retired.len(), usize::from(previous.is_some()));
            if let Some(previous) = previous {
                assert_eq!(retired[0].id, previous);
            }
            previous = Some(fresh.id);
            assert_eq!(images.current.len(), 2);
        }
        assert_eq!(images.replace(Vec::new()).len(), 2);
        assert!(images.current.is_empty());
    }
}
