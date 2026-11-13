//! Runtime locale selection: English is the source/fallback, bundled language
//! packs live in the exe, and user overrides live in the data directory's
//! `locales/` folder (normally %LOCALAPPDATA%\SpeedMeter\locales).
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};

#[derive(Debug, Deserialize)]
struct LanguagePack {
    code: String,
    name: String,
    #[serde(default)]
    translations: HashMap<String, String>,
}

static LANGUAGES: OnceLock<Vec<LanguagePack>> = OnceLock::new();
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

const BUNDLED: &[&str] = &[include_str!("../../locales/zh-CN.json")];

fn valid_pack(pack: &LanguagePack) -> bool {
    pack.code.len() >= 2
        && pack.code.len() <= 32
        && pack
            .code
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        && !pack.code.eq_ignore_ascii_case("en-US")
        && !pack.name.trim().is_empty()
        && !pack.translations.is_empty()
}

fn decode(text: &str) -> Option<LanguagePack> {
    let pack = serde_json::from_str::<LanguagePack>(text.trim_start_matches('\u{feff}')).ok()?;
    valid_pack(&pack).then_some(pack)
}

fn load_from(dir: &Path) -> Vec<LanguagePack> {
    let mut packs = vec![LanguagePack {
        code: "en-US".into(),
        name: "English".into(),
        translations: HashMap::new(),
    }];

    // Bundled packs remain available when a user has no locale directory.
    for json in BUNDLED {
        if let Some(pack) = decode(json) {
            packs.push(pack);
        }
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .collect();
        files.sort();
        for file in files {
            let Ok(meta) = std::fs::metadata(&file) else {
                continue;
            };
            if !meta.is_file() || meta.len() > 1024 * 1024 {
                continue;
            }
            let Ok(json) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Some(pack) = decode(&json) else { continue };
            // A file must be named after its code. Never read subfolders.
            if file.file_stem().and_then(|s| s.to_str()) != Some(pack.code.as_str()) {
                continue;
            }
            if let Some(existing) = packs
                .iter_mut()
                .find(|p| p.code.eq_ignore_ascii_case(&pack.code))
            {
                // An edited bundled language pack replaces its builtin version.
                *existing = pack;
            } else {
                packs.push(pack);
            }
        }
    }
    packs
}

fn languages() -> &'static Vec<LanguagePack> {
    LANGUAGES.get_or_init(|| load_from(&crate::config::data_dir().join("locales")))
}

pub fn choices() -> Vec<(String, String)> {
    languages()
        .iter()
        .map(|p| (p.code.clone(), p.name.clone()))
        .collect()
}

pub fn available(code: &str) -> bool {
    languages().iter().any(|p| p.code == code)
}

pub fn set(code: &str) {
    let index = languages().iter().position(|p| p.code == code).unwrap_or(0);
    ACTIVE.store(index, Ordering::Relaxed);
}

fn selected_for_locale(locale: &str, available: &[LanguagePack]) -> String {
    if let Some(pack) = available
        .iter()
        .find(|p| p.code.eq_ignore_ascii_case(locale))
    {
        return pack.code.clone();
    }
    // Windows can report zh-Hans-* for a Simplified Chinese system.
    let lower = locale.to_ascii_lowercase();
    if (lower.starts_with("zh-hans") || lower == "zh-sg")
        && available.iter().any(|p| p.code == "zh-CN")
    {
        return "zh-CN".into();
    }
    "en-US".into()
}

/// Follow Windows for a fresh installation; fall back to English when no pack exists.
pub fn default_language() -> String {
    let mut buf = [0u16; 85];
    // UI language, not regional number/date format preference.
    let lang_id = unsafe { GetUserDefaultUILanguage() };
    let len = unsafe { LCIDToLocaleName(lang_id as u32, Some(&mut buf), 0) };
    if len <= 1 || len as usize > buf.len() {
        return "en-US".into();
    }
    selected_for_locale(
        &String::from_utf16_lossy(&buf[..len as usize - 1]),
        languages(),
    )
}

/// Source text is English; missing translations always display it verbatim.
pub fn tr<'a>(source: &'a str) -> &'a str {
    languages()
        .get(ACTIVE.load(Ordering::Relaxed))
        .and_then(|pack| pack.translations.get(source))
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .unwrap_or(source)
}

pub fn tr_with(source: &str, vars: &[(&str, &str)]) -> String {
    let mut out = tr(source).to_string();
    for (name, value) in vars {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_pack_and_default_fallback() {
        let packs = load_from(Path::new("this-directory-should-not-exist"));
        assert_eq!(packs[0].code, "en-US");
        assert!(packs.iter().any(|p| p.code == "zh-CN"));
        assert_eq!(selected_for_locale("zh-CN", &packs), "zh-CN");
        assert_eq!(selected_for_locale("zh-Hans-CN", &packs), "zh-CN");
        assert_eq!(selected_for_locale("zh-TW", &packs), "en-US");
        assert_eq!(selected_for_locale("fr-FR", &packs), "en-US");
    }

    #[test]
    fn user_pack_overrides_builtin_and_discovers_new_language() {
        let dir = std::env::temp_dir().join(format!(
            "speedmeter-locales-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("zh-CN.json"), "\u{feff}{\"code\":\"zh-CN\",\"name\":\"简体中文\",\"translations\":{\"Live\":\"自定义实时\"}}").unwrap();
        std::fs::write(
            dir.join("ko-KR.json"),
            r#"{"code":"ko-KR","name":"한국어","translations":{"Live":"실시간"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "invalid json").unwrap();
        let packs = load_from(&dir);
        assert_eq!(packs.len(), 3);
        assert_eq!(
            packs
                .iter()
                .find(|p| p.code == "zh-CN")
                .unwrap()
                .translations
                .get("Live")
                .unwrap(),
            "自定义实时"
        );
        assert_eq!(
            packs.iter().find(|p| p.code == "ko-KR").unwrap().name,
            "한국어"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_translation_and_formatting_are_safe() {
        assert_eq!(
            tr_with("Downloaded {amount}", &[("amount", "42 KB")]),
            "Downloaded 42 KB"
        );
        assert_eq!(tr("Untranslated sentence"), "Untranslated sentence");
    }
}
