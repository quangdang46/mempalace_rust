//! fact_checker.rs — Verify text against known facts in the palace.
//!
//! Purely offline. No network calls.

use crate::config::Config;
use crate::knowledge_graph::KnowledgeGraph;
use regex::Regex;
use std::collections::HashSet;
use std::path::Path;

pub use self::FactIssueType::*;

/// Fact issues detected in text.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FactIssue {
    #[serde(rename = "type")]
    pub issue_type: FactIssueType,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim: Option<Claim>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kg_fact: Option<KgFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FactIssueType {
    SimilarName,
    RelationshipMismatch,
    StaleFact,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Claim {
    pub predicate: String,
    pub object: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct KgFact {
    pub predicate: String,
    pub object: String,
}

/// Check text for fact contradictions against the entity registry and KG.
pub fn check_text(text: &str) -> Vec<FactIssue> {
    if text.is_empty() {
        return Vec::new();
    }

    let config = match Config::load() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut issues = Vec::new();

    // Load entity registry names
    let entity_names = load_known_entity_names();
    issues.extend(check_entity_confusion(text, &entity_names));

    // Load KG and check contradictions
    issues.extend(check_kg_contradictions(text, &config.palace_path));

    issues
}

// ── entity-name confusion ────────────────────────────────────────────────────

fn load_known_entity_names() -> HashSet<String> {
    let mut names = HashSet::new();
    if let Ok(registry_path) = Config::registry_file_path() {
        if let Ok(registry) = crate::entity_registry::EntityRegistry::load(&registry_path) {
            for name in registry.people().keys() {
                names.insert(name.clone());
            }
        }
    }
    names
}

fn check_entity_confusion(text: &str, all_names: &HashSet<String>) -> Vec<FactIssue> {
    if all_names.is_empty() {
        return Vec::new();
    }

    // Which names from the registry actually appear in the text?
    let mentioned: Vec<&str> = all_names
        .iter()
        .filter(|name| {
            let pattern = format!("\\b({})\\b", regex::escape(name));
            Regex::new(&pattern)
                .map(|re| re.is_match(text))
                .unwrap_or(false)
        })
        .map(|s| s.as_str())
        .collect();

    if mentioned.is_empty() {
        return Vec::new();
    }

    let mut issues = Vec::new();
    let mut seen_pairs: HashSet<(String, String)> = HashSet::new();

    for name_a in &mentioned {
        let a_lower = name_a.to_lowercase();
        for name_b in all_names {
            if name_b == *name_a {
                continue;
            }
            // Dedupe by unordered pair
            let pair_key = (
                std::cmp::min(a_lower.as_str(), name_b.as_str()).to_string(),
                std::cmp::max(a_lower.as_str(), name_b.as_str()).to_string(),
            );
            if seen_pairs.contains(&pair_key) {
                continue;
            }
            // If name_b is also mentioned, skip (both names in text = two people)
            if mentioned.contains(&name_b.as_str()) {
                seen_pairs.insert(pair_key);
                continue;
            }

            let distance = edit_distance(&a_lower, &name_b.to_lowercase());
            if distance > 0 && distance <= 2 {
                issues.push(FactIssue {
                    issue_type: FactIssueType::SimilarName,
                    detail: format!(
                        "'{}' mentioned — did you mean '{}'? (edit distance {})",
                        name_a, name_b, distance
                    ),
                    names: Some(vec![name_a.to_string(), name_b.to_string()]),
                    distance: Some(distance),
                    entity: None,
                    claim: None,
                    kg_fact: None,
                    valid_to: None,
                });
                seen_pairs.insert(pair_key);
            }
        }
    }

    issues
}

// ── KG contradictions ─────────────────────────────────────────────────────────

// "Bob is Alice's brother" → subject=Bob, possessor=Alice, role=brother
// "Alice's brother is Bob" → possessor=Alice, role=brother, subject=Bob

fn check_kg_contradictions(text: &str, palace_path: &Path) -> Vec<FactIssue> {
    let claims = extract_claims(text);
    if claims.is_empty() {
        return Vec::new();
    }

    let kg_db_path = palace_path.join("knowledge_graph.sqlite3");
    let Ok(kg) = KnowledgeGraph::open(&kg_db_path) else {
        return Vec::new();
    };

    let mut issues = Vec::new();
    let now = chrono_now_date();

    for claim in claims {
        let Ok(facts) = kg.query_entity(&claim.subject, None, "outgoing") else {
            continue;
        };
        if facts.is_empty() {
            continue;
        }

        let current_facts: Vec<_> = facts.iter().filter(|f| f.current).collect();

        // Mismatch: same (subject, object) pair but different predicate
        for fact in &current_facts {
            let kg_obj = &fact.object;
            if !objects_match(kg_obj, &claim.object) {
                continue;
            }
            let kg_pred = fact.predicate.to_lowercase();
            if !kg_pred.is_empty() && kg_pred != claim.predicate {
                issues.push(FactIssue {
                    issue_type: FactIssueType::RelationshipMismatch,
                    detail: format!(
                        "Text says '{}' but KG records {} {} {}",
                        claim.span, claim.subject, kg_pred, kg_obj
                    ),
                    names: None,
                    distance: None,
                    entity: Some(claim.subject.clone()),
                    claim: Some(Claim {
                        predicate: claim.predicate.clone(),
                        object: claim.object.clone(),
                    }),
                    kg_fact: Some(KgFact {
                        predicate: kg_pred,
                        object: kg_obj.clone(),
                    }),
                    valid_to: None,
                });
            }
        }

        // Stale fact: exact match but valid_to is in the past
        for fact in &facts {
            if fact.current {
                continue;
            }
            let kg_pred = fact.predicate.to_lowercase();
            if kg_pred != claim.predicate {
                continue;
            }
            if !objects_match(&fact.object, &claim.object) {
                continue;
            }
            if let Some(valid_to) = &fact.valid_to {
                if valid_to.as_str() < now.as_str() {
                    issues.push(FactIssue {
                        issue_type: FactIssueType::StaleFact,
                        detail: format!(
                            "Text says '{}' but KG marks this fact closed on {}",
                            claim.span, valid_to
                        ),
                        names: None,
                        distance: None,
                        entity: Some(claim.subject.clone()),
                        claim: None,
                        kg_fact: None,
                        valid_to: Some(valid_to.clone()),
                    });
                }
            }
        }
    }

    issues
}

struct ParsedClaim {
    subject: String,
    predicate: String,
    object: String,
    span: String,
}

fn get_claim_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"\b([A-Z][\w-]+)\s+is\s+([A-Z][\w-]+)'s\s+([a-z]{3,20})\b").unwrap(),
        Regex::new(r"\b([A-Z][\w-]+)'s\s+([a-z]{3,20})\s+is\s+([A-Z][\w-]+)\b").unwrap(),
    ]
}

fn extract_claims(text: &str) -> Vec<ParsedClaim> {
    let mut claims = Vec::new();
    let patterns = get_claim_patterns();
    for (i, pat) in patterns.iter().enumerate() {
        for cap in pat.captures_iter(text) {
            let (_, groups) = cap.extract::<3>();
            let (subject, possessor, role) = if i == 0 {
                (groups[0], groups[1], groups[2])
            } else {
                (groups[2], groups[0], groups[1])
            };
            claims.push(ParsedClaim {
                subject: subject.to_string(),
                predicate: role.to_lowercase(),
                object: possessor.to_string(),
                span: groups[0].to_string(),
            });
        }
    }
    claims
}

fn objects_match(kg_obj: &str, claim_obj: &str) -> bool {
    if kg_obj.is_empty() || claim_obj.is_empty() {
        return false;
    }
    kg_obj.trim().eq_ignore_ascii_case(claim_obj.trim())
}

#[allow(clippy::manual_is_multiple_of)]
fn chrono_now_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let secs_per_day: u64 = 86400;
    let days = now.as_secs() / secs_per_day;
    let mut y: u64 = 1970;
    let mut remaining = days;
    while remaining >= 365 {
        let is_leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        let days_in_y = if is_leap { 366 } else { 365 };
        if remaining >= days_in_y {
            remaining -= days_in_y;
            y += 1;
        } else {
            break;
        }
    }
    let days_per_month: [u64; 12] = if (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1usize;
    for (i, &dpm) in days_per_month.iter().enumerate() {
        if remaining < dpm {
            month = i + 1;
            break;
        }
        remaining -= dpm;
    }
    let day = remaining + 1;
    format!("{:04}-{:02}-{:02}", y, month, day)
}

// ── Levenshtein distance ─────────────────────────────────────────────────────

fn edit_distance(s1: &str, s2: &str) -> usize {
    if s1.len() < s2.len() {
        return edit_distance(s2, s1);
    }
    if s2.is_empty() {
        return s1.len();
    }
    let mut prev: Vec<usize> = (0..=s2.len()).collect();
    for (i, c1) in s1.chars().enumerate() {
        let mut curr = vec![i + 1];
        for (j, c2) in s2.chars().enumerate() {
            let cost = if c1 == c2 { 0 } else { 1 };
            curr.push(std::cmp::min(
                prev[j + 1] + 1,
                std::cmp::min(curr[j] + 1, prev[j] + cost),
            ));
        }
        prev = curr;
    }
    prev[s2.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edit_distance() {
        assert_eq!(edit_distance("hello", "hello"), 0);
        assert_eq!(edit_distance("hello", "hallo"), 1);
        assert_eq!(edit_distance("hello", "helo"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn test_extract_claims() {
        let claims = extract_claims("Bob is Alice's brother");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].subject, "Bob");
        assert_eq!(claims[0].predicate, "brother");
        assert_eq!(claims[0].object, "Alice");
    }

    #[test]
    fn test_objects_match() {
        assert!(objects_match("Alice", "alice"));
        assert!(objects_match("Alice", "Alice"));
        assert!(!objects_match("", "Alice"));
    }

    #[test]
    fn test_check_text_empty() {
        let result = check_text("");
        assert!(result.is_empty());
    }
}
