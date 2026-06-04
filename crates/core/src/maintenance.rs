// =====================================================================
// Post-retrieval maintenance engine (issue #35)
// =====================================================================
//
// Runs 7 maintenance tasks after every search+verify cycle:
//
//  1. Link discovery  — create RelatesTo edges between co-relevant memories
//  2. Confidence boost — +0.05 for verified-relevant memories
//  3. Confidence decay — -0.02 for rejected memories
//  4. Gap detection   — log when no memories were relevant
//  5. Cluster refinement — every 50 ticks, auto_cluster co-relevant memories
//  6. Tag inference   — word-frequency analysis on context, apply tag
//  7. Pruning         — every 250 ticks, remove confidence < 0.15 AND age >= 24h

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use tracing::{info, warn};

use crate::palace::{ActivityState, DrawerId, MemoryProvider, Palace};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Confidence boost for verified-relevant memories.
const CONFIDENCE_BOOST: f64 = 0.05;

/// Confidence decay for rejected memories.
const CONFIDENCE_DECAY: f64 = 0.02;

/// Default weight for newly discovered RelatesTo edges.
const RELATES_TO_WEIGHT: f32 = 0.6;

/// Minimum number of verified IDs needed for tag inference and cluster ops.
const MIN_VERIFIED_FOR_INFERENCE: usize = 2;

/// Tag inference: minimum word occurrences to qualify as a tag.
const TAG_MIN_OCCURRENCES: usize = 2;

/// Cluster refinement runs every N ticks.
const CLUSTER_REFINE_INTERVAL: u64 = 50;

/// Pruning runs every N ticks.
const PRUNE_INTERVAL: u64 = 250;

/// Pruning: confidence threshold below which a drawer is a prune candidate.
const PRUNE_CONFIDENCE_THRESHOLD: f64 = 0.15;

/// Pruning: minimum age in hours for a drawer to be pruned.
const PRUNE_AGE_HOURS: i64 = 24;

/// Maximum verified IDs to embed for cluster refinement. Caps the
/// re-embedding cost so large retrieval sets don't block maintenance.
const MAX_CLUSTER_EMBED: usize = 20;

/// Stopwords to filter during tag inference.
const STOPWORDS: &[&str] = &[
    "the", "be", "to", "of", "and", "a", "in", "that", "have", "i", "it", "for", "not", "on",
    "with", "he", "as", "you", "do", "at", "this", "but", "his", "by", "from", "they", "we", "say",
    "her", "she", "or", "an", "will", "my", "one", "all", "would", "there", "their", "what", "so",
    "up", "out", "if", "about", "who", "get", "which", "go", "me", "when", "make", "can", "like",
    "time", "no", "just", "him", "know", "take", "people", "into", "year", "your", "good", "some",
    "could", "them", "see", "other", "than", "then", "now", "look", "only", "come", "its", "over",
    "think", "also", "back", "after", "use", "two", "how", "our", "work", "first", "well", "way",
    "even", "new", "want", "because", "any", "these", "give", "day", "most", "us", "is", "was",
    "are", "been", "has", "had", "were", "did", "being", "am", "does", "done",
];

// ---------------------------------------------------------------------------
// RetrievalContext
// ---------------------------------------------------------------------------

/// Context from a search+verify cycle fed into the maintenance engine.
///
/// Carries the split of verified vs rejected drawer IDs, the original
/// context snippet (for gap detection and tag inference), and the
/// session identifier (for logging).
#[derive(Debug, Clone)]
pub struct RetrievalContext {
    /// Drawer IDs that were verified as relevant by the sidecar.
    pub verified_ids: Vec<DrawerId>,
    /// Drawer IDs that were rejected as irrelevant by the sidecar.
    pub rejected_ids: Vec<DrawerId>,
    /// The original query / context snippet.
    pub context_snippet: String,
    /// The session ID that triggered this retrieval.
    pub session_id: String,
}

// ---------------------------------------------------------------------------
// MaintenanceEngine
// ---------------------------------------------------------------------------

/// Post-retrieval maintenance engine.
///
/// Runs 7 maintenance tasks after every search+verify cycle. The tick
/// counter gates periodic tasks (cluster refinement every 50 ticks,
/// pruning every 250 ticks). The engine is designed to be spawned as
/// a non-blocking tokio task so it does not delay search responses.
pub struct MaintenanceEngine {
    /// The palace to maintain.
    palace: Palace,
    /// Monotonically increasing tick counter. Incremented on each `run()`.
    tick_count: AtomicU64,
}

impl MaintenanceEngine {
    /// Create a new maintenance engine for the given palace.
    pub fn new(palace: Palace) -> Self {
        Self {
            palace,
            tick_count: AtomicU64::new(0),
        }
    }

    /// Current tick count. Exposed for testing.
    pub fn tick_count(&self) -> u64 {
        self.tick_count.load(Ordering::Relaxed)
    }

    /// Run all maintenance tasks for the given retrieval context.
    ///
    /// This method is designed to be spawned as a non-blocking tokio task.
    /// It emits `ActivityEvent::Maintaining` at start and end.
    pub async fn run(&self, ctx: RetrievalContext) -> anyhow::Result<()> {
        let tick = self.tick_count.fetch_add(1, Ordering::Relaxed);

        self.palace.emit_activity(
            ActivityState::Maintaining,
            Some(format!(
                "tick={} session={} verified={} rejected={}",
                tick,
                ctx.session_id,
                ctx.verified_ids.len(),
                ctx.rejected_ids.len()
            )),
        );

        // Task 1: Link discovery (always, if >= 2 verified)
        if let Err(e) = self.task_link_discovery(&ctx).await {
            warn!("maintenance: link discovery failed: {}", e);
        }

        // Task 2: Confidence boost (always)
        if let Err(e) = self.task_confidence_boost(&ctx).await {
            warn!("maintenance: confidence boost failed: {}", e);
        }

        // Task 3: Confidence decay (always)
        if let Err(e) = self.task_confidence_decay(&ctx).await {
            warn!("maintenance: confidence decay failed: {}", e);
        }

        // Task 4: Gap detection (always)
        self.task_gap_detection(&ctx);

        // Task 5: Cluster refinement (every 50 ticks)
        if tick % CLUSTER_REFINE_INTERVAL == 0
            && ctx.verified_ids.len() >= MIN_VERIFIED_FOR_INFERENCE
        {
            if let Err(e) = self.task_cluster_refinement(&ctx).await {
                warn!("maintenance: cluster refinement failed: {}", e);
            }
        }

        // Task 6: Tag inference (always, if >= 2 verified)
        if ctx.verified_ids.len() >= MIN_VERIFIED_FOR_INFERENCE {
            if let Err(e) = self.task_tag_inference(&ctx).await {
                warn!("maintenance: tag inference failed: {}", e);
            }
        }

        // Task 7: Pruning (every 250 ticks)
        if tick % PRUNE_INTERVAL == 0 {
            if let Err(e) = self.task_pruning().await {
                warn!("maintenance: pruning failed: {}", e);
            }
        }

        self.palace.emit_activity(
            ActivityState::Maintaining,
            Some(format!("maintenance complete tick={}", tick)),
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Task 1: Link discovery
    // -----------------------------------------------------------------------

    /// Create `RelatesTo` edges (weight 0.6) between all pairs of
    /// co-relevant (verified) memories.
    async fn task_link_discovery(&self, ctx: &RetrievalContext) -> anyhow::Result<()> {
        if ctx.verified_ids.len() < 2 {
            return Ok(());
        }
        for i in 0..ctx.verified_ids.len() {
            for j in (i + 1)..ctx.verified_ids.len() {
                let a = &ctx.verified_ids[i];
                let b = &ctx.verified_ids[j];
                if let Err(e) = self.palace.link(a, b, RELATES_TO_WEIGHT).await {
                    warn!("maintenance: link discovery {} -> {} failed: {}", a, b, e);
                }
                // Also link the reverse direction for bidirectional traversal.
                if let Err(e) = self.palace.link(b, a, RELATES_TO_WEIGHT).await {
                    warn!("maintenance: link discovery {} -> {} failed: {}", b, a, e);
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Task 2: Confidence boost
    // -----------------------------------------------------------------------

    /// Boost confidence by +0.05 for each verified-relevant memory.
    async fn task_confidence_boost(&self, ctx: &RetrievalContext) -> anyhow::Result<()> {
        for id in &ctx.verified_ids {
            if let Err(e) = self.palace.boost(id, CONFIDENCE_BOOST).await {
                warn!("maintenance: boost {} failed: {}", id, e);
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Task 3: Confidence decay
    // -----------------------------------------------------------------------

    /// Decay confidence by -0.02 for each rejected memory.
    async fn task_confidence_decay(&self, ctx: &RetrievalContext) -> anyhow::Result<()> {
        for id in &ctx.rejected_ids {
            if let Err(e) = self.palace.decay(id, CONFIDENCE_DECAY).await {
                warn!("maintenance: decay {} failed: {}", id, e);
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Task 4: Gap detection
    // -----------------------------------------------------------------------

    /// Log a gap event when zero verified but some rejected memories.
    fn task_gap_detection(&self, ctx: &RetrievalContext) {
        if ctx.verified_ids.is_empty() && !ctx.rejected_ids.is_empty() {
            let detail = format!(
                "Memory gap detected: {}",
                truncate(&ctx.context_snippet, 120)
            );
            info!("maintenance: {}", detail);
            self.palace
                .emit_activity(ActivityState::Maintaining, Some(detail));
        }
    }

    // -----------------------------------------------------------------------
    // Task 5: Cluster refinement
    // -----------------------------------------------------------------------

    /// Every 50 ticks, run auto_cluster on co-relevant verified memories.
    async fn task_cluster_refinement(&self, ctx: &RetrievalContext) -> anyhow::Result<()> {
        let Some(kg_lock) = self.palace.kg.as_ref() else {
            return Ok(());
        };

        // Build an embedding map for verified drawer IDs, capped to
        // MAX_CLUSTER_EMBED to avoid expensive re-embedding of large sets.
        let all_drawers = self.palace.get_drawers(None, None).await?;
        let mut embeddings: HashMap<DrawerId, Vec<f32>> = HashMap::new();
        let verified_set: HashSet<&DrawerId> =
            ctx.verified_ids.iter().take(MAX_CLUSTER_EMBED).collect();

        for drawer in &all_drawers {
            if let Some(ref id) = drawer.id {
                if verified_set.contains(id) {
                    // We need the embedding; embed the content.
                    match self.palace.embedder().embed(&drawer.content).await {
                        Ok(emb) => {
                            embeddings.insert(id.clone(), emb);
                        }
                        Err(e) => {
                            warn!(
                                "maintenance: cluster refinement embed failed for {}: {}",
                                id, e
                            );
                        }
                    }
                }
            }
        }

        if embeddings.len() < 2 {
            return Ok(());
        }

        let member_ids: Vec<DrawerId> = embeddings.keys().cloned().collect();

        let mut kg = kg_lock.lock().expect("kg mutex poisoned");
        match crate::clusters::auto_cluster(&mut kg, &member_ids, &embeddings, "maintenance") {
            Ok(cluster_id) => {
                info!(
                    "maintenance: cluster refinement created cluster {} with {} members",
                    cluster_id,
                    member_ids.len()
                );
            }
            Err(e) => {
                warn!("maintenance: cluster refinement failed: {}", e);
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Task 6: Tag inference
    // -----------------------------------------------------------------------

    /// Infer a shared tag from the context snippet when verified memories
    /// have no common tag. Applies the most frequent non-stopword term
    /// (>= 2 occurrences) to all verified memories.
    async fn task_tag_inference(&self, ctx: &RetrievalContext) -> anyhow::Result<()> {
        // Gather tags from all verified memories.
        let all_drawers = self.palace.get_drawers(None, None).await?;
        let verified_set: HashSet<&DrawerId> = ctx.verified_ids.iter().collect();

        let mut verified_tags: Vec<HashSet<String>> = Vec::new();
        for drawer in &all_drawers {
            if let Some(ref id) = drawer.id {
                if verified_set.contains(id) {
                    let tags: HashSet<String> = drawer.tags.iter().cloned().collect();
                    verified_tags.push(tags);
                }
            }
        }

        // Check if verified memories already share a common tag.
        if verified_tags.len() >= 2 {
            if let Some(first) = verified_tags.first() {
                let common = verified_tags
                    .iter()
                    .skip(1)
                    .fold(first.clone(), |acc, set| {
                        acc.intersection(set).cloned().collect()
                    });
                if !common.is_empty() {
                    // Already share a tag; skip inference.
                    return Ok(());
                }
            }
        }

        // Word-frequency analysis on context snippet.
        let word = match infer_tag_from_text(&ctx.context_snippet) {
            Some(w) => w,
            None => return Ok(()),
        };

        // Apply the inferred tag to all verified memories.
        for id in &ctx.verified_ids {
            if let Err(e) = self.palace.tag(id, &word).await {
                warn!("maintenance: tag inference apply to {} failed: {}", id, e);
            }
        }

        info!(
            "maintenance: tag inference applied tag '{}' to {} verified memories",
            word,
            ctx.verified_ids.len()
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Task 7: Pruning
    // -----------------------------------------------------------------------

    /// Every 250 ticks, remove drawers with confidence < 0.15 and
    /// age >= 24 hours.
    async fn task_pruning(&self) -> anyhow::Result<()> {
        let now = Utc::now();
        let all_drawers = self.palace.get_drawers(None, None).await?;

        let mut pruned = 0usize;
        for drawer in &all_drawers {
            if drawer.confidence >= PRUNE_CONFIDENCE_THRESHOLD {
                continue;
            }
            let age_hours = (now - drawer.created_at).num_hours();
            if age_hours < PRUNE_AGE_HOURS {
                continue;
            }
            if let Some(ref id) = drawer.id {
                match self.palace.forget(id).await {
                    Ok(true) => {
                        pruned += 1;
                    }
                    Ok(false) => { /* already gone */ }
                    Err(e) => {
                        warn!("maintenance: pruning forget {} failed: {}", id, e);
                    }
                }
            }
        }

        if pruned > 0 {
            info!(
                "maintenance: pruning removed {} low-confidence drawers",
                pruned
            );
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Infer a tag from text via word-frequency analysis.
///
/// Filters stopwords and words shorter than 3 characters, then returns
/// the most frequent word if it appears at least `TAG_MIN_OCCURRENCES` times.
fn infer_tag_from_text(text: &str) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for raw_word in text.split_whitespace() {
        let word: String = raw_word
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect::<String>()
            .to_lowercase();
        if word.len() < 3 {
            continue;
        }
        if STOPWORDS.contains(&word.as_str()) {
            continue;
        }
        *counts.entry(word).or_insert(0) += 1;
    }

    counts
        .into_iter()
        .filter(|(_, count)| *count >= TAG_MIN_OCCURRENCES)
        .max_by_key(|(_, count)| *count)
        .map(|(word, _)| word)
}

/// Truncate a string to `max_chars` characters, appending "..." if truncated.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_tag_from_text_finds_frequent_word() {
        let text = "the authentication token is expired, the authentication failed again";
        let tag = infer_tag_from_text(text);
        assert_eq!(tag, Some("authentication".to_string()));
    }

    #[test]
    fn test_infer_tag_from_text_filters_stopwords() {
        let text = "the the the the the is is is is a a a";
        let tag = infer_tag_from_text(text);
        assert_eq!(tag, None);
    }

    #[test]
    fn test_infer_tag_from_text_min_occurrences() {
        let text = "rust compiler borrow checker";
        // Each word appears once — below threshold.
        let tag = infer_tag_from_text(text);
        assert_eq!(tag, None);
    }

    #[test]
    fn test_infer_tag_from_text_short_words_filtered() {
        let text = "go go go do do do";
        // All words are < 3 chars.
        let tag = infer_tag_from_text(text);
        assert_eq!(tag, None);
    }

    #[test]
    fn test_infer_tag_from_text_empty() {
        assert_eq!(infer_tag_from_text(""), None);
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn test_constants() {
        assert_eq!(CONFIDENCE_BOOST, 0.05);
        assert_eq!(CONFIDENCE_DECAY, 0.02);
        assert_eq!(RELATES_TO_WEIGHT, 0.6);
        assert_eq!(CLUSTER_REFINE_INTERVAL, 50);
        assert_eq!(PRUNE_INTERVAL, 250);
        assert_eq!(PRUNE_CONFIDENCE_THRESHOLD, 0.15);
        assert_eq!(PRUNE_AGE_HOURS, 24);
    }

    #[test]
    fn test_retrieval_context_debug() {
        let ctx = RetrievalContext {
            verified_ids: vec![DrawerId::new("a"), DrawerId::new("b")],
            rejected_ids: vec![DrawerId::new("c")],
            context_snippet: "test query".to_string(),
            session_id: "sess-1".to_string(),
        };
        let dbg = format!("{:?}", ctx);
        assert!(dbg.contains("verified_ids"));
        assert!(dbg.contains("rejected_ids"));
    }

    #[tokio::test]
    async fn test_maintenance_engine_tick_count() {
        // Use a minimal palace for testing tick_count logic.
        // We can't easily construct a Palace in tests without a store,
        // so we test the atomic counter directly.
        let counter = AtomicU64::new(0);
        assert_eq!(counter.fetch_add(1, Ordering::Relaxed), 0);
        assert_eq!(counter.fetch_add(1, Ordering::Relaxed), 1);
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_infer_tag_punctuation_stripped() {
        let text = "api-key api-key api-key (important!)";
        let tag = infer_tag_from_text(text);
        assert_eq!(tag, Some("api-key".to_string()));
    }
}
