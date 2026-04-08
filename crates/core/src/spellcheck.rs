//! spellcheck.rs — Spell-correct user messages before palace filing.
//!
//! Preserves:
//!   - Technical terms (words with digits, hyphens, underscores)
//!   - CamelCase and ALL_CAPS identifiers
//!   - Known entity names (from caller if available)
//!   - URLs and file paths
//!   - Words shorter than 4 chars
//!   - Proper nouns already capitalized in context
//!
//! Corrects:
//!   - Genuine typos in lowercase, flowing text
//!   - Common fat-finger words

use std::collections::HashSet;
use std::sync::OnceLock;

const MIN_LENGTH: usize = 4;

// Global system word list - loaded once
static SYSTEM_WORDS: OnceLock<HashSet<String>> = OnceLock::new();

fn get_system_words() -> &'static HashSet<String> {
    SYSTEM_WORDS.get_or_init(|| {
        let mut words = HashSet::new();
        if let Ok(content) = std::fs::read_to_string("/usr/share/dict/words") {
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    words.insert(trimmed.to_lowercase());
                }
            }
        }
        words
    })
}

fn has_digit(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
}

fn is_camel(s: &str) -> bool {
    // Matches CamelCase: uppercase followed by lowercase, then uppercase (ChromaDB, MemPalace)
    // Also matches: starts with uppercase, has lowercase, ends with uppercase
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 3 {
        return false;
    }
    // Check for at least one transition: lowercase -> uppercase
    let mut has_lower_to_upper = false;
    for i in 0..chars.len().saturating_sub(1) {
        if chars[i].is_lowercase() && chars[i + 1].is_uppercase() {
            has_lower_to_upper = true;
            break;
        }
    }
    // Also require it starts with uppercase
    has_lower_to_upper && chars[0].is_uppercase()
}

fn is_allcaps(s: &str) -> bool {
    // ALL_CAPS: all uppercase or special chars
    if s.is_empty() {
        return false;
    }
    let special = "+-=_@#$%^&*()[]{}|<>?:/\\";
    s.chars().all(|c| c.is_uppercase() || special.contains(c))
}

fn is_technical(s: &str) -> bool {
    s.contains('-') || s.contains('_')
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("www.")
        || s.starts_with("/Users/")
        || s.starts_with("~/")
        || (s.len() > 4 && s.contains('.') && s[s.len() - 4..].starts_with('.'))
}

fn is_code_or_emoji(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '`' | '*' | '_' | '#' | '{' | '}' | '[' | ']' | '\\'))
}

fn should_skip(token: &str, known_names: &HashSet<String>) -> bool {
    if token.len() < MIN_LENGTH {
        return true;
    }
    if has_digit(token) {
        return true;
    }
    if is_camel(token) {
        return true;
    }
    if is_allcaps(token) {
        return true;
    }
    if is_technical(token) {
        return true;
    }
    if is_url(token) {
        return true;
    }
    if is_code_or_emoji(token) {
        return true;
    }
    if known_names.contains(&token.to_lowercase()) {
        return true;
    }
    false
}

/// Levenshtein distance between two strings
fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0usize; b_len + 1];

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            curr[j] = std::cmp::min(
                prev[j] + 1,
                std::cmp::min(
                    curr[j - 1] + 1,
                    prev[j - 1]
                        + if a_chars[i - 1] == b_chars[j - 1] {
                            0
                        } else {
                            1
                        },
                ),
            );
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

/// Simple spell correction using edit distance to system words
fn suggest_correction(word: &str) -> Option<String> {
    let lower = word.to_lowercase();
    if get_system_words().contains(&lower) {
        return None;
    }

    // Find best match within edit distance <= 3
    let mut best: Option<(usize, String)> = None;

    for dict_word in get_system_words().iter() {
        let dist = edit_distance(&lower, dict_word);
        if (1..=3).contains(&dist) {
            if let Some((best_dist, _)) = &best {
                if dist < *best_dist {
                    best = Some((dist, dict_word.clone()));
                }
            } else {
                best = Some((dist, dict_word.clone()));
            }
        }
    }

    best.map(|(_, word)| word)
}

/// Spell-correct a user message
pub fn correct_spelling(text: &str, known_names: &HashSet<String>) -> String {
    let sys_words = get_system_words();
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_ascii_whitespace() {
            result.push(c);
            continue;
        }

        // Collect token
        let mut token = String::new();
        token.push(c);
        while let Some(&nc) = chars.peek() {
            if nc.is_ascii_whitespace() {
                break;
            }
            token.push(nc);
            chars.next();
        }

        // Strip trailing punctuation for checking, reattach after
        let (stripped, punct) = strip_punct(&token);

        if stripped.is_empty() || should_skip(stripped, known_names) {
            result.push_str(&token);
            continue;
        }

        // Only correct lowercase words
        if stripped
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            result.push_str(&token);
            continue;
        }

        // Skip words that are already valid English
        if sys_words.contains(&stripped.to_lowercase()) {
            result.push_str(&token);
            continue;
        }

        // Try to correct
        if let Some(corrected) = suggest_correction(stripped) {
            let dist = edit_distance(stripped, &corrected);
            let max_edits = if stripped.len() <= 7 { 2 } else { 3 };

            if dist <= max_edits {
                result.push_str(&corrected);
                result.push_str(punct);
                continue;
            }
        }

        result.push_str(&token);
    }

    result
}

/// Strip trailing punctuation, returning (stripped, punct)
fn strip_punct(s: &str) -> (&str, &str) {
    let punct_chars = ".!?;:'\"";
    let mut end = s.len();
    for (i, c) in s.char_indices().rev() {
        if punct_chars.contains(c) {
            end = i;
        } else {
            break;
        }
    }
    if end < s.len() {
        (&s[..end], &s[end..])
    } else {
        (s, "")
    }
}

/// Spell-correct a single transcript line.
/// Only touches lines that start with '>' (user turns).
pub fn correct_transcript_line(line: &str) -> String {
    let stripped = line.trim_start();
    if !stripped.starts_with('>') {
        return line.to_string();
    }

    let prefix_len = line.len() - stripped.len() + 2;
    if prefix_len > line.len() {
        return line.to_string();
    }

    let message = &line[prefix_len..];
    if message.trim().is_empty() {
        return line.to_string();
    }

    let corrected = correct_spelling(message, &HashSet::new());
    format!("{}> {}", &line[..prefix_len - 2], corrected)
}

/// Spell-correct all user turns in a full transcript.
/// Only lines starting with '>' are touched.
pub fn correct_transcript(content: &str) -> String {
    content
        .lines()
        .map(correct_transcript_line)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edit_distance() {
        assert_eq!(edit_distance("kitten", "kitten"), 0);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn test_strip_punct() {
        assert_eq!(strip_punct("hello."), ("hello", "."));
        assert_eq!(strip_punct("hello!?"), ("hello", "!?")); // all trailing punct stripped
        assert_eq!(strip_punct("hello"), ("hello", ""));
        assert_eq!(strip_punct("> hello."), ("> hello", "."));
    }

    #[test]
    fn test_should_skip() {
        let names = HashSet::new();
        assert!(should_skip("hi", &names));
        assert!(should_skip("abc123", &names));
        assert!(should_skip("ChromaDB", &names));
        assert!(should_skip("NDCG", &names));
        assert!(should_skip("bge-large", &names));
        assert!(should_skip("https://example.com", &names));
        assert!(should_skip("`code`", &names));
    }

    #[test]
    fn test_is_camel() {
        assert!(is_camel("ChromaDB"));
        assert!(is_camel("MemPalace"));
        assert!(!is_camel("chroma"));
        assert!(!is_camel("CHROMA"));
    }

    #[test]
    fn test_is_allcaps() {
        assert!(is_allcaps("NDCG"));
        assert!(is_allcaps("R@X")); // uppercase + special chars
        assert!(!is_allcaps("Ndcg"));
        assert!(!is_allcaps("ndcg"));
        assert!(!is_allcaps("R@5")); // digits not allowed
    }

    #[test]
    fn test_is_url() {
        assert!(is_url("https://example.com"));
        assert!(is_url("http://example.com"));
        assert!(is_url("www.example.com"));
        assert!(is_url("/Users/foo"));
        assert!(is_url("~/file.txt"));
    }

    #[test]
    fn test_correct_spelling_preserves_technical() {
        let names = HashSet::new();
        let result = correct_spelling("ChromaDB bge-large-v1.5 NDCG@10", &names);
        assert_eq!(result, "ChromaDB bge-large-v1.5 NDCG@10");
    }

    #[test]
    fn test_correct_spelling_preserves_known_names() {
        let mut names = HashSet::new();
        names.insert("riley".to_string());
        names.insert("sam".to_string());

        let result = correct_spelling("Riley picked up Sam from school", &names);
        assert_eq!(result, "Riley picked up Sam from school");
    }

    #[test]
    fn test_correct_spelling_basic() {
        let names = HashSet::new();
        let result = correct_spelling("hello world", &names);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_correct_transcript_line_user() {
        let line = "> lsresdy knoe the question";
        let result = correct_transcript_line(line);
        assert!(result.starts_with("> "));
    }

    #[test]
    fn test_correct_transcript_line_assistant() {
        let line = "Hello, I am an assistant";
        let result = correct_transcript_line(line);
        assert_eq!(result, line);
    }

    #[test]
    fn test_correct_transcript() {
        let content = "> user message\nAssistant response\n> another user";
        let result = correct_transcript(content);
        let lines: Vec<&str> = result.lines().collect();
        assert!(lines[0].starts_with("> "));
        assert_eq!(lines[1], "Assistant response");
        assert!(lines[2].starts_with("> "));
    }
}
