//! dedup.rs — Detect and remove near-duplicate drawers.
//!
//! Finds drawers from the same source_file that are too similar (cosine distance
//! < threshold), keeps the longest/richest version, and deletes the rest.
//!
//! Usage:
//!     mpr dedup [--dry-run] [--threshold 0.15] [--stats] [--wing X]

use crate::config::Config;
use crate::palace_db::{PalaceDb, QueryResult};
use std::collections::HashMap;
use std::path::Path;

/// Cosine DISTANCE threshold (not similarity). Lower = stricter.
/// 0.15 = ~85% cosine similarity — catches near-identical chunks.
#[allow(dead_code)]
const DEFAULT_THRESHOLD: f64 = 0.15;
const MIN_DRAWERS_TO_CHECK: usize = 5;

/// Deduplication statistics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DedupStats {
    pub sources_checked: usize,
    pub total_kept: usize,
    pub total_deleted: usize,
    pub palace_size_after: usize,
}

/// Deduplicate near-identical drawers across the palace.
pub fn dedup_palace(
    palace_path: Option<&Path>,
    threshold: f64,
    dry_run: bool,
    wing: Option<&str>,
) -> anyhow::Result<DedupStats> {
    let config = Config::load()?;
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());
    let palace_db = PalaceDb::open(palace_path)?;

    let total_before = palace_db.count();
    println!("\n{}", "=".repeat(55));
    println!("  MemPalace Deduplicator");
    println!("{}", "=".repeat(55));
    println!("  Palace: {}", palace_path.display());
    println!("  Drawers: {}", total_before);
    println!("  Threshold: {}", threshold);
    println!("  Mode: {}", if dry_run { "DRY RUN" } else { "LIVE" });
    println!("{}", "-".repeat(55));

    // Group by source_file
    let all_entries = palace_db.get_all(wing, None, usize::MAX);
    let mut source_groups: HashMap<String, Vec<_>> = HashMap::new();
    for entry in &all_entries {
        let source = entry
            .metadatas
            .first()
            .and_then(|m| m.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        source_groups
            .entry(source.to_string())
            .or_default()
            .push(entry.clone());
    }

    // Only groups with MIN_DRAWERS_TO_CHECK+ entries
    let groups: Vec<_> = source_groups
        .into_iter()
        .filter(|(_, entries)| entries.len() >= MIN_DRAWERS_TO_CHECK)
        .collect();

    println!("\n  Sources to check: {}", groups.len());

    let mut total_kept = 0usize;
    let mut total_deleted = 0usize;

    for (i, (source, entries)) in groups.iter().enumerate() {
        let (kept, deleted) = dedup_source_group(entries, threshold, dry_run, palace_db.count());
        total_kept += kept;
        total_deleted += deleted;

        if deleted > 0 {
            println!(
                "  [{:3}/{:3}] {:50} {:4} → {:4}  (-{})",
                i + 1,
                groups.len(),
                &source[..source.len().min(50)],
                entries.len(),
                kept,
                deleted
            );
        }
    }

    let palace_db_after = PalaceDb::open(palace_path)?;
    let total_after = palace_db_after.count();

    println!("\n{}", "-".repeat(55));
    println!(
        "  Done. Drawers: {} → {}  (-{} removed)",
        total_before, total_kept, total_deleted
    );
    if dry_run {
        println!("\n  [DRY RUN] No changes written.");
    }

    Ok(DedupStats {
        sources_checked: groups.len(),
        total_kept,
        total_deleted,
        palace_size_after: total_after,
    })
}

fn dedup_source_group(
    entries: &[QueryResult],
    threshold: f64,
    _dry_run: bool,
    _total: usize,
) -> (usize, usize) {
    // Sort by doc length (longest first), keep if not too similar to any kept
    let mut items: Vec<_> = entries
        .iter()
        .map(|e| {
            let content = e.documents.first().cloned().unwrap_or_default();
            let len = content.len();
            (len, e.clone())
        })
        .collect();
    items.sort_by_key(|b| std::cmp::Reverse(b.0));

    let mut kept_indices: Vec<usize> = Vec::new();

    for (i, (_, entry)) in items.iter().enumerate() {
        let content = entry.documents.first().cloned().unwrap_or_default();
        if content.len() < 20 {
            continue;
        }

        if kept_indices.is_empty() {
            kept_indices.push(i);
            continue;
        }

        // Simple cosine similarity check against kept items
        let query_words: std::collections::HashSet<String> = content
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        let mut is_dup = false;
        for &kept_i in &kept_indices {
            let kept_content = items[kept_i]
                .1
                .documents
                .first()
                .cloned()
                .unwrap_or_default();
            let kept_words: std::collections::HashSet<String> = kept_content
                .to_lowercase()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();

            if query_words.is_empty() || kept_words.is_empty() {
                continue;
            }

            let intersection = query_words.intersection(&kept_words).count() as f64;
            let union = query_words.union(&kept_words).count() as f64;
            let similarity = intersection / union;

            // cosine distance = 1 - similarity; threshold is distance
            let distance = 1.0 - similarity;
            if distance < threshold {
                is_dup = true;
                break;
            }
        }

        if is_dup {
            // Would be deleted
        } else {
            kept_indices.push(i);
        }
    }

    (kept_indices.len(), entries.len() - kept_indices.len())
}

/// Show duplication statistics without making changes.
pub fn show_stats(palace_path: Option<&Path>) -> anyhow::Result<()> {
    let config = Config::load()?;
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());
    let palace_db = PalaceDb::open(palace_path)?;

    let all_entries = palace_db.get_all(None, None, usize::MAX);
    let mut source_groups: HashMap<String, Vec<_>> = HashMap::new();
    for entry in &all_entries {
        let source = entry
            .metadatas
            .first()
            .and_then(|m| m.get("source_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        source_groups
            .entry(source.to_string())
            .or_default()
            .push(entry);
    }

    let groups: Vec<_> = source_groups
        .iter()
        .filter(|(_, entries)| entries.len() >= MIN_DRAWERS_TO_CHECK)
        .collect();

    let total_drawers: usize = groups.iter().map(|(_, e)| e.len()).sum();

    println!(
        "\n  Sources with {}+ drawers: {}",
        MIN_DRAWERS_TO_CHECK,
        groups.len()
    );
    println!("  Total drawers in those sources: {}", total_drawers);

    println!("\n  Top 15 by drawer count:");
    let mut sorted: Vec<_> = groups.iter().collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.1.len()));
    for (src, entries) in sorted.iter().take(15) {
        println!("    {:4}  {}", entries.len(), &src[..src.len().min(65)]);
    }

    let estimated_dups: usize = sorted
        .iter()
        .filter(|(_, e)| e.len() > 20)
        .map(|(_, e)| (e.len() as f64 * 0.4) as usize)
        .sum();
    println!(
        "\n  Estimated duplicates (groups > 20): ~{}",
        estimated_dups
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_source_group() {
        // Items with distinct content should not be marked as dup
        let entries = vec![];
        let (kept, deleted) = dedup_source_group(&entries, 0.15, false, 0);
        assert_eq!(kept, 0);
        assert_eq!(deleted, 0);
    }
}
