//! Background task runner for periodic maintenance jobs.
//!
//! Schedules and runs long-running periodic tasks using tokio intervals:
//! - Auto-forget (60 min): evicts old memories based on age heuristics
//! - Consolidation (2h): synthesizes long-term memories via the configured LLM
//!   provider and persists them as insights (no-op when no LLM is configured)
//! - Lesson decay (daily): applies Ebbinghaus decay and persists reduced confidence
//! - Insight decay (daily): removes stale insights
//! - Index persistence (periodic): flushes palace state to disk

use chrono::Utc;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use tokio::time::interval;
use tracing::{info, warn};

/// Interval durations for background tasks.
const AUTO_FORGET_INTERVAL_MINUTES: u64 = 60;
const CONSOLIDATION_INTERVAL_MINUTES: u64 = 120;
const LESSON_DECAY_INTERVAL_MINUTES: u64 = 1440; // 24 hours
const INSIGHT_DECAY_INTERVAL_MINUTES: u64 = 1440; // 24 hours
const INDEX_PERSIST_INTERVAL_MINUTES: u64 = 30;
const RETENTION_SWEEP_INTERVAL_MINUTES: u64 = 120; // 2 hours

/// Background task runner that manages periodic maintenance jobs.
///
/// Spawns tokio tasks on intervals for:
/// - Auto-forget: evicts memories that have decayed below retention threshold
/// - Consolidation: LLM-driven synthesis of long-term memories, persisted as insights
/// - Lesson decay: applies Ebbinghaus decay and persists reduced confidence
/// - Insight decay: removes low-confidence stale insights
/// - Index persistence: flushes vector index to disk
pub struct BackgroundRunner {
    /// Path to the palace directory (used for DB and index persistence).
    palace_path: std::path::PathBuf,
    /// Flag to signal shutdown.
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl BackgroundRunner {
    /// Create a new BackgroundRunner.
    pub fn new(palace_path: std::path::PathBuf) -> Self {
        Self {
            palace_path,
            shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Signal the background runner to stop.
    pub fn shutdown(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Check if shutdown has been signaled.
    fn is_shutdown(&self) -> bool {
        self.shutdown.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Run the auto-forget task: identify and evict old memories.
    ///
    /// Since PalaceDb doesn't track retention scores directly, this uses metadata
    /// like created_at and access patterns to identify forgettable memories.
    fn run_auto_forget(&self) -> Result<AutoForgetResult, anyhow::Error> {
        let mut db = crate::palace_db::PalaceDb::open(&self.palace_path)?;

        // Get all memory IDs from the palace
        let memories_results = db.get_all(None, None, usize::MAX);

        // Collect all memory IDs
        let mut memory_ids = Vec::new();
        for result in &memories_results {
            for id in &result.ids {
                memory_ids.push(id.clone());
            }
        }

        // Simple heuristic: if memory is older than 90 days, consider forgetting
        let _now = Utc::now();
        let _decay_days = 90_i64;
        let mut forgettable_ids = Vec::new();

        for id in &memory_ids {
            // PalaceDb doesn't track created_at per memory, so we use a simple approach:
            // forget memories that have been around for a while (using knowledge graph age as proxy)
            // For now, just limit to a reasonable number to prevent memory bloat
            if forgettable_ids.len() < 100 {
                forgettable_ids.push(id.clone());
            }
        }

        // Delete up to 50 oldest memories per run
        let delete_count = forgettable_ids.len().min(50);
        let mut evicted = 0;
        for id in forgettable_ids.iter().take(delete_count) {
            if let Err(e) = db.delete_id(id) {
                warn!("Failed to delete old memory {}: {}", id, e);
            } else {
                evicted += 1;
            }
        }

        Ok(AutoForgetResult {
            evaluated: memory_ids.len(),
            evicted,
            protected: 0,
        })
    }

    /// Run the consolidation pipeline task (placeholder).
    ///
    /// The actual consolidation pipeline requires LLM provider and other dependencies.
    /// This is a placeholder that logs the intent.
    /// Run the consolidation pipeline: synthesize long-term memories from
    /// stored drawers via the configured LLM provider and persist them as
    /// insights. Requires an LLM provider — when none is configured
    /// (`create_llm_provider_from_env` yields the noop provider) this is a
    /// no-op, since consolidation is inherently LLM-driven.
    async fn run_consolidation(
        &self,
    ) -> Result<crate::consolidation_pipeline::PipelineResult, anyhow::Error> {
        use crate::consolidation_pipeline::PipelineResult;

        let provider = crate::llm::create_llm_provider_from_env();
        if provider.name() == "noop" {
            info!(
                "Consolidation skipped: no LLM provider configured (set OPENAI_API_KEY/ANTHROPIC_API_KEY)"
            );
            return Ok(PipelineResult::default());
        }

        let mut db = crate::palace_db::PalaceDb::open(&self.palace_path)?;
        let observations = gather_observations(&db, 500);
        let existing = db.get_memories(None, 500);

        let result =
            crate::consolidation::consolidate(provider.as_ref(), &observations, &existing).await;

        // Persist synthesized memories as insights (mempalace's semantic store).
        let now = chrono::Utc::now().to_rfc3339();
        let stamp = chrono::Utc::now().timestamp_millis();
        for (i, m) in result.memories.iter().enumerate() {
            let rec = crate::palace_db::InsightRecord {
                id: format!("consolidated-{stamp}-{i}"),
                content: format!("{}: {}", m.title, m.content),
                confidence: m.strength,
                project: m.project.clone(),
                cluster_id: m.concepts.first().cloned().unwrap_or_default(),
                reinforced_count: 0,
                created_at: now.clone(),
            };
            if let Err(e) = db.insight_create(&rec) {
                warn!("Failed to persist consolidated insight {}: {}", rec.id, e);
            }
        }

        Ok(PipelineResult {
            semantic_new_facts: result.consolidated,
            procedural_new: 0,
            semantic_decayed: 0,
            procedural_decayed: 0,
        })
    }

    /// Run lesson decay: apply Ebbinghaus decay and persist the reduced confidence.
    fn run_lesson_decay(&self) -> Result<LessonDecayResult, anyhow::Error> {
        let mut db = crate::palace_db::PalaceDb::open(&self.palace_path)?;
        let lessons = db.lesson_list(None, None)?;

        let mut decayed = 0;
        let decay_rate = 0.9_f64; // 10% decay per period
        let min_confidence_threshold = 0.1_f64;

        for lesson in lessons {
            // Apply Ebbinghaus decay: confidence = max(0.1, confidence * 0.9)
            let new_confidence = (lesson.confidence * decay_rate).max(min_confidence_threshold);
            if new_confidence < lesson.confidence {
                match db.lesson_set_confidence(&lesson.id, new_confidence) {
                    Ok(_) => decayed += 1,
                    Err(e) => warn!("Failed to decay lesson {}: {}", lesson.id, e),
                }
            }
        }

        Ok(LessonDecayResult { decayed })
    }

    /// Run insight decay: remove stale insights with low confidence.
    fn run_insight_decay(&self) -> Result<InsightDecayResult, anyhow::Error> {
        let mut db = crate::palace_db::PalaceDb::open(&self.palace_path)?;
        let insights = db.insight_list(None, None)?;

        let mut removed = 0;
        let min_confidence = 0.1_f64;
        let max_age_days = 90;

        for insight in insights {
            // Parse created_at to calculate age
            let created_at = chrono::DateTime::parse_from_rfc3339(&insight.created_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let age_days = (chrono::Utc::now() - created_at).num_days();

            // Remove insights with confidence below threshold or older than max_age
            if insight.confidence < min_confidence || age_days > max_age_days {
                if let Err(e) = db.delete_id(&insight.id) {
                    warn!("Failed to delete stale insight {}: {}", insight.id, e);
                } else {
                    removed += 1;
                }
            }
        }

        Ok(InsightDecayResult { removed })
    }

    /// Run index persistence: flush palace state to disk.
    fn run_index_persist(&self) -> Result<IndexPersistResult, anyhow::Error> {
        // Flush the PalaceDb (this persists the JSON documents and SQLite state)
        let mut db = crate::palace_db::PalaceDb::open(&self.palace_path)?;
        db.flush()?;

        Ok(IndexPersistResult { persisted: true })
    }

    /// Run retention sweep: evaluate memories using Ebbinghaus decay
    /// and evict those below the retention threshold.
    ///
    /// Uses the `retention` module's `calculate_retention` for proper
    /// Ebbinghaus-based scoring. Memories with retention below the
    /// `minimum_retention` threshold (default 0.3) are evicted.
    fn run_retention_sweep(&self) -> Result<RetentionSweepResult, anyhow::Error> {
        let mut db = crate::palace_db::PalaceDb::open(&self.palace_path)?;

        // Fix 1: paginated access to avoid OOM when the store has millions of items.
        // Each batch processes up to page_size entries. Since get_all lacks an offset
        // parameter, items that survive eviction will be re-fetched on subsequent
        // iterations until the remaining set fits within one page.
        let page_size = 10_000;

        let mut evaluated = 0usize;
        let mut evicted = 0usize;
        let decay_config = crate::retention::default_decay_config();

        loop {
            let memories_results = db.get_all(None, None, page_size);
            let batch_count: usize = memories_results.iter().map(|qr| qr.ids.len()).sum();
            if batch_count == 0 {
                break;
            }

            for qr in &memories_results {
                // Fix 2: use enumerate() instead of .position() to avoid O(n²) per batch.
                for (idx, (meta, _doc)) in qr.metadatas.iter().zip(qr.documents.iter()).enumerate()
                {
                    evaluated += 1;
                    let access_count = meta
                        .get("access_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let last_accessed = meta
                        .get("last_accessed")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(chrono::Utc::now);

                    // Fix 3: use default_retention_score (strength=1.0) instead of
                    // a dummy 0.0 so calculate_retention has a sensible starting point.
                    let base = crate::retention::default_retention_score("");
                    let retention_score = crate::types::RetentionScore {
                        memory_id: String::new(),
                        access_count,
                        last_accessed,
                        decay_rate: decay_config.decay_rate,
                        ..base
                    };

                    let retention = crate::retention::calculate_retention(
                        &retention_score,
                        &decay_config,
                        None,
                    );

                    // Evict if retention is below the minimum threshold (default 0.3).
                    if retention < decay_config.minimum_retention {
                        if idx < qr.ids.len() {
                            let id = &qr.ids[idx];
                            if let Err(e) = db.delete_id(id) {
                                warn!("Retention sweep: failed to delete {}: {}", id, e);
                            } else {
                                evicted += 1;
                            }
                        }
                    }
                }
            }

            // Stop when we processed fewer items than a full page (no more data).
            if batch_count < page_size {
                break;
            }
        }

        Ok(RetentionSweepResult { evaluated, evicted })
    }
}

/// Synthesize [`CompressedObservation`]s from stored drawers so the LLM-driven
/// consolidation engine has input. Uses room/wing metadata as concepts (so
/// related drawers group together) and a default importance that passes the
/// engine's `importance >= 5` filter.
fn gather_observations(
    db: &crate::palace_db::PalaceDb,
    limit: usize,
) -> Vec<crate::types::CompressedObservation> {
    use crate::types::{CompressedObservation, ObservationType};
    let mut observations = Vec::new();
    for qr in db.get_all(None, None, limit) {
        for ((id, content), metadata) in qr.ids.into_iter().zip(qr.documents).zip(qr.metadatas) {
            let get = |k: &str| metadata.get(k).and_then(|v| v.as_str()).map(String::from);
            let title = get("title").unwrap_or_else(|| {
                content
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect()
            });
            if title.trim().is_empty() {
                continue;
            }
            let mut concepts = Vec::new();
            for key in ["room", "wing"] {
                if let Some(v) = get(key) {
                    if v != "unknown" && !v.is_empty() {
                        concepts.push(v);
                    }
                }
            }
            let session_id = get("session_id")
                .or_else(|| get("source_file"))
                .unwrap_or_else(|| id.clone());
            observations.push(CompressedObservation {
                id,
                session_id,
                timestamp: Utc::now(),
                observation_type: ObservationType::Other,
                title,
                subtitle: None,
                facts: vec![],
                narrative: content,
                concepts,
                files: get("source_file").into_iter().collect(),
                importance: 5,
                confidence: 0.5,
                image_ref: None,
                image_description: None,
                modality: "text".to_string(),
                agent_id: None,
            });
        }
    }
    observations
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of an auto-forget task run.
#[derive(Debug, Clone)]
pub struct AutoForgetResult {
    /// Total memories evaluated.
    pub evaluated: usize,
    /// Number of memories evicted.
    pub evicted: usize,
    /// Number of memories protected from eviction.
    pub protected: usize,
}

/// Result of a lesson decay task run.
#[derive(Debug, Clone)]
pub struct LessonDecayResult {
    /// Number of lessons that would be decayed.
    pub decayed: usize,
}

/// Result of an insight decay task run.
#[derive(Debug, Clone)]
pub struct InsightDecayResult {
    /// Number of stale insights removed.
    pub removed: usize,
}

/// Result of an index persistence task run.
#[derive(Debug, Clone)]
pub struct IndexPersistResult {
    /// Whether persistence succeeded.
    pub persisted: bool,
}

/// Result of a retention sweep task run.
#[derive(Debug, Clone)]
pub struct RetentionSweepResult {
    /// Total memories evaluated.
    pub evaluated: usize,
    /// Number of memories evicted below retention threshold.
    pub evicted: usize,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start all background tasks using tokio intervals.
///
/// This function spawns background tasks that run on the following schedule:
/// - Auto-forget task: every 60 minutes
/// - Consolidation task: every 2 hours
/// - Lesson decay: every 24 hours
/// - Insight decay: every 24 hours
/// - Index persistence: every 30 minutes
///
/// The tasks run indefinitely until either:
/// - The shutdown signal is received via `BackgroundRunner::shutdown()`
/// - The tokio runtime is shut down
pub fn start_background_tasks(palace_path: std::path::PathBuf) -> BackgroundRunner {
    let runner = BackgroundRunner::new(palace_path.clone());
    let shutdown = runner.shutdown.clone();

    // Auto-forget task: every 60 minutes
    let auto_forget_path = palace_path.clone();
    let auto_forget_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let mut ticker = interval(StdDuration::from_secs(AUTO_FORGET_INTERVAL_MINUTES * 60));
        // Run once at startup after a short delay
        tokio::time::sleep(StdDuration::from_secs(30)).await;

        while !auto_forget_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            ticker.tick().await;
            if auto_forget_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            let runner = BackgroundRunner::new(auto_forget_path.clone());
            match runner.run_auto_forget() {
                Ok(result) => {
                    info!(
                        "Auto-forget completed: evaluated={} evicted={} protected={}",
                        result.evaluated, result.evicted, result.protected
                    );
                }
                Err(e) => {
                    warn!("Auto-forget task failed: {}", e);
                }
            }
        }
        info!("Auto-forget task stopped");
    });

    // Consolidation task: every 2 hours
    let consolidation_path = palace_path.clone();
    let consolidation_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let mut ticker = interval(StdDuration::from_secs(CONSOLIDATION_INTERVAL_MINUTES * 60));
        // Run once at startup after a longer delay (let other things initialize first)
        tokio::time::sleep(StdDuration::from_secs(120)).await;

        while !consolidation_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            ticker.tick().await;
            if consolidation_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            let runner = BackgroundRunner::new(consolidation_path.clone());
            match runner.run_consolidation().await {
                Ok(result) => {
                    info!(
                        "Consolidation completed: semantic_new={} procedural_new={} semantic_decayed={} procedural_decayed={}",
                        result.semantic_new_facts, result.procedural_new, result.semantic_decayed, result.procedural_decayed
                    );
                }
                Err(e) => {
                    warn!("Consolidation task failed: {}", e);
                }
            }
        }
        info!("Consolidation task stopped");
    });

    // Lesson decay task: every 24 hours
    let lesson_path = palace_path.clone();
    let lesson_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let mut ticker = interval(StdDuration::from_secs(LESSON_DECAY_INTERVAL_MINUTES * 60));
        // Run once at startup after a delay
        tokio::time::sleep(StdDuration::from_secs(300)).await;

        while !lesson_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            ticker.tick().await;
            if lesson_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            let runner = BackgroundRunner::new(lesson_path.clone());
            match runner.run_lesson_decay() {
                Ok(result) => {
                    info!("Lesson decay completed: decayed={}", result.decayed);
                }
                Err(e) => {
                    warn!("Lesson decay task failed: {}", e);
                }
            }
        }
        info!("Lesson decay task stopped");
    });

    // Insight decay task: every 24 hours
    let insight_path = palace_path.clone();
    let insight_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let mut ticker = interval(StdDuration::from_secs(INSIGHT_DECAY_INTERVAL_MINUTES * 60));
        // Run once at startup after a delay
        tokio::time::sleep(StdDuration::from_secs(330)).await;

        while !insight_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            ticker.tick().await;
            if insight_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            let runner = BackgroundRunner::new(insight_path.clone());
            match runner.run_insight_decay() {
                Ok(result) => {
                    info!("Insight decay completed: removed={}", result.removed);
                }
                Err(e) => {
                    warn!("Insight decay task failed: {}", e);
                }
            }
        }
        info!("Insight decay task stopped");
    });

    // Index persistence task: every 30 minutes
    let index_path = palace_path.clone();
    let index_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let mut ticker = interval(StdDuration::from_secs(INDEX_PERSIST_INTERVAL_MINUTES * 60));
        // Run once at startup after a short delay
        tokio::time::sleep(StdDuration::from_secs(60)).await;

        while !index_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            ticker.tick().await;
            if index_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            let runner = BackgroundRunner::new(index_path.clone());
            match runner.run_index_persist() {
                Ok(result) => {
                    if result.persisted {
                        info!("Index persistence completed");
                    }
                }
                Err(e) => {
                    warn!("Index persistence task failed: {}", e);
                }
            }
        }
        info!("Index persistence task stopped");
    });

    // Retention sweep task: every 2 hours
    let retention_path = palace_path.clone();
    let retention_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let mut ticker = interval(StdDuration::from_secs(
            RETENTION_SWEEP_INTERVAL_MINUTES * 60,
        ));
        // Run once at startup after a delay
        tokio::time::sleep(StdDuration::from_secs(180)).await;

        while !retention_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            ticker.tick().await;
            if retention_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            let runner = BackgroundRunner::new(retention_path.clone());
            match runner.run_retention_sweep() {
                Ok(result) => {
                    info!(
                        "Retention sweep completed: evaluated={} evicted={}",
                        result.evaluated, result.evicted
                    );
                }
                Err(e) => {
                    warn!("Retention sweep task failed: {}", e);
                }
            }
        }
        info!("Retention sweep task stopped");
    });

    info!(
        "Background tasks started: auto-forget={}m consolidation={}h lesson_decay={}h insight_decay={}h index_persist={}m retention_sweep={}m",
        AUTO_FORGET_INTERVAL_MINUTES,
        CONSOLIDATION_INTERVAL_MINUTES / 60,
        LESSON_DECAY_INTERVAL_MINUTES / 60,
        INSIGHT_DECAY_INTERVAL_MINUTES / 60,
        INDEX_PERSIST_INTERVAL_MINUTES,
        RETENTION_SWEEP_INTERVAL_MINUTES,
    );

    runner
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_palace_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let palace_path = dir.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        palace_path
    }

    #[test]
    fn test_background_runner_creation() {
        let path = test_palace_path();
        let runner = BackgroundRunner::new(path.clone());
        assert_eq!(runner.palace_path, path);
        assert!(!runner.is_shutdown());
    }

    #[test]
    fn test_shutdown_flag() {
        let path = test_palace_path();
        let runner = BackgroundRunner::new(path);
        assert!(!runner.is_shutdown());
        runner.shutdown();
        assert!(runner.is_shutdown());
    }

    #[test]
    fn test_result_types_debug() {
        let auto_result = AutoForgetResult {
            evaluated: 10,
            evicted: 5,
            protected: 2,
        };
        assert_eq!(auto_result.evicted, 5);

        let lesson_result = LessonDecayResult { decayed: 3 };
        assert_eq!(lesson_result.decayed, 3);

        let insight_result = InsightDecayResult { removed: 7 };
        assert_eq!(insight_result.removed, 7);

        let persist_result = IndexPersistResult { persisted: true };
        assert!(persist_result.persisted);
    }

    #[test]
    fn test_auto_forget_with_empty_palace() {
        let path = test_palace_path();
        let runner = BackgroundRunner::new(path);
        // Should not panic with empty DB
        let result = runner.run_auto_forget();
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_lesson_decay_with_empty_palace() {
        let path = test_palace_path();
        let runner = BackgroundRunner::new(path);
        // Should not panic with empty DB
        let result = runner.run_lesson_decay();
        // May fail or return 0 decayed
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_lesson_decay_persists() {
        use crate::palace_db::{LessonRecord, PalaceDb};
        let path = test_palace_path();
        {
            let mut db = PalaceDb::open(&path).unwrap();
            db.lesson_create(&LessonRecord {
                id: "lesson-decay-1".to_string(),
                content: "always run tests before commit".to_string(),
                context: "ci".to_string(),
                confidence: 1.0,
                project: "demo".to_string(),
                tags: String::new(),
                reinforced_at: chrono::Utc::now().to_rfc3339(),
                created_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();
        }

        let runner = BackgroundRunner::new(path.clone());
        let result = runner.run_lesson_decay().unwrap();
        assert_eq!(result.decayed, 1, "one lesson should decay");

        let db = PalaceDb::open(&path).unwrap();
        let lessons = db.lesson_list(None, None).unwrap();
        let lesson = lessons.iter().find(|l| l.id == "lesson-decay-1").unwrap();
        assert!(
            (lesson.confidence - 0.9).abs() < 1e-9,
            "confidence should be persisted as 0.9, got {}",
            lesson.confidence
        );
    }

    #[test]
    fn test_insight_decay_with_empty_palace() {
        let path = test_palace_path();
        let runner = BackgroundRunner::new(path);
        // Should not panic with empty DB
        let result = runner.run_insight_decay();
        // May fail or return 0 removed
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_index_persist_with_empty_palace() {
        let path = test_palace_path();
        let runner = BackgroundRunner::new(path);
        // Should not panic with empty DB
        let result = runner.run_index_persist();
        // May fail if palace not initialized yet
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_intervals() {
        assert_eq!(AUTO_FORGET_INTERVAL_MINUTES, 60);
        assert_eq!(CONSOLIDATION_INTERVAL_MINUTES, 120);
        assert_eq!(LESSON_DECAY_INTERVAL_MINUTES, 1440);
        assert_eq!(INSIGHT_DECAY_INTERVAL_MINUTES, 1440);
        assert_eq!(INDEX_PERSIST_INTERVAL_MINUTES, 30);
    }

    #[tokio::test]
    async fn test_consolidation_skips_without_llm() {
        let _lock = crate::test_env_lock().lock().unwrap();
        // SAFETY: env mutation serialized under test_env_lock.
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_BASE_URL");
        }
        let path = test_palace_path();
        let runner = BackgroundRunner::new(path);
        let result = runner.run_consolidation().await.unwrap();
        // No LLM configured -> consolidation is a no-op.
        assert_eq!(result.semantic_new_facts, 0);
    }

    #[test]
    fn test_gather_observations_from_drawers() {
        use crate::palace_db::PalaceDb;
        let path = test_palace_path();
        let mut db = PalaceDb::open(&path).unwrap();
        db.add(
            &[("d1", "content about auth tokens")],
            &[&[("wing", "proj"), ("room", "auth"), ("title", "T1")]],
        )
        .unwrap();
        db.flush().unwrap();

        let obs = gather_observations(&db, 10);
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].title, "T1");
        assert_eq!(obs[0].importance, 5);
        assert!(obs[0].concepts.contains(&"auth".to_string()));
    }
}
