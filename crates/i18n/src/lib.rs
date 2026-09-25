//! Internationalization core shared by the Studio and generated installers.
//!
//! * [`Language`] — the seven built-in languages.
//! * [`Direction`] — LTR / RTL as a first-class layout property.
//! * [`detect_system_language`] — system locale detection.
//! * [`define_messages!`] — compile-time string tables. A language whose
//!   `lang-*` feature is disabled is not compiled into the binary and falls
//!   back to English.
//! * [`installer`] — the strings of the default installer templates.

#![no_std]

#[cfg(feature = "detect")]
extern crate std;

mod format;
pub mod installer;

pub use format::{FormatArgs, format_into};

/// A built-in language. English is the fallback and is always compiled in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[repr(u8)]
pub enum Language {
    En = 0,
    Tr = 1,
    Ar = 2,
    Es = 3,
    Fr = 4,
    De = 5,
    Ru = 6,
}

/// Horizontal text/layout direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Ltr,
    Rtl,
}

impl Direction {
    #[inline]
    pub const fn is_rtl(self) -> bool {
        matches!(self, Direction::Rtl)
    }
}

impl Language {
    /// Every built-in language, in canonical order.
    pub const ALL: [Language; 7] = [
        Language::En,
        Language::Tr,
        Language::Ar,
        Language::Es,
        Language::Fr,
        Language::De,
        Language::Ru,
    ];

    /// Fallback language.
    pub const FALLBACK: Language = Language::En;

    /// BCP-47 primary language subtag.
    pub const fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Tr => "tr",
            Language::Ar => "ar",
            Language::Es => "es",
            Language::Fr => "fr",
            Language::De => "de",
            Language::Ru => "ru",
        }
    }

    /// Name of the language written in the language itself.
    pub const fn native_name(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::Tr => "Türkçe",
            Language::Ar => "العربية",
            Language::Es => "Español",
            Language::Fr => "Français",
            Language::De => "Deutsch",
            Language::Ru => "Русский",
        }
    }

    pub const fn direction(self) -> Direction {
        match self {
            Language::Ar => Direction::Rtl,
            _ => Direction::Ltr,
        }
    }

    /// Whether the string tables of this language are compiled into this
    /// binary. English is always compiled.
    pub const fn is_compiled(self) -> bool {
        match self {
            Language::En => true,
            Language::Tr => cfg!(feature = "lang-tr"),
            Language::Ar => cfg!(feature = "lang-ar"),
            Language::Es => cfg!(feature = "lang-es"),
            Language::Fr => cfg!(feature = "lang-fr"),
            Language::De => cfg!(feature = "lang-de"),
            Language::Ru => cfg!(feature = "lang-ru"),
        }
    }

    /// Parses a locale tag such as `tr`, `tr-TR`, `pt_BR.UTF-8` or `ar-EG`.
    /// Only the primary subtag is considered. Returns `None` for languages
    /// that are not built in.
    pub fn from_tag(tag: &str) -> Option<Language> {
        let primary = tag
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or_default()
            .trim();
        if primary.len() != 2 && primary.len() != 3 {
            return None;
        }
        let mut buf = [0u8; 3];
        for (dst, src) in buf.iter_mut().zip(primary.bytes()) {
            *dst = src.to_ascii_lowercase();
        }
        match &buf[..primary.len()] {
            b"en" | b"eng" => Some(Language::En),
            b"tr" | b"tur" => Some(Language::Tr),
            b"ar" | b"ara" => Some(Language::Ar),
            b"es" | b"spa" => Some(Language::Es),
            b"fr" | b"fra" | b"fre" => Some(Language::Fr),
            b"de" | b"deu" | b"ger" => Some(Language::De),
            b"ru" | b"rus" => Some(Language::Ru),
            _ => None,
        }
    }

    /// Resolves the best language among `enabled` for a locale tag.
    /// Falls back to English when enabled, otherwise to the first enabled
    /// language.
    pub fn negotiate(tag: Option<&str>, enabled: &[Language]) -> Language {
        if let Some(lang) = tag.and_then(Language::from_tag)
            && enabled.contains(&lang)
            && lang.is_compiled()
        {
            return lang;
        }
        if enabled.contains(&Language::FALLBACK) || enabled.is_empty() {
            Language::FALLBACK
        } else {
            enabled[0]
        }
    }
}

impl core::fmt::Display for Language {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.code())
    }
}

/// Returns the user's preferred language tag, if one can be determined.
///
/// On Linux this honours `LC_ALL`, `LC_MESSAGES` and `LANG`; on Windows the
/// user default UI locale.
#[cfg(feature = "detect")]
pub fn detect_system_locale() -> Option<std::string::String> {
    sys_locale::get_locale()
}

/// Detects the system language and negotiates it against `enabled`.
#[cfg(feature = "detect")]
pub fn detect_system_language(enabled: &[Language]) -> Language {
    let tag = detect_system_locale();
    Language::negotiate(tag.as_deref(), enabled)
}

/// Defines a message enum and its per-language string table.
///
/// Every row must provide all seven languages; this makes missing
/// translations a compile error. Languages whose `lang-*` feature is disabled
/// **in the invoking crate** are compiled out and resolve to English.
///
/// ```ignore
/// define_messages! {
///     pub enum Msg {
///         Install => { en: "Install", tr: "Kur", ar: "تثبيت", es: "Instalar",
///                      fr: "Installer", de: "Installieren", ru: "Установить" },
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_messages {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $id:ident => {
                    en: $en:literal, tr: $tr:literal, ar: $ar:literal, es: $es:literal,
                    fr: $fr:literal, de: $de:literal, ru: $ru:literal $(,)?
                }
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        $vis enum $name {
            $( $(#[$vmeta])* $id ),*
        }

        impl $name {
            /// Every message, in declaration order.
            pub const ALL: &'static [$name] = &[ $( $name::$id ),* ];

            /// Returns the text of this message in `lang`, or English when
            /// `lang` is not compiled in.
            #[allow(unreachable_patterns)]
            pub fn text(self, lang: $crate::Language) -> &'static str {
                match lang {
                    #[cfg(feature = "lang-tr")]
                    $crate::Language::Tr => match self { $( $name::$id => $tr ),* },
                    #[cfg(feature = "lang-ar")]
                    $crate::Language::Ar => match self { $( $name::$id => $ar ),* },
                    #[cfg(feature = "lang-es")]
                    $crate::Language::Es => match self { $( $name::$id => $es ),* },
                    #[cfg(feature = "lang-fr")]
                    $crate::Language::Fr => match self { $( $name::$id => $fr ),* },
                    #[cfg(feature = "lang-de")]
                    $crate::Language::De => match self { $( $name::$id => $de ),* },
                    #[cfg(feature = "lang-ru")]
                    $crate::Language::Ru => match self { $( $name::$id => $ru ),* },
                    _ => match self { $( $name::$id => $en ),* },
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_locale_tags() {
        assert_eq!(Language::from_tag("tr-TR"), Some(Language::Tr));
        assert_eq!(Language::from_tag("ar_EG.UTF-8"), Some(Language::Ar));
        assert_eq!(Language::from_tag("DE"), Some(Language::De));
        assert_eq!(Language::from_tag("en-US"), Some(Language::En));
        assert_eq!(Language::from_tag("fra"), Some(Language::Fr));
        assert_eq!(Language::from_tag("pt-BR"), None);
        assert_eq!(Language::from_tag(""), None);
        assert_eq!(Language::from_tag("C"), None);
    }

    #[test]
    fn negotiation_falls_back() {
        let enabled = [Language::En, Language::De];
        assert_eq!(Language::negotiate(Some("de-AT"), &enabled), Language::De);
        assert_eq!(Language::negotiate(Some("tr-TR"), &enabled), Language::En);
        assert_eq!(Language::negotiate(None, &enabled), Language::En);
        assert_eq!(
            Language::negotiate(Some("ru"), &[Language::De]),
            Language::De
        );
    }

    #[test]
    fn only_arabic_is_rtl() {
        for lang in Language::ALL {
            assert_eq!(lang.direction().is_rtl(), lang == Language::Ar);
        }
    }
}
