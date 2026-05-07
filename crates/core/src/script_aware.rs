//! Script-aware word boundary detection for Unicode text.
//!
//! Provides proper word boundary handling for non-Latin scripts including:
//! - Latin scripts (English, European languages)
//! - Cyrillic (Russian, Ukrainian, etc.)
//! - CJK characters (Chinese, Japanese, Korean)
//! - Arabic script (Arabic, Persian, Urdu)
//! - Other Unicode scripts

use regex::{Regex, RegexBuilder};
use std::sync::LazyLock;
use unicode_script::{Script, UnicodeScript};

/// Script type for text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptType {
    /// Latin script (English, French, German, etc.)
    Latin,
    /// Cyrillic script (Russian, Ukrainian, Bulgarian, etc.)
    Cyrillic,
    /// CJK characters (Chinese, Japanese, Korean)
    Cjk,
    /// Arabic script (Arabic, Persian, Urdu, etc.)
    Arabic,
    /// Other/unknown script
    Other,
}

/// Detect the dominant script in a text sample.
pub fn detect_script(text: &str) -> ScriptType {
    let mut latin_count = 0;
    let mut cyrillic_count = 0;
    let mut cjk_count = 0;
    let mut arabic_count = 0;
    let mut other_count = 0;

    for c in text.chars() {
        match c.script() {
            Script::Latin => latin_count += 1,
            Script::Cyrillic => cyrillic_count += 1,
            Script::Han | Script::Hiragana | Script::Katakana | Script::Hangul => cjk_count += 1,
            Script::Arabic => arabic_count += 1,
            _ => other_count += 1,
        }
    }

    let total = latin_count + cyrillic_count + cjk_count + arabic_count + other_count;
    if total == 0 {
        return ScriptType::Other;
    }

    // Find the dominant script (at least 50% threshold)
    let threshold = total / 2;
    
    if latin_count > threshold {
        ScriptType::Latin
    } else if cyrillic_count > threshold {
        ScriptType::Cyrillic
    } else if cjk_count > threshold {
        ScriptType::Cjk
    } else if arabic_count > threshold {
        ScriptType::Arabic
    } else {
        // Fallback to the highest count even if below threshold
        let max = latin_count.max(cyrillic_count).max(cjk_count).max(arabic_count).max(other_count);
        if max == latin_count {
            ScriptType::Latin
        } else if max == cyrillic_count {
            ScriptType::Cyrillic
        } else if max == cjk_count {
            ScriptType::Cjk
        } else if max == arabic_count {
            ScriptType::Arabic
        } else {
            ScriptType::Other
        }
    }
}

/// Get word boundary regex pattern for a given script type.
pub fn get_word_boundary_pattern(script_type: ScriptType) -> &'static str {
    match script_type {
        ScriptType::Latin => r"\b",
        ScriptType::Cyrillic => r"\b",
        ScriptType::Cjk => r"(?<![\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}])",
        ScriptType::Arabic => r"\b",
        ScriptType::Other => r"\b",
    }
}

/// Get character class pattern for a given script type.
pub fn get_char_class_pattern(script_type: ScriptType) -> &'static str {
    match script_type {
        ScriptType::Latin => r"[A-Za-z]",
        ScriptType::Cyrillic => r"[А-Яа-я]",
        ScriptType::Cjk => r"[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]",
        ScriptType::Arabic => r"[\u0600-\u06FF]",
        ScriptType::Other => r"\w",
    }
}

/// Unicode-aware word boundary regex for Latin scripts.
static LATIN_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z]+(?:'[A-Za-z]+)?\b").unwrap()
});

/// Unicode-aware word boundary regex for Cyrillic scripts.
static CYRILLIC_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[А-Яа-яЁё]+(?:-[А-Яа-яЁё]+)?\b").unwrap()
});

/// Unicode-aware word boundary regex for CJK scripts (character-based).
static CJK_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    // CJK doesn't use word boundaries in the same way - match individual characters or sequences
    Regex::new(r"[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]+").unwrap()
});

/// Unicode-aware word boundary regex for Arabic scripts.
static ARABIC_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[\u0600-\u06FF\u0750-\u077F\u08A0-\u08FF\uFB50-\uFDFF\uFE70-\uFEFF]+").unwrap()
});

/// Generic Unicode word regex (fallback for other scripts).
static GENERIC_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\w+\b").unwrap()
});

/// Get appropriate word regex for the detected script.
pub fn get_word_regex(script_type: ScriptType) -> &'static Regex {
    match script_type {
        ScriptType::Latin => &LATIN_WORD_RE,
        ScriptType::Cyrillic => &CYRILLIC_WORD_RE,
        ScriptType::Cjk => &CJK_WORD_RE,
        ScriptType::Arabic => &ARABIC_WORD_RE,
        ScriptType::Other => &GENERIC_WORD_RE,
    }
}

/// Split text into words using script-aware boundaries.
pub fn split_into_words(text: &str) -> Vec<String> {
    let script_type = detect_script(text);
    let regex = get_word_regex(script_type);
    
    regex
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Check if a character is a word boundary for the given script.
pub fn is_word_boundary(c: char, script_type: ScriptType) -> bool {
    match script_type {
        ScriptType::Latin | ScriptType::Cyrillic | ScriptType::Arabic => {
            c.is_whitespace() || c.is_ascii_punctuation()
        }
        ScriptType::Cjk => {
            // CJK characters are typically word boundaries themselves
            !c.is_alphanumeric()
        }
        ScriptType::Other => c.is_whitespace(),
    }
}

/// Normalize text for comparison across scripts.
pub fn normalize_for_script(text: &str) -> String {
    let script_type = detect_script(text);
    
    match script_type {
        ScriptType::Latin => {
            // Lowercase and normalize
            text.to_lowercase()
        }
        ScriptType::Cyrillic => {
            // Lowercase (Cyrillic has case)
            text.to_lowercase()
        }
        ScriptType::Cjk => {
            // CJK doesn't have case, but might want to normalize variants
            text.to_string()
        }
        ScriptType::Arabic => {
            // Arabic has some normalization forms
            text.to_lowercase()
        }
        ScriptType::Other => text.to_lowercase(),
    }
}

/// Build a case-insensitive regex pattern for a word, respecting script.
pub fn build_word_pattern(word: &str, script_type: ScriptType) -> Regex {
    let escaped = regex::escape(word);
    
    let pattern = match script_type {
        ScriptType::Latin | ScriptType::Cyrillic | ScriptType::Arabic => {
            format!(r"(?i)\b{}\b", escaped)
        }
        ScriptType::Cjk => {
            // CJK doesn't use word boundaries
            escaped.clone()
        }
        ScriptType::Other => {
            format!(r"(?i)\b{}\b", escaped)
        }
    };
    
    RegexBuilder::new(&pattern)
        .build()
        .unwrap_or_else(|_| Regex::new(&escaped).unwrap())
}

/// Check if text contains only the specified script.
pub fn is_script_only(text: &str, script_type: ScriptType) -> bool {
    for c in text.chars() {
        let char_script = match script_type {
            ScriptType::Latin => c.script() == Script::Latin,
            ScriptType::Cyrillic => c.script() == Script::Cyrillic,
            ScriptType::Cjk => matches!(
                c.script(),
                Script::Han | Script::Hiragana | Script::Katakana | Script::Hangul
            ),
            ScriptType::Arabic => c.script() == Script::Arabic,
            ScriptType::Other => true,
        };
        
        if !char_script && !c.is_whitespace() && !c.is_ascii_punctuation() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_script_latin() {
        assert_eq!(detect_script("Hello world"), ScriptType::Latin);
        assert_eq!(detect_script("Bonjour le monde"), ScriptType::Latin);
    }

    #[test]
    fn test_detect_script_cyrillic() {
        assert_eq!(detect_script("Привет мир"), ScriptType::Cyrillic);
        assert_eq!(detect_script("Здравствуйте"), ScriptType::Cyrillic);
    }

    #[test]
    fn test_detect_script_cjk() {
        assert_eq!(detect_script("你好世界"), ScriptType::Cjk);
        assert_eq!(detect_script("こんにちは"), ScriptType::Cjk);
        assert_eq!(detect_script("안녕하세요"), ScriptType::Cjk);
    }

    #[test]
    fn test_detect_script_arabic() {
        assert_eq!(detect_script("مرحبا"), ScriptType::Arabic);
        assert_eq!(detect_script("السلام عليكم"), ScriptType::Arabic);
    }

    #[test]
    fn test_split_into_words_latin() {
        let words = split_into_words("Hello world, how are you?");
        assert_eq!(words, vec!["Hello", "world", "how", "are", "you"]);
    }

    #[test]
    fn test_split_into_words_cyrillic() {
        let words = split_into_words("Привет мир");
        assert_eq!(words, vec!["Привет", "мир"]);
    }

    #[test]
    fn test_split_into_words_cjk() {
        let words = split_into_words("你好世界");
        // CJK splits by characters/sequences
        assert!(!words.is_empty());
    }

    #[test]
    fn test_normalize_for_script() {
        assert_eq!(normalize_for_script("Hello"), "hello");
        assert_eq!(normalize_for_script("Привет"), "привет");
        assert_eq!(normalize_for_script("你好"), "你好");
    }

    #[test]
    fn test_is_script_only() {
        assert!(is_script_only("Hello world", ScriptType::Latin));
        assert!(is_script_only("Привет мир", ScriptType::Cyrillic));
        assert!(is_script_only("你好世界", ScriptType::Cjk));
        assert!(!is_script_only("Hello мир", ScriptType::Latin));
    }

    #[test]
    fn test_build_word_pattern() {
        let pattern = build_word_pattern("hello", ScriptType::Latin);
        assert!(pattern.is_match("hello world"));
        assert!(pattern.is_match("Hello world"));
        assert!(!pattern.is_match("helloworld")); // Word boundary
    }
}
