//! The toolbar row vocabulary: which controls exist, their stable
//! settings-file ids, and the factory row. Shared by the overlay's
//! toolbar, the placement math and the settings window's editor —
//! the file-format contract lives in the `id()` strings, so they are
//! never renamed (files exist in the wild).

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

/// All valid ids, comma-joined for error messages.
pub(super) fn known_ids() -> String {
    let mut ids: Vec<&str> = ToolbarButton::ALL.iter().map(|b| b.id()).collect();
    ids.extend(["separator"]);
    ids.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::settings::Settings;

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
}
