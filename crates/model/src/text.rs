//! Localized text entered by the developer (product description, custom
//! labels). Written in project files either as a plain string or as a table
//! keyed by language code.

use inst_i18n::Language;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LocalizedText {
    #[default]
    Empty,
    /// Same text for every language.
    Plain(String),
    /// Per-language text; `en` (or the first entry) is the fallback.
    Translated(BTreeMap<Language, String>),
}

impl LocalizedText {
    pub fn plain(s: impl Into<String>) -> Self {
        let s = s.into();
        if s.is_empty() {
            LocalizedText::Empty
        } else {
            LocalizedText::Plain(s)
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            LocalizedText::Empty => true,
            LocalizedText::Plain(s) => s.is_empty(),
            LocalizedText::Translated(map) => map.values().all(String::is_empty),
        }
    }

    /// Text for `lang`, falling back to English, then to any translation.
    pub fn get(&self, lang: Language) -> &str {
        match self {
            LocalizedText::Empty => "",
            LocalizedText::Plain(s) => s,
            LocalizedText::Translated(map) => map
                .get(&lang)
                .or_else(|| map.get(&Language::FALLBACK))
                .or_else(|| map.values().next())
                .map_or("", String::as_str),
        }
    }

    /// Languages that have an explicit translation.
    pub fn translated_languages(&self) -> impl Iterator<Item = Language> + '_ {
        let keys = match self {
            LocalizedText::Translated(map) => Some(map.keys().copied()),
            _ => None,
        };
        keys.into_iter().flatten()
    }

    pub fn set(&mut self, lang: Language, text: String) {
        match self {
            LocalizedText::Translated(map) => {
                map.insert(lang, text);
            }
            LocalizedText::Plain(existing) if lang != Language::FALLBACK => {
                let mut map = BTreeMap::new();
                map.insert(Language::FALLBACK, std::mem::take(existing));
                map.insert(lang, text);
                *self = LocalizedText::Translated(map);
            }
            _ => *self = LocalizedText::plain(text),
        }
    }
}

impl From<&str> for LocalizedText {
    fn from(s: &str) -> Self {
        LocalizedText::plain(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_english() {
        let mut t = LocalizedText::plain("Order management");
        t.set(Language::Tr, "Sipariş yönetimi".into());
        assert_eq!(t.get(Language::Tr), "Sipariş yönetimi");
        assert_eq!(t.get(Language::De), "Order management");
        assert_eq!(t.translated_languages().count(), 2);
    }
}
