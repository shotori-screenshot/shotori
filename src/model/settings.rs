//! # Persistent user settings (`~/.config/shotori/settings.toml`)
//!
//! Everything a user can configure that is not a color (colors live in
//! `theme.toml` — see [`crate::ui::theme::load`]). v1: which buttons the
//! selection toolbar shows; future sections (theme picker, language)
//! join the same file.
//!
//! Lifecycle mirrors the theme: loaded once at startup ([`init`]) and
//! read everywhere via [`current`]. The screenshot overlay is a
//! run-and-exit process, so there is deliberately no live reload —
//! `shotori settings` (or the tray menu) opens the editor window,
//! every toggle writes straight through to the file, and the NEXT
//! capture picks the change up. A half-edited or wedged editor can
//! therefore never corrupt a running session.
//!
//! Unknown ids and unknown sections are tolerated, not fatal: a file
//! written by a NEWER shotori (extra buttons, extra sections) degrades
//! to the subset this binary understands, and [`Settings::save`]
//! round-trips the unknown parts so the editor never eats keys it does
//! not know about.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

/// Settings file name inside the config dir (see [`config_dir`]).
pub const FILE_NAME: &str = "settings.toml";

/// One toggleable row-one button of the selection toolbar. Variant
/// order is the canonical display order; [`ToolbarButton::id`] strings
/// are the file format — never rename one (files exist in the wild).
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
}

/// The row-one groups the toolbar draws: the tool cluster, the action
/// cluster, then the confirm pair. A separator sits between the last
/// two and disappears when either side is fully hidden (see
/// [`Settings::row1_separator_visible`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarCluster {
    Tools,
    Actions,
    Confirm,
}

impl ToolbarButton {
    /// Every button, in display order. The settings window lists and
    /// the toolbar render both iterate this — one order everywhere.
    pub const ALL: [Self; 19] = [
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
        Self::Save,
        Self::Pin,
        Self::Scroll,
        Self::Cancel,
        Self::Copy,
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
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|b| b.id() == id)
    }

    /// Which row-one group the button belongs to (drives both the
    /// settings window's sections and the separator predicate).
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
        }
    }
}

/// User settings. Default = the factory toolbar (nothing hidden).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Settings {
    /// Row-one toolbar buttons the user hid; absent = visible. Hidden
    /// buttons keep their keyboard shortcuts — only the visible row
    /// changes.
    hidden: BTreeSet<ToolbarButton>,
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
    hidden: Vec<String>,
}

impl Settings {
    pub fn toolbar_visible(&self, button: ToolbarButton) -> bool {
        !self.hidden.contains(&button)
    }

    pub fn set_toolbar_visible(&mut self, button: ToolbarButton, visible: bool) {
        if visible {
            self.hidden.remove(&button);
        } else {
            self.hidden.insert(button);
        }
    }

    /// How many row-one buttons are hidden — the placement width basis
    /// shrinks by one button footprint per hidden button (all
    /// configurable buttons live on row one).
    pub fn hidden_row1_count(&self) -> usize {
        self.hidden.len()
    }

    /// The group break between the action cluster and the confirm pair
    /// stays only while it separates something: it vanishes when
    /// either side is fully hidden. One predicate for the render and
    /// the width basis — they cannot drift.
    pub fn row1_separator_visible(&self) -> bool {
        let mut actions = false;
        let mut confirm = false;
        for button in ToolbarButton::ALL {
            match button.cluster() {
                ToolbarCluster::Actions => actions |= self.toolbar_visible(button),
                ToolbarCluster::Confirm => confirm |= self.toolbar_visible(button),
                ToolbarCluster::Tools => {}
            }
        }
        actions && confirm
    }

    /// Load from `path` (missing file → defaults, silently). Errors are
    /// returned, never fatal: a broken file must not take the
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
        let mut settings = Self::default();
        let mut errors = Vec::new();
        for id in file.toolbar.hidden {
            match ToolbarButton::from_id(&id) {
                Some(button) => {
                    settings.hidden.insert(button);
                }
                None => errors.push(format!(
                    "{}: unknown toolbar button {id:?} (skipped; known ids: {})",
                    path.display(),
                    ToolbarButton::ALL
                        .iter()
                        .map(|b| b.id())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
        (settings, errors)
    }

    /// Write through to `path`. The existing document is parsed and
    /// re-serialized so unknown keys/sections (written by a newer
    /// shotori) survive the edit; a corrupt existing file is replaced
    /// outright — it was unusable, and the editor is the recovery
    /// path. Comments do not survive (Value-level round-trip).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        // toml::from_str, not str::parse: Value's FromStr accepts a
        // single VALUE expression ("[a]\nx=1" parses as an array then
        // chokes on the rest) — a document needs the serde path.
        let mut doc: toml::Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
        let hidden = toml::Value::Array(
            ToolbarButton::ALL
                .iter()
                .filter(|b| self.hidden.contains(b))
                .map(|b| toml::Value::from(b.id()))
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
        toolbar.insert("hidden".into(), hidden);
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
static DEFAULT: Settings = Settings {
    hidden: BTreeSet::new(),
};

/// The process-wide settings, installed once at startup ([`init`]).
/// Before/without init: the factory defaults.
pub fn current() -> &'static Settings {
    ACTIVE.get().unwrap_or(&DEFAULT)
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

    fn hidden(list: &[ToolbarButton]) -> Settings {
        let mut s = Settings::default();
        for b in list {
            s.set_toolbar_visible(*b, false);
        }
        s
    }

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for button in ToolbarButton::ALL {
            assert_eq!(ToolbarButton::from_id(button.id()), Some(button));
            assert!(seen.insert(button.id()), "duplicate id {}", button.id());
        }
        assert_eq!(ToolbarButton::from_id("translate"), None);
    }

    #[test]
    fn defaults_show_everything() {
        let s = Settings::default();
        for button in ToolbarButton::ALL {
            assert!(s.toolbar_visible(button));
        }
        assert_eq!(s.hidden_row1_count(), 0);
        assert!(s.row1_separator_visible());
    }

    #[test]
    fn visibility_toggles() {
        let mut s = Settings::default();
        s.set_toolbar_visible(ToolbarButton::Pin, false);
        s.set_toolbar_visible(ToolbarButton::Scroll, false);
        assert!(!s.toolbar_visible(ToolbarButton::Pin));
        assert!(s.toolbar_visible(ToolbarButton::Copy));
        assert_eq!(s.hidden_row1_count(), 2);
        s.set_toolbar_visible(ToolbarButton::Pin, true);
        assert!(s.toolbar_visible(ToolbarButton::Pin));
        assert_eq!(s.hidden_row1_count(), 1);
    }

    #[test]
    fn separator_needs_both_sides() {
        // default: both sides populated → visible
        assert!(Settings::default().row1_separator_visible());
        // every action hidden → nothing on the left of the break
        let s = hidden(&[
            ToolbarButton::Ocr,
            ToolbarButton::Save,
            ToolbarButton::Pin,
            ToolbarButton::Scroll,
        ]);
        assert!(!s.row1_separator_visible());
        // confirm pair hidden → nothing on the right
        let s = hidden(&[ToolbarButton::Cancel, ToolbarButton::Copy]);
        assert!(!s.row1_separator_visible());
        // one action left, both confirms left → still meaningful
        let s = hidden(&[ToolbarButton::Scroll]);
        assert!(s.row1_separator_visible());
    }

    #[test]
    fn load_missing_file_is_default_silently() {
        let (s, errors) = Settings::load(Some(Path::new("/nonexistent/shotori/settings.toml")));
        assert_eq!(s, Settings::default());
        assert!(errors.is_empty());
    }

    #[test]
    fn load_reads_hidden_and_skips_unknown_ids() {
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
        assert_eq!(errors.len(), 1, "the unknown id must be reported");
        assert!(errors[0].contains("bogus"));
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
        let s = hidden(&[ToolbarButton::Pin, ToolbarButton::Eraser]);
        s.save(&path).expect("save must create parent dirs");
        let (back, errors) = Settings::load(Some(&path));
        assert!(errors.is_empty());
        assert_eq!(back, s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[toolbar]"));
        assert!(text.contains("\"pin\""));
    }

    #[test]
    fn save_preserves_unknown_keys_and_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "[toolbar]\nhidden = [\"pin\"]\n[future_section]\nlanguage = \"de\"\n",
        )
        .unwrap();
        hidden(&[ToolbarButton::Scroll]).save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("future_section"),
            "unknown sections must survive an edit: {text}"
        );
        assert!(text.contains("language"));
        // the edit replaced pin-hidden with scroll-hidden
        let (back, _) = Settings::load(Some(&path));
        assert_eq!(back, hidden(&[ToolbarButton::Scroll]));
    }
}
