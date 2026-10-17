//! The OCR model-set vocabulary: the curated sets the settings
//! window offers, with stable ids that double as rapidocr-core's
//! registry names. The file-format side (the `[ocr]` section) lives
//! in the parent; this module is the id/label/metadata table.

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
/// The factory OCR configuration — deliberately the historical default,
/// so existing installs see no behavior change until they opt in.
pub const DEFAULT_OCR_MODEL: OcrModel = OcrModel::Ppocrv6Small;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::settings::Settings;

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
}
