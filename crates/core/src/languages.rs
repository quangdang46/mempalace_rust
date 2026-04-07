//! Languages module - pluggable language support for entity detection and AAAK compression.
//!
//! Each language implements the Language trait with language-specific patterns.
//! Currently supports Latin (default) and Russian (Cyrillic).

use regex::{Regex, RegexBuilder};
use std::collections::HashMap;

/// Language trait for language-specific processing.
pub trait Language {
    /// Get the language code (e.g., "en", "ru").
    fn code(&self) -> &str;

    /// Get the language name (e.g., "English", "Russian").
    fn name(&self) -> &str;

    /// Get uppercase Unicode character class regex pattern.
    fn uppercase_pattern(&self) -> &str;

    /// Get lowercase Unicode character class regex pattern.
    fn lowercase_pattern(&self) -> &str;

    /// Get person verb patterns for this language.
    fn person_verb_patterns(&self) -> Vec<&'static str>;

    /// Check if a character is uppercase in this language.
    fn is_uppercase(&self, c: char) -> bool {
        c.to_uppercase().to_string().chars().next() == Some(c)
    }

    /// Check if a character is lowercase in this language.
    fn is_lowercase(&self, c: char) -> bool {
        c.to_lowercase().to_string().chars().next() == Some(c)
    }

    fn proper_noun_regex(&self) -> Regex {
        let upper = self.uppercase_pattern();
        let lower = self.lowercase_pattern();
        // Match proper nouns: Upper followed by mixed case letters (allows CamelCase like MemPalace)
        let pattern = format!(r"\b([{0}][{0}{1}]{{1,19}})\b", upper, lower);
        Regex::new(&pattern).unwrap()
    }

    fn multi_word_proper_noun_regex(&self) -> Regex {
        let upper = self.uppercase_pattern();
        let lower = self.lowercase_pattern();
        let pattern = format!(r"\b([{0}][{1}]+(?:\s+[{0}][{1}]+)+)\b", upper, lower);
        Regex::new(&pattern).unwrap()
    }
}

/// English language implementation.
pub struct English;

impl Language for English {
    fn code(&self) -> &str {
        "en"
    }

    fn name(&self) -> &str {
        "English"
    }

    fn uppercase_pattern(&self) -> &str {
        r"\p{Lu}"
    }

    fn lowercase_pattern(&self) -> &str {
        r"\p{Ll}"
    }

    fn person_verb_patterns(&self) -> Vec<&'static str> {
        vec![
            r"\b{name}\s+said\b",
            r"\b{name}\s+asked\b",
            r"\b{name}\s+told\b",
            r"\b{name}\s+replied\b",
            r"\b{name}\s+laughed\b",
            r"\b{name}\s+smiled\b",
            r"\b{name}\s+cried\b",
            r"\b{name}\s+felt\b",
            r"\b{name}\s+thinks?\b",
            r"\b{name}\s+wants?\b",
            r"\b{name}\s+loves?\b",
            r"\b{name}\s+hates?\b",
            r"\b{name}\s+knows?\b",
            r"\b{name}\s+decided\b",
            r"\b{name}\s+pushed\b",
            r"\b{name}\s+wrote\b",
            r"\bhey\s+{name}\b",
            r"\bthanks?\s+{name}\b",
            r"\bhi\s+{name}\b",
            r"\bdear\s+{name}\b",
        ]
    }
}

/// Russian language implementation with Cyrillic script support.
pub struct Russian;

/// Note: Uses general Unicode categories \p{Lu}/\p{Ll} which match Cyrillic letters too.
/// Cyrillic uppercase: А-Я includes А, Б, В, Г, Д, Е, Ё, Ж, З, И, Й, К, Л, М, Н, О, П, Р, С, Т, У, Ф, Х, Ц, Ч, Ш, Щ, Ъ, Ы, Ь, Э, Ю, Я
/// Cyrillic lowercase: а-я includes а, б, в, г, д, е, ё, ж, з, и, й, к, л, м, н, о, п, р, с, т, у, ф, х, ц, ч, ш, щ, ъ, ы, ь, э, ю, я
impl Language for Russian {
    fn code(&self) -> &str {
        "ru"
    }

    fn name(&self) -> &str {
        "Russian"
    }

    fn uppercase_pattern(&self) -> &str {
        r"\p{Lu}"
    }

    fn lowercase_pattern(&self) -> &str {
        r"\p{Ll}"
    }

    /// 33 Russian person verb patterns (Russian verbs for person actions).
    fn person_verb_patterns(&self) -> Vec<&'static str> {
        vec![
            r"\b{name}\s+сказал\b",
            r"\b{name}\s+спросил\b",
            r"\b{name}\s+ответил\b",
            r"\b{name}\s+плакал\b",
            r"\b{name}\s+улыбнулся\b",
            r"\b{name}\s+думает\b",
            r"\b{name}\s+хочет\b",
            r"\b{name}\s+любит\b",
            r"\b{name}\s+ненавидит\b",
            r"\b{name}\s+знает\b",
            r"\b{name}\s+решил\b",
            r"\b{name}\s+написал\b",
            r"\b{name}\s+читал\b",
            r"\b{name}\s+работал\b",
            r"\b{name}\s+играл\b",
            r"\b{name}\s+встретил\b",
            r"\b{name}\s+видел\b",
            r"\b{name}\s+слышал\b",
            r"\b{name}\s+понял\b",
            r"\b{name}\s+объяснил\b",
            r"\b{name}\s+показал\b",
            r"\b{name}\s+дал\b",
            r"\b{name}\s+взял\b",
            r"\b{name}\s+положил\b",
            r"\b{name}\s+открыл\b",
            r"\b{name}\s+закрыл\b",
            r"\b{name}\s+нашел\b",
            r"\b{name}\s+потерял\b",
            r"\b{name}\s+ждал\b",
            r"\b{name}\s+пришел\b",
            r"\b{name}\s+ушел\b",
            r"\b{name}\s+вернулся\b",
            r"\b{name}\s+начал\b",
            r"\b{name}\s+закончил\b",
        ]
    }
}

/// Get all supported languages.
pub fn all_languages() -> Vec<(&'static str, Box<dyn Language + Send + Sync>)> {
    vec![
        ("en", Box::new(English) as Box<dyn Language + Send + Sync>),
        ("ru", Box::new(Russian)),
    ]
}

/// Detect language from text based on script patterns.
pub fn detect_language(text: &str) -> Option<&'static str> {
    let has_cyrillic = text.chars().any(|c| {
        let category = UnicodeExt::from_char(c);
        category == UnicodeExt::CyrillicLu || category == UnicodeExt::CyrillicLl
    });

    if has_cyrillic {
        Some("ru")
    } else {
        Some("en")
    }
}

/// Unicode character categories for detection.
#[derive(Debug, PartialEq)]
enum UnicodeExt {
    CyrillicLu,
    CyrillicLl,
    Other,
}

impl UnicodeExt {
    fn from_char(c: char) -> Self {
        // Check Cyrillic uppercase (А-Я, includes Ё)
        if '\u{0410}' <= c && c <= '\u{042F}' || c == '\u{0401}' {
            return UnicodeExt::CyrillicLu;
        }
        // Check Cyrillic lowercase (а-я, includes ё)
        if '\u{0430}' <= c && c <= '\u{044F}' || c == '\u{0451}' {
            return UnicodeExt::CyrillicLl;
        }
        UnicodeExt::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_english_proper_noun_regex() {
        let lang = English;
        let re = lang.proper_noun_regex();
        assert!(re.is_match("John"));
        assert!(re.is_match("MemPalace"));
        assert!(re.is_match("Alice Smith"));
    }

    #[test]
    fn test_russian_proper_noun_regex() {
        let lang = Russian;
        let re = lang.proper_noun_regex();
        assert!(re.is_match("Иван"));
        assert!(re.is_match("Мария"));
        assert!(re.is_match("Петр"));
    }

    #[test]
    fn test_russian_verb_patterns() {
        let lang = Russian;
        let patterns = lang.person_verb_patterns();
        assert!(patterns.len() >= 33);
        // Check some Russian verbs
        assert!(patterns.iter().any(|p| p.contains("сказал")));
        assert!(patterns.iter().any(|p| p.contains("спросил")));
        assert!(patterns.iter().any(|p| p.contains("думает")));
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("Hello world"), Some("en"));
        assert_eq!(detect_language("Привет мир"), Some("ru"));
        assert_eq!(detect_language("Hello Привет world"), Some("ru"));
    }

    #[test]
    fn test_language_codes() {
        assert_eq!(English.code(), "en");
        assert_eq!(Russian.code(), "ru");
    }
}
