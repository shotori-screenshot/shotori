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
        // The notify child resolves this label at click time; a missing
        // key would surface as the raw key on the button.
        assert_eq!(t!("notify.open_text").to_string(), "Open text");
        // YAML 1.1 parsers read a bare `off:` as the boolean false —
        // rust-i18n must keep it a string key or this lookup degrades
        // to the raw key on the OCR page.
        assert!(
            t!("settings.ocr_page.custom.off")
                .to_string()
                .starts_with("Off —")
        );
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

#[cfg(test)]
mod key_audit {
    //! The table is resolved at runtime, so a typo'd key compiles fine
    //! and only surfaces as raw key text in the UI (the
    //! `notify.scroll.cross_screen` bug). These tests pin the yml and
    //! the source together, in both directions.

    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Keys built with `format!` never appear as a `t!("…")` literal.
    /// Everything under these prefixes is exempt from the dead-key
    /// direction (they are still covered in the other direction: the
    /// literal fragments like `ocr.model.{}.label` must exist, which
    /// `every_t_literal_exists_in_yml` checks via the concrete ids
    /// used in tests).
    const DYNAMIC_PREFIXES: &[&str] = &["ocr.model."];

    /// Locale keys = every yml node whose children are the language
    /// columns. The file uses strict 2-space indentation with no
    /// block scalars, so an indentation walk is exact.
    fn yml_keys() -> BTreeSet<String> {
        let text = std::fs::read_to_string(yml_path()).unwrap();
        let mut keys = BTreeSet::new();
        // (indent, segment) path down to the current line
        let mut stack: Vec<(usize, String)> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((name, _value)) = trimmed.split_once(':') else {
                continue;
            };
            let indent = line.len() - trimmed.len();
            let name = name.trim();
            while stack.last().is_some_and(|(d, _)| *d >= indent) {
                stack.pop();
            }
            // A language column closes its parent as a complete key.
            if name == "en" || name == "zh-CN" {
                if !stack.is_empty() {
                    keys.insert(
                        stack
                            .iter()
                            .map(|(_, s)| s.as_str())
                            .collect::<Vec<_>>()
                            .join("."),
                    );
                }
                continue;
            }
            stack.push((indent, name.to_string()));
        }
        keys
    }

    fn yml_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("locales/ui.yml")
    }

    /// Source-referenced keys, two sets:
    /// - `t_literals`: keys passed literally to `t!` (asserted to exist);
    /// - `dotted`: every dotted lowercase string literal anywhere
    ///   (covers keys built through `match`/tuple tables — the sidebar
    ///   section keys, the theme color labels).
    fn referenced() -> (BTreeSet<String>, BTreeSet<String>) {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut t_literals = BTreeSet::new();
        let mut dotted = BTreeSet::new();
        let mut files = Vec::new();
        collect_rs(&src, &mut files);
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let bytes = text.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i..].starts_with(b"t!(") {
                    let mut j = i + 3;
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < bytes.len()
                        && bytes[j] == b'"'
                        && let Some((key, _)) = read_literal(bytes, j)
                    {
                        // `format!(` also ends in `t!(` — only
                        // key-shaped literals are t! candidates
                        if looks_like_key(&key) {
                            t_literals.insert(key);
                        }
                    }
                }
                if bytes[i] == b'"' {
                    if let Some((literal, _end)) = read_literal(bytes, i) {
                        if looks_like_key(&literal) {
                            dotted.insert(literal);
                        }
                        i = end_of_string(bytes, i);
                        continue;
                    }
                    // malformed literal: skip to its closing quote so
                    // escaped text is never re-scanned as key material
                    i = end_of_string(bytes, i);
                    continue;
                }
                i += 1;
            }
        }
        (t_literals, dotted)
    }

    /// Locale keys are lowercase dotted paths; anything else (format
    /// strings, hex colors, `{}` placeholders) is not a key candidate.
    fn looks_like_key(literal: &str) -> bool {
        !literal.is_empty()
            && literal.contains('.')
            && literal
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
    }

    /// Read a plain string literal starting at `open` (the `"`).
    /// Returns (content, index after closing quote), or None when the
    /// literal contains escapes this audit doesn't model.
    fn read_literal(bytes: &[u8], open: usize) -> Option<(String, usize)> {
        let mut out = String::new();
        let mut i = open + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => return Some((out, i + 1)),
                b'\\' => return None,
                _ => out.push(bytes[i] as char),
            }
            i += 1;
        }
        None
    }

    fn end_of_string(bytes: &[u8], open: usize) -> usize {
        let mut i = open + 1;
        while i < bytes.len() {
            match bytes[i] {
                // skip the escaped byte so `\"` doesn't end the literal
                b'\\' => i += 1,
                b'"' => return i + 1,
                _ => {}
            }
            i += 1;
        }
        i
    }

    fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// The fallback-degradation test above passes a deliberately
    /// absent key; exempt it from the exists check.
    const INTENTIONAL_MISSES: &[&str] = &["no.such.key"];

    #[test]
    fn every_t_literal_exists_in_yml() {
        let keys = yml_keys();
        let (t_literals, _) = referenced();
        let missing: Vec<&String> = t_literals
            .iter()
            .filter(|k| !keys.contains(*k) && !INTENTIONAL_MISSES.contains(&k.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "t! keys with no yml entry (rust-i18n would print the raw key in the UI): {missing:?}"
        );
    }

    #[test]
    fn every_yml_key_is_referenced_somewhere() {
        let (_, dotted) = referenced();
        let dead: Vec<String> = yml_keys()
            .into_iter()
            .filter(|k| {
                !dotted.contains(k) && !DYNAMIC_PREFIXES.iter().any(|prefix| k.starts_with(prefix))
            })
            .collect();
        assert!(
            dead.is_empty(),
            "locale keys no source code references (delete them or fix the reference): {dead:?}"
        );
    }

    #[test]
    fn dynamic_model_keys_exist_for_every_offered_model() {
        // The whitelisted `ocr.model.*` family, expanded against the
        // real model list: each id must have all three columns.
        let keys = yml_keys();
        let expected_suffixes = [".label", ".langs", ".tier"];
        for model in crate::model::settings::OcrModel::ALL {
            for suffix in expected_suffixes {
                let key = format!("ocr.model.{}{suffix}", model.id());
                assert!(keys.contains(&key), "missing locale key: {key}");
            }
        }
    }
}
