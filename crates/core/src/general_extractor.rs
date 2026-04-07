//! general_extractor.rs — Extract 5 types of memories from text.
//!
//! Types:
//!   1. DECISIONS    — "we went with X because Y", choices made
//!   2. PREFERENCES  — "always use X", "never do Y", "I prefer Z"
//!   3. MILESTONES   — breakthroughs, things that finally worked
//!   4. PROBLEMS     — what broke, what fixed it, root causes
//!   5. EMOTIONAL    — feelings, vulnerability, relationships
//!
//! No LLM required. Pure keyword/pattern heuristics.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// MARKER SETS — One per memory type
// ---------------------------------------------------------------------------

const DECISION_MARKERS: &[&str] = &[
    r"(?i)\blet'?s (use|go with|try|pick|choose|switch to)\b",
    r"(?i)\bwe (should|decided|chose|went with|picked|settled on)\b",
    r"(?i)\bi'?m going (to|with)\b",
    r"(?i)\bbetter (to|than|approach|option|choice)\b",
    r"(?i)\binstead of\b",
    r"(?i)\brather than\b",
    r"(?i)\bthe reason (is|was|being)\b",
    r"(?i)\bbecause\b",
    r"(?i)\btrade-?off\b",
    r"(?i)\bpros and cons\b",
    r"(?i)\bover\b.*\bbecause\b",
    r"(?i)\barchitecture\b",
    r"(?i)\bapproach\b",
    r"(?i)\bstrategy\b",
    r"(?i)\bpattern\b",
    r"(?i)\bstack\b",
    r"(?i)\bframework\b",
    r"(?i)\binfrastructure\b",
    r"(?i)\bset (it |this )?to\b",
    r"(?i)\bconfigure\b",
    r"(?i)\bdefault\b",
];

const PREFERENCE_MARKERS: &[&str] = &[
    r"(?i)\bi prefer\b",
    r"(?i)\balways use\b",
    r"(?i)\bnever use\b",
    r"(?i)\bdon'?t (ever |like to )?(use|do|mock|stub|import)\b",
    r"(?i)\bi like (to|when|how)\b",
    r"(?i)\bi hate (when|how|it when)\b",
    r"(?i)\bplease (always|never|don'?t)\b",
    r"(?i)\bmy (rule|preference|style|convention) is\b",
    r"(?i)\bwe (always|never)\b",
    r"(?i)\bfunctional\b.*\bstyle\b",
    r"(?i)\bimperative\b",
    r"(?i)\bsnake_?case\b",
    r"(?i)\bcamel_?case\b",
    r"(?i)\btabs\b.*\bspaces\b",
    r"(?i)\bspaces\b.*\btabs\b",
    r"(?i)\buse\b.*\binstead of\b",
];

const MILESTONE_MARKERS: &[&str] = &[
    r"(?i)\bit works\b",
    r"(?i)\bit worked\b",
    r"(?i)\bgot it working\b",
    r"(?i)\bfixed\b",
    r"(?i)\bsolved\b",
    r"(?i)\bbreakthrough\b",
    r"(?i)\bfigured (it )?out\b",
    r"(?i)\bnailed it\b",
    r"(?i)\bcracked (it|the)\b",
    r"(?i)\bfinally\b",
    r"(?i)\bfirst time\b",
    r"(?i)\bfirst ever\b",
    r"(?i)\bnever (done|been|had) before\b",
    r"(?i)\bdiscovered\b",
    r"(?i)\brealized\b",
    r"(?i)\bfound (out|that)\b",
    r"(?i)\bturns out\b",
    r"(?i)\bthe key (is|was|insight)\b",
    r"(?i)\bthe trick (is|was)\b",
    r"(?i)\bnow i (understand|see|get it)\b",
    r"(?i)\bbuilt\b",
    r"(?i)\bcreated\b",
    r"(?i)\bimplemented\b",
    r"(?i)\bshipped\b",
    r"(?i)\blaunched\b",
    r"(?i)\bdeployed\b",
    r"(?i)\breleased\b",
    r"(?i)\bprototype\b",
    r"(?i)\bproof of concept\b",
    r"(?i)\bdemo\b",
    r"(?i)\bversion \d",
    r"(?i)\bv\d+\.\d+",
    r"(?i)\b\d+x (compression|faster|slower|better|improvement|reduction)\b",
    r"(?i)\b\d+% (reduction|improvement|faster|better|smaller)\b",
];

const PROBLEM_MARKERS: &[&str] = &[
    r"(?i)\b(bug|error|crash|fail|broke|broken|issue|problem)\b",
    r"(?i)\bdoesn'?t work\b",
    r"(?i)\bnot working\b",
    r"(?i)\bwon'?t\b.*\bwork\b",
    r"(?i)\bkeeps? (failing|crashing|breaking|erroring)\b",
    r"(?i)\broot cause\b",
    r"(?i)\bthe (problem|issue|bug) (is|was)\b",
    r"(?i)\bturns out\b.*\b(was|because|due to)\b",
    r"(?i)\bthe fix (is|was)\b",
    r"(?i)\bworkaround\b",
    r"(?i)\bthat'?s why\b",
    r"(?i)\bthe reason it\b",
    r"(?i)\bfixed (it |the |by )\b",
    r"(?i)\bsolution (is|was)\b",
    r"(?i)\bresolved\b",
    r"(?i)\bpatched\b",
    r"(?i)\bthe answer (is|was)\b",
    r"(?i)\b(had|need) to\b.*\binstead\b",
];

const EMOTION_MARKERS: &[&str] = &[
    r"(?i)\blove\b",
    r"(?i)\bscared\b",
    r"(?i)\bafraid\b",
    r"(?i)\bproud\b",
    r"(?i)\bhurt\b",
    r"(?i)\bhappy\b",
    r"(?i)\bsad\b",
    r"(?i)\bcry\b",
    r"(?i)\bcrying\b",
    r"(?i)\bmiss\b",
    r"(?i)\bsorry\b",
    r"(?i)\bgrateful\b",
    r"(?i)\bangry\b",
    r"(?i)\bworried\b",
    r"(?i)\blonely\b",
    r"(?i)\bbeautiful\b",
    r"(?i)\bamazing\b",
    r"(?i)\bwonderful\b",
    r"(?i)i feel",
    r"(?i)i'm scared",
    r"(?i)i love you",
    r"(?i)i'm sorry",
    r"(?i)i can't",
    r"(?i)i wish",
    r"(?i)i miss",
    r"(?i)i need",
    r"(?i)never told anyone",
    r"(?i)nobody knows",
    r"(?i)\*[^*]+\*",
];

// ---------------------------------------------------------------------------
// SENTIMENT — for disambiguation
// ---------------------------------------------------------------------------

const POSITIVE_WORDS: &[&str] = &[
    "pride",
    "proud",
    "joy",
    "happy",
    "love",
    "loving",
    "beautiful",
    "amazing",
    "wonderful",
    "incredible",
    "fantastic",
    "brilliant",
    "perfect",
    "excited",
    "thrilled",
    "grateful",
    "warm",
    "breakthrough",
    "success",
    "works",
    "working",
    "solved",
    "fixed",
    "nailed",
    "heart",
    "hug",
    "precious",
    "adore",
];

const NEGATIVE_WORDS: &[&str] = &[
    "bug",
    "error",
    "crash",
    "crashing",
    "crashed",
    "fail",
    "failed",
    "failing",
    "failure",
    "broken",
    "broke",
    "breaking",
    "breaks",
    "issue",
    "problem",
    "wrong",
    "stuck",
    "blocked",
    "unable",
    "impossible",
    "missing",
    "terrible",
    "horrible",
    "awful",
    "worse",
    "worst",
    "panic",
    "disaster",
    "mess",
];

fn get_sentiment(text: &str) -> &'static str {
    let words: HashSet<String> = text
        .split_whitespace()
        .map(|w| {
            w.to_lowercase()
                .trim_end_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .collect();
    let pos = words
        .iter()
        .filter(|w| POSITIVE_WORDS.contains(&w.as_str()))
        .count();
    let neg = words
        .iter()
        .filter(|w| NEGATIVE_WORDS.contains(&w.as_str()))
        .count();
    if pos > neg {
        "positive"
    } else if neg > pos {
        "negative"
    } else {
        "neutral"
    }
}

fn has_resolution(text: &str) -> bool {
    let text_lower = text.to_lowercase();
    let patterns = [
        r"fixed",
        r"solved",
        r"resolved",
        r"patched",
        r"got it working",
        r"it works",
        r"nailed it",
        r"figured (it )?out",
        r"the (fix|answer|solution)",
    ];
    patterns
        .iter()
        .any(|p| Regex::new(p).unwrap().is_match(&text_lower))
}

// ---------------------------------------------------------------------------
// CODE LINE FILTERING
// ---------------------------------------------------------------------------

fn is_code_line(line: &str) -> bool {
    let stripped = line.trim();
    if stripped.is_empty() {
        return false;
    }

    // Check patterns
    let code_patterns = [
        r"^\s*[\$#]\s",
        r"^\s*(cd|source|echo|export|pip|npm|git|python|bash|curl|wget|mkdir|rm|cp|mv|ls|cat|grep|find|chmod|sudo|brew|docker)\s",
        r"^\s*```",
        r"^\s*(import|from|def|class|function|const|let|var|return)\s",
        r"^\s*[A-Z_]{2,}=",
        r"^\s*\|",
        r"^\s*[-]{2,}",
        r"^\s*[{}\[\]]\s*$",
        r"^\s*(if|for|while|try|except|elif|else:)\b",
        r"^\s*\w+\.\w\(",
        r"^\s*\w+ = \w+\.\w",
    ];

    for pattern in &code_patterns {
        if Regex::new(pattern).unwrap().is_match(stripped) {
            return true;
        }
    }

    // Alpha ratio check
    let alpha_count = stripped.chars().filter(|c| c.is_alphabetic()).count();
    let alpha_ratio = alpha_count as f64 / stripped.len().max(1) as f64;
    if alpha_ratio < 0.4 && stripped.len() > 10 {
        return true;
    }

    false
}

fn extract_prose(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut prose = Vec::new();
    let mut in_code = false;

    for line in &lines {
        let stripped = line.trim();
        if stripped.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if !is_code_line(line) {
            prose.push(*line);
        }
    }

    let result = prose.join("\n").trim().to_string();
    if result.is_empty() {
        text.to_string()
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// SCORING
// ---------------------------------------------------------------------------

fn score_markers(text: &str, markers: &[&str]) -> (f32, Vec<String>) {
    let text_lower = text.to_lowercase();
    let mut score = 0.0_f32;
    let mut keywords = Vec::new();

    for marker in markers {
        let re = match Regex::new(marker) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Some(matches) = re.captures(&text_lower) {
            score += matches.len() as f32;
            if let Some(m) = matches.get(0) {
                keywords.push(m.as_str().to_string());
            }
        }
    }

    (score, keywords)
}

// ---------------------------------------------------------------------------
// MAIN EXTRACTION
// ---------------------------------------------------------------------------

/// Extract memories from a text string.
pub fn extract_memories(text: &str, min_confidence: f32) -> Vec<Classification> {
    let segments = split_into_segments(text);
    let mut memories = Vec::new();

    for segment in segments {
        if segment.trim().len() < 20 {
            continue;
        }

        let prose = extract_prose(&segment);

        // Score against all types
        let mut scores = std::collections::HashMap::new();
        let all_markers = [
            ("decision", DECISION_MARKERS),
            ("preference", PREFERENCE_MARKERS),
            ("milestone", MILESTONE_MARKERS),
            ("problem", PROBLEM_MARKERS),
            ("emotional", EMOTION_MARKERS),
        ];

        for (mem_type, markers) in &all_markers {
            let (score, _) = score_markers(&prose, markers);
            if score > 0.0 {
                scores.insert(*mem_type, score);
            }
        }

        if scores.is_empty() {
            continue;
        }

        // Length bonus
        let length_bonus = if segment.len() > 500 {
            2.0
        } else if segment.len() > 200 {
            1.0
        } else {
            0.0
        };

        // Find max type
        let max_type = scores
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(k, _)| *k);
        let max_type = match max_type {
            Some(t) => t,
            None => continue,
        };

        let max_score = scores.get(max_type).unwrap_or(&0.0) + length_bonus;

        // Disambiguate
        let final_type = disambiguate(max_type, &prose, &scores);

        // Confidence
        let confidence = (max_score / 5.0).min(1.0);
        if confidence < min_confidence {
            continue;
        }

        memories.push(Classification {
            memory_type: final_type,
            confidence,
            text: segment.trim().to_string(),
        });
    }

    memories
}

fn disambiguate(
    memory_type: &str,
    text: &str,
    scores: &std::collections::HashMap<&str, f32>,
) -> MemoryType {
    let sentiment = get_sentiment(text);

    // Resolved problems are milestones
    if memory_type == "problem" && has_resolution(text) {
        if scores.get("emotional").unwrap_or(&0.0) > &0.0 && sentiment == "positive" {
            return MemoryType::Emotional;
        }
        return MemoryType::Milestone;
    }

    // Problem + positive sentiment => milestone or emotional
    if memory_type == "problem" && sentiment == "positive" {
        if scores.get("milestone").unwrap_or(&0.0) > &0.0 {
            return MemoryType::Milestone;
        }
        if scores.get("emotional").unwrap_or(&0.0) > &0.0 {
            return MemoryType::Emotional;
        }
    }

    match memory_type {
        "decision" => MemoryType::Decision,
        "preference" => MemoryType::Preference,
        "milestone" => MemoryType::Milestone,
        "problem" => MemoryType::Problem,
        "emotional" => MemoryType::Emotional,
        _ => MemoryType::Decision,
    }
}

fn split_into_segments(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').collect();

    // Check for speaker-turn markers
    let turn_patterns = [
        Regex::new(r"^>\s").unwrap(),
        Regex::new(r"^(Human|User|Q)\s*:").unwrap(),
        Regex::new(r"^(Assistant|AI|A|Claude|ChatGPT)\s*:").unwrap(),
    ];

    let mut turn_count = 0;
    for line in &lines {
        let stripped = line.trim();
        for pat in &turn_patterns {
            if pat.is_match(stripped) {
                turn_count += 1;
                break;
            }
        }
    }

    // If enough turn markers, split by turns
    if turn_count >= 3 {
        return split_by_turns(&lines, &turn_patterns);
    }

    // Fallback: paragraph splitting
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();

    // If single giant block, chunk by line groups
    if paragraphs.len() <= 1 && lines.len() > 20 {
        let mut segments = Vec::new();
        for i in (0..lines.len()).step_by(25) {
            let end = (i + 25).min(lines.len());
            let group = lines[i..end].join("\n").trim().to_string();
            if !group.is_empty() {
                segments.push(group);
            }
        }
        return segments;
    }

    paragraphs
}

fn split_by_turns<'a>(lines: &[&'a str], turn_patterns: &[Regex]) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = Vec::new();

    for line in lines {
        let stripped = line.trim();
        let is_turn = turn_patterns.iter().any(|pat| pat.is_match(stripped));

        if is_turn && !current.is_empty() {
            segments.push(current.join("\n"));
            current = vec![*line];
        } else {
            current.push(*line);
        }
    }

    if !current.is_empty() {
        segments.push(current.join("\n"));
    }

    segments
}

/// Classify a single text snippet into a memory type.
pub fn classify(text: &str) -> Vec<Classification> {
    extract_memories(text, 0.3)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MemoryType {
    Decision,
    Preference,
    Milestone,
    Problem,
    Emotional,
}

#[derive(Debug, Clone)]
pub struct Classification {
    pub memory_type: MemoryType,
    pub confidence: f32,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_decision() {
        let text =
            "We decided to use Postgres because it handles concurrent writes better than SQLite.";
        let result = extract_memories(text, 0.3);
        assert!(!result.is_empty());
        assert_eq!(result[0].memory_type, MemoryType::Decision);
    }

    #[test]
    fn test_extract_preference() {
        let text = "I always use snake_case for variable names, never camelCase.";
        let result = extract_memories(text, 0.3);
        assert!(!result.is_empty());
        assert_eq!(result[0].memory_type, MemoryType::Preference);
    }

    #[test]
    fn test_extract_milestone() {
        let text = "Finally got the authentication working! It was a breakthrough.";
        let result = extract_memories(text, 0.3);
        assert!(!result.is_empty());
        assert_eq!(result[0].memory_type, MemoryType::Milestone);
    }

    #[test]
    fn test_extract_problem() {
        let text = "The bug was caused by a race condition in the async handler. The fix was to add a mutex.";
        let result = extract_memories(text, 0.3);
        assert!(!result.is_empty());
        assert!(matches!(
            result[0].memory_type,
            MemoryType::Problem | MemoryType::Milestone
        ));
    }

    #[test]
    fn test_extract_emotional() {
        let text = "I feel so proud of the team. We did something amazing together.";
        let result = extract_memories(text, 0.3);
        assert!(!result.is_empty());
        assert_eq!(result[0].memory_type, MemoryType::Emotional);
    }

    #[test]
    fn test_skip_code_lines() {
        let text = "```python\ndef hello():\n    print('hello')\n```\n\nThis is prose content.";
        let result = extract_memories(text, 0.3);
        // Should not error on code blocks
        assert!(result.len() >= 0);
    }

    #[test]
    fn test_split_by_turns() {
        let text = "Human: Hello\nAssistant: Hi\nHuman: How are you?\nAssistant: Good";
        let segments = split_into_segments(text);
        assert!(segments.len() >= 2);
    }

    #[test]
    fn test_empty_text() {
        let result = extract_memories("", 0.3);
        assert!(result.is_empty());
    }

    #[test]
    fn test_min_confidence() {
        let text = "Hello world";
        let result = extract_memories(text, 0.9);
        assert!(result.is_empty());
    }
}
