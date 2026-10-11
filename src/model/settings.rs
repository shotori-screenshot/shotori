//! # Persistent user settings (`~/.config/shotori/settings.toml`)
//!
//! Everything a user can configure that is not a color (colors live in
//! `theme.toml` — see [`crate::ui::theme::load`]). v1: the selection
//! toolbar's row-one layout — WHICH controls show and in WHAT order,
//! edited by dragging in the settings window (`shotori settings`).
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
use std::sync::OnceLock;

use serde::Deserialize;

/// Settings file name inside the config dir (see [`config_dir`]).
pub const FILE_NAME: &str = "settings.toml";

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

/// User settings. Default = the factory row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    row: Vec<ToolbarItem>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            row: DEFAULT_ROW.to_vec(),
        }
    }
}

/// The subset of the file this version understands. Unknown keys are
/// ignored on load (forward compat) and preserved on save.
#[derive(Deserialize, Default)]
#[serde(default)]
struct File {
    toolbar: ToolbarFile,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ToolbarFile {
    /// The ordered row (current format). Absent → fall back to
    /// `hidden` (the pre-ordering format), then the factory row.
    items: Option<Vec<String>>,
    /// Legacy visibility-only format: factory row minus these ids.
    hidden: Vec<String>,
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
        (Self { row }, errors)
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

static ACTIVE: OnceLock<Settings> = OnceLock::new();
static FACTORY: OnceLock<Settings> = OnceLock::new();

/// The process-wide settings, installed once at startup ([`init`]).
/// Before/without init: the factory row (lazily materialized — a
/// `Vec`-backed default is not const-constructible with its content).
pub fn current() -> &'static Settings {
    match ACTIVE.get() {
        Some(settings) => settings,
        None => FACTORY.get_or_init(Settings::default),
    }
}

/// Startup pickup: read the config file (unless `--no-config`), report
/// problems to stderr like the theme loader does, install the result.
pub fn init(args: &crate::args::Cli) {
    let path = (!args.no_config).then(config_path).flatten();
    let (settings, errors) = Settings::load(path.as_deref());
    for error in &errors {
        eprintln!("[shotori] settings: {error}");
    }
    let _ = ACTIVE.set(settings);
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
}
