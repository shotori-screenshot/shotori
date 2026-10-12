//! # i18n: user-facing strings (`locales/ui.yml`, en + zh-CN)
//!
//! Shotori rides the SAME engine gpui-component embeds (rust-i18n):
//! our table and the component library's table are separate, but the
//! active locale is process-global — one `set_locale` switches both.
//! The macro call below embeds `locales/` at compile time, so the
//! single-file deploy is unchanged.
//!
//! Lifecycle mirrors the theme/settings pickers: each process entry
//! resolves the language once at startup ([`init`], [`init_child`]).
//! The overlay is run-and-exit, so there is no live switch there; the
//! settings window may flip the locale live later (re-render is
//! enough — `t!` reads the global on every call).

pub use rust_i18n::t;

use crate::model::settings::{Language, Settings};

/// Resolve the language for THIS process and install it. `no_config`
/// skips the settings file (the `--no-config` flag — same contract as
/// the theme/settings pickup).
pub fn init(no_config: bool) {
    let language = if no_config {
        Language::Auto
    } else {
        Settings::load(crate::model::settings::config_path().as_deref())
            .0
            .language()
    };
    apply(language);
}

/// Child-process variant (`--notify`): dispatched before clap, carries
/// no flags — always honor the settings file. A missing/corrupt file
/// degrades silently to the system detection (then English).
pub fn init_child() {
    apply_language_from(crate::model::settings::config_path().as_deref());
}

/// Resolve the language from a settings file (missing/corrupt → Auto)
/// and install it. Split from [`init_child`] so tests can point at a
/// throwaway file instead of the user's real config dir.
fn apply_language_from(path: Option<&std::path::Path>) {
    apply(Settings::load(path).0.language());
}

/// Map a language choice to a rust-i18n locale and install it.
fn apply(language: Language) {
    let locale = match language {
        Language::Auto => detect(),
        Language::English => "en",
        Language::ChineseSimplified => "zh-CN",
    };
    rust_i18n::set_locale(locale);
}

/// Install a language choice at runtime (the settings window's live
/// language switch). Callers refresh their windows afterwards — `t!`
/// resolves on every call, so a re-render picks the new locale up.
pub fn set(language: Language) {
    apply(language);
}

/// The system locale mapped onto a language we ship: any Chinese
/// variant (zh-CN, zh-TW, zh_HK, …) reads the simplified column for
/// now — the table carries one Chinese column; everything else falls
/// back to English. Splitting traditional Chinese out later is an
/// extra column, nothing structural.
fn detect() -> &'static str {
    if sys_locale::get_locale().is_some_and(|l| l.to_ascii_lowercase().starts_with("zh")) {
        "zh-CN"
    } else {
        "en"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `set_locale` is process-global and cargo runs tests in parallel —
    /// serialize the two locale-mutating tests against each other (and
    /// against any future one).
    static LOCALE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn embedded_keys_translate_and_missing_ones_degrade_to_the_key() {
        let _guard = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        // The table must actually be embedded: a present key resolves,
        // an absent one returns itself (rust-i18n's fallback signal).
        assert_eq!(t!("notify.view_image").to_string(), "View image");
        assert_eq!(t!("no.such.key").to_string(), "no.such.key");
    }

    #[test]
    fn interpolation_placeholders_substitute() {
        // rust-i18n 4 substitutes `%{var}` — a `{{var}}` literal in the
        // table renders raw (the "保存至 {{path}}" footer bug). Guard
        // the syntax: substituted in, no placeholder left behind.
        let _guard = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        let line = t!("ocr.setup.confirm", model = "PP-OCRv5").to_string();
        assert!(line.contains("PP-OCRv5"), "{line}");
        assert!(!line.contains('{'), "{line}");
    }

    #[test]
    fn child_entry_honors_the_settings_file_language() {
        let _guard = LOCALE_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.toml"), "language = \"zh-CN\"\n").unwrap();
        apply_language_from(Some(&dir.path().join("settings.toml")));
        let locale = rust_i18n::locale().to_string();
        let view = t!("notify.view_image").to_string();
        // restore before any assert can bail out and leak the override
        // into other tests
        rust_i18n::set_locale("en");
        assert_eq!(locale, "zh-CN");
        assert_eq!(view, "查看图片");
    }
}
