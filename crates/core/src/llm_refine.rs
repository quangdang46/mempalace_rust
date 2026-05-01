//! llm_refine.rs — Optional LLM refinement of regex-detected entities.
//!
//! Takes candidate set produced by phase-1 detection (manifests, git
//! authors, regex on prose) and reclassifies each via LLM as
//! PERSON / PROJECT / TOPIC / COMMON_WORD / AMBIGUOUS.
//!
//! Design constraints:
//! - Opt-in. Default init path never calls this module.
//! - Local-first by default (Ollama).
//! - Batch processing (25 candidates per LLM call).
//! - Don't feed raw corpus to LLM — candidates + sampled contexts only.

use crate::llm_client::LlmProvider;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BATCH_SIZE: usize = 25;
const CONTEXT_LINES_PER_CANDIDATE: usize = 3;
const CONTEXT_WINDOW_CHARS: usize = 240;

const SYSTEM_PROMPT: &str = r#"You are helping organize a user's memory palace by classifying capitalized tokens found in their files.

For each candidate, pick exactly ONE label:
- PERSON: a specific real person the user knows (colleague, family, character they write about)
- PROJECT: a named product, codebase, or effort the user works on
- TOPIC: a recurring theme or subject (not a person, not a project) — cities, technologies, concepts
- COMMON_WORD: an English word, verb, or fragment that isn't a named entity at all (e.g. "Created", "Before", "Never")
- AMBIGUOUS: context is insufficient to decide between two of the above

Frameworks, runtimes, APIs, cloud services, vendors, and third-party products
(e.g. Angular, OpenAPI, Terraform, Bun, Google) are TOPIC unless the context
clearly says this is the user's own named codebase, product, or active effort.

Use the provided context lines to disambiguate. A capitalized word that only appears in metadata ("Created: 2026-04-24") is COMMON_WORD. A name that appears with pronouns and dialogue is PERSON.

Respond with JSON only. Schema:
{"classifications": [{"name": "<exact candidate name>", "label": "<LABEL>", "reason": "<one short sentence>"}]}

One entry per candidate, same order as the input."#;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityLabel {
    Person,
    Project,
    Topic,
    CommonWord,
    Ambiguous,
}

impl EntityLabel {
    fn from_str(s: &str) -> Self {
        match s.trim().to_uppercase().as_str() {
            "PERSON" => EntityLabel::Person,
            "PROJECT" => EntityLabel::Project,
            "TOPIC" => EntityLabel::Topic,
            "COMMON_WORD" => EntityLabel::CommonWord,
            _ => EntityLabel::Ambiguous,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            EntityLabel::Person => "PERSON",
            EntityLabel::Project => "PROJECT",
            EntityLabel::Topic => "TOPIC",
            EntityLabel::CommonWord => "COMMON_WORD",
            EntityLabel::Ambiguous => "AMBIGUOUS",
        }
    }

    fn as_bucket(&self) -> &'static str {
        match self {
            EntityLabel::Person => "people",
            EntityLabel::Project => "projects",
            EntityLabel::Topic => "topics",
            EntityLabel::CommonWord => return "", // dropped
            EntityLabel::Ambiguous => "uncertain",
        }
    }
}

#[derive(Debug)]
pub struct Classification {
    pub name: String,
    pub label: EntityLabel,
    pub reason: String,
}

#[derive(Debug)]
pub struct RefineResult {
    pub merged: DetectedEntities,
    pub reclassified: usize,
    pub dropped: usize,
    pub errors: Vec<String>,
    pub batches_completed: usize,
    pub batches_total: usize,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DetectedEntities {
    pub people: Vec<EntityEntry>,
    pub projects: Vec<EntityEntry>,
    pub topics: Vec<EntityEntry>,
    pub uncertain: Vec<EntityEntry>,
}

#[derive(Debug, Clone)]
pub struct EntityEntry {
    pub name: String,
    pub entry_type: String,
    pub signals: Vec<String>,
}

impl DetectedEntities {
    pub fn from_detected_map(detected: &HashMap<String, Vec<EntityEntry>>) -> Self {
        Self {
            people: detected.get("people").cloned().unwrap_or_default(),
            projects: detected.get("projects").cloned().unwrap_or_default(),
            topics: detected.get("topics").cloned().unwrap_or_default(),
            uncertain: detected.get("uncertain").cloned().unwrap_or_default(),
        }
    }

    pub fn to_map(&self) -> HashMap<String, Vec<EntityEntry>> {
        let mut m = HashMap::new();
        if !self.people.is_empty() {
            m.insert("people".to_string(), self.people.clone());
        }
        if !self.projects.is_empty() {
            m.insert("projects".to_string(), self.projects.clone());
        }
        if !self.topics.is_empty() {
            m.insert("topics".to_string(), self.topics.clone());
        }
        if !self.uncertain.is_empty() {
            m.insert("uncertain".to_string(), self.uncertain.clone());
        }
        m
    }
}

// ---------------------------------------------------------------------------
// Context collection
// ---------------------------------------------------------------------------

fn collect_contexts(corpus_lines: &[String], name: &str, max_lines: usize) -> Vec<String> {
    let needle_regex = match Regex::new(&format!(r"(?<!\w){}(?!\w)", regex::escape(name))) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in corpus_lines {
        if !needle_regex.is_match(line) {
            continue;
        }
        let trimmed = line
            .trim()
            .chars()
            .take(CONTEXT_WINDOW_CHARS)
            .collect::<String>();
        if trimmed.is_empty() || seen.contains(&trimmed) {
            continue;
        }
        seen.insert(trimmed.clone());
        out.push(trimmed);
        if out.len() >= max_lines {
            break;
        }
    }
    out
}

fn build_user_prompt(candidates_with_contexts: &[(String, String, Vec<String>)]) -> String {
    let mut parts = vec!["CANDIDATES:".to_string()];
    for (i, (name, current_type, contexts)) in candidates_with_contexts.iter().enumerate() {
        parts.push(format!(
            "\n{}. {}  (currently: {})",
            i + 1,
            name,
            current_type
        ));
        if contexts.is_empty() {
            parts.push("   > (no context available)".to_string());
        } else {
            for c in contexts {
                parts.push(format!("   > {}", c));
            }
        }
    }
    parts.join("\n")
}

// ---------------------------------------------------------------------------
// JSON parsing
// ---------------------------------------------------------------------------

fn extract_json_candidates(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }

    let mut candidates = vec![text.to_string()];

    // Try markdown code blocks
    let re_code = Regex::new(r"```(?:json)?\s*([\s\S]*?)\s*```").unwrap();
    for cap in re_code.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let candidate = m.as_str().trim().to_string();
            if !candidate.is_empty() && !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }

    // Try finding JSON objects directly
    for (opener, closer) in [('{', '}'), ('[', ']')] {
        let mut depth = 0;
        let mut in_string = false;
        let mut escaped = false;
        let mut start = None;
        for (i, ch) in text.char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            if ch == '"' {
                in_string = true;
            } else if ch == opener {
                if start.is_none() {
                    start = Some(i);
                }
                depth += 1;
            } else if ch == closer {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        let candidate = text[s..=i].trim().to_string();
                        if !candidate.is_empty() && !candidates.contains(&candidate) {
                            candidates.push(candidate);
                        }
                    }
                    start = None;
                }
            }
        }
    }

    candidates
}

fn parse_response(text: &str, expected_names: &[String]) -> HashMap<String, (EntityLabel, String)> {
    let mut result = HashMap::new();

    for candidate in extract_json_candidates(text) {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&candidate) {
            let classifications = data.get("classifications").and_then(|v| v.as_array());
            let entries = match classifications {
                Some(arr) => arr.clone(),
                None if data.is_array() => data.as_array().unwrap().clone(),
                _ => continue,
            };

            let expected_lower: HashMap<String, String> = expected_names
                .iter()
                .map(|n| (n.to_lowercase(), n.clone()))
                .collect();

            for entry in entries {
                let obj = match entry.as_object() {
                    Some(o) => o,
                    None => continue,
                };

                let name = obj
                    .get("name")
                    .or(obj.get("candidate"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let label = obj
                    .get("label")
                    .or(obj.get("type"))
                    .or(obj.get("classification"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let reason = obj
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.chars().take(120).collect::<String>())
                    .unwrap_or_default();

                let (Some(name), Some(label)) = (name, label) else {
                    continue;
                };

                let canonical = expected_lower
                    .get(&name.to_lowercase())
                    .cloned()
                    .unwrap_or(name.clone());
                let lbl = EntityLabel::from_str(&label);
                result.insert(canonical, (lbl, reason));
            }
            break; // first parseable candidate wins
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Classification merging
// ---------------------------------------------------------------------------

fn is_authoritative_person(entry: &EntityEntry) -> bool {
    let signals_str = entry.signals.join(" ").to_lowercase();
    signals_str.contains("commit") && signals_str.contains("repo")
}

fn is_authoritative_project(entry: &EntityEntry) -> bool {
    let signals_str = entry.signals.join(" ").to_lowercase();
    let manifest_markers = ["package.json", "pyproject.toml", "cargo.toml", "go.mod"];
    manifest_markers.iter().any(|m| signals_str.contains(m)) || signals_str.contains("commit")
}

fn apply_classifications(
    detected: &DetectedEntities,
    decisions: &HashMap<String, (EntityLabel, String)>,
    allow_project_promotions: bool,
) -> (DetectedEntities, usize, usize) {
    let mut new_detected = DetectedEntities::default();
    let mut reclassified = 0usize;
    let mut dropped = 0usize;

    let label_to_bucket: HashMap<String, &str> = [
        ("PERSON", "people"),
        ("PROJECT", "projects"),
        ("TOPIC", "topics"),
        ("AMBIGUOUS", "uncertain"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    let bucket_to_type: HashMap<&str, &str> = [
        ("people", "person"),
        ("projects", "project"),
        ("topics", "topic"),
        ("uncertain", "uncertain"),
    ]
    .into_iter()
    .collect();

    let buckets = [
        ("people", &detected.people),
        ("projects", &detected.projects),
        ("uncertain", &detected.uncertain),
    ];

    for (old_bucket, entries) in buckets {
        for entry in entries {
            if old_bucket == "people" && is_authoritative_person(entry) {
                new_detected.people.push(entry.clone());
                continue;
            }
            if old_bucket == "projects" && is_authoritative_project(entry) {
                new_detected.projects.push(entry.clone());
                continue;
            }

            let decision = decisions.get(&entry.name);

            let Some((label, reason)) = decision else {
                // No LLM opinion — keep as-is
                match old_bucket {
                    "people" => new_detected.people.push(entry.clone()),
                    "projects" => new_detected.projects.push(entry.clone()),
                    "uncertain" => new_detected.uncertain.push(entry.clone()),
                    _ => {}
                }
                continue;
            };

            if *label == EntityLabel::CommonWord {
                dropped += 1;
                continue;
            }

            let target_bucket = label_to_bucket
                .get(label.as_str())
                .copied()
                .unwrap_or("uncertain");

            let final_bucket = if *label == EntityLabel::Project
                && !allow_project_promotions
                && !is_authoritative_project(entry)
            {
                "uncertain"
            } else {
                target_bucket
            };

            let mut updated = entry.clone();
            let mut signals = updated.signals.clone();
            let reason_short = if reason.is_empty() {
                format!("LLM: {}", label.as_str().to_lowercase())
            } else {
                format!("LLM: {} — {}", label.as_str().to_lowercase(), reason)
            };
            signals.push(reason_short);
            updated.signals = signals;
            updated.entry_type = bucket_to_type
                .get(final_bucket)
                .unwrap_or(&"uncertain")
                .to_string();

            if final_bucket != old_bucket {
                reclassified += 1;
            }

            match final_bucket {
                "people" => new_detected.people.push(updated),
                "projects" => new_detected.projects.push(updated),
                "topics" => new_detected.topics.push(updated),
                "uncertain" => new_detected.uncertain.push(updated),
                _ => {}
            }
        }
    }

    (new_detected, reclassified, dropped)
}

// ---------------------------------------------------------------------------
// Main refinement entry point
// ---------------------------------------------------------------------------

/// Reclassify detected entities using the LLM provider.
///
/// `cancel_flag` is an atomic boolean that, when set to true, will cause
/// the refinement to stop after the current batch and return partial results.
pub fn refine_entities(
    detected: &DetectedEntities,
    corpus_text: &str,
    provider: &dyn LlmProvider,
    batch_size: usize,
    show_progress: bool,
    allow_project_promotions: bool,
    cancel_flag: Option<&AtomicBool>,
) -> RefineResult {
    let batch_size = if batch_size == 0 {
        BATCH_SIZE
    } else {
        batch_size
    };

    // Collect candidates from people/projects/uncertain buckets
    let mut candidates: Vec<(String, String)> = Vec::new();
    let current_type = [
        ("people", "person"),
        ("projects", "project"),
        ("uncertain", "uncertain"),
    ];

    for (bucket_name, type_str) in current_type {
        let entries = match bucket_name {
            "people" => &detected.people,
            "projects" => &detected.projects,
            "uncertain" => &detected.uncertain,
            _ => continue,
        };
        for entry in entries {
            if bucket_name == "people" && is_authoritative_person(entry) {
                continue;
            }
            if bucket_name == "projects" && is_authoritative_project(entry) {
                continue;
            }
            candidates.push((entry.name.clone(), type_str.to_string()));
        }
    }

    let corpus_lines: Vec<String> = if corpus_text.is_empty() {
        vec![]
    } else {
        corpus_text.lines().map(|s| s.to_string()).collect()
    };

    // Deduplicate while preserving order
    let mut seen = HashSet::new();
    let mut unique: Vec<(String, String)> = Vec::new();
    for (name, kind) in candidates {
        if seen.insert(name.clone()) {
            unique.push((name, kind));
        }
    }

    if unique.is_empty() {
        return RefineResult {
            merged: detected.clone(),
            reclassified: 0,
            dropped: 0,
            errors: vec![],
            batches_completed: 0,
            batches_total: 0,
            cancelled: false,
        };
    }

    // Build batches
    let mut batches: Vec<Vec<(String, String, Vec<String>)>> = Vec::new();
    for chunk in unique.chunks(batch_size) {
        let enriched: Vec<(String, String, Vec<String>)> = chunk
            .iter()
            .map(|(name, kind)| {
                let contexts = collect_contexts(&corpus_lines, name, CONTEXT_LINES_PER_CANDIDATE);
                (name.clone(), kind.clone(), contexts)
            })
            .collect();
        batches.push(enriched);
    }

    let mut all_decisions: HashMap<String, (EntityLabel, String)> = HashMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut completed = 0usize;
    let mut cancelled = false;

    for (idx, batch) in batches.iter().enumerate() {
        if show_progress && !batch.is_empty() {
            eprint_progress(idx, batches.len(), &batch[0].0, batches.len());
        }

        // Check cancellation
        if let Some(flag) = cancel_flag {
            if flag.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
        }

        let names_in_batch: Vec<String> = batch.iter().map(|(n, _, _)| n.clone()).collect();
        let user_prompt = build_user_prompt(batch);

        let resp = match provider.classify(SYSTEM_PROMPT, &user_prompt, true) {
            Ok(r) => r,
            Err(e) => {
                errors.push(format!("batch {}: {}", idx + 1, e));
                continue;
            }
        };

        let decisions = parse_response(&resp.text, &names_in_batch);
        if decisions.is_empty() {
            errors.push(format!("batch {}: could not parse response", idx + 1));
        }
        all_decisions.extend(decisions);
        completed += 1;

        if show_progress {
            eprint_progress(
                idx + 1,
                batches.len(),
                &batch[batch.len().saturating_sub(1)].0,
                batches.len(),
            );
        }
    }

    if show_progress {
        eprintln!();
    }

    let (merged, reclassified, dropped) =
        apply_classifications(detected, &all_decisions, allow_project_promotions);

    RefineResult {
        merged,
        reclassified,
        dropped,
        errors,
        batches_completed: completed,
        batches_total: batches.len(),
        cancelled,
    }
}

fn eprint_progress(batch_idx: usize, total: usize, current_name: &str, _total_batches: usize) {
    let width = 40;
    let filled = if total > 0 {
        (width * batch_idx) / total
    } else {
        0
    };
    let bar = format!("{:█<1$}{:░<2$}", "", filled, width - filled);
    let name = if current_name.len() > 30 {
        &current_name[..30]
    } else {
        current_name
    };
    eprint!(
        "\r  LLM refine: [{:40}] batch {}/{}  current: {:.<30}",
        bar, batch_idx, total, name
    );
}

// ---------------------------------------------------------------------------
// Corpus text collection
// ---------------------------------------------------------------------------

const PROSE_EXTENSIONS: &[&str] = &[".md", ".txt", ".rst", ".markdown"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_label_from_str() {
        assert_eq!(EntityLabel::from_str("PERSON"), EntityLabel::Person);
        assert_eq!(EntityLabel::from_str("project"), EntityLabel::Project);
        assert_eq!(EntityLabel::from_str("INVALID"), EntityLabel::Ambiguous);
    }

    #[test]
    fn test_parse_empty() {
        let result = parse_response("", &["Alice".to_string()]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_apply_classifications_drops_common_word() {
        let detected = DetectedEntities {
            people: vec![EntityEntry {
                name: "The".to_string(),
                entry_type: "person".to_string(),
                signals: vec![],
            }],
            projects: vec![],
            topics: vec![],
            uncertain: vec![],
        };
        let decisions = HashMap::from([(
            "The".to_string(),
            (EntityLabel::CommonWord, "English word".to_string()),
        )]);

        let (merged, reclassified, dropped) = apply_classifications(&detected, &decisions, true);
        assert_eq!(dropped, 1);
        assert!(merged.people.is_empty());
    }

    #[test]
    fn test_is_authoritative_person() {
        let entry = EntityEntry {
            name: "John".to_string(),
            entry_type: "person".to_string(),
            signals: vec!["commit".to_string(), "repo".to_string()],
        };
        assert!(is_authoritative_person(&entry));
    }

    #[test]
    fn test_collect_contexts() {
        let corpus = vec![
            "Alice worked on the project".to_string(),
            "Alice is a developer".to_string(),
            "Bob collaborated with Alice".to_string(),
        ];
        let ctx = collect_contexts(&corpus, "Alice", 2);
        assert!(ctx.len() <= 2);
    }
}
