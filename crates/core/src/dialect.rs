use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::Path;

fn emotion_signals() -> &'static [(&'static str, &'static str)] {
    &[
        ("decided", "determ"),
        ("prefer", "convict"),
        ("worried", "anx"),
        ("excited", "excite"),
        ("frustrated", "frust"),
        ("confused", "confuse"),
        ("love", "love"),
        ("hate", "rage"),
        ("hope", "hope"),
        ("fear", "fear"),
        ("trust", "trust"),
        ("happy", "joy"),
        ("sad", "grief"),
        ("surprised", "surprise"),
        ("grateful", "grat"),
        ("curious", "curious"),
        ("wonder", "wonder"),
        ("anxious", "anx"),
        ("relieved", "relief"),
        ("satisf", "satis"),
        ("disappoint", "grief"),
        ("concern", "anx"),
    ]
}

fn flag_signals() -> &'static [(&'static str, &'static str)] {
    &[
        ("decided", "DECISION"),
        ("chose", "DECISION"),
        ("switched", "DECISION"),
        ("migrated", "DECISION"),
        ("replaced", "DECISION"),
        ("instead of", "DECISION"),
        ("because", "DECISION"),
        ("founded", "ORIGIN"),
        ("created", "ORIGIN"),
        ("started", "ORIGIN"),
        ("born", "ORIGIN"),
        ("launched", "ORIGIN"),
        ("first time", "ORIGIN"),
        ("core", "CORE"),
        ("fundamental", "CORE"),
        ("essential", "CORE"),
        ("principle", "CORE"),
        ("belief", "CORE"),
        ("always", "CORE"),
        ("never forget", "CORE"),
        ("turning point", "PIVOT"),
        ("changed everything", "PIVOT"),
        ("realized", "PIVOT"),
        ("breakthrough", "PIVOT"),
        ("epiphany", "PIVOT"),
        ("api", "TECHNICAL"),
        ("database", "TECHNICAL"),
        ("architecture", "TECHNICAL"),
        ("deploy", "TECHNICAL"),
        ("infrastructure", "TECHNICAL"),
        ("algorithm", "TECHNICAL"),
        ("framework", "TECHNICAL"),
        ("server", "TECHNICAL"),
        ("config", "TECHNICAL"),
    ]
}

fn stop_words() -> &'static [&'static str] {
    &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "about",
        "between", "through", "during", "before", "after", "above", "below", "up", "down", "out",
        "off", "over", "under", "again", "further", "then", "once", "here", "there", "when",
        "where", "why", "how", "all", "each", "every", "both", "few", "more", "most", "other",
        "some", "such", "no", "nor", "not", "only", "own", "same", "so", "than", "too", "very",
        "just", "don", "now", "and", "but", "or", "if", "while", "that", "this", "these", "those",
        "it", "its", "i", "we", "you", "he", "she", "they", "me", "him", "her", "us", "them", "my",
        "your", "his", "our", "their", "what", "which", "who", "whom", "also", "much", "many",
        "like", "because", "since", "get", "got", "use", "used", "using", "make", "made", "thing",
        "things", "way", "well", "really", "want", "need",
    ]
}

fn decision_words() -> &'static [&'static str] {
    &[
        "decided",
        "because",
        "instead",
        "prefer",
        "switched",
        "chose",
        "realized",
        "important",
        "key",
        "critical",
        "discovered",
        "learned",
        "conclusion",
        "solution",
        "reason",
        "why",
        "breakthrough",
        "insight",
    ]
}

fn token_regex() -> Regex {
    Regex::new(r"[a-zA-Z][a-zA-Z_-]{2,}").expect("valid token regex")
}

fn split_sentence_regex() -> Regex {
    Regex::new(r"[.!?\n]+").expect("valid sentence split regex")
}

fn clean_name_regex() -> Regex {
    Regex::new(r"[^a-zA-Z]").expect("valid clean-name regex")
}

pub fn count_tokens(text: &str) -> usize {
    let words = text.split_whitespace().count();
    std::cmp::max(1, ((words as f64) * 1.3).floor() as usize)
}

fn detect_emotions(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    let mut seen = HashSet::new();
    let mut detected = Vec::new();
    for (keyword, code) in emotion_signals() {
        if lowered.contains(keyword) && seen.insert(*code) {
            detected.push((*code).to_string());
        }
        if detected.len() >= 3 {
            break;
        }
    }
    detected
}

fn detect_flags(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    let mut seen = HashSet::new();
    let mut detected = Vec::new();
    for (keyword, flag) in flag_signals() {
        if lowered.contains(keyword) && seen.insert(*flag) {
            detected.push((*flag).to_string());
        }
        if detected.len() >= 3 {
            break;
        }
    }
    detected
}

fn extract_topics(text: &str, max_topics: usize) -> Vec<String> {
    let stop: HashSet<&str> = stop_words().iter().copied().collect();
    let regex = token_regex();
    let mut freq: HashMap<String, usize> = HashMap::new();

    let words: Vec<String> = regex
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    for word in &words {
        let lowered = word.to_lowercase();
        if lowered.len() < 3 || stop.contains(lowered.as_str()) {
            continue;
        }
        *freq.entry(lowered).or_insert(0) += 1;
    }

    for word in &words {
        let lowered = word.to_lowercase();
        if stop.contains(lowered.as_str()) {
            continue;
        }
        if word
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            if let Some(value) = freq.get_mut(&lowered) {
                *value += 2;
            }
        }
        if (word.contains('_')
            || word.contains('-')
            || word.chars().skip(1).any(|c| c.is_uppercase()))
            && freq.contains_key(&lowered)
        {
            if let Some(value) = freq.get_mut(&lowered) {
                *value += 2;
            }
        }
    }

    let mut ranked: Vec<(String, usize)> = freq.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(max_topics)
        .map(|(word, _)| word)
        .collect()
}

fn extract_key_sentence(text: &str) -> String {
    let splitter = split_sentence_regex();
    let mut scored: Vec<(i32, String)> = splitter
        .split(text)
        .map(str::trim)
        .filter(|s| s.len() > 10)
        .map(|sentence| {
            let lowered = sentence.to_lowercase();
            let mut score = 0;
            for word in decision_words() {
                if lowered.contains(word) {
                    score += 2;
                }
            }
            if sentence.len() < 80 {
                score += 1;
            }
            if sentence.len() < 40 {
                score += 1;
            }
            if sentence.len() > 150 {
                score -= 2;
            }
            (score, sentence.to_string())
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let Some((_, mut best)) = scored.into_iter().next() else {
        return String::new();
    };
    if best.len() > 55 {
        best.truncate(52);
        best.push_str("...");
    }
    best
}

fn detect_entities_in_text(text: &str, people_map: &HashMap<String, String>) -> Vec<String> {
    let lowered = text.to_lowercase();
    let mut found = Vec::new();

    for (name, code) in people_map {
        if name.chars().any(|c| c.is_uppercase())
            && lowered.contains(&name.to_lowercase())
            && !found.contains(code)
        {
            found.push(code.clone());
        }
    }
    if !found.is_empty() {
        return found;
    }

    let cleaner = clean_name_regex();
    for (index, word) in text.split_whitespace().enumerate() {
        let clean = cleaner.replace_all(word, "").to_string();
        let valid_name = clean.len() >= 2
            && clean
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
            && clean.chars().skip(1).all(|c| c.is_lowercase())
            && index > 0
            && !stop_words().contains(&clean.to_lowercase().as_str());
        if valid_name {
            let code = clean.chars().take(3).collect::<String>().to_uppercase();
            if !found.contains(&code) {
                found.push(code);
            }
            if found.len() >= 3 {
                break;
            }
        }
    }

    found
}

pub fn compress(text: &str, people_map: &HashMap<String, String>) -> String {
    compress_with_metadata(text, people_map, None)
}

pub fn compress_with_metadata(
    text: &str,
    people_map: &HashMap<String, String>,
    metadata: Option<&HashMap<String, serde_json::Value>>,
) -> String {
    if text.trim().is_empty() {
        return String::new();
    }

    let entities = detect_entities_in_text(text, people_map);
    let entity_str = if entities.is_empty() {
        "???".to_string()
    } else {
        entities.into_iter().take(3).collect::<Vec<_>>().join("+")
    };

    let topics = extract_topics(text, 3);
    let topic_str = if topics.is_empty() {
        "misc".to_string()
    } else {
        topics.join("_")
    };

    let quote = extract_key_sentence(text);
    let emotions = detect_emotions(text);
    let flags = detect_flags(text);

    let mut lines = Vec::new();
    if let Some(meta) = metadata {
        let source = meta
            .get("source_file")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let wing = meta.get("wing").and_then(|v| v.as_str()).unwrap_or("");
        let room = meta.get("room").and_then(|v| v.as_str()).unwrap_or("");
        let date = meta.get("date").and_then(|v| v.as_str()).unwrap_or("");
        if !source.is_empty() || !wing.is_empty() {
            let source_stem = if source.is_empty() {
                "?".to_string()
            } else {
                Path::new(source)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "?".to_string())
            };
            lines.push(format!(
                "{}|{}|{}|{}",
                if wing.is_empty() { "?" } else { wing },
                if room.is_empty() { "?" } else { room },
                if date.is_empty() { "?" } else { date },
                source_stem
            ));
        }
    }

    let mut parts = vec![format!("0:{entity_str}"), topic_str];
    if !quote.is_empty() {
        parts.push(format!("\"{quote}\""));
    }
    if !emotions.is_empty() {
        parts.push(emotions.join("+"));
    }
    if !flags.is_empty() {
        parts.push(flags.join("+"));
    }
    lines.push(parts.join("|"));

    lines.join("\n")
}

pub fn decompress(aaak_text: &str, _people_map: &HashMap<String, String>) -> String {
    aaak_text.trim().to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompressionStats {
    pub original_tokens_est: usize,
    pub summary_tokens_est: usize,
    pub size_ratio: f64,
    pub original_chars: usize,
    pub summary_chars: usize,
    pub note: &'static str,
}

pub fn compression_stats(original: &str, compressed: &str) -> CompressionStats {
    let original_tokens_est = count_tokens(original);
    let summary_tokens_est = count_tokens(compressed);
    let ratio = original_tokens_est as f64 / std::cmp::max(summary_tokens_est, 1) as f64;

    CompressionStats {
        original_tokens_est,
        summary_tokens_est,
        size_ratio: (ratio * 10.0).round() / 10.0,
        original_chars: original.len(),
        summary_chars: compressed.len(),
        note: "Estimates only. Use tiktoken for accurate counts. AAAK is lossy.",
    }
}

pub fn get_aaak_spec() -> &'static str {
    r#"AAAK Dialect -- Structured Symbolic Summary Format

AAAK is a lossy summarization layer that extracts entities, topics, key sentences,
emotions, and flags into a compact structure. It is not lossless compression and
the original text cannot be reconstructed from AAAK output.

FORMAT:
  Header: FILE_OR_WING|ROOM|DATE|TITLE
  Summary: 0:ENTITIES|topic_keywords|\"key_quote\"|EMOTIONS|FLAGS

FLAGS:
  ORIGIN, CORE, SENSITIVE, PIVOT, GENESIS, DECISION, TECHNICAL

EXAMPLE:
  driftwood|auth-migration|2026-01-15|decision-log
  0:KAI+MAY|clerk_auth_migration|\"Kai recommended Clerk over Auth0\"|convict|DECISION+TECHNICAL

Read AAAK as a compact summary that points back to the verbatim drawers."#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens_estimate_matches_word_heuristic() {
        assert_eq!(count_tokens("one two three four"), 5);
    }

    #[test]
    fn test_compress_emits_lossy_summary_shape() {
        let mut people = HashMap::new();
        people.insert("Alice".to_string(), "ALC".to_string());
        let compressed = compress(
            "Alice decided to migrate the auth system because the database and API setup were fragile.",
            &people,
        );

        assert!(compressed.starts_with("0:"));
        assert!(compressed.contains("ALC"));
        assert!(compressed.contains("DECISION") || compressed.contains("TECHNICAL"));
        assert!(compressed.contains('"'));
    }

    #[test]
    fn test_compress_with_metadata_emits_header() {
        let people = HashMap::new();
        let mut metadata = HashMap::new();
        metadata.insert("wing".to_string(), serde_json::json!("driftwood"));
        metadata.insert("room".to_string(), serde_json::json!("auth-migration"));
        metadata.insert("date".to_string(), serde_json::json!("2026-01-15"));
        metadata.insert(
            "source_file".to_string(),
            serde_json::json!("notes/decision-log.txt"),
        );

        let compressed = compress_with_metadata(
            "We decided to use Clerk instead of Auth0 because pricing was better.",
            &people,
            Some(&metadata),
        );

        let mut lines = compressed.lines();
        assert_eq!(
            lines.next().unwrap(),
            "driftwood|auth-migration|2026-01-15|decision-log"
        );
        assert!(lines.next().unwrap().starts_with("0:"));
    }

    #[test]
    fn test_decompress_is_identity_for_lossy_summary() {
        let people = HashMap::new();
        let aaak = "0:ALC|auth_migration|\"Alice decided to migrate\"|determ|DECISION";
        assert_eq!(decompress(aaak, &people), aaak);
    }

    #[test]
    fn test_compression_stats_use_lossy_summary_fields() {
        let stats = compression_stats("one two three four five six", "0:ALC|topic|\"quote\"");
        assert!(stats.original_tokens_est >= stats.summary_tokens_est);
        assert!(stats.size_ratio >= 1.0);
        assert_eq!(
            stats.note,
            "Estimates only. Use tiktoken for accurate counts. AAAK is lossy."
        );
    }

    #[test]
    fn test_get_aaak_spec_mentions_lossy_summary() {
        let spec = get_aaak_spec();
        assert!(spec.contains("lossy summarization"));
        assert!(spec.contains("FORMAT:"));
        assert!(spec.contains("Read AAAK as a compact summary"));
    }

    #[test]
    fn test_compress_empty_and_decompress_empty() {
        let people = HashMap::new();
        assert_eq!(compress("", &people), "");
        assert_eq!(decompress("", &people), "");
    }
}
