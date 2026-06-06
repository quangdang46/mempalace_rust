//! CJK script-aware segmentation.
//!
//! Splits text on script boundaries (Han / Hiragana / Katakana / Hangul) for search indexing.
//!
//! When the `cjk-jieba` feature is enabled, [`segment_cjk_with_jieba`] additionally
//! runs the Han script runs through `jieba-rs` to produce true Chinese word
//! tokens (1:1 with mempalace's jieba path). When the feature is disabled
//! (the default), Han runs are kept as whole-script runs via [`segment_cjk`].

use unicode_script::UnicodeScript;

/// Script classification for a text run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// CJK ideographs (Chinese, Japanese kanji, Korean hanja)
    Han,
    /// Japanese hiragana
    Hiragana,
    /// Japanese katakana
    Katakana,
    /// Korean hangul
    Hangul,
    /// Latin alphabet
    Latin,
    /// Any other Unicode script
    Other,
}

impl Script {
    fn from_unicode_script(script: unicode_script::Script) -> Self {
        use unicode_script::Script as U;
        match script {
            U::Han => Script::Han,
            U::Hiragana => Script::Hiragana,
            U::Katakana => Script::Katakana,
            U::Hangul => Script::Hangul,
            U::Latin => Script::Latin,
            _ => Script::Other,
        }
    }
}

/// Detect the dominant script in a piece of text.
///
/// Priority: Han > Hiragana/Katakana > Hangul > Latin > Other.
/// Returns the first non-Other script found when scanning left to right.
pub fn detect_script(text: &str) -> Script {
    for ch in text.chars() {
        let us = ch.script();
        let script = Script::from_unicode_script(us);
        if script != Script::Other {
            return script;
        }
    }
    Script::Other
}

/// Returns true if the text contains any CJK characters (Han, Hiragana, Katakana, or Hangul).
pub fn has_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        let us = ch.script();
        matches!(
            us,
            unicode_script::Script::Han
                | unicode_script::Script::Hiragana
                | unicode_script::Script::Katakana
                | unicode_script::Script::Hangul
        )
    })
}

/// Returns true if the given script is a CJK script (Han, Hiragana, Katakana, Hangul).
fn is_cjk_script(script: Script) -> bool {
    matches!(
        script,
        Script::Han | Script::Hiragana | Script::Katakana | Script::Hangul
    )
}

/// Returns true if the given script should be grouped with Latin/Other for segmentation.
fn is_latin_like(script: Script) -> bool {
    matches!(script, Script::Latin | Script::Other)
}

/// Segment text on script boundaries, returning a list of contiguous script-run tokens.
///
/// Han, Hiragana, Katakana, and Hangul each form their own tokens.
/// Latin and Other text are grouped together as a single contiguous token.
/// Empty runs are discarded.
pub fn segment_cjk(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut tokens: Vec<String> = Vec::new();
    let mut current_script: Option<Script> = None;
    let mut current_chunk = String::new();

    for ch in text.chars() {
        let script = Script::from_unicode_script(ch.script());
        let is_latin = is_latin_like(script);

        match current_script {
            None => {
                // First character
                current_script = Some(script);
                current_chunk.push(ch);
            }
            Some(cur) if cur == script => {
                // Same script — extend current chunk
                current_chunk.push(ch);
            }
            Some(cur) if is_latin_like(cur) && is_latin => {
                // Both Latin-like (Latin or Other) — extend current chunk
                current_chunk.push(ch);
            }
            Some(cur) if is_cjk_script(cur) && is_latin => {
                // CJK → Latin/Other boundary — split
                if !current_chunk.is_empty() {
                    tokens.push(current_chunk.clone());
                }
                current_chunk.clear();
                current_chunk.push(ch);
                current_script = Some(script);
            }
            Some(cur) if is_latin && is_cjk_script(script) => {
                // Latin/Other → CJK boundary — split
                if !current_chunk.is_empty() {
                    tokens.push(current_chunk.clone());
                }
                current_chunk.clear();
                current_chunk.push(ch);
                current_script = Some(script);
            }
            Some(cur) if is_cjk_script(cur) && is_cjk_script(script) => {
                // CJK → CJK boundary (different CJK scripts) — split
                if !current_chunk.is_empty() {
                    tokens.push(current_chunk.clone());
                }
                current_chunk.clear();
                current_chunk.push(ch);
                current_script = Some(script);
            }
            _ => {
                // Default: split on script change
                if !current_chunk.is_empty() {
                    tokens.push(current_chunk.clone());
                }
                current_chunk.clear();
                current_chunk.push(ch);
                current_script = Some(script);
            }
        }
    }

    // Emit the final chunk
    if !current_chunk.is_empty() {
        tokens.push(current_chunk);
    }

    tokens
}

/// True Chinese word segmentation via `jieba-rs`, 1:1 with mempalace.
///
/// Falls back to [`segment_cjk`] (script-boundary splitting) for the
/// non-Han runs (Hiragana, Katakana, Hangul, Latin, Other). Only the
/// Han runs are passed through jieba for word-level tokens.
#[cfg(feature = "cjk-jieba")]
pub fn segment_cjk_with_jieba(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    // First split on script boundaries, then re-segment the Han runs
    // through jieba for true Chinese word tokens.
    let mut out: Vec<String> = Vec::new();
    for run in segment_cjk(text) {
        if detect_script(&run) == Script::Han {
            // Use jieba's HMM mode for ambiguous words (jieba-rs default).
            let jieba = jieba_rs::Jieba::new();
            out.extend(
                jieba
                    .cut(&run, true)
                    .into_iter()
                    .map(|t| t.word.to_string()),
            );
        } else {
            out.push(run);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_script_han() {
        assert_eq!(detect_script("中文"), Script::Han);
    }

    #[test]
    fn test_detect_script_katakana() {
        assert_eq!(detect_script("カタカナ"), Script::Katakana);
    }

    #[test]
    fn test_detect_script_hiragana() {
        assert_eq!(detect_script("ひらがな"), Script::Hiragana);
    }

    #[test]
    fn test_detect_script_latin() {
        assert_eq!(detect_script("hello"), Script::Latin);
    }

    #[test]
    fn test_has_cjk_true() {
        assert!(has_cjk("hello 中文"));
    }

    #[test]
    fn test_has_cjk_false() {
        assert!(!has_cjk("hello world"));
    }

    #[test]
    fn test_segment_cjk_mixed() {
        let tokens = segment_cjk("hello中文world");
        assert_eq!(tokens, vec!["hello", "中文", "world"]);
    }

    #[test]
    fn test_segment_cjk_japanese() {
        let tokens = segment_cjk("日本語のテスト");
        // Should be split on script boundaries: 日本語 / の / テスト
        // Hiragana and Katakana each get their own run
        assert!(tokens.len() >= 3, "expected >=3 tokens, got {:?}", tokens);
    }

    #[test]
    fn test_segment_cjk_empty() {
        assert!(segment_cjk("").is_empty());
    }

    #[test]
    fn test_segment_cjk_only_latin() {
        assert_eq!(segment_cjk("hello world"), vec!["hello world"]);
    }

    #[test]
    fn test_segment_cjk_only_cjk() {
        let tokens = segment_cjk("中文");
        assert_eq!(tokens, vec!["中文"]);
    }

    #[test]
    fn test_detect_script_other() {
        assert_eq!(detect_script("🧠"), Script::Other);
    }

    #[test]
    fn test_has_cjk_katakana() {
        assert!(has_cjk("カタカナ"));
    }

    #[test]
    fn test_has_cjk_hiragana() {
        assert!(has_cjk("ひらがな"));
    }

    #[test]
    fn test_has_cjk_hangul() {
        assert!(has_cjk("한글"));
    }

    #[test]
    fn test_segment_cjk_hangul() {
        let tokens = segment_cjk("한글테스트");
        // Hangul runs as one token
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], "한글테스트");
    }

    #[test]
    fn test_segment_cjk_mixed_latin_cjk() {
        let tokens = segment_cjk("abc한글def");
        assert_eq!(tokens, vec!["abc", "한글", "def"]);
    }
}
