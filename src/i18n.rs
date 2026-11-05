//! Bilingual user interface strings.
//!
//! English is the key: every call site carries its own Chinese translation, so a
//! missing string cannot compile. The active language is process-wide because the
//! widgets that render text are spread across helper functions.
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

static CHINESE: AtomicBool = AtomicBool::new(false);

pub fn set(lang: Lang) {
    CHINESE.store(lang == Lang::Zh, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if CHINESE.load(Ordering::Relaxed) {
        Lang::Zh
    } else {
        Lang::En
    }
}

/// Choose between the English and Chinese wording of one string.
pub fn t(en: &'static str, zh: &'static str) -> &'static str {
    choose(CHINESE.load(Ordering::Relaxed), en, zh)
}

fn choose(chinese: bool, en: &'static str, zh: &'static str) -> &'static str {
    if chinese { zh } else { en }
}

/// `format!` needs a literal template, so this macro carries both languages. Each
/// template formats its own placeholders, which lets Chinese reorder them with `{0}`.
#[macro_export]
macro_rules! t_format {
    ($en:literal, $zh:literal $(, $arg:expr)* $(,)?) => {{
        if $crate::i18n::current() == $crate::i18n::Lang::Zh {
            format!($zh $(, $arg)*)
        } else {
            format!($en $(, $arg)*)
        }
    }};
}

fn text_is_chinese(text: &str) -> bool {
    text.contains("zh")
}

/// The language the operating system offers to the user, used as the default.
pub fn detect() -> Lang {
    if cfg!(windows) {
        return windows_ui_language();
    }
    // An explicit environment locale always wins.
    for variable in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(variable)
            && !value.is_empty()
            && value != "C"
            && value != "POSIX"
        {
            return if text_is_chinese(&value) {
                Lang::Zh
            } else {
                Lang::En
            };
        }
    }
    if cfg!(target_os = "macos") {
        // Launching from Finder leaves the environment empty, so ask the preferences daemon.
        let output = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLocale"])
            .output();
        if let Ok(output) = output {
            let locale = String::from_utf8_lossy(&output.stdout);
            return if text_is_chinese(&locale) {
                Lang::Zh
            } else {
                Lang::En
            };
        }
    }
    Lang::En
}

#[cfg(windows)]
fn windows_ui_language() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    // The primary language is the low 10 bits of the LANGID; 0x04 is Chinese.
    let langid = unsafe { GetUserDefaultUILanguage() } as u32;
    if langid & 0x3FF == 0x04 {
        Lang::Zh
    } else {
        Lang::En
    }
}

#[cfg(not(windows))]
fn windows_ui_language() -> Lang {
    Lang::En
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_language_gets_its_own_wording() {
        assert_eq!(choose(false, "Scan", "扫描"), "Scan");
        assert_eq!(choose(true, "Scan", "扫描"), "扫描");
    }
    #[test]
    fn detection_never_panics() {
        let _ = detect();
    }
}
