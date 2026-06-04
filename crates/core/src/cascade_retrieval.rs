//! Cascade retrieval (issue #31, mp-migration 6/8).
//!
//! Implements the jcode-style cascade retrieval: a BFS from embedding-search
//! seed IDs that follows typed memory edges with traversal weights and
//! decays the score by `0.7^depth`.
//!
//! ## Algorithm
//!
//! 1. Initialize a queue with `(id, seed_score, depth=0)` for each seed.
//! 2. BFS loop: for each popped `(id, score, depth)`:
//!    - Skip if `depth >= max_depth`.
//!    - Look up typed edges from `id` in *both* directions (the graph is
//!      treated as undirected for retrieval, matching jcode behaviour).
//!    - For each edge, compute `new_score = score * edge_weight * 0.7`
//!      and enqueue the target with `depth+1`.
//!    - **Tag fan-out**: when the edge is a `HasTag` edge, the target is a
//!      tag. Look up all *incoming* `HasTag` edges to that tag (i.e. every
//!      drawer that shares this tag) and enqueue each source (other than
//!      the current node) with the decayed score. The tag entity itself is
//!      not added to the result set — only the fan-out sources are.
//! 3. When a node is reachable via multiple paths, the **maximum** score
//!    wins (we never decrease a node's recorded score).
//! 4. Return the top-`max_results` `(DrawerId, score)` pairs ordered by score
//!    descending. Tie-breaks on `DrawerId` for deterministic output.
//!
//! The traversal weights come from
//! [`crate::types::MemoryEdgeKind::traversal_weight`] and are
//! canonical jcode values:
//!
//! | kind        | weight |
//! |-------------|--------|
//! | HasTag      | 0.8    |
//! | InCluster   | 0.6    |
//! | RelatesTo   | W (per-edge) |
//! | Supersedes  | 0.9    |
//! | Contradicts | 0.3    |
//! | DerivedFrom | 0.7    |
//!
//! ## Usage
//!
//! ```ignore
//! let embedding_seeds: Vec<(DrawerId, f32)> = palace
//!     .search_with_embedding(&query_vec, &scope)
//!     .await?
//!     .into_iter()
//!     .filter_map(|hit| /* project to (id, similarity) */)
//!     .collect();
//! let expanded = cascade_retrieve(&kg, &embedding_seeds, 3, 50);
//! ```
//!
//! See `MemoryProvider::cascade_search` for the high-level wrapper that
//! combines embedding search and cascade retrieval in a single call.

use std::collections::{HashMap, VecDeque};

use crate::knowledge_graph::{EntityQueryResult, KnowledgeGraph};
use crate::palace::DrawerId;
use crate::types::MemoryEdgeKind;

/// Decay factor applied per hop. jcode's constant.
pub const DECAY: f32 = 0.7;

/// A single BFS entry.
#[derive(Debug, Clone)]
struct Entry {
    /// Entity id (lowercased, normalized — matches `entity_id()` in KG).
    id: String,
    /// The score propagated along the BFS so far.
    score: f32,
    /// BFS depth: seeds start at 0, every hop increments by 1.
    depth: usize,
}

/// Traverse a knowledge graph from embedding-search seed IDs, expanding
/// along typed memory edges with score decay.
///
/// `seed_ids` is a list of `(drawer_id, embedding_similarity)` pairs — the
/// output of an embedding search that we want to expand.
///
/// Returns the top-`max_results` `(DrawerId, cascade_score)` pairs ordered
/// by score descending. The seed drawers themselves are also included in
/// the result (with their seed scores) when they survive the top-K cut.
///
/// `max_depth = 0` means "seeds only, no expansion."
pub fn cascade_retrieve(
    kg: &KnowledgeGraph,
    seed_ids: &[(DrawerId, f32)],
    max_depth: usize,
    max_results: usize,
) -> Vec<(DrawerId, f32)> {
    // BFS queue: (entity_id, propagated_score, depth).
    let mut queue: VecDeque<Entry> = VecDeque::new();
    // Best-known score per entity id.
    let mut best: HashMap<String, f32> = HashMap::new();

    // Seed the BFS. Each seed is rooted at depth 0; its own score is its
    // embedding similarity (or whatever the caller passed in).
    for (id, score) in seed_ids {
        let eid = normalize_entity_id(&id.0);
        let prev = best.get(&eid).copied();
        // Use a small epsilon for zero/invalid scores so the BFS can
        // still traverse edges from these seeds. Without this, a store
        // that returns similarity=0.0 (BM25-only, missing embedding)
        // would produce an empty cascade result.
        let score = if score.is_finite() && *score > 0.0 {
            *score
        } else {
            1e-6
        };
        match prev {
            Some(p) if p >= score => {} // keep existing max
            _ => {
                best.insert(eid.clone(), score);
                queue.push_back(Entry {
                    id: eid,
                    score,
                    depth: 0,
                });
            }
        }
    }

    while let Some(entry) = queue.pop_front() {
        if entry.depth >= max_depth {
            continue;
        }

        // We need both the typed edge kind and the target id for each
        // outgoing edge. The KG exposes the typed columns via
        // `query_outgoing` (raw) and `query_outgoing_by_kind` (one kind at
        // a time). The raw query gives us everything; we then look up the
        // typed kind via `edge_kind` + `weight` and parse back to a
        // `MemoryEdgeKind` for the canonical traversal weight.
        // Traverse edges in both directions: an edge A->B means B can
        // also discover A through the reverse direction, just as jcode's
        // cascade treats the graph as undirected for retrieval purposes.
        let edges = match kg.query_entity(&entry.id, None, None, "both") {
            Ok(rows) => rows,
            Err(_) => continue,
        };

        for row in edges {
            let Some(kind) = typed_kind_of(&row) else {
                continue;
            };
            let edge_weight = kind.traversal_weight() as f32;
            // Determine the "other end" of the edge: for outgoing edges
            // the target is the object; for incoming edges it's the subject.
            let target = if row.direction == "incoming" {
                normalize_entity_id(&row.subject)
            } else {
                normalize_entity_id(&row.object)
            };
            // Skip self-loops.
            if target == entry.id {
                continue;
            }
            let new_score = entry.score * edge_weight * DECAY;

            if matches!(kind, MemoryEdgeKind::HasTag) {
                // Tag fan-out: identify the tag entity and enumerate all
                // incoming `HasTag` edges to it. For outgoing HasTag the
                // tag is the target; for incoming HasTag the current node
                // is the tag itself.
                let (tag_id, exclude) = if row.direction == "incoming" {
                    (entry.id.clone(), target.clone())
                } else {
                    (target.clone(), entry.id.clone())
                };
                tag_fan_out(
                    kg,
                    &tag_id,
                    &exclude,
                    new_score,
                    entry.depth + 1,
                    &mut best,
                    &mut queue,
                );
            } else {
                try_enqueue(&target, new_score, entry.depth + 1, &mut best, &mut queue);
            }
        }
    }

    // Project the score map to (DrawerId, f32). Sort by score desc, then
    // id asc for determinism. Truncate to max_results.
    let mut scored: Vec<(String, f32)> = best.into_iter().collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(max_results);
    scored
        .into_iter()
        .map(|(id, score)| (DrawerId(id), score))
        .collect()
}

/// Enumerate all incoming `HasTag` edges to `tag_id` and enqueue each
/// source (other than `exclude`) with the decayed score.
fn tag_fan_out(
    kg: &KnowledgeGraph,
    tag_id: &str,
    exclude: &str,
    new_score: f32,
    new_depth: usize,
    best: &mut HashMap<String, f32>,
    queue: &mut VecDeque<Entry>,
) {
    let incoming = match kg.query_incoming_by_kind(tag_id, &MemoryEdgeKind::HasTag) {
        Ok(rows) => rows,
        Err(_) => return,
    };
    for triple in incoming {
        let source = normalize_entity_id(&triple.subject);
        if source == exclude {
            continue;
        }
        try_enqueue(&source, new_score, new_depth, best, queue);
    }
}

/// MAX-score semantics: if `id` already has a score at least as high as
/// `new_score`, do nothing. Otherwise, update the recorded score and
/// enqueue for further expansion.
fn try_enqueue(
    id: &str,
    new_score: f32,
    new_depth: usize,
    best: &mut HashMap<String, f32>,
    queue: &mut VecDeque<Entry>,
) {
    if !new_score.is_finite() || new_score <= 0.0 {
        return;
    }
    match best.get(id).copied() {
        Some(existing) if existing >= new_score => return,
        _ => {
            best.insert(id.to_string(), new_score);
            queue.push_back(Entry {
                id: id.to_string(),
                score: new_score,
                depth: new_depth,
            });
        }
    }
}

/// Map an [`EntityQueryResult`]'s `edge_kind`/`weight` columns back to
/// the typed [`MemoryEdgeKind`]. Returns `None` for untyped triples
/// (those written through the generic `add_triple` path without an
/// `edge_kind`).
fn typed_kind_of(t: &EntityQueryResult) -> Option<MemoryEdgeKind> {
    let kind_str = t.edge_kind.as_deref()?;
    MemoryEdgeKind::from_kind_and_weight(kind_str, t.weight)
}

/// Mirror the KG's internal entity-id normalization (lowercase, spaces
/// to underscores, strip apostrophes). This avoids relying on the private
/// `KnowledgeGraph::entity_id` while still producing the same key.
fn normalize_entity_id(name: &str) -> String {
    name.to_lowercase().replace(' ', "_").replace('\'', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_graph::KnowledgeGraph;
    use crate::types::MemoryEdgeKind;
    use std::path::Path;

    fn fresh_kg() -> KnowledgeGraph {
        KnowledgeGraph::open(Path::new(":memory:")).unwrap()
    }

    fn id(s: &str) -> DrawerId {
        DrawerId(s.to_string())
    }

    #[test]
    fn cascade_returns_seeds_when_depth_zero() {
        let kg = fresh_kg();
        let seeds = vec![(id("a"), 0.9_f32), (id("b"), 0.5_f32)];
        let out = cascade_retrieve(&kg, &seeds, 0, 10);
        // With max_depth=0, only the seeds are returned (in score order).
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, id("a"));
        assert!((out[0].1 - 0.9).abs() < 1e-6);
        assert_eq!(out[1].0, id("b"));
        assert!((out[1].1 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn cascade_score_decay_matches_formula() {
        // seed_score * edge_weight * 0.7^(depth+1)
        //   depth=0 (1 hop): score * 0.9 * 0.7
        //   depth=1 (2 hops): score * 0.9 * 0.7 * 0.9 * 0.7
        let mut kg = fresh_kg();
        kg.add_memory_edge("a", "b", &MemoryEdgeKind::Supersedes)
            .unwrap();
        kg.add_memory_edge("b", "c", &MemoryEdgeKind::Supersedes)
            .unwrap();

        let seeds = vec![(id("a"), 1.0_f32)];
        let out = cascade_retrieve(&kg, &seeds, 2, 10);

        let score_of = |target: &str| -> f32 {
            out.iter()
                .find(|(d, _)| d.0 == target)
                .map(|(_, s)| *s)
                .unwrap_or(-1.0)
        };

        // b is reachable in 1 hop: 1.0 * 0.9 * 0.7 = 0.63
        let sb = score_of("b");
        assert!((sb - 0.63).abs() < 1e-5, "expected b≈0.63, got {}", sb);
        // c is reachable in 2 hops: 1.0 * 0.9 * 0.7 * 0.9 * 0.7 = 0.3969
        let sc = score_of("c");
        assert!((sc - 0.3969).abs() < 1e-4, "expected c≈0.3969, got {}", sc);
    }

    #[test]
    fn cascade_keeps_max_score_for_multi_path() {
        let mut kg = fresh_kg();
        // Two paths from a -> c:
        //   direct:      a -[RelatesTo 0.5]-> c   → 1.0 * 0.5 * 0.7  = 0.35
        //   via b:       a -[RelatesTo 1.0]-> b
        //                b -[RelatesTo 1.0]-> c   → 1.0 * 1.0 * 0.7 * 1.0 * 0.7 = 0.49
        kg.add_memory_edge("a", "b", &MemoryEdgeKind::RelatesTo { weight: 1.0 })
            .unwrap();
        kg.add_memory_edge("b", "c", &MemoryEdgeKind::RelatesTo { weight: 1.0 })
            .unwrap();
        kg.add_memory_edge("a", "c", &MemoryEdgeKind::RelatesTo { weight: 0.5 })
            .unwrap();

        let seeds = vec![(id("a"), 1.0_f32)];
        let out = cascade_retrieve(&kg, &seeds, 3, 10);

        let c_score = out
            .iter()
            .find(|(d, _)| d.0 == "c")
            .map(|(_, s)| *s)
            .expect("c should appear");
        // Best path is via b: 0.49
        assert!((c_score - 0.49).abs() < 1e-4, "got {}", c_score);
    }

    #[test]
    fn cascade_tag_fan_out_reaches_all_drawers_with_tag() {
        let mut kg = fresh_kg();
        // Three drawers share the "rust" tag.
        for d in &["a", "b", "c"] {
            kg.add_memory_edge(*d, "rust", &MemoryEdgeKind::HasTag)
                .unwrap();
        }
        // A RelatesTo edge from a to d (an unrelated drawer) seeds the cascade.
        kg.add_memory_edge("a", "d", &MemoryEdgeKind::RelatesTo { weight: 1.0 })
            .unwrap();

        let seeds = vec![(id("d"), 1.0_f32)];
        let out = cascade_retrieve(&kg, &seeds, 3, 50);

        // d (seed), a (1-hop via RelatesTo), then a's tag fan-out hits
        // b and c at depth 2.
        for label in &["a", "b", "c", "d"] {
            assert!(
                out.iter().any(|(d_id, _)| d_id.0 == *label),
                "expected drawer `{}` in cascade result, got {:?}",
                label,
                out.iter().map(|(d, _)| d.0.clone()).collect::<Vec<_>>()
            );
        }
        // The tag entity itself ("rust") should NOT appear in the result.
        assert!(
            !out.iter().any(|(d, _)| d.0 == "rust"),
            "tag entity should not appear in result"
        );
    }

    #[test]
    fn cascade_respects_max_depth() {
        let mut kg = fresh_kg();
        kg.add_memory_edge("a", "b", &MemoryEdgeKind::RelatesTo { weight: 1.0 })
            .unwrap();
        kg.add_memory_edge("b", "c", &MemoryEdgeKind::RelatesTo { weight: 1.0 })
            .unwrap();
        kg.add_memory_edge("c", "d", &MemoryEdgeKind::RelatesTo { weight: 1.0 })
            .unwrap();

        let seeds = vec![(id("a"), 1.0_f32)];
        // depth=1 reaches only b; c and d are 2+ hops away.
        let out = cascade_retrieve(&kg, &seeds, 1, 10);
        let labels: Vec<String> = out.iter().map(|(d, _)| d.0.clone()).collect();
        assert!(labels.contains(&"a".to_string()));
        assert!(labels.contains(&"b".to_string()));
        assert!(!labels.contains(&"c".to_string()));
        assert!(!labels.contains(&"d".to_string()));
    }

    #[test]
    fn cascade_respects_max_results() {
        let mut kg = fresh_kg();
        for i in 0..10 {
            let drawer = format!("d{}", i);
            kg.add_memory_edge("seed", &drawer, &MemoryEdgeKind::RelatesTo { weight: 1.0 })
                .unwrap();
        }
        let seeds = vec![(id("seed"), 1.0_f32)];
        let out = cascade_retrieve(&kg, &seeds, 2, 3);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn cascade_top_k_is_sorted_by_score_desc() {
        let mut kg = fresh_kg();
        kg.add_memory_edge("s", "x", &MemoryEdgeKind::RelatesTo { weight: 0.2 })
            .unwrap();
        kg.add_memory_edge("s", "y", &MemoryEdgeKind::RelatesTo { weight: 0.8 })
            .unwrap();
        kg.add_memory_edge("s", "z", &MemoryEdgeKind::RelatesTo { weight: 0.5 })
            .unwrap();
        let seeds = vec![(id("s"), 1.0_f32)];
        let out = cascade_retrieve(&kg, &seeds, 1, 5);
        // Expect y > z > x in score order.
        let scores: Vec<f32> = out
            .iter()
            .filter(|(d, _)| d.0 != "s")
            .map(|(_, s)| *s)
            .collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn cascade_normalizes_entity_ids() {
        // Entity names with spaces / mixed case should match the KG's
        // normalized ids. We bypass `add_memory_edge` (which forces a
        // canonical form) and instead write a typed edge directly. Since
        // the typed-edge API also normalizes, we only verify that
        // mixed-case seeds and lowercase KG rows line up.
        let mut kg = fresh_kg();
        kg.add_memory_edge("Seed", "Target", &MemoryEdgeKind::RelatesTo { weight: 1.0 })
            .unwrap();
        let seeds = vec![(id("SEED"), 1.0_f32)];
        let out = cascade_retrieve(&kg, &seeds, 1, 10);
        assert!(
            out.iter().any(|(d, _)| d.0 == "target"),
            "expected `target` in {:?}",
            out.iter().map(|(d, _)| d.0.clone()).collect::<Vec<_>>()
        );
    }
}
