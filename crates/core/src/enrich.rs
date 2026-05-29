//! Enrichment — port of upstream `enrich.ts`.
//!
//! Enriches content by adding file context, search results, and bug memories.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Enriched context for a file or topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichmentResult {
    pub file_contexts: Vec<FileContext>,
    pub related_memories: Vec<MemorySnippet>,
    pub bug_memories: Vec<MemorySnippet>,
    pub patterns: Vec<String>,
}

/// Context about a specific file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContext {
    pub path: String,
    pub content_summary: String,
    pub last_modified: String,
    pub related_files: Vec<String>,
}

/// A snippet from memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySnippet {
    pub id: String,
    pub title: String,
    pub content: String,
    pub relevance: f64,
}

/// Enrich content with file context.
pub fn enrich_with_file_context(
    file_path: &str,
    memories: &[crate::types::Memory],
) -> Vec<FileContext> {
    let related: Vec<_> = memories.iter()
        .filter(|m| m.files.iter().any(|f| f.contains(file_path) || file_path.contains(f)))
        .collect();

    if related.is_empty() {
        return Vec::new();
    }

    let mut contexts = Vec::new();
    let content_summary = related.iter()
        .map(|m| m.title.as_str())
        .collect::<Vec<_>>()
        .join("; ");

    let related_files: Vec<_> = related.iter()
        .flat_map(|m| m.files.iter().cloned())
        .filter(|f| f != file_path)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    contexts.push(FileContext {
        path: file_path.to_string(),
        content_summary,
        last_modified: chrono::Utc::now().to_rfc3339(),
        related_files,
    });
    contexts
}

/// Search for related memories.
pub fn search_related_memories(
    query: &str,
    memories: &[crate::types::Memory],
    limit: usize,
) -> Vec<MemorySnippet> {
    let query_lower = query.to_lowercase();
    let terms: Vec<_> = query_lower.split_whitespace()
        .filter(|t| t.len() > 2)
        .collect();

    let mut scored: Vec<_> = memories.iter()
        .map(|m| {
            let text = format!("{} {} {}", m.title, m.content, m.concepts.join(" ")).to_lowercase();
            let match_count = terms.iter().filter(|t| text.contains(**t)).count();
            let relevance = if terms.is_empty() { 0.0 } else { match_count as f64 / terms.len() as f64 };
            MemorySnippet {
                id: m.id.clone(),
                title: m.title.clone(),
                content: m.content.clone(),
                relevance,
            }
        })
        .filter(|s| s.relevance > 0.0)
        .collect();

    scored.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

/// Find bug-related memories.
pub fn find_bug_memories(
    file_path: &str,
    memories: &[crate::types::Memory],
    limit: usize,
) -> Vec<MemorySnippet> {
    let bug_terms = ["bug", "error", "crash", "fix", "issue", "broken", "fail"];

    memories.iter()
        .filter(|m| {
            m.files.iter().any(|f| f.contains(file_path) || file_path.contains(f))
                && m.concepts.iter().any(|c| {
                    let c_lower = c.to_lowercase();
                    bug_terms.iter().any(|bt| c_lower.contains(bt))
                })
        })
        .map(|m| MemorySnippet {
            id: m.id.clone(),
            title: m.title.clone(),
            content: m.content.clone(),
            relevance: m.strength,
        })
        .take(limit)
        .collect()
}

/// Full enrichment pipeline.
pub fn enrich(
    file_path: &str,
    query: &str,
    memories: &[crate::types::Memory],
    search_limit: usize,
) -> EnrichmentResult {
    let file_contexts = enrich_with_file_context(file_path, memories);
    let related_memories = search_related_memories(query, memories, search_limit);
    let bug_memories = find_bug_memories(file_path, memories, search_limit);

    let patterns: Vec<_> = memories.iter()
        .flat_map(|m| m.concepts.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .take(10)
        .collect();

    EnrichmentResult {
        file_contexts,
        related_memories,
        bug_memories,
        patterns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Memory, MemoryType};

    fn test_memory(id: &str, title: &str, content: &str, files: Vec<&str>, concepts: Vec<&str>) -> Memory {
        Memory {
            id: id.into(), created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
            memory_type: MemoryType::Semantic, title: title.into(), content: content.into(),
            concepts: concepts.into_iter().map(String::from).collect(),
            files: files.into_iter().map(String::from).collect(),
            session_ids: vec![], strength: 0.8, version: 1, parent_id: None,
            supersedes: vec![], related_ids: vec![], source_observation_ids: vec![],
            is_latest: true, forget_after: None, image_ref: None, agent_id: None,
            project: "test".into(),
        }
    }

    #[test]
    fn test_enrich_with_file_context() {
        let memories = vec![
            test_memory("m-1", "Auth setup", "Uses JWT", vec!["src/auth.rs"], vec!["auth"]),
            test_memory("m-2", "Middleware", "Token validation", vec!["src/auth.rs", "src/middleware.rs"], vec!["middleware"]),
        ];
        let contexts = enrich_with_file_context("src/auth.rs", &memories);
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].path, "src/auth.rs");
        assert_eq!(contexts[0].related_files.len(), 1);
    }

    #[test]
    fn test_search_related_memories() {
        let memories = vec![
            test_memory("m-1", "Auth setup", "Uses JWT for authentication", vec![], vec!["auth", "jwt"]),
            test_memory("m-2", "Database", "Uses PostgreSQL", vec![], vec!["database"]),
        ];
        let results = search_related_memories("JWT authentication", &memories, 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "m-1");
    }

    #[test]
    fn test_find_bug_memories() {
        let memories = vec![
            test_memory("m-1", "Auth bug fix", "Fixed token expiry bug", vec!["src/auth.rs"], vec!["bug", "auth"]),
            test_memory("m-2", "Database setup", "Uses PostgreSQL", vec!["src/db.rs"], vec!["database"]),
        ];
        let bugs = find_bug_memories("src/auth.rs", &memories, 5);
        assert_eq!(bugs.len(), 1);
        assert_eq!(bugs[0].id, "m-1");
    }

    #[test]
    fn test_enrich_full_pipeline() {
        let memories = vec![
            test_memory("m-1", "Auth setup", "Uses JWT", vec!["src/auth.rs"], vec!["auth", "bug"]),
        ];
        let result = enrich("src/auth.rs", "JWT auth", &memories, 5);
        assert!(!result.file_contexts.is_empty());
        assert!(!result.related_memories.is_empty());
        assert!(!result.patterns.is_empty());
    }

    #[test]
    fn test_search_no_match() {
        let memories = vec![
            test_memory("m-1", "Auth", "JWT", vec![], vec!["auth"]),
        ];
        let results = search_related_memories("completely unrelated xyz", &memories, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_enrich_empty_memories() {
        let result = enrich("src/main.rs", "test", &[], 5);
        assert!(result.file_contexts.is_empty());
        assert!(result.related_memories.is_empty());
        assert!(result.bug_memories.is_empty());
    }
}
