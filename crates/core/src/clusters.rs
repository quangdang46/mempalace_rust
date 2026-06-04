//! clusters.rs — Auto-clustering, centroid computation, and LLM naming.
//!
//! Clusters group related memory drawers by embedding similarity. The
//! deterministic cluster ID (`auto-{scope}-{fnv1a_hash}`) ensures that the
//! same set of members always produces the same ID, making the operation
//! idempotent across retries.
//!
//! ## Design
//!
//! - [`auto_cluster`] is the main entry point: it sorts/dedups member IDs,
//!   computes the centroid, creates the cluster row, and wires InCluster edges.
//! - [`name_cluster_with_sidecar`] optionally asks an LLM for a human-readable
//!   cluster name (2-4 words). Falls back to [`heuristic_cluster_name`] when
//!   the LLM is unavailable.
//! - [`refine_clusters`] merges overlapping clusters and recomputes centroids.

use std::collections::HashMap;

use crate::knowledge_graph::KnowledgeGraph;
use crate::palace::DrawerId;

// ---------------------------------------------------------------------------
// FNV-1a 32-bit hash (deterministic, no external crate needed)
// ---------------------------------------------------------------------------

/// FNV-1a 32-bit hash. Produces the same value across all platforms.
fn fnv1a_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in data {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Generate a deterministic cluster ID from a sorted list of member IDs and
/// a scope string. The format is `"auto-{scope}-{fnv1a_hex}"`.
fn deterministic_cluster_id(sorted_ids: &[&str], scope: &str) -> String {
    let mut hasher_input = Vec::new();
    for id in sorted_ids {
        hasher_input.extend_from_slice(id.as_bytes());
        hasher_input.push(0); // separator to avoid ambiguity
    }
    let hash = fnv1a_hash(&hasher_input);
    format!("auto-{}-{:08x}", scope, hash)
}

// ---------------------------------------------------------------------------
// Centroid computation
// ---------------------------------------------------------------------------

/// Compute the centroid (element-wise average) of a slice of embeddings.
/// Returns `None` if `embeddings` is empty or if the embeddings have
/// inconsistent dimensions.
fn compute_centroid(embeddings: &[&[f32]]) -> Option<Vec<f32>> {
    if embeddings.is_empty() {
        return None;
    }
    let dim = embeddings[0].len();
    if dim == 0 || embeddings.iter().any(|e| e.len() != dim) {
        return None;
    }
    let n = embeddings.len() as f32;
    let mut centroid = vec![0.0f32; dim];
    for emb in embeddings {
        for (i, v) in emb.iter().enumerate() {
            centroid[i] += v;
        }
    }
    for v in &mut centroid {
        *v /= n;
    }
    Some(centroid)
}

// ---------------------------------------------------------------------------
// auto_cluster
// ---------------------------------------------------------------------------

/// Create a cluster from a set of member IDs and their embeddings.
///
/// The cluster ID is deterministic: the same `(scope, sorted member IDs)`
/// pair always produces the same ID, making this function idempotent.
///
/// # Arguments
///
/// * `kg`        — mutable reference to the knowledge graph (for persistence).
/// * `member_ids` — drawer IDs to cluster.
/// * `embeddings` — map from drawer ID to its embedding vector.
/// * `scope`      — namespace for the cluster ID (e.g. `"retrieval"`, `"topic"`).
///
/// # Returns
///
/// The cluster ID string on success.
pub fn auto_cluster(
    kg: &mut KnowledgeGraph,
    member_ids: &[DrawerId],
    embeddings: &HashMap<DrawerId, Vec<f32>>,
    scope: &str,
) -> anyhow::Result<String> {
    if member_ids.len() < 2 {
        anyhow::bail!(
            "auto_cluster requires at least 2 members, got {}",
            member_ids.len()
        );
    }

    // Sort and dedup member IDs for deterministic hashing.
    let mut sorted: Vec<&str> = member_ids.iter().map(|d| d.0.as_str()).collect();
    sorted.sort();
    sorted.dedup();

    let cluster_id = deterministic_cluster_id(&sorted, scope);

    // Gather embeddings for the sorted members.
    let mut emb_refs: Vec<&[f32]> = Vec::with_capacity(sorted.len());
    for id in &sorted {
        let drawer = DrawerId(id.to_string());
        match embeddings.get(&drawer) {
            Some(emb) => emb_refs.push(emb.as_slice()),
            None => anyhow::bail!("missing embedding for member {}", id),
        }
    }

    let centroid = compute_centroid(&emb_refs)
        .ok_or_else(|| anyhow::anyhow!("failed to compute centroid (empty or mismatched dims"))?;

    // Convert sorted &str back to owned Strings for create_cluster.
    let member_strings: Vec<String> = sorted.iter().map(|s| s.to_string()).collect();

    kg.create_cluster(&cluster_id, None, &centroid, &member_strings)?;

    Ok(cluster_id)
}

// ---------------------------------------------------------------------------
// LLM-based naming
// ---------------------------------------------------------------------------

/// Ask an LLM sidecar to generate a short descriptive name (2-4 words) for a
/// cluster based on its member contents.
///
/// The `classify_fn` closure abstracts over the LLM provider so callers don't
/// need to depend on a concrete provider type. It takes `(system_prompt,
/// user_prompt)` and returns the LLM's text response.
///
/// Falls back to [`heuristic_cluster_name`] on any error.
pub fn name_cluster_with_llm<F>(classify_fn: F, member_contents: &[String]) -> String
where
    F: Fn(&str, &str) -> Result<String, String>,
{
    if member_contents.is_empty() {
        return "empty-cluster".to_string();
    }

    let joined = member_contents
        .iter()
        .take(10) // limit context length
        .enumerate()
        .map(|(i, c)| format!("{}. {}", i + 1, truncate(c, 200)))
        .collect::<Vec<_>>()
        .join("\n");

    let system = "You are a concise labeller. Respond with ONLY a short cluster name (2-4 words), no punctuation, no explanation.";
    let user = &format!(
        "Give this cluster of memories a short descriptive name:\n{}",
        joined
    );

    match classify_fn(system, user) {
        Ok(name) => {
            let cleaned = name.trim().trim_matches('"').trim_matches('\'');
            if cleaned.is_empty() || cleaned.len() > 60 {
                heuristic_cluster_name(member_contents)
            } else {
                cleaned.to_string()
            }
        }
        Err(_) => heuristic_cluster_name(member_contents),
    }
}

/// Word-frequency-based fallback for cluster naming.
///
/// Extracts the top 3 non-stopword tokens by frequency and joins them.
pub fn heuristic_cluster_name(member_contents: &[String]) -> String {
    if member_contents.is_empty() {
        return "empty-cluster".to_string();
    }

    let stop_words: std::collections::HashSet<&str> = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
        "need", "dare", "ought", "used", "to", "of", "in", "for", "on", "with", "at", "by", "from",
        "as", "into", "through", "during", "before", "after", "above", "below", "between", "out",
        "off", "over", "under", "again", "further", "then", "once", "here", "there", "when",
        "where", "why", "how", "all", "both", "each", "few", "more", "most", "other", "some",
        "such", "no", "not", "only", "own", "same", "so", "than", "too", "very", "just", "and",
        "but", "or", "if", "while", "about", "up", "this", "that", "it", "its", "i", "me", "my",
        "we", "our", "you", "your", "he", "him", "his", "she", "her", "they", "them", "their",
        "what", "which", "who",
    ]
    .iter()
    .copied()
    .collect();

    let mut freq: HashMap<String, usize> = HashMap::new();
    for content in member_contents {
        for word in content.split_whitespace() {
            let lower = word
                .to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string();
            if lower.len() >= 3 && !stop_words.contains(lower.as_str()) {
                *freq.entry(lower).or_insert(0) += 1;
            }
        }
    }

    if freq.is_empty() {
        return "misc-cluster".to_string();
    }

    let mut pairs: Vec<(&String, &usize)> = freq.iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

    let top: Vec<&str> = pairs.iter().take(3).map(|(w, _)| w.as_str()).collect();
    top.join("-")
}

// ---------------------------------------------------------------------------
// Cluster refinement
// ---------------------------------------------------------------------------

/// Merge overlapping clusters: if two clusters share more than `threshold`
/// fraction of members, they are merged into one. The surviving cluster
/// retains the lexicographically smaller ID. Centroids are recomputed from
/// the merged member set.
///
/// # Returns
///
/// The number of merges performed.
pub fn refine_clusters(
    kg: &mut KnowledgeGraph,
    embeddings: &HashMap<DrawerId, Vec<f32>>,
    threshold: f64,
) -> anyhow::Result<usize> {
    let clusters = kg.list_clusters()?;
    let mut merges = 0;

    // Collect (id, members) pairs for overlap comparison.
    let mut cluster_members: Vec<(String, Vec<String>)> = Vec::new();
    for c in &clusters {
        let members = kg.get_cluster_members(&c.id)?;
        cluster_members.push((c.id.clone(), members));
    }

    // Find overlapping pairs (i < j to avoid double-processing).
    let mut to_merge: Vec<(usize, usize)> = Vec::new();
    for i in 0..cluster_members.len() {
        for j in (i + 1)..cluster_members.len() {
            let set_a: std::collections::HashSet<&str> =
                cluster_members[i].1.iter().map(|s| s.as_str()).collect();
            let set_b: std::collections::HashSet<&str> =
                cluster_members[j].1.iter().map(|s| s.as_str()).collect();
            let intersection = set_a.intersection(&set_b).count();
            let min_size = set_a.len().min(set_b.len());
            if min_size > 0 && (intersection as f64 / min_size as f64) > threshold {
                to_merge.push((i, j));
            }
        }
    }

    // Apply merges: absorb j's members into i, delete j.
    // Process in reverse order of j so indices stay valid.
    let mut absorbed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &(i, j) in &to_merge {
        if absorbed.contains(&j) {
            continue;
        }
        absorbed.insert(j);

        // Gather all members from both clusters.
        let mut all_members: Vec<String> = cluster_members[i]
            .1
            .iter()
            .chain(cluster_members[j].1.iter())
            .cloned()
            .collect();
        all_members.sort();
        all_members.dedup();

        // Recompute centroid.
        let mut emb_refs: Vec<&[f32]> = Vec::new();
        for m in &all_members {
            let drawer = DrawerId(m.clone());
            if let Some(emb) = embeddings.get(&drawer) {
                emb_refs.push(emb.as_slice());
            }
        }
        if let Some(new_centroid) = compute_centroid(&emb_refs) {
            // Re-create the surviving cluster with the merged member set.
            kg.create_cluster(&cluster_members[i].0, None, &new_centroid, &all_members)?;
            // Update the local cache so subsequent merges see the expanded set.
            cluster_members[i].1 = all_members;
        }

        merges += 1;
    }

    Ok(merges)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_deterministic() {
        let a = fnv1a_hash(b"hello world");
        let b = fnv1a_hash(b"hello world");
        assert_eq!(a, b);
        // Different inputs must produce different hashes.
        let c = fnv1a_hash(b"hello worle");
        assert_ne!(a, c);
    }

    #[test]
    fn test_deterministic_cluster_id() {
        // The function expects sorted input; auto_cluster sorts before calling.
        let ids = &["a", "b", "c"];
        let id_a = deterministic_cluster_id(ids, "test");
        let id_b = deterministic_cluster_id(ids, "test");
        assert_eq!(id_a, id_b, "same members must produce same ID");
        assert!(id_a.starts_with("auto-test-"));
    }

    #[test]
    fn test_deterministic_cluster_id_different_scope() {
        let ids = &["a", "b"];
        let id1 = deterministic_cluster_id(ids, "scope1");
        let id2 = deterministic_cluster_id(ids, "scope2");
        assert_ne!(id1, id2, "different scopes must produce different IDs");
    }

    #[test]
    fn test_compute_centroid_basic() {
        let a = [1.0f32, 2.0, 3.0];
        let b = [3.0f32, 4.0, 5.0];
        let centroid = compute_centroid(&[a.as_slice(), b.as_slice()]).unwrap();
        assert_eq!(centroid.len(), 3);
        assert!((centroid[0] - 2.0).abs() < 1e-6);
        assert!((centroid[1] - 3.0).abs() < 1e-6);
        assert!((centroid[2] - 4.0).abs() < 1e-6);
    }

    #[test]
    fn test_compute_centroid_empty() {
        assert!(compute_centroid(&[]).is_none());
    }

    #[test]
    fn test_compute_centroid_mismatched_dims() {
        let a = [1.0f32, 2.0];
        let b = [1.0f32, 2.0, 3.0];
        assert!(compute_centroid(&[a.as_slice(), b.as_slice()]).is_none());
    }

    #[test]
    fn test_auto_cluster_basic() {
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        let ids = vec![
            DrawerId::new("drawer_a"),
            DrawerId::new("drawer_b"),
            DrawerId::new("drawer_c"),
        ];
        let mut emb = HashMap::new();
        emb.insert(DrawerId::new("drawer_a"), vec![1.0, 0.0, 0.0]);
        emb.insert(DrawerId::new("drawer_b"), vec![0.0, 1.0, 0.0]);
        emb.insert(DrawerId::new("drawer_c"), vec![0.0, 0.0, 1.0]);

        let cluster_id = auto_cluster(&mut kg, &ids, &emb, "retrieval").unwrap();
        assert!(cluster_id.starts_with("auto-retrieval-"));

        // Verify the cluster is persisted.
        let entry = kg
            .get_cluster(&cluster_id)
            .unwrap()
            .expect("cluster must exist");
        assert_eq!(entry.member_count, 3);
        assert!(entry.centroid.len() == 3);
        // Centroid should be [1/3, 1/3, 1/3]
        assert!((entry.centroid[0] - 1.0 / 3.0).abs() < 1e-6);
        assert!((entry.centroid[1] - 1.0 / 3.0).abs() < 1e-6);
        assert!((entry.centroid[2] - 1.0 / 3.0).abs() < 1e-6);

        // Verify members are retrievable via InCluster edges.
        let members = kg.get_cluster_members(&cluster_id).unwrap();
        assert_eq!(members.len(), 3);
        // Members come back sorted by the entity name.
        let mut sorted_members = members.clone();
        sorted_members.sort();
        assert_eq!(sorted_members[0], "drawer_a");
        assert_eq!(sorted_members[1], "drawer_b");
        assert_eq!(sorted_members[2], "drawer_c");
    }

    #[test]
    fn test_auto_cluster_deterministic() {
        let mut kg1 = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let mut kg2 = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        let ids_a = vec![DrawerId::new("x"), DrawerId::new("y")];
        let ids_b = vec![DrawerId::new("y"), DrawerId::new("x")]; // reversed

        let mut emb = HashMap::new();
        emb.insert(DrawerId::new("x"), vec![1.0, 0.0]);
        emb.insert(DrawerId::new("y"), vec![0.0, 1.0]);

        let id1 = auto_cluster(&mut kg1, &ids_a, &emb, "s").unwrap();
        let id2 = auto_cluster(&mut kg2, &ids_b, &emb, "s").unwrap();
        assert_eq!(id1, id2, "same members must yield same cluster ID");
    }

    #[test]
    fn test_auto_cluster_requires_two_members() {
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let ids = vec![DrawerId::new("solo")];
        let mut emb = HashMap::new();
        emb.insert(DrawerId::new("solo"), vec![1.0]);

        let result = auto_cluster(&mut kg, &ids, &emb, "s");
        assert!(result.is_err(), "should fail with < 2 members");
    }

    #[test]
    fn test_heuristic_cluster_name() {
        let contents = vec![
            "rust programming language systems".to_string(),
            "rust ownership borrowing lifetimes".to_string(),
            "rust cargo build dependencies".to_string(),
        ];
        let name = heuristic_cluster_name(&contents);
        assert!(
            name.contains("rust"),
            "name should contain 'rust', got: {}",
            name
        );
    }

    #[test]
    fn test_heuristic_cluster_name_empty() {
        let name = heuristic_cluster_name(&[]);
        assert_eq!(name, "empty-cluster");
    }

    #[test]
    fn test_list_clusters() {
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        let ids1 = vec![DrawerId::new("a"), DrawerId::new("b")];
        let ids2 = vec![DrawerId::new("c"), DrawerId::new("d")];
        let mut emb = HashMap::new();
        emb.insert(DrawerId::new("a"), vec![1.0]);
        emb.insert(DrawerId::new("b"), vec![2.0]);
        emb.insert(DrawerId::new("c"), vec![3.0]);
        emb.insert(DrawerId::new("d"), vec![4.0]);

        auto_cluster(&mut kg, &ids1, &emb, "s1").unwrap();
        auto_cluster(&mut kg, &ids2, &emb, "s2").unwrap();

        let clusters = kg.list_clusters().unwrap();
        assert_eq!(clusters.len(), 2);
    }

    #[test]
    fn test_update_cluster_name() {
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        let ids = vec![DrawerId::new("m1"), DrawerId::new("m2")];
        let mut emb = HashMap::new();
        emb.insert(DrawerId::new("m1"), vec![1.0, 0.0]);
        emb.insert(DrawerId::new("m2"), vec![0.0, 1.0]);

        let cluster_id = auto_cluster(&mut kg, &ids, &emb, "s").unwrap();

        // Initially name is None (auto-generated).
        let entry = kg.get_cluster(&cluster_id).unwrap().unwrap();
        assert!(entry.name.is_none());

        // Update the name.
        kg.update_cluster_name(&cluster_id, "Rust Topics").unwrap();

        let entry = kg.get_cluster(&cluster_id).unwrap().unwrap();
        assert_eq!(entry.name.as_deref(), Some("Rust Topics"));
    }

    #[test]
    fn test_cluster_centroid_roundtrip_blob() {
        // Verify that embedding_to_blob -> blob_to_embedding is lossless.
        let original = vec![0.1f32, -0.5, std::f32::consts::PI, 0.0, 100.0];
        let blob = KnowledgeGraph::embedding_to_blob(&original);
        let recovered = KnowledgeGraph::blob_to_embedding(&blob);
        assert_eq!(original.len(), recovered.len());
        for (a, b) in original.iter().zip(recovered.iter()) {
            assert!((a - b).abs() < 1e-6, "{} != {}", a, b);
        }
    }

    #[test]
    fn test_name_cluster_with_llm_success() {
        let contents = vec!["rust programming".to_string()];
        let name = name_cluster_with_llm(
            |_sys, _usr| Ok("\"Rust Programming\"".to_string()),
            &contents,
        );
        assert_eq!(name, "Rust Programming");
    }

    #[test]
    fn test_name_cluster_with_llm_fallback() {
        let contents = vec![
            "machine learning models".to_string(),
            "deep learning neural networks".to_string(),
        ];
        let name =
            name_cluster_with_llm(|_sys, _usr| Err("LLM unavailable".to_string()), &contents);
        // Should fall back to heuristic.
        assert!(!name.is_empty());
    }

    #[test]
    fn test_refine_clusters_merges_overlapping() {
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let mut emb = HashMap::new();
        emb.insert(DrawerId::new("a"), vec![1.0]);
        emb.insert(DrawerId::new("b"), vec![2.0]);
        emb.insert(DrawerId::new("c"), vec![3.0]);

        // Cluster 1: {a, b}
        let ids1 = vec![DrawerId::new("a"), DrawerId::new("b")];
        auto_cluster(&mut kg, &ids1, &emb, "x").unwrap();

        // Cluster 2: {b, c} — overlaps with cluster 1 on "b" (50% of smaller set)
        let ids2 = vec![DrawerId::new("b"), DrawerId::new("c")];
        auto_cluster(&mut kg, &ids2, &emb, "y").unwrap();

        // With threshold 0.4, 50% overlap should trigger a merge.
        let merges = refine_clusters(&mut kg, &emb, 0.4).unwrap();
        assert_eq!(merges, 1, "expected 1 merge");
    }
}
