//! # UI: gpui windows and elements
//!
//! - [`overlay`]: the layer-shell overlay assembly (events → model,
//!   model → rendering); deliberately thin — logic lives in
//!   `crate::model`, actions in `crate::actions`
//! - [`hud`]: selection backdrop / label / hint bar / hover outline
//! - [`toolbar`]: the post-selection button row
//! - [`placement`] lives in `crate::model` — the geometry both of these
//!   consume
//! - [`ocr_setup`]: first-run model-download dialog
//! - [`ocr_osd`]: the tiny "recognizing…" chip shown while background
//!   OCR runs after the overlays have unmapped
//! - [`pin`]: the selection as a floating pinned window (贴图)
//! - [`scroll_bar`]: the long-screenshot chrome (region frame + preview
//!   panel; chrome-free on full-screen single-monitor sessions — see
//!   that module's docs)
//! - [`settings`]: the settings toplevel (`shotori settings`) — the
//!   one non-overlay window; edits the toolbar layout in
//!   `model::settings`
//! - [`e2e`]: runtime debug backdoors driving headless tests
//!   (`SHOTORI_DEBUG_*`)
//! - [`theme`]: visual constants
//! - [`image_util`]: RGBA → RenderImage (the BGRA contract)

pub(crate) mod color_picker;
pub mod e2e;
pub mod hud;
pub mod image_util;
pub mod ocr_osd;
pub mod ocr_setup;
pub mod overlay;
pub mod pin;
pub mod scroll_bar;
pub mod settings;
pub mod theme;
pub mod toolbar;

pub(crate) mod text_editor;

pub(crate) mod text_input;
