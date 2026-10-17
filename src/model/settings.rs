//! # Persistent user settings (`~/.config/shotori/settings.toml`)
//!
//! Everything a user can configure that is not a color (colors live in
//! `theme.toml` — see [`crate::ui::theme::load`]). v1: the selection
//! toolbar's row-one layout — WHICH controls show and in WHAT order,
//! edited by dragging in the settings window (`shotori settings`).
//! v2: the OCR model set and an "assume upright text" speed toggle.
//!
//! Lifecycle mirrors the theme: loaded once at startup ([`init`]) and
//! read everywhere via [`current`]. The screenshot overlay is a
//! run-and-exit process, so there is deliberately no live reload —
//! the settings window writes straight through to the file, and the
//! NEXT capture picks the change up. A half-edited or wedged editor
//! can therefore never corrupt a running session.
//!
//! Unknown ids and unknown sections are tolerated, not fatal: a file
//! written by a NEWER shotori (extra buttons, extra sections) degrades
//! to the subset this binary understands, and [`Settings::save`]
//! round-trips the unknown parts so the editor never eats keys it does
//! not know about.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::Deserialize;

/// Settings file name inside the config dir (see [`config_dir`]).
pub const FILE_NAME: &str = "settings.toml";

/// How many swatches the annotation palette carries. Lives here (not
/// in the theme): the swatch board is toolbar configuration — WHICH
/// colors the brushes offer — not part of the visual appearance. The
/// built-in themes never disagreed on it either (every palette
/// inherited dark's board), so the move changed no behavior.
pub const PALETTE: usize = 7;

/// Swatch display names (UI copy stays in code; settings carry only
/// colors). Index-aligned with [`DEFAULT_ANNOTATION_COLORS`].
pub const PALETTE_NAMES: [&str; PALETTE] =
    ["Red", "Orange", "Yellow", "Green", "Blue", "Black", "White"];

/// The factory swatch board — the historical palette every built-in
/// theme carried. Opaque on purpose: exported PNGs must be solid.
pub const DEFAULT_ANNOTATION_COLORS: [u32; PALETTE] = [
    0xFF4545FF, 0xFF8A32FF, 0xFFD43BFF, 0x40C878FF, 0x409CFFFF, 0x222222FF, 0xFFFFFFFF,
];

/// One toggleable row-one button of the selection toolbar. Variant
/// order is the canonical order (the factory row, the settings
/// palette); [`ToolbarButton::id`] strings are the file format — never
/// rename one (files exist in the wild).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ToolbarButton {
    Select,
    Rectangle,
    Ellipse,
    Line,
    Polyline,
    Arrow,
    Number,
    Pencil,
    Highlighter,
    Mosaic,
    Eraser,
    Text,
    Clear,
    Ocr,
    Save,
    Pin,
    Scroll,
    Cancel,
    Copy,
    Settings,
}

/// The row-one groups the settings palette groups buttons by (the
/// toolbar itself renders one flat sequence — the factory row's two
/// separators reproduce the two-cluster look).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarCluster {
    Tools,
    Actions,
    Confirm,
}

impl ToolbarButton {
    /// Every button, in factory/display order. The settings palette
    /// lists and the factory row both iterate this — one order
    /// everywhere.
    pub const ALL: [Self; 20] = [
        Self::Select,
        Self::Rectangle,
        Self::Ellipse,
        Self::Line,
        Self::Polyline,
        Self::Arrow,
        Self::Number,
        Self::Pencil,
        Self::Highlighter,
        Self::Mosaic,
        Self::Eraser,
        Self::Text,
        Self::Clear,
        Self::Ocr,
        Self::Pin,
        Self::Scroll,
        Self::Save,
        Self::Copy,
        Self::Settings,
        Self::Cancel,
    ];

    /// Stable id for the settings file.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Rectangle => "rectangle",
            Self::Ellipse => "ellipse",
            Self::Line => "line",
            Self::Polyline => "polyline",
            Self::Arrow => "arrow",
            Self::Number => "number",
            Self::Pencil => "pencil",
            Self::Highlighter => "highlighter",
            Self::Mosaic => "mosaic",
            Self::Eraser => "eraser",
            Self::Text => "text",
            Self::Clear => "clear",
            Self::Ocr => "ocr",
            Self::Save => "save",
            Self::Pin => "pin",
            Self::Scroll => "scroll",
            Self::Cancel => "cancel",
            Self::Copy => "copy",
            Self::Settings => "settings",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|b| b.id() == id)
    }

    /// Which palette group the button lists under.
    pub const fn cluster(self) -> ToolbarCluster {
        match self {
            Self::Select
            | Self::Rectangle
            | Self::Ellipse
            | Self::Line
            | Self::Polyline
            | Self::Arrow
            | Self::Number
            | Self::Pencil
            | Self::Highlighter
            | Self::Mosaic
            | Self::Eraser
            | Self::Text
            | Self::Clear => ToolbarCluster::Tools,
            Self::Ocr | Self::Save | Self::Pin | Self::Scroll => ToolbarCluster::Actions,
            Self::Cancel | Self::Copy => ToolbarCluster::Confirm,
            // app-level, not selection-level — groups with the confirm
            // end of the bar and the palette's Actions section
            Self::Settings => ToolbarCluster::Confirm,
        }
    }
}

/// One slot of the configured toolbar row. A row is an ordered list of
/// these — the order IS the customization.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarItem {
    Button(ToolbarButton),
    /// The thin vertical group break. Any number may appear — grouping
    /// is the one layout concept the toolbar needs (a "flexible gap"
    /// variant was tried and dropped: the bar hugs its content, so a
    /// flexible gap can never flex — it was just a separator without
    /// the line).
    Separator,
}

impl ToolbarItem {
    /// Stable id for the settings file (same vocabulary as button ids).
    pub const fn id(self) -> &'static str {
        match self {
            Self::Button(button) => button.id(),
            Self::Separator => "separator",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "separator" => Some(Self::Separator),
            _ => ToolbarButton::from_id(id).map(Self::Button),
        }
    }

    /// Human name for the drag ghost / palette chip of a layout item.
    pub fn layout_name(self) -> Option<&'static str> {
        match self {
            Self::Separator => Some("Separator"),
            Self::Button(_) => None,
        }
    }
}

/// The factory row — the maintainer's preferred default (captured
/// from a live editor session): the tool cluster, then the action
/// group ending in Copy, then a group break before the gear-and-
/// cancel pair. A fresh install, "Reset to defaults" and legacy
/// `hidden`-format migration all land here.
pub const DEFAULT_ROW: [ToolbarItem; 22] = [
    ToolbarItem::Button(ToolbarButton::Select),
    ToolbarItem::Button(ToolbarButton::Rectangle),
    ToolbarItem::Button(ToolbarButton::Ellipse),
    ToolbarItem::Button(ToolbarButton::Line),
    ToolbarItem::Button(ToolbarButton::Polyline),
    ToolbarItem::Button(ToolbarButton::Arrow),
    ToolbarItem::Button(ToolbarButton::Number),
    ToolbarItem::Button(ToolbarButton::Pencil),
    ToolbarItem::Button(ToolbarButton::Highlighter),
    ToolbarItem::Button(ToolbarButton::Mosaic),
    ToolbarItem::Button(ToolbarButton::Eraser),
    ToolbarItem::Button(ToolbarButton::Text),
    ToolbarItem::Button(ToolbarButton::Clear),
    ToolbarItem::Separator,
    ToolbarItem::Button(ToolbarButton::Ocr),
    ToolbarItem::Button(ToolbarButton::Pin),
    ToolbarItem::Button(ToolbarButton::Scroll),
    ToolbarItem::Button(ToolbarButton::Save),
    ToolbarItem::Button(ToolbarButton::Copy),
    ToolbarItem::Separator,
    ToolbarItem::Button(ToolbarButton::Settings),
    ToolbarItem::Button(ToolbarButton::Cancel),
];

/// The curated OCR model sets a user can pick (settings window → OCR).
/// The `id()` strings are the file format AND rapidocr-core's registry
/// names — one vocabulary, never rename one. Sets NOT listed here
/// (rapidocr ships more, e.g. the superseded ppocrv4-en-mobile) stay
/// accepted on load for forward/backward compat but are not offered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OcrModel {
    Ppocrv6Tiny,
    Ppocrv6Small,
    Ppocrv6Medium,
    Ppocrv5EnMobile,
    Ppocrv5ChMobile,
    Ppocrv5ChServer,
}

impl OcrModel {
    /// Every offered model, in settings-display order: the speed/quality
    /// ladder first (the default sits in the middle), then language
    /// variants.
    pub const ALL: [Self; 6] = [
        Self::Ppocrv6Tiny,
        Self::Ppocrv6Small,
        Self::Ppocrv6Medium,
        Self::Ppocrv5EnMobile,
        Self::Ppocrv5ChMobile,
        Self::Ppocrv5ChServer,
    ];

    /// Stable id for the settings file (= rapidocr's registry name).
    pub const fn id(self) -> &'static str {
        match self {
            Self::Ppocrv6Tiny => "ppocrv6-tiny",
            Self::Ppocrv6Small => "ppocrv6-small",
            Self::Ppocrv6Medium => "ppocrv6-medium",
            Self::Ppocrv5EnMobile => "ppocrv5-en-mobile",
            Self::Ppocrv5ChMobile => "ppocrv5-ch-mobile",
            Self::Ppocrv5ChServer => "ppocrv5-ch-server",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|m| m.id() == id)
    }

    /// Human-readable model name (setup dialog, settings rows).
    /// Locale-dependent since i18n — resolved through the shared table
    /// (`ocr.model.{id}.label`), so this cannot stay `const`. The key
    /// is derived from the stable [`OcrModel::id`] at runtime (hence
    /// `String`, not `Cow<'static>`: rust-i18n's missing-key fallback
    /// borrows the lookup key); a missing column degrades to English.
    pub fn label(self) -> String {
        let key = format!("ocr.model.{}.label", self.id());
        crate::i18n::t!(key.as_str()).to_string()
    }

    /// Which scripts the recognition dictionary covers
    /// (`ocr.model.{id}.langs`).
    pub fn languages(self) -> String {
        let key = format!("ocr.model.{}.langs", self.id());
        crate::i18n::t!(key.as_str()).to_string()
    }

    /// Speed/accuracy/download positioning
    /// (`ocr.model.{id}.tier`).
    pub fn tier(self) -> String {
        let key = format!("ocr.model.{}.tier", self.id());
        crate::i18n::t!(key.as_str()).to_string()
    }
}

/// The UI language (settings.toml top-level `language` key). `Auto`
/// follows the system locale; the rest name a `locales/ui.yml` column.
/// Unknown values degrade to Auto with a load warning (same contract
/// as every other id in this file).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    Auto,
    English,
    ChineseSimplified,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::Auto, Self::English, Self::ChineseSimplified];

    /// Stable id for the settings file (= the rust-i18n locale name,
    /// except Auto which defers to the system).
    pub const fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::ChineseSimplified => "zh-CN",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|l| l.id() == id)
    }

    /// Sidebar-row label. Only the "Auto" row is translated (languages
    /// name themselves — an English row must read "English" in every
    /// locale). `String` for the same dynamic-key reason as
    /// [`OcrModel::label`].
    pub fn label(self) -> String {
        match self {
            Self::Auto => crate::i18n::t!("settings.general.language.system").to_string(),
            Self::English => "English".to_string(),
            Self::ChineseSimplified => "简体中文".to_string(),
        }
    }
}

/// The factory OCR configuration — deliberately the historical default,
/// so existing installs see no behavior change until they opt in.
pub const DEFAULT_OCR_MODEL: OcrModel = OcrModel::Ppocrv6Small;

/// User settings. Default = the factory row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    row: Vec<ToolbarItem>,
    ocr_model: OcrModel,
    /// Assume text lines are upright and skip the orientation
    /// classifier — screenshots almost always are, so this trades a
    /// rare rotated-line fixup for one inference stage less.
    ocr_assume_upright: bool,
    /// UI language (`language` top-level key; Auto = system locale).
    language: Language,
    /// Settings-window sidebar collapsed (written through on every
    /// toggle; the next `shotori settings` restores the rail).
    sidebar_collapsed: bool,
    /// Pixel-based smart region selection (hover outline, click snap,
    /// wheel hierarchy over detector candidates). Off = every selection
    /// is drawn by hand. Compositor window snapping is a different
    /// layer and stays available either way (the detector's quality
    /// itself is issue #27's topic).
    smart_region: bool,
    /// The annotation swatch board (`[toolbar] colors`) — the brush
    /// colors the overlay's row two offers. See [`PALETTE`].
    annotation_colors: [u32; PALETTE],
    /// The configurable shortcut keystrokes, resolved: factory
    /// defaults with the file's `[keys]` overrides applied (see
    /// `crate::actions::CONFIGURABLE_KEYS` for the id list).
    keys: std::collections::BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            row: DEFAULT_ROW.to_vec(),
            ocr_model: DEFAULT_OCR_MODEL,
            ocr_assume_upright: false,
            language: Language::Auto,
            sidebar_collapsed: false,
            smart_region: true,
            annotation_colors: DEFAULT_ANNOTATION_COLORS,
            keys: crate::actions::default_keys(),
        }
    }
}

/// The subset of the file this version understands. Unknown keys are
/// ignored on load (forward compat) and preserved on save.
#[derive(Deserialize, Default)]
#[serde(default)]
struct File {
    toolbar: ToolbarFile,
    ocr: OcrFile,
    /// Per-command keystroke overrides (`[keys]`, id → keystroke —
    /// see `crate::actions::CONFIGURABLE_KEYS`). Absent = all factory.
    keys: Option<std::collections::BTreeMap<String, String>>,
    /// Top-level `language = "auto" | "en" | "zh-CN"`.
    language: Option<String>,
    /// Settings-window sidebar collapsed (`sidebar_collapsed`,
    /// absent = expanded).
    sidebar_collapsed: Option<bool>,
    /// Smart region selection on/off (`smart_region`, absent = on —
    /// preference files written before the key existed keep the
    /// shipped behavior).
    smart_region: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ToolbarFile {
    /// The ordered row (current format). Absent → fall back to
    /// `hidden` (the pre-ordering format), then the factory row.
    items: Option<Vec<String>>,
    /// Legacy visibility-only format: factory row minus these ids.
    hidden: Vec<String>,
    /// The annotation swatch board, as #RRGGBBAA hex strings (exactly
    /// [`PALETTE`] of them, opaque). Absent → the factory board.
    colors: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OcrFile {
    /// Rapidocr registry name of the model set (see [`OcrModel::id`]).
    model: Option<String>,
    /// Skip the text-line orientation classifier (screenshots are
    /// upright); absent = false.
    assume_upright: Option<bool>,
}

impl Settings {
    /// Commit a drop into preview slot `slot` (slot i sits before row
    /// item i as DISPLAYED during the drag — i.e. after the lifted
    /// item has left the flow; the editor hides the carried item, so
    /// what the gap-preview showed is exactly what this lands). `from`
    /// = `Some(src)` reorders an item already on the row (`src` is the
    /// ORIGINAL index); `None` inserts a palette item against the
    /// full row.
    pub fn apply_drop(
        item: ToolbarItem,
        row: &[ToolbarItem],
        slot: usize,
        from: Option<usize>,
    ) -> Vec<ToolbarItem> {
        let mut row = row.to_vec();
        let item = match from {
            Some(src) if src < row.len() => row.remove(src),
            _ => item,
        };
        row.insert(slot.min(row.len()), item);
        row
    }
    /// The configured row, in display order.
    pub fn row(&self) -> &[ToolbarItem] {
        &self.row
    }

    /// Install a new row (the settings editor's commits; rows built by
    /// the editor are valid by construction — sanitization happens at
    /// load time).
    pub fn set_row(&mut self, row: Vec<ToolbarItem>) {
        self.row = row;
    }

    /// The selected OCR model set (default: [`DEFAULT_OCR_MODEL`]).
    pub fn ocr_model(&self) -> OcrModel {
        self.ocr_model
    }

    pub fn set_ocr_model(&mut self, model: OcrModel) {
        self.ocr_model = model;
    }

    /// Whether the orientation classifier is skipped (upright text
    /// assumed). Kept alongside the model: together they decide which
    /// files the OCR engine needs.
    pub fn ocr_assume_upright(&self) -> bool {
        self.ocr_assume_upright
    }

    pub fn set_ocr_assume_upright(&mut self, assume: bool) {
        self.ocr_assume_upright = assume;
    }

    /// The UI language (default Auto = follow the system).
    pub fn language(&self) -> Language {
        self.language
    }

    pub fn set_language(&mut self, language: Language) {
        self.language = language;
    }

    /// Settings-window sidebar collapsed (persisted; absent = expanded).
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }

    pub fn set_sidebar_collapsed(&mut self, collapsed: bool) {
        self.sidebar_collapsed = collapsed;
    }

    /// Whether pixel-based smart region selection is enabled (default on).
    pub fn smart_region(&self) -> bool {
        self.smart_region
    }

    pub fn set_smart_region(&mut self, enabled: bool) {
        self.smart_region = enabled;
    }

    /// The resolved shortcut keystrokes (id → keystroke, defaults
    /// included). The settings editor reads this to render the key
    /// caps; the overlay binds from it at startup.
    pub fn keys(&self) -> &std::collections::BTreeMap<String, String> {
        &self.keys
    }

    /// Rebind one configurable command (the shortcut recorder; the id
    /// and keystroke were validated against the registry before this).
    pub fn set_key(&mut self, id: &str, keystroke: String) {
        self.keys.insert(id.to_owned(), keystroke);
    }

    /// Restore every shortcut to its factory keystroke.
    pub fn reset_keys(&mut self) {
        self.keys = crate::actions::default_keys();
    }

    /// A button shows iff it has a slot in the row; hidden buttons
    /// keep their keyboard shortcuts.
    pub fn toolbar_visible(&self, button: ToolbarButton) -> bool {
        self.row.contains(&ToolbarItem::Button(button))
    }

    /// Row composition stats for the placement width basis (see
    /// `model::placement::row1_basis`).
    pub fn row1_button_count(&self) -> usize {
        self.row
            .iter()
            .filter(|item| matches!(item, ToolbarItem::Button(_)))
            .count()
    }

    pub fn row1_separator_count(&self) -> usize {
        self.row
            .iter()
            .filter(|item| matches!(item, ToolbarItem::Separator))
            .count()
    }

    /// Drop a button's duplicate slots (keep the first — a button
    /// dispatches one action, listing it twice is a hand-edit typo)
    /// and report each drop.
    fn sanitize(row: Vec<ToolbarItem>) -> (Vec<ToolbarItem>, Vec<String>) {
        let mut seen = std::collections::BTreeSet::new();
        let mut clean = Vec::with_capacity(row.len());
        let mut errors = Vec::new();
        for (position, item) in row.into_iter().enumerate() {
            if let ToolbarItem::Button(button) = item
                && !seen.insert(button)
            {
                errors.push(format!(
                    "duplicate toolbar button {} at position {} (kept the first)",
                    button.id(),
                    position
                ));
                continue;
            }
            clean.push(item);
        }
        (clean, errors)
    }

    /// The annotation swatch board ([`PALETTE`] colors, opaque).
    pub fn annotation_colors(&self) -> &[u32; PALETTE] {
        &self.annotation_colors
    }

    /// Load from `path` (missing file → factory row, silently). Errors
    /// are returned, never fatal: a broken file must not take the
    /// screenshot session down with it.
    pub fn load(path: Option<&Path>) -> (Self, Vec<String>) {
        let Some(path) = path else {
            return (Self::default(), Vec::new());
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), Vec::new());
            }
            Err(e) => return (Self::default(), vec![format!("{}: {e}", path.display())]),
        };
        let file: File = match toml::from_str(&text) {
            Ok(file) => file,
            Err(e) => return (Self::default(), vec![format!("{}: {e}", path.display())]),
        };
        let mut errors = Vec::new();
        let row = if let Some(items) = file.toolbar.items {
            let mut row = Vec::with_capacity(items.len());
            for id in items {
                match ToolbarItem::from_id(&id) {
                    Some(item) => row.push(item),
                    None => errors.push(format!(
                        "{}: unknown toolbar item {id:?} (skipped; known ids: {})",
                        path.display(),
                        known_ids()
                    )),
                }
            }
            row
        } else if !file.toolbar.hidden.is_empty() {
            // Legacy `hidden` files (visibility-only customization):
            // factory row minus the hidden buttons — order untouched.
            let mut hidden = std::collections::BTreeSet::new();
            for id in file.toolbar.hidden {
                match ToolbarButton::from_id(&id) {
                    Some(button) => {
                        hidden.insert(button);
                    }
                    None => errors.push(format!(
                        "{}: unknown toolbar button {id:?} (skipped)",
                        path.display()
                    )),
                }
            }
            DEFAULT_ROW
                .into_iter()
                .filter(|item| !matches!(item, ToolbarItem::Button(b) if hidden.contains(b)))
                .collect()
        } else {
            DEFAULT_ROW.to_vec()
        };
        let (row, sanitize_errors) = Self::sanitize(row);
        errors.extend(sanitize_errors);
        // [ocr]: an unknown model id (hand-edit, or a file written by a
        // newer/older shotori) degrades to the default with a warning —
        // same contract as unknown toolbar ids.
        let ocr_model = match file.ocr.model.as_deref() {
            None => DEFAULT_OCR_MODEL,
            Some(id) => match OcrModel::from_id(id) {
                Some(model) => model,
                None => {
                    errors.push(format!(
                        "{}: unknown OCR model {id:?} (using {})",
                        path.display(),
                        DEFAULT_OCR_MODEL.id()
                    ));
                    DEFAULT_OCR_MODEL
                }
            },
        };
        let ocr_assume_upright = file.ocr.assume_upright.unwrap_or(false);
        // [toolbar] colors: the swatch board. Same degrade contract as
        // every other field — any problem (wrong count, bad hex,
        // translucent color) falls back to the factory board with a
        // warning, never a fatal error.
        let mut annotation_colors = DEFAULT_ANNOTATION_COLORS;
        if let Some(colors) = &file.toolbar.colors {
            match parse_palette(colors) {
                Ok(parsed) => annotation_colors = parsed,
                Err(e) => errors.push(format!(
                    "{}: toolbar colors: {e} (using the factory board)",
                    path.display()
                )),
            }
        }
        // Top-level language: unknown value (hand-edit, newer shotori)
        // degrades to Auto with a warning.
        let language = match file.language.as_deref() {
            None => Language::Auto,
            Some(id) => match Language::from_id(id) {
                Some(language) => language,
                None => {
                    errors.push(format!(
                        "{}: unknown language {id:?} (using auto)",
                        path.display()
                    ));
                    Language::Auto
                }
            },
        };
        // [keys]: resolved against the registry — unknown ids,
        // unparseable keystrokes and duplicates degrade per entry with
        // a warning (same contract as the other fields).
        let empty = std::collections::BTreeMap::new();
        let keys =
            crate::actions::resolve_keys(path, file.keys.as_ref().unwrap_or(&empty), &mut errors);
        (
            Self {
                row,
                ocr_model,
                ocr_assume_upright,
                language,
                sidebar_collapsed: file.sidebar_collapsed.unwrap_or(false),
                smart_region: file.smart_region.unwrap_or(true),
                annotation_colors,
                keys,
            },
            errors,
        )
    }

    /// Write through to `path`. The existing document is parsed and
    /// re-serialized so unknown keys/sections (written by a newer
    /// shotori) survive the edit; a corrupt existing file is replaced
    /// outright — it was unusable, and the editor is the recovery
    /// path. Comments do not survive (Value-level round-trip). The
    /// legacy `hidden` key is removed: `items` supersedes it.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        // toml::from_str, not str::parse: Value's FromStr accepts a
        // single VALUE expression ("[a]\nx=1" parses as an array then
        // chokes on the rest) — a document needs the serde path.
        let mut doc: toml::Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
        let items = toml::Value::Array(
            self.row
                .iter()
                .map(|item| toml::Value::from(item.id()))
                .collect(),
        );
        let Some(table) = doc.as_table_mut() else {
            return Err(format!(
                "{}: not a settings table — refusing to overwrite",
                path.display()
            ));
        };
        let toolbar = table
            .entry("toolbar")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(toolbar) = toolbar.as_table_mut() else {
            return Err(format!(
                "{}: [toolbar] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        toolbar.remove("hidden");
        toolbar.insert("items".into(), items);
        let colors = toml::Value::Array(
            self.annotation_colors
                .iter()
                .map(|c| toml::Value::from(format!("#{c:08X}")))
                .collect(),
        );
        toolbar.insert("colors".into(), colors);
        table.insert("language".into(), toml::Value::from(self.language.id()));
        table.insert(
            "sidebar_collapsed".into(),
            toml::Value::from(self.sidebar_collapsed),
        );
        table.insert("smart_region".into(), toml::Value::from(self.smart_region));
        let ocr = table
            .entry("ocr")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(ocr) = ocr.as_table_mut() else {
            return Err(format!(
                "{}: [ocr] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        ocr.insert("model".into(), toml::Value::from(self.ocr_model.id()));
        ocr.insert(
            "assume_upright".into(),
            toml::Value::from(self.ocr_assume_upright),
        );
        // [keys]: overwrite the known ids, keep unknown ones (the same
        // forward-compat contract as every other section)
        let keys = table
            .entry("keys")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(keys) = keys.as_table_mut() else {
            return Err(format!(
                "{}: [keys] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        for (id, keystroke) in &self.keys {
            keys.insert(id.clone(), toml::Value::from(keystroke.as_str()));
        }
        let body = toml::to_string(&doc).map_err(|e| format!("{}: {e}", path.display()))?;
        let text = format!(
            "# Shotori settings — managed by `shotori settings`; unknown keys are preserved.\n{body}"
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// All valid ids, comma-joined for error messages.
fn known_ids() -> String {
    let mut ids: Vec<&str> = ToolbarButton::ALL.iter().map(|b| b.id()).collect();
    ids.extend(["separator"]);
    ids.join(", ")
}

/// `~/.config/shotori` (XDG_CONFIG_HOME-aware) — shared by the theme
/// and settings loaders so the two files can never disagree about
/// where the config dir is.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join(crate::APP_ID))
}

/// Where settings.toml lives, if a config dir is resolvable at all.
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(FILE_NAME))
}

// The installed settings as a leaked box: `current()` hands out
// &'static Settings (every read site borrows it), so a runtime edit
// swaps the pointer instead of mutating in place. Each edit leaks one
// Settings (~a hundred bytes) — the overlay is run-and-exit and the
// settings window edits at human speed, so this is bounded and
// deliberate; the alternative (current() returning a clone per read)
// taxes every render frame to save nothing.
static ACTIVE: RwLock<Option<&'static Settings>> = RwLock::new(None);
static FACTORY: OnceLock<Settings> = OnceLock::new();

/// The overlay path: update the ACTIVE annotation board in-process
/// and write the file through. A failed save keeps the in-memory edit
/// (the running session keeps the color; the next capture falls back
/// to whatever the file says) and reports on stderr.
pub fn set_annotation_color(ix: usize, color: u32) {
    if ix >= PALETTE {
        return;
    }
    let mut next = current().clone();
    next.annotation_colors[ix] = color;
    install(next);
    if let Some(path) = config_path()
        && let Err(e) = current().save(&path)
    {
        eprintln!("[shotori] settings: {e}");
    }
}

/// Install a settings value as the process's active one.
fn install(settings: Settings) {
    let leaked: &'static Settings = Box::leak(Box::new(settings));
    if let Ok(mut guard) = ACTIVE.write() {
        *guard = Some(leaked);
    }
}

/// Parse the `[toolbar] colors` array: exactly [`PALETTE`] opaque hex
/// colors (#RRGGBB or #RRGGBBAA with AA=FF — exported PNGs must be
/// solid). Hand-rolled rather than shared with the theme loader: the
/// dependency direction is model ← ui, never the other way.
fn parse_palette(colors: &[String]) -> Result<[u32; PALETTE], String> {
    if colors.len() != PALETTE {
        return Err(format!(
            "expected exactly {PALETTE} colors, got {}",
            colors.len()
        ));
    }
    let mut parsed = DEFAULT_ANNOTATION_COLORS;
    for (i, color) in colors.iter().enumerate() {
        parsed[i] = parse_hex_color(color)
            .map_err(|e| format!("swatch {} ({:?}): {e}", i + 1, PALETTE_NAMES[i]))?;
    }
    Ok(parsed)
}

fn parse_hex_color(s: &str) -> Result<u32, String> {
    let digits = s
        .strip_prefix('#')
        .ok_or_else(|| format!("{s:?}: expected #RRGGBB or #RRGGBBAA"))?;
    let value = u32::from_str_radix(digits, 16).map_err(|_| format!("{s:?} is not a hex color"))?;
    let value = match digits.len() {
        6 => value << 8 | 255,
        8 => value,
        n => return Err(format!("{s:?}: want 6 or 8 hex digits, got {n}")),
    };
    if value & 0xFF != 0xFF {
        return Err(format!("{s:?}: swatches must be opaque (alpha FF)"));
    }
    Ok(value)
}

/// The process-wide settings, installed once at startup ([`init`]).
/// Before/without init: the factory row (lazily materialized — a
/// `Vec`-backed default is not const-constructible with its content).
pub fn current() -> &'static Settings {
    if let Some(settings) = ACTIVE.read().ok().and_then(|guard| *guard) {
        return settings;
    }
    FACTORY.get_or_init(Settings::default)
}

/// Startup pickup: read the config file (unless `--no-config`), report
/// problems to stderr like the theme loader does, install the result.
pub fn init(args: &crate::args::Cli) {
    let path = (!args.no_config).then(config_path).flatten();
    let (settings, errors) = Settings::load(path.as_deref());
    for error in &errors {
        eprintln!("[shotori] settings: {error}");
    }
    install(settings);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_ids_round_trip_and_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for button in ToolbarButton::ALL {
            assert_eq!(ToolbarButton::from_id(button.id()), Some(button));
            assert!(seen.insert(button.id()), "duplicate id {}", button.id());
        }
        assert!(seen.insert("separator"), "duplicate id separator");
        assert_eq!(
            ToolbarItem::from_id("separator"),
            Some(ToolbarItem::Separator)
        );
        assert_eq!(ToolbarItem::from_id("translate"), None);
        // the dropped spacer variant degrades to a warning, not a load
        // failure (files written by the short-lived v1 editor)
        assert_eq!(ToolbarItem::from_id("spacer"), None);
    }

    #[test]
    fn factory_row_is_the_maintainer_default() {
        let row = Settings::default().row().to_vec();
        // tools | ocr pin scroll save copy | settings cancel — two
        // group breaks, cancel (not the gear) closes the row
        assert_eq!(row.len(), 22);
        assert_eq!(row[0], ToolbarItem::Button(ToolbarButton::Select));
        assert_eq!(row[13], ToolbarItem::Separator);
        assert_eq!(row[19], ToolbarItem::Separator);
        assert_eq!(row[20], ToolbarItem::Button(ToolbarButton::Settings));
        assert_eq!(
            *row.last().unwrap(),
            ToolbarItem::Button(ToolbarButton::Cancel)
        );
        // every button placed exactly once
        for button in ToolbarButton::ALL {
            assert!(Settings::default().toolbar_visible(button));
        }
        assert_eq!(Settings::default().row1_button_count(), 20);
        assert_eq!(Settings::default().row1_separator_count(), 2);
    }

    #[test]
    fn row_order_controls_visibility_and_sequence() {
        let mut s = Settings::default();
        s.set_row(vec![
            ToolbarItem::Button(ToolbarButton::Copy),
            ToolbarItem::Button(ToolbarButton::Select),
            ToolbarItem::Separator,
        ]);
        assert!(s.toolbar_visible(ToolbarButton::Copy));
        assert!(!s.toolbar_visible(ToolbarButton::Save));
        assert_eq!(s.row1_button_count(), 2);
        assert_eq!(s.row1_separator_count(), 1);
        // an empty row is a valid "hide everything" toolbar
        s.set_row(Vec::new());
        assert_eq!(s.row1_button_count(), 0);
    }

    #[test]
    fn load_missing_file_is_factory_silently() {
        let (s, errors) = Settings::load(Some(Path::new("/nonexistent/shotori/settings.toml")));
        assert_eq!(s, Settings::default());
        assert!(errors.is_empty());
    }

    #[test]
    fn load_reads_the_ordered_row_and_skips_unknown_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "[toolbar]\nitems = [\"copy\", \"bogus\", \"separator\", \"save\"]\n",
        )
        .unwrap();
        let (s, errors) = Settings::load(Some(&path));
        assert_eq!(
            s.row(),
            &[
                ToolbarItem::Button(ToolbarButton::Copy),
                ToolbarItem::Separator,
                ToolbarItem::Button(ToolbarButton::Save),
            ]
        );
        assert_eq!(errors.len(), 1, "the unknown id must be reported");
        assert!(errors[0].contains("bogus"));
        assert!(
            errors[0].contains("separator"),
            "known ids are listed in the error"
        );
    }

    #[test]
    fn drops_reorder_and_insert_at_the_right_slot() {
        use ToolbarButton as B;
        let btn = |b: ToolbarButton| ToolbarItem::Button(b);
        let row = [btn(B::Copy), btn(B::Save), btn(B::Pin)];

        // palette insert at the front / middle / past-the-end (clamps)
        assert_eq!(
            Settings::apply_drop(btn(B::Ocr), &row, 0, None),
            [btn(B::Ocr), btn(B::Copy), btn(B::Save), btn(B::Pin)]
        );
        assert_eq!(
            Settings::apply_drop(btn(B::Ocr), &row, 2, None),
            [btn(B::Copy), btn(B::Save), btn(B::Ocr), btn(B::Pin)]
        );
        assert_eq!(
            Settings::apply_drop(btn(B::Ocr), &row, 99, None),
            [btn(B::Copy), btn(B::Save), btn(B::Pin), btn(B::Ocr)]
        );

        // reorder: slots are indexed against the displayed row — the
        // lifted item is already gone. Copy lifts → [Save, Pin]; the
        // gap between Save and Pin is slot 1
        assert_eq!(
            Settings::apply_drop(btn(B::Copy), &row, 1, Some(0)),
            [btn(B::Save), btn(B::Copy), btn(B::Pin)]
        );
        // drag Pin (slot 2) to the very front: Pin lifts → [Copy, Save],
        // slot 0 = before Copy
        assert_eq!(
            Settings::apply_drop(btn(B::Pin), &row, 0, Some(2)),
            [btn(B::Pin), btn(B::Copy), btn(B::Save)]
        );
        // drop back onto its own gap: Save lifts → [Copy, Pin], the
        // middle gap is slot 1 — no-op
        assert_eq!(
            Settings::apply_drop(btn(B::Save), &row, 1, Some(1)),
            row.to_vec()
        );
        // a stale source (row changed mid-drag) degrades to an insert
        assert_eq!(
            Settings::apply_drop(btn(B::Pin), &row, 0, Some(9)),
            [btn(B::Pin), btn(B::Copy), btn(B::Save), btn(B::Pin)]
        );

        // layout items stack: a second separator drops in fine
        let with_sep = [btn(B::Copy), ToolbarItem::Separator];
        assert_eq!(
            Settings::apply_drop(ToolbarItem::Separator, &with_sep, 2, None),
            [btn(B::Copy), ToolbarItem::Separator, ToolbarItem::Separator]
        );
    }

    #[test]
    fn load_dedupes_repeated_buttons_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "[toolbar]\nitems = [\"copy\", \"copy\", \"save\"]\n").unwrap();
        let (s, errors) = Settings::load(Some(&path));
        assert_eq!(s.row1_button_count(), 2);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("duplicate"));
    }

    #[test]
    fn legacy_hidden_files_keep_working() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "[toolbar]\nhidden = [\"pin\", \"scroll\", \"bogus\"]\n",
        )
        .unwrap();
        let (s, errors) = Settings::load(Some(&path));
        assert!(!s.toolbar_visible(ToolbarButton::Pin));
        assert!(!s.toolbar_visible(ToolbarButton::Scroll));
        assert!(s.toolbar_visible(ToolbarButton::Copy));
        // factory order otherwise, both separators intact
        assert_eq!(s.row1_button_count(), 18);
        assert_eq!(s.row1_separator_count(), 2);
        assert_eq!(errors.len(), 1, "unknown hidden id reported");
    }

    #[test]
    fn load_corrupt_file_falls_back_with_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "not [ toml").unwrap();
        let (s, errors) = Settings::load(Some(&path));
        assert_eq!(s, Settings::default());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE_NAME);
        let mut s = Settings::default();
        s.set_row(vec![
            ToolbarItem::Button(ToolbarButton::Pin),
            ToolbarItem::Separator,
            ToolbarItem::Button(ToolbarButton::Eraser),
        ]);
        s.save(&path).expect("save must create parent dirs");
        let (back, errors) = Settings::load(Some(&path));
        assert!(errors.is_empty());
        assert_eq!(back, s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[toolbar]"));
        assert!(text.contains("\"separator\""));
    }

    #[test]
    fn keys_round_trip_and_per_entry_degrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut s = Settings::default();
        s.set_key("copy", "ctrl-y".into());
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[keys]"));
        let (back, errors) = Settings::load(Some(&path));
        assert!(errors.is_empty());
        assert_eq!(back, s);

        // Hand-edited file: the valid override applies; the invalid
        // keystroke and the unknown id each warn and degrade.
        std::fs::write(
            &path,
            "[keys]\ncopy = \"ctrl-y\"\nsave = \"oops\"\nlegacy = \"ctrl-1\"\n",
        )
        .unwrap();
        let (back, errors) = Settings::load(Some(&path));
        assert_eq!(back.keys().get("copy").unwrap(), "ctrl-y");
        assert_eq!(back.keys().get("save").unwrap(), "ctrl-s");
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn save_preserves_unknown_keys_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "[keys]\nfuture_id = \"ctrl-9\"\n").unwrap();
        Settings::default().save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("future_id"), "unknown ids survive a save");
    }

    #[test]
    fn save_preserves_unknown_keys_and_drops_legacy_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "[toolbar]\nhidden = [\"pin\"]\n[future_section]\nlanguage = \"de\"\n",
        )
        .unwrap();
        Settings::default().save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("future_section"),
            "unknown sections must survive an edit: {text}"
        );
        assert!(
            !text.contains("hidden"),
            "the legacy key must not survive next to items: {text}"
        );
        assert!(text.contains("items"));
    }

    #[test]
    fn ocr_model_metadata_keys_all_resolve() {
        // label/languages/tier derive their table keys from id() at
        // runtime — a typo'd id column would silently fall back to the
        // KEY ITSELF in the UI. Guard: every field must resolve to
        // something other than the key.
        for model in OcrModel::ALL {
            for (field, value) in [
                ("label", model.label()),
                ("langs", model.languages()),
                ("tier", model.tier()),
            ] {
                assert_ne!(
                    value,
                    format!("ocr.model.{}.{}", model.id(), field),
                    "{} table column is missing for {}",
                    field,
                    model.id()
                );
            }
        }
    }

    #[test]
    fn language_ids_round_trip_and_default_is_auto() {
        for language in Language::ALL {
            assert_eq!(Language::from_id(language.id()), Some(language));
        }
        assert_eq!(Language::from_id("zh-TW"), None);
        assert_eq!(Settings::default().language(), Language::Auto);

        // file round trip + unknown degradation
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut s = Settings::default();
        s.set_language(Language::ChineseSimplified);
        s.save(&path).unwrap();
        let (back, errors) = Settings::load(Some(&path));
        assert!(errors.is_empty());
        assert_eq!(back.language(), Language::ChineseSimplified);
        assert!(std::fs::read_to_string(&path).unwrap().contains("zh-CN"));

        std::fs::write(&path, "language = \"klingon\"\n").unwrap();
        let (s, errors) = Settings::load(Some(&path));
        assert_eq!(s.language(), Language::Auto);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("klingon"));
    }

    #[test]
    fn ocr_model_ids_round_trip_and_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for model in OcrModel::ALL {
            assert_eq!(OcrModel::from_id(model.id()), Some(model));
            assert!(seen.insert(model.id()), "duplicate id {}", model.id());
            // the curated ids must all exist in rapidocr's registry —
            // ocr.rs resolves them by name at engine init
            assert!(
                rapidocr_core::model::model_set_by_name(model.id()).is_some(),
                "{} is not a registered rapidocr model set",
                model.id()
            );
        }
        assert_eq!(OcrModel::from_id("ppocrv4-en-mobile"), None);
        assert_eq!(Settings::default().ocr_model(), DEFAULT_OCR_MODEL);
        assert!(!Settings::default().ocr_assume_upright());
    }

    #[test]
    fn ocr_settings_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut s = Settings::default();
        s.set_ocr_model(OcrModel::Ppocrv5EnMobile);
        s.set_ocr_assume_upright(true);
        s.save(&path).unwrap();
        let (back, errors) = Settings::load(Some(&path));
        assert!(errors.is_empty());
        assert_eq!(back.ocr_model(), OcrModel::Ppocrv5EnMobile);
        assert!(back.ocr_assume_upright());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[ocr]"));
        assert!(text.contains("ppocrv5-en-mobile"));
    }

    #[test]
    fn sidebar_collapse_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut s = Settings::default();
        s.set_sidebar_collapsed(true);
        s.save(&path).unwrap();
        let (back, errors) = Settings::load(Some(&path));
        assert!(errors.is_empty());
        assert!(back.sidebar_collapsed());
        // the factory default is expanded, and an absent key loads as
        // expanded (preference files written before the key existed)
        assert!(!Settings::default().sidebar_collapsed());
        let (fresh, _) = Settings::load(Some(&dir.path().join("nonexistent.toml")));
        assert!(!fresh.sidebar_collapsed());
    }

    #[test]
    fn smart_region_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut s = Settings::default();
        assert!(s.smart_region());
        s.set_smart_region(false);
        s.save(&path).unwrap();
        let (back, errors) = Settings::load(Some(&path));
        assert!(errors.is_empty());
        assert!(!back.smart_region());
        // The factory default is enabled, and an absent key (a
        // preference file written before the key existed) loads as
        // enabled — turning the feature off is an explicit act.
        assert!(Settings::default().smart_region());
        let (fresh, _) = Settings::load(Some(&dir.path().join("nonexistent.toml")));
        assert!(fresh.smart_region());
    }

    #[test]
    fn unknown_ocr_model_degrades_to_the_default_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "[ocr]\nmodel = \"ppocrv4-en-mobile\"\nassume_upright = true\n",
        )
        .unwrap();
        let (s, errors) = Settings::load(Some(&path));
        assert_eq!(s.ocr_model(), DEFAULT_OCR_MODEL);
        // the toggle is independent of the model id — it survives
        assert!(s.ocr_assume_upright());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("ppocrv4-en-mobile"));
    }

    #[test]
    fn ocr_save_preserves_unknown_keys_in_the_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "[ocr]\nmodel = \"ppocrv6-tiny\"\ntext_score = 0.7\n").unwrap();
        let mut s = Settings::default();
        s.set_ocr_model(OcrModel::Ppocrv6Medium);
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("text_score"),
            "unknown [ocr] keys must survive an edit: {text}"
        );
        assert!(text.contains("ppocrv6-medium"));
    }

    #[test]
    fn annotation_colors_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut s = Settings::default();
        s.annotation_colors[2] = 0x123456FF;
        s.save(&path).unwrap();
        let (loaded, errors) = Settings::load(Some(&path));
        assert!(errors.is_empty());
        assert_eq!(loaded.annotation_colors[2], 0x123456FF);
        // untouched swatches keep the factory values
        assert_eq!(loaded.annotation_colors[0], DEFAULT_ANNOTATION_COLORS[0]);
    }

    #[test]
    fn bad_annotation_colors_degrade_to_the_factory_board() {
        for body in [
            // wrong count
            "[toolbar]\ncolors = [\"#111111\"]\n",
            // not hex
            "[toolbar]\ncolors = [\"red\", \"#FF8A32\", \"#FFD43B\", \"#40C878\", \"#409CFF\", \"#222222\", \"#FFFFFF\"]\n",
            // translucent — exported PNGs must be solid
            "[toolbar]\ncolors = [\"#FF454580\", \"#FF8A32\", \"#FFD43B\", \"#40C878\", \"#409CFF\", \"#222222\", \"#FFFFFF\"]\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(FILE_NAME);
            std::fs::write(&path, body).unwrap();
            let (loaded, errors) = Settings::load(Some(&path));
            assert_eq!(loaded.annotation_colors, DEFAULT_ANNOTATION_COLORS);
            assert_eq!(errors.len(), 1, "body: {body}");
        }
    }
}
