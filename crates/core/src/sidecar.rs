//! LLM-based extraction sidecar (issue #32).
//!
//! Calls an LLM to extract structured memories from transcripts,
//! check relevance of stored memories against current context, and
//! detect contradictions between new and existing information.
//!
//! Mirrors jcode's `Sidecar` (`crates/jcode-base/src/sidecar.rs`)
//! but uses mempalace's `LlmProvider` trait instead of direct
//! reqwest calls, so it benefits from the `FallbackChain` and
//! `CircuitBreaker` infrastructure.
//!
//! Feature-gated behind `llm-sidecar` — when disabled, the
//! `extract_from_transcript` pipeline falls back to heuristic-only
//! extraction via `general_extractor`.

use anyhow::{Context, Result};
use tracing::warn;

use crate::llm::LlmProvider;
use crate::palace::{DrawerKind, SearchHit};

/// Default max tokens for sidecar LLM calls.
const DEFAULT_MAX_TOKENS: u32 = 1024;

// ---------------------------------------------------------------------------
// ExtractedMemory
// ---------------------------------------------------------------------------

/// A single memory extracted from a transcript by the LLM sidecar.
///
/// Mirrors jcode's `ExtractedMemory` but maps the category string
/// onto mempalace's [`DrawerKind`] vocabulary so downstream code
/// can file the result as a typed drawer without string matching.
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    /// The drawer kind this memory maps to.
    pub category: DrawerKind,
    /// Concise statement of the memory (1-2 sentences, <200 chars preferred).
    pub content: String,
    /// Trust level: "high" (user stated), "medium" (observed), "low" (inferred).
    pub trust: String,
}

// ---------------------------------------------------------------------------
// VerifiedHit (issue #33)
// ---------------------------------------------------------------------------

/// A search hit annotated with sidecar relevance verification.
///
/// Wraps a [`SearchHit`] with the relevance verdict and an optional
/// human-readable explanation from the LLM. Downstream consumers
/// (e.g. jcode's adapter) can surface the `reason` to the user to
/// explain why a result was kept or dropped.
#[derive(Debug, Clone)]
pub struct VerifiedHit {
    /// The original search hit.
    pub hit: SearchHit,
    /// Whether the sidecar judged this hit relevant to the query.
    pub relevant: bool,
    /// Optional explanation from the LLM (the `REASON:` line).
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Sidecar
// ---------------------------------------------------------------------------

/// LLM-based extraction sidecar.
///
/// Uses the same prompt vocabulary as jcode's `Sidecar` (CATEGORY|CONTENT|TRUST
/// pipe-delimited output, RELEVANT: yes/no for relevance checks) but
/// delegates HTTP calls to mempalace's [`LlmProvider`] trait, which
/// supports fallback chains and circuit breakers.
pub struct Sidecar {
    provider: Box<dyn LlmProvider>,
    max_tokens: u32,
}

impl Sidecar {
    /// Create a new sidecar wrapping the given LLM provider.
    ///
    /// Uses [`DEFAULT_MAX_TOKENS`] (1024). Call [`with_max_tokens`] to
    /// override.
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self {
            provider,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// Override the max-tokens budget for LLM calls.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// The name of the underlying LLM provider (for logging / metadata).
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    // -----------------------------------------------------------------------
    // Relevance checking
    // -----------------------------------------------------------------------

    /// Check if a stored memory is relevant to the current context.
    ///
    /// Returns `(is_relevant, explanation)`. Uses the same prompt format
    /// as jcode's sidecar (`RELEVANT: yes/no`, `REASON: ...`).
    pub async fn check_relevance(
        &self,
        memory_content: &str,
        current_context: &str,
    ) -> Result<(bool, String)> {
        let system = r#"You are a memory relevance checker. Your job is to determine if a stored memory is relevant to the current context.

Respond in this exact format:
RELEVANT: yes/no
REASON: <brief explanation>

Be conservative - only say "yes" if the memory would actually be useful for the current task."#;

        let prompt = format!(
            "## Stored Memory\n{}\n\n## Current Context\n{}\n\nIs this memory relevant to the current context?",
            memory_content, current_context
        );

        let response = self.complete(system, &prompt).await?;

        // Parse response
        let mut is_relevant = false;
        for line in response.lines() {
            let line = line.trim();
            if line.len() >= 9 && line[..9].eq_ignore_ascii_case("relevant:") {
                let value = line[9..].trim();
                is_relevant = value.eq_ignore_ascii_case("yes") || value.starts_with("yes");
                break;
            }
        }
        let reason = response
            .lines()
            .find(|line| line.to_lowercase().starts_with("reason:"))
            .map(|line| line.trim_start_matches(|c: char| !c.is_alphabetic()).trim())
            .unwrap_or(&response)
            .to_string();

        Ok((is_relevant, reason))
    }

    // -----------------------------------------------------------------------
    // Batch relevance verification (issue #33)
    // -----------------------------------------------------------------------

    /// Verify a batch of search hits for relevance to `query`.
    ///
    /// Processes hits sequentially in chunks of `batch_size` (default 5).
    /// Returns a [`VerifiedHit`] for every input hit — callers filter on
    /// `relevant == true` to get the final result set.
    ///
    /// Sequential processing is used because the sidecar's LLM provider
    /// is behind `&self` (not `Arc`), which prevents spawning concurrent
    /// tasks. The batch_size parameter is retained for future use with
    /// an `Arc`-wrapped provider.
    pub async fn verify_hits(
        &self,
        hits: &[SearchHit],
        query: &str,
        batch_size: usize,
    ) -> Result<Vec<VerifiedHit>> {
        let batch_size = batch_size.max(1);
        let mut verified = Vec::with_capacity(hits.len());

        for chunk in hits.chunks(batch_size) {
            for hit in chunk {
                let (relevant, reason) = self.check_relevance(&hit.text, query).await?;
                verified.push(VerifiedHit {
                    hit: hit.clone(),
                    relevant,
                    reason: Some(reason),
                });
            }
        }

        Ok(verified)
    }

    // -----------------------------------------------------------------------
    // Memory extraction
    // -----------------------------------------------------------------------

    /// Extract memories from a transcript.
    ///
    /// Returns a list of [`ExtractedMemory`] with category, content, and
    /// trust level parsed from the LLM's pipe-delimited output
    /// (`CATEGORY|CONTENT|TRUST`, one per line).
    pub async fn extract_memories(
        &self,
        transcript: &str,
        existing: &[String],
    ) -> Result<Vec<ExtractedMemory>> {
        let mut system = String::from(
            r#"You are a memory extraction assistant. Extract important NEW learnings from the conversation that should be remembered for future sessions.

Categories (use EXACTLY one of these):
- fact: Technical facts about the codebase, architecture, patterns, dependencies, tools, environment
- preference: User preferences, workflow habits, UX expectations, coding style, conventions, how they want the assistant to behave
- correction: Mistakes that were corrected, bugs found and fixed, wrong assumptions, things the user corrected
- entity: Named entities worth tracking - people, projects, services, repos, teams

Categorization rules:
- If it describes what the USER WANTS or HOW THEY LIKE THINGS, it is "preference", not "fact"
- If it describes a BUG FIX or MISTAKE, it is "correction", not "fact"
- "fact" is for objective technical information about code/systems, not user behavior

IMPORTANT - Do NOT extract:
- Transient debugging details, compile errors, or intermediate build steps
- Specific commit hashes, git operations, or "changes were committed/pushed" details
- Line-by-line code changes like "X was updated to Y in file Z" - these belong in git history, not memory
- Self-evident project context (e.g., the project name, repo URL, language) that is already in the system prompt
- Redundant variations of information already known (check the "Already known" list carefully)

Quality bar: Only extract information that would ACTUALLY BE USEFUL if recalled in a future session on a different topic. Ask: "Would a developer benefit from knowing this weeks from now?"

For each memory, output in this format (one per line):
CATEGORY|CONTENT|TRUST

Where:
- CATEGORY is one of: fact, preference, correction, entity
- CONTENT is a concise statement (1-2 sentences max, under 200 characters preferred)
- TRUST is one of: high (user stated), medium (observed), low (inferred)

Output ONLY the formatted lines, no other text. If no NEW memories worth extracting, output nothing."#,
        );

        if !existing.is_empty() {
            system.push_str("\n\nAlready known (do NOT re-extract these or close paraphrases):\n");
            for mem in existing.iter().take(80) {
                system.push_str("- ");
                // Truncate long existing memories to keep the prompt compact.
                let truncated = if mem.len() > 150 {
                    let truncated: String = mem.chars().take(147).collect();
                    format!("{truncated}...")
                } else {
                    mem.clone()
                };
                system.push_str(&truncated);
                system.push('\n');
            }
        }

        let response = self
            .complete(&system, transcript)
            .await
            .context("sidecar: extract_memories LLM call failed")?;

        let memories = response
            .lines()
            .filter(|line| line.contains('|'))
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(3, '|').collect();
                if parts.len() >= 3 {
                    let category_str = parts[0].trim().to_lowercase();
                    let content = parts[1].trim().to_string();
                    let trust = parts[2].trim().to_lowercase();

                    if content.is_empty() {
                        return None;
                    }

                    let category = from_category_str(&category_str);

                    Some(ExtractedMemory {
                        category,
                        content,
                        trust,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(memories)
    }

    // -----------------------------------------------------------------------
    // Contradiction detection
    // -----------------------------------------------------------------------

    /// Check if new information contradicts existing information.
    ///
    /// Returns `true` if the LLM determines the statements are
    /// contradictory. Uses the same prompt as jcode's sidecar.
    pub async fn check_contradiction(
        &self,
        new_content: &str,
        existing_content: &str,
    ) -> Result<bool> {
        let system = "You are a contradiction detector. Given two statements, determine if the new information directly contradicts the existing information. Reply with exactly YES or NO.";

        let prompt = format!(
            "## Existing Information\n{}\n\n## New Information\n{}\n\nDoes the new information contradict the existing information?",
            existing_content, new_content
        );

        let response = self.complete(system, &prompt).await?;
        let trimmed = response.trim().to_uppercase();
        Ok(trimmed.starts_with("YES"))
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Send a completion request to the underlying LLM provider.
    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let completion = self
            .provider
            .complete(system, user)
            .await
            .map_err(|e| anyhow::anyhow!("sidecar LLM error: {}", e))?;

        if completion.text.is_empty() {
            warn!(
                provider = %self.provider.name(),
                model = %self.provider.model(),
                "sidecar: LLM returned empty response"
            );
        }

        Ok(completion.text)
    }
}

// ---------------------------------------------------------------------------
// Category mapping
// ---------------------------------------------------------------------------

/// Map a jcode-style category string onto mempalace's [`DrawerKind`].
///
/// Mapping (matching `MemoryType::to_drawer_kind` from `general_extractor`):
///   "fact"       -> DrawerKind::Fact
///   "preference" -> DrawerKind::Preference
///   "correction" -> DrawerKind::Correction
///   "entity"     -> DrawerKind::Entity
///   "event"      -> DrawerKind::Event
///   "discovery"  -> DrawerKind::Discovery
///   "advice"     -> DrawerKind::Advice
///   _            -> DrawerKind::Raw  (unknown categories get Raw)
fn from_category_str(s: &str) -> DrawerKind {
    match s {
        "fact" => DrawerKind::Fact,
        "preference" => DrawerKind::Preference,
        "correction" => DrawerKind::Correction,
        "entity" => DrawerKind::Entity,
        "event" => DrawerKind::Event,
        "discovery" => DrawerKind::Discovery,
        "advice" => DrawerKind::Advice,
        _ => DrawerKind::Raw,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::NoopProvider;

    /// Build a Sidecar backed by a NoopProvider (returns empty completions).
    fn noop_sidecar() -> Sidecar {
        Sidecar::new(Box::new(NoopProvider::new()))
    }

    #[test]
    fn test_extracted_memory_fields() {
        let mem = ExtractedMemory {
            category: DrawerKind::Fact,
            content: "Rust uses ownership for memory safety".into(),
            trust: "high".into(),
        };
        assert_eq!(mem.category, DrawerKind::Fact);
        assert_eq!(mem.content, "Rust uses ownership for memory safety");
        assert_eq!(mem.trust, "high");
    }

    #[test]
    fn test_from_category_str_known() {
        assert_eq!(from_category_str("fact"), DrawerKind::Fact);
        assert_eq!(from_category_str("preference"), DrawerKind::Preference);
        assert_eq!(from_category_str("correction"), DrawerKind::Correction);
        assert_eq!(from_category_str("entity"), DrawerKind::Entity);
        assert_eq!(from_category_str("event"), DrawerKind::Event);
        assert_eq!(from_category_str("discovery"), DrawerKind::Discovery);
        assert_eq!(from_category_str("advice"), DrawerKind::Advice);
    }

    #[test]
    fn test_from_category_str_unknown_defaults_to_raw() {
        assert_eq!(from_category_str("unknown"), DrawerKind::Raw);
        assert_eq!(from_category_str(""), DrawerKind::Raw);
        assert_eq!(from_category_str("SOMETHING_ELSE"), DrawerKind::Raw);
    }

    #[test]
    fn test_sidecar_provider_name() {
        let sc = noop_sidecar();
        assert_eq!(sc.provider_name(), "noop");
    }

    #[test]
    fn test_sidecar_max_tokens_override() {
        let sc = Sidecar::new(Box::new(NoopProvider::new())).with_max_tokens(2048);
        assert_eq!(sc.max_tokens, 2048);
    }

    #[tokio::test]
    async fn test_check_relevance_noop_returns_false() {
        let sc = noop_sidecar();
        // NoopProvider returns empty text, so parsing finds no "RELEVANT: yes".
        let (relevant, _reason) = sc
            .check_relevance("some memory", "some context")
            .await
            .expect("check_relevance should not error with noop");
        assert!(!relevant, "noop provider should return not-relevant");
    }

    #[tokio::test]
    async fn test_extract_memories_noop_returns_empty() {
        let sc = noop_sidecar();
        let memories = sc
            .extract_memories("some transcript", &[])
            .await
            .expect("extract_memories should not error with noop");
        assert!(
            memories.is_empty(),
            "noop provider should return no memories"
        );
    }

    #[tokio::test]
    async fn test_extract_memories_noop_with_existing() {
        let sc = noop_sidecar();
        let existing = vec!["already known fact".to_string()];
        let memories = sc
            .extract_memories("some transcript", &existing)
            .await
            .expect("extract_memories should not error with noop");
        assert!(memories.is_empty());
    }

    #[tokio::test]
    async fn test_check_contradiction_noop_returns_false() {
        let sc = noop_sidecar();
        // NoopProvider returns empty text, which does not start with "YES".
        let contradicts = sc
            .check_contradiction("new info", "old info")
            .await
            .expect("check_contradiction should not error with noop");
        assert!(!contradicts, "noop provider should return no contradiction");
    }

    // -----------------------------------------------------------------------
    // VerifiedHit + verify_hits (issue #33)
    // -----------------------------------------------------------------------

    #[test]
    fn test_verified_hit_fields() {
        let hit = SearchHit {
            text: "hello".into(),
            wing: None,
            room: None,
            source_file: String::new(),
            similarity: 0.9,
            bm25_score: None,
            combined_score: None,
        };
        let vh = VerifiedHit {
            hit: hit.clone(),
            relevant: true,
            reason: Some("matches query".into()),
        };
        assert!(vh.relevant);
        assert_eq!(vh.hit.text, "hello");
        assert_eq!(vh.reason.as_deref(), Some("matches query"));
    }

    #[tokio::test]
    async fn test_verify_hits_noop_all_irrelevant() {
        let sc = noop_sidecar();
        let hits = vec![
            SearchHit {
                text: "alpha".into(),
                wing: None,
                room: None,
                source_file: String::new(),
                similarity: 0.9,
                bm25_score: None,
                combined_score: None,
            },
            SearchHit {
                text: "beta".into(),
                wing: None,
                room: None,
                source_file: String::new(),
                similarity: 0.8,
                bm25_score: None,
                combined_score: None,
            },
        ];
        let verified = sc
            .verify_hits(&hits, "some query", 5)
            .await
            .expect("verify_hits should not error with noop");
        assert_eq!(verified.len(), 2);
        // NoopProvider returns empty text -> no "RELEVANT: yes" -> all irrelevant.
        assert!(!verified[0].relevant);
        assert!(!verified[1].relevant);
    }

    #[tokio::test]
    async fn test_verify_hits_empty_input() {
        let sc = noop_sidecar();
        let empty: Vec<SearchHit> = vec![];
        let verified = sc
            .verify_hits(&empty, "query", 5)
            .await
            .expect("verify_hits with empty input");
        assert!(verified.is_empty());
    }
}
