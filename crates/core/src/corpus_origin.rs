use regex::Regex;
use serde::{Deserialize, Serialize};

const AI_UNAMBIGUOUS_TERMS: &[&str] = &[
    "Anthropic",
    "Claude Code",
    "Claude 3",
    "Claude 4",
    "claude mcp",
    "CLAUDE.md",
    ".claude/",
    "ChatGPT",
    "GPT-4",
    "GPT-3",
    "GPT-5",
    "OpenAI",
    "gpt-4o",
    "gpt-4-turbo",
    "o1-preview",
    "o3",
    "gemini-pro",
    "gemini-1.5",
    "Google AI",
    "Mixtral",
    "Cohere",
    "MCP",
    "LLM",
    "RAG",
    "fine-tune",
    "context window",
    "embedding",
];

const AI_AMBIGUOUS_TERMS: &[&str] = &[
    "Claude", "Opus", "Sonnet", "Haiku", "Gemini", "Bard", "Llama", "Mistral",
];

const TURN_MARKERS: &[&str] = &[
    r"\buser\s*:\s*",
    r"\bassistant\s*:\s*",
    r"\bhuman\s*:\s*",
    r"\bai\s*:\s*",
    r"\b>>>\s*User\b",
    r"\b>>>\s*Assistant\b",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusOriginResult {
    pub likely_ai_dialogue: bool,
    pub confidence: f64,
    pub primary_platform: Option<String>,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub agent_persona_names: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

fn brand_pattern(term: &str) -> String {
    let escaped = regex::escape(term);
    let prefix = if term
        .chars()
        .next()
        .map(|c| c.is_alphanumeric() || c == '_')
        .unwrap_or(false)
    {
        r"\b"
    } else {
        ""
    };
    let suffix = if term
        .chars()
        .last()
        .map(|c| c.is_alphanumeric() || c == '_')
        .unwrap_or(false)
    {
        r"\b"
    } else {
        ""
    };
    format!("{}{}{}", prefix, escaped, suffix)
}

pub fn detect_origin_heuristic(samples: &[&str]) -> CorpusOriginResult {
    let combined = samples.join("\n\n");
    let total_chars = combined.len().max(1);

    let mut unambiguous_hits: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut total_unambiguous = 0usize;
    for term in AI_UNAMBIGUOUS_TERMS {
        let pattern = brand_pattern(term);
        let re = Regex::new(&pattern).unwrap_or_else(|_| Regex::new("(?i)").unwrap());
        let matches: Vec<_> = re.find_iter(&combined).collect();
        if !matches.is_empty() {
            let count = matches.len();
            unambiguous_hits.insert(term.to_string(), count);
            total_unambiguous += count;
        }
    }

    let mut ambiguous_hits: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut total_ambiguous = 0usize;
    for term in AI_AMBIGUOUS_TERMS {
        let pattern = brand_pattern(term);
        let re = Regex::new(&pattern).unwrap_or_else(|_| Regex::new("(?i)").unwrap());
        let matches: Vec<_> = re.find_iter(&combined).collect();
        if !matches.is_empty() {
            let count = matches.len();
            ambiguous_hits.insert(term.to_string(), count);
            total_ambiguous += count;
        }
    }

    let mut turn_hits = 0usize;
    let mut turn_types_found = 0usize;
    for pattern in TURN_MARKERS {
        let re = Regex::new(pattern).unwrap_or_else(|_| Regex::new("(?i)").unwrap());
        let matches: Vec<_> = re.find_iter(&combined).collect();
        if !matches.is_empty() {
            turn_hits += matches.len();
            turn_types_found += 1;
        }
    }

    let has_ai_context = total_unambiguous > 0 || turn_hits > 0;
    let counted_brand_hits = total_unambiguous + if has_ai_context { total_ambiguous } else { 0 };

    let brand_density = counted_brand_hits as f64 / (total_chars as f64 / 1000.0);
    let turn_density = turn_hits as f64 / (total_chars as f64 / 1000.0);

    let mut evidence: Vec<String> = Vec::new();
    let mut shown_hits = unambiguous_hits.clone();
    if has_ai_context {
        for (k, v) in &ambiguous_hits {
            shown_hits.insert(k.clone(), *v);
        }
    }
    if !shown_hits.is_empty() {
        let mut top_terms: Vec<_> = shown_hits.iter().collect();
        top_terms.sort_by(|a, b| b.1.cmp(a.1));
        let terms_str: Vec<String> = top_terms
            .iter()
            .take(5)
            .map(|(k, v)| format!("'{}' ({}x)", k, v))
            .collect();
        evidence.push(format!("AI brand terms: {}", terms_str.join(", ")));
    } else if !ambiguous_hits.is_empty() && !has_ai_context {
        let mut suppressed: Vec<_> = ambiguous_hits.iter().collect();
        suppressed.sort_by(|a, b| b.1.cmp(a.1));
        let terms_str: Vec<String> = suppressed
            .iter()
            .take(3)
            .map(|(k, v)| format!("'{}' ({}x)", k, v))
            .collect();
        evidence.push(format!(
            "Ambiguous terms present but suppressed (no co-occurring AI signal): {}",
            terms_str.join(", ")
        ));
    }
    if turn_hits > 0 {
        evidence.push(format!(
            "Turn markers detected: {} occurrences across {} pattern types",
            turn_hits, turn_types_found
        ));
    }

    const MEANINGFUL_TEXT_FLOOR: usize = 150;

    if brand_density >= 0.5 || turn_density >= 2.0 {
        let confidence = (0.6 + 0.1 * (brand_density + turn_density)).min(0.95);
        return CorpusOriginResult {
            likely_ai_dialogue: true,
            confidence,
            primary_platform: None,
            user_name: None,
            agent_persona_names: Vec::new(),
            evidence,
        };
    }

    if counted_brand_hits == 0 && turn_hits == 0 && total_chars >= MEANINGFUL_TEXT_FLOOR {
        let mut narrative_evidence = evidence;
        narrative_evidence.push(format!(
            "no unambiguous AI signal across {} chars of text — pure narrative",
            total_chars
        ));
        return CorpusOriginResult {
            likely_ai_dialogue: false,
            confidence: 0.9,
            primary_platform: None,
            user_name: None,
            agent_persona_names: Vec::new(),
            evidence: narrative_evidence,
        };
    }

    let reason = if counted_brand_hits > 0 || turn_hits > 0 {
        "weak signal"
    } else {
        "insufficient text"
    };
    let mut final_evidence = evidence;
    final_evidence.push(format!(
        "{} — applying default-stance (ai_dialogue=True, low confidence). Tier 2 LLM check recommended to confirm or override.",
        reason
    ));
    CorpusOriginResult {
        likely_ai_dialogue: true,
        confidence: 0.4,
        primary_platform: None,
        user_name: None,
        agent_persona_names: Vec::new(),
        evidence: final_evidence,
    }
}

pub fn resolve_corpus_origin(
    palace_path: &std::path::Path,
    _llm_provider: Option<&str>,
) -> CorpusOriginResult {
    let origin_path = palace_path.join(".mempalace").join("origin.json");
    let content = if origin_path.exists() {
        std::fs::read_to_string(&origin_path).ok()
    } else {
        None
    };
    let samples: Vec<&str> = content.as_ref().map_or_else(Vec::new, |c| vec![c.as_str()]);
    detect_origin_heuristic(&samples)
}

pub fn write_origin_json(
    palace_path: &std::path::Path,
    result: &CorpusOriginResult,
) -> std::io::Result<()> {
    let origin_path = palace_path.join(".mempalace").join("origin.json");
    if let Some(parent) = origin_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(result)?;
    std::fs::write(&origin_path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_ai_dialogue_with_unambiguous_terms() {
        let samples = vec![
            "user: Hello\nassistant: Hi, I'm Claude.",
            "ChatGPT is great!",
        ];
        let result = detect_origin_heuristic(&samples);
        assert!(result.likely_ai_dialogue);
        assert!(result.confidence > 0.4);
    }

    #[test]
    fn test_detect_ai_dialogue_with_turn_markers() {
        let samples = vec!["user: What is Rust?\nassistant: Rust is a programming language."];
        let result = detect_origin_heuristic(&samples);
        assert!(result.likely_ai_dialogue);
    }

    #[test]
    fn test_detect_narrative_pure_text() {
        let samples = vec!["Once upon a time in a far away land there lived a brave knight who defended the kingdom."];
        let result = detect_origin_heuristic(&samples);
        assert!(!result.likely_ai_dialogue);
        assert_eq!(result.confidence, 0.9);
    }

    #[test]
    fn test_ambiguous_terms_suppressed_without_context() {
        let samples = vec![
            "My friend Claude told me a story about a haiku.",
            "The gemini constellation is visible tonight.",
        ];
        let result = detect_origin_heuristic(&samples);
        assert!(!result.likely_ai_dialogue);
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_ambiguous_terms_counted_with_context() {
        let samples = vec!["user: Write a poem.\nassistant: Sure, here's a haiku for you."];
        let result = detect_origin_heuristic(&samples);
        assert!(result.likely_ai_dialogue);
    }

    #[test]
    fn test_insufficient_text_defaults_to_ai() {
        let samples = vec!["Hello"];
        let result = detect_origin_heuristic(&samples);
        assert!(result.likely_ai_dialogue);
        assert_eq!(result.confidence, 0.4);
    }

    #[test]
    fn test_confidence_clamped_to_valid_range() {
        let samples = vec!["user: test\nassistant: test"];
        let result = detect_origin_heuristic(&samples);
        assert!(result.confidence >= 0.0 && result.confidence <= 1.0);
    }

    #[test]
    fn test_evidence_includes_brand_terms() {
        let samples = vec!["I love using Claude Code and ChatGPT for my projects."];
        let result = detect_origin_heuristic(&samples);
        assert!(!result.evidence.is_empty());
        assert!(result.evidence.iter().any(|e| e.contains("AI brand terms")));
    }

    #[test]
    fn test_evidence_includes_turn_markers() {
        let samples = vec!["user: Hello\nassistant: Hi there!"];
        let result = detect_origin_heuristic(&samples);
        assert!(result.evidence.iter().any(|e| e.contains("Turn markers")));
    }
}
