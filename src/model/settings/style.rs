//! Visual & layout chrome preset for screenshot surfaces (`ui_style` in `settings.toml`).
//!
//! Kept orthogonal to [`crate::ui::theme::Theme`] (which owns color tokens):
//! [`UiStyle`] governs geometry, corner radii, surface translucency, shadow
//! depth, selection handles, loupe shape, and toolbar anchoring. Adding a
//! future layout family (e.g. an edge-hugging rail preset) adds a
//! variant here and dispatches through `model::placement`, `ui::hud` and
//! `ui::toolbar` without coupling layout to palettes.

/// The screenshot UI presentation preset.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum UiStyle {
    /// Shotori's original compact floating bar, solid accent handles,
    /// rectangular loupe, and square swatches.
    #[default]
    Classic,
    /// Frosted translucent capsule bars, centered anchoring,
    /// concentric rounded controls, white-ringed handles, circular loupe,
    /// and circular color-ring swatches.
    Capsule,
}

impl UiStyle {
    /// Every selectable style in Settings display order.
    pub const ALL: [Self; 2] = [Self::Classic, Self::Capsule];

    /// Stable identifier in `settings.toml` (`ui_style = "classic" | "capsule"`).
    pub const fn id(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Capsule => "capsule",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.id() == id)
    }

    /// Translation key for the style card's title.
    pub const fn title_key(self) -> &'static str {
        match self {
            Self::Classic => "settings.appearance.style.classic.title",
            Self::Capsule => "settings.appearance.style.capsule.title",
        }
    }

    /// Translation key for the style card's description.
    pub const fn body_key(self) -> &'static str {
        match self {
            Self::Classic => "settings.appearance.style.classic.body",
            Self::Capsule => "settings.appearance.style.capsule.body",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UiStyle;

    #[test]
    fn ui_style_ids_round_trip_and_reject_unknowns() {
        for style in UiStyle::ALL {
            assert_eq!(UiStyle::from_id(style.id()), Some(style));
        }
        assert_eq!(UiStyle::from_id("unknown"), None);
        assert_eq!(UiStyle::default(), UiStyle::Classic);
    }
}
