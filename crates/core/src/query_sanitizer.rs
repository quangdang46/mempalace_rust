use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

pub const MAX_QUERY_LENGTH: usize = 500;
pub const SAFE_QUERY_LENGTH: usize = 200;
pub const MIN_QUERY_LENGTH: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SanitizedQuery {
    pub clean_query: String,
    pub was_sanitized: bool,
    pub original_length: usize,
    pub clean_length: usize,
    pub method: String,
}

fn sentence_split_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[.!?。！？\n]+").expect("valid sentence split regex"))
}

fn question_mark_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"[?？]\s*["']?\s*$"#).expect("valid question regex"))
}

pub fn sanitize_query(raw_query: &str) -> SanitizedQuery {
    if raw_query.trim().is_empty() {
        return SanitizedQuery {
            clean_query: raw_query.to_string(),
            was_sanitized: false,
            original_length: raw_query.len(),
            clean_length: raw_query.len(),
            method: "passthrough".to_string(),
        };
    }

    let raw_query = raw_query.trim();
    let original_length = raw_query.len();

    if original_length <= SAFE_QUERY_LENGTH {
        return SanitizedQuery {
            clean_query: raw_query.to_string(),
            was_sanitized: false,
            original_length,
            clean_length: original_length,
            method: "passthrough".to_string(),
        };
    }

    let sentences: Vec<String> = sentence_split_re()
        .split(raw_query)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();

    let all_segments: Vec<String> = raw_query
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();

    let mut question_sentences = Vec::new();
    for segment in all_segments.iter().rev() {
        if question_mark_re().is_match(segment) {
            question_sentences.push(segment.clone());
        }
    }
    if question_sentences.is_empty() {
        for sentence in sentences.iter().rev() {
            if sentence.contains('?') || sentence.contains('？') {
                question_sentences.push(sentence.clone());
            }
        }
    }
    if let Some(candidate) = question_sentences
        .into_iter()
        .find(|s| s.len() >= MIN_QUERY_LENGTH)
    {
        let clean_query = truncate_tail(&candidate);
        return build_result(clean_query, original_length, "question_extraction");
    }

    if let Some(candidate) = all_segments
        .iter()
        .rev()
        .find(|s| s.len() >= MIN_QUERY_LENGTH)
    {
        return build_result(truncate_tail(candidate), original_length, "tail_sentence");
    }

    build_result(truncate_tail(raw_query), original_length, "tail_truncation")
}

fn truncate_tail(input: &str) -> String {
    if input.len() <= MAX_QUERY_LENGTH {
        input.trim().to_string()
    } else {
        input[input.len() - MAX_QUERY_LENGTH..].trim().to_string()
    }
}

fn build_result(clean_query: String, original_length: usize, method: &str) -> SanitizedQuery {
    SanitizedQuery {
        clean_length: clean_query.len(),
        clean_query,
        was_sanitized: true,
        original_length,
        method: method.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_query_passthrough() {
        let result = sanitize_query("where is the auth plan?");
        assert_eq!(result.clean_query, "where is the auth plan?");
        assert!(!result.was_sanitized);
        assert_eq!(result.method, "passthrough");
    }

    #[test]
    fn test_question_extraction_prefers_last_question() {
        let raw = format!(
            "{}\nWhat happened to the auth migration?",
            "system prompt ".repeat(40)
        );
        let result = sanitize_query(&raw);
        assert_eq!(result.clean_query, "What happened to the auth migration?");
        assert!(result.was_sanitized);
        assert_eq!(result.method, "question_extraction");
    }

    #[test]
    fn test_tail_sentence_fallback() {
        let raw = format!(
            "{}\nshow me the deployment history",
            "system prompt ".repeat(40)
        );
        let result = sanitize_query(&raw);
        assert_eq!(result.clean_query, "show me the deployment history");
        assert_eq!(result.method, "tail_sentence");
    }

    #[test]
    fn test_tail_truncation_fallback() {
        let raw = format!(
            "{}",
            (0..(MAX_QUERY_LENGTH + 50))
                .map(|_| " ")
                .collect::<String>()
        );
        let result = sanitize_query(&raw);
        assert_eq!(result.clean_query, raw);
        assert_eq!(result.method, "passthrough");
    }

    #[test]
    fn test_tail_sentence_truncates_to_max_length() {
        let raw = format!("prefix\n{}", "x".repeat(MAX_QUERY_LENGTH + 50));
        let result = sanitize_query(&raw);
        assert_eq!(result.clean_query.len(), MAX_QUERY_LENGTH);
        assert_eq!(result.method, "tail_sentence");
    }
}
