//! Metrics library for LongMemEval benchmarks.
//! Implements DCG, NDCG, and Recall@K matching the Python reference exactly.

use std::collections::{HashMap, HashSet};

/// Compute DCG (Discounted Cumulative Gain) with log2(rank+1) discounting.
/// Python reference: longmemeval_bench.py:53-56
pub fn dcg(relevances: &[f64], k: usize) -> f64 {
    relevances
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, &rel)| rel / (i as f64 + 2.0).log2())
        .sum()
}

/// Compute NDCG (Normalized DCG) given ranked corpus indices and ground truth.
/// Python reference: longmemeval_bench.py:58-68
pub fn ndcg(
    rankings: &[usize],
    correct_ids: &HashSet<String>,
    corpus_ids: &[String],
    k: usize,
) -> f64 {
    let relevances: Vec<f64> = rankings[..k]
        .iter()
        .map(|idx| {
            let corpus_id = corpus_ids.get(*idx);
            match corpus_id {
                Some(cid) if correct_ids.contains(cid) => 1.0,
                _ => 0.0,
            }
        })
        .collect();

    let ideal = {
        let mut ideal = relevances.clone();
        ideal.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        ideal
    };

    let idcg = dcg(&ideal, k);
    if idcg == 0.0 {
        return 0.0;
    }

    dcg(&relevances, k) / idcg
}

/// Recall@K (any) — whether at least one correct ID appears in top K.
/// Python reference: longmemeval_bench.py:71-74
pub fn recall_at_k(
    rankings: &[usize],
    correct_ids: &HashSet<String>,
    corpus_ids: &[String],
    k: usize,
) -> f64 {
    let top_k_ids: HashSet<String> = rankings[..k]
        .iter()
        .filter_map(|idx| corpus_ids.get(*idx).cloned())
        .collect();

    if correct_ids.is_empty() {
        return 1.0; // vacuously true
    }

    if top_k_ids.iter().any(|id| correct_ids.contains(id)) {
        1.0
    } else {
        0.0
    }
}

/// Recall@K (all) — whether ALL correct IDs appear in top K.
/// Python reference: longmemeval_bench.py:76-79
pub fn recall_all_at_k(
    rankings: &[usize],
    correct_ids: &HashSet<String>,
    corpus_ids: &[String],
    k: usize,
) -> f64 {
    let top_k_ids: HashSet<String> = rankings[..k]
        .iter()
        .filter_map(|idx| corpus_ids.get(*idx).cloned())
        .collect();

    if correct_ids.is_empty() {
        return 1.0;
    }

    if correct_ids.iter().all(|id| top_k_ids.contains(id)) {
        1.0
    } else {
        0.0
    }
}

/// Combined evaluation returning recall_any, recall_all, and ndcg at each K.
/// Python reference: longmemeval_bench.py:71-80
pub fn evaluate_retrieval(
    rankings: &[usize],
    correct_ids: &HashSet<String>,
    corpus_ids: &[String],
    ks: &[usize],
) -> HashMap<String, f64> {
    let mut results = HashMap::new();

    for &k in ks {
        let recall_any = recall_at_k(rankings, correct_ids, corpus_ids, k);
        let recall_all = recall_all_at_k(rankings, correct_ids, corpus_ids, k);
        let ndcg_score = ndcg(rankings, correct_ids, corpus_ids, k);

        results.insert(format!("recall_any@{k}"), recall_any);
        results.insert(format!("recall_all@{k}"), recall_all);
        results.insert(format!("ndcg@{k}"), ndcg_score);
    }

    results
}

/// Session ID extraction from corpus ID (handles turn-level IDs).
/// Python reference: longmemeval_bench.py:83-88
pub fn session_id_from_corpus_id(corpus_id: &str) -> String {
    if let Some(pos) = corpus_id.rfind("_turn_") {
        corpus_id[..pos].to_string()
    } else {
        corpus_id.to_string()
    }
}

/// Normalize answer text for F1 scoring (used in LoCoMo benchmarks).
/// Python reference: locomo_bench.py:179-185
pub fn normalize_answer(s: &str) -> String {
    let s = s.replace(',', "");
    let re = regex::Regex::new("(?i)\\b(a|an|the|and)\\b").unwrap();
    let s = re.replace_all(&s, " ");
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Token-level F1 score between prediction and ground truth.
/// Python reference: locomo_bench.py:187-199
pub fn f1_score(prediction: &str, ground_truth: &str) -> f64 {
    let pred_normalized = normalize_answer(prediction);
    let truth_normalized = normalize_answer(ground_truth);
    let pred_tokens: HashSet<_> = pred_normalized.split_whitespace().collect();
    let truth_tokens: HashSet<_> = truth_normalized.split_whitespace().collect();

    if pred_tokens.is_empty() || truth_tokens.is_empty() {
        return 0.0;
    }

    let common = pred_tokens.intersection(&truth_tokens).count();
    if common == 0 {
        return 0.0;
    }

    let precision = common as f64 / pred_tokens.len() as f64;
    let recall = common as f64 / truth_tokens.len() as f64;

    (2.0 * precision * recall) / (precision + recall)
}

/// Aggregated benchmark metrics over a collection of questions.
#[derive(Debug, Default)]
pub struct BenchmarkMetrics {
    pub ks: Vec<usize>,
    pub recall_any: HashMap<usize, Vec<f64>>,
    pub recall_all: HashMap<usize, Vec<f64>>,
    pub ndcg: HashMap<usize, Vec<f64>>,
}

impl BenchmarkMetrics {
    pub fn new(ks: Vec<usize>) -> Self {
        let mut recall_any = HashMap::new();
        let mut recall_all = HashMap::new();
        let mut ndcg = HashMap::new();

        for &k in &ks {
            recall_any.insert(k, Vec::new());
            recall_all.insert(k, Vec::new());
            ndcg.insert(k, Vec::new());
        }

        Self {
            ks,
            recall_any,
            recall_all,
            ndcg,
        }
    }

    /// Add a single question's retrieval results.
    pub fn add(
        &mut self,
        rankings: &[usize],
        correct_ids: &HashSet<String>,
        corpus_ids: &[String],
    ) {
        for &k in &self.ks {
            self.recall_any.get_mut(&k).unwrap().push(recall_at_k(
                rankings,
                correct_ids,
                corpus_ids,
                k,
            ));

            self.recall_all.get_mut(&k).unwrap().push(recall_all_at_k(
                rankings,
                correct_ids,
                corpus_ids,
                k,
            ));

            self.ndcg
                .get_mut(&k)
                .unwrap()
                .push(ndcg(rankings, correct_ids, corpus_ids, k));
        }
    }

    /// Compute mean of all accumulated metrics.
    pub fn mean(&self) -> HashMap<String, f64> {
        let mut results = HashMap::new();

        for &k in &self.ks {
            let recall_any_vals = &self.recall_any[&k];
            let recall_all_vals = &self.recall_all[&k];
            let ndcg_vals = &self.ndcg[&k];

            if !recall_any_vals.is_empty() {
                let mean_recall_any: f64 =
                    recall_any_vals.iter().sum::<f64>() / recall_any_vals.len() as f64;
                let mean_recall_all: f64 =
                    recall_all_vals.iter().sum::<f64>() / recall_all_vals.len() as f64;
                let mean_ndcg: f64 = ndcg_vals.iter().sum::<f64>() / ndcg_vals.len() as f64;

                results.insert(format!("recall_any@{k}"), mean_recall_any);
                results.insert(format!("recall_all@{k}"), mean_recall_all);
                results.insert(format!("ndcg@{k}"), mean_ndcg);
            }
        }

        results
    }

    /// Format metrics as a CSV row string.
    pub fn to_csv_row(&self) -> String {
        let means = self.mean();
        let mut parts = Vec::new();

        for &k in &self.ks {
            if let Some(&v) = means.get(&format!("recall_any@{k}")) {
                parts.push(format!("recall_any@{k}={:.4}", v));
            }
            if let Some(&v) = means.get(&format!("recall_all@{k}")) {
                parts.push(format!("recall_all@{k}={:.4}", v));
            }
            if let Some(&v) = means.get(&format!("ndcg@{k}")) {
                parts.push(format!("ndcg@{k}={:.4}", v));
            }
        }

        parts.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dcg_known_values() {
        // DCG with single relevant doc at rank 1 = rel/log2(2) = 1/1 = 1
        let relevances = vec![1.0];
        assert!((dcg(&relevances, 1) - 1.0).abs() < 1e-6);

        // DCG with 2 relevances
        let relevances = vec![1.0, 0.5];
        let d = dcg(&relevances, 2);
        let expected = 1.0 / 2.0_f64.log2() + 0.5 / 3.0_f64.log2();
        assert!((d - expected).abs() < 1e-6);
    }

    #[test]
    fn test_ndcg_perfect_ranking() {
        // Perfect ranking: relevant docs at top
        let rankings = vec![0, 1, 2];
        let correct_ids =
            HashSet::from(["doc0".to_string(), "doc1".to_string(), "doc2".to_string()]);
        let corpus_ids = vec!["doc0".to_string(), "doc1".to_string(), "doc2".to_string()];

        let score = ndcg(&rankings, &correct_ids, &corpus_ids, 3);
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_ndcg_empty_relevance() {
        let rankings = vec![0, 1, 2];
        let correct_ids = HashSet::from(["docX".to_string()]);
        let corpus_ids = vec!["doc0".to_string(), "doc1".to_string(), "doc2".to_string()];

        let score = ndcg(&rankings, &correct_ids, &corpus_ids, 3);
        assert!((score - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_recall_at_k_hit() {
        let rankings = vec![0, 1, 2, 3, 4];
        let correct_ids = HashSet::from(["doc2".to_string()]);
        let corpus_ids = vec![
            "doc0".to_string(),
            "doc1".to_string(),
            "doc2".to_string(),
            "doc3".to_string(),
            "doc4".to_string(),
        ];

        let score = recall_at_k(&rankings, &correct_ids, &corpus_ids, 5);
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_recall_at_k_miss() {
        let rankings = vec![0, 1, 2];
        let correct_ids = HashSet::from(["docX".to_string()]);
        let corpus_ids = vec!["doc0".to_string(), "doc1".to_string(), "doc2".to_string()];

        let score = recall_at_k(&rankings, &correct_ids, &corpus_ids, 3);
        assert!((score - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_recall_all_at_k_partial() {
        // Only 2 of 3 correct docs in top-3
        let rankings = vec![0, 1, 2, 3, 4];
        let correct_ids =
            HashSet::from(["doc0".to_string(), "doc1".to_string(), "doc4".to_string()]);
        let corpus_ids = vec![
            "doc0".to_string(),
            "doc1".to_string(),
            "doc2".to_string(),
            "doc3".to_string(),
            "doc4".to_string(),
        ];

        let score = recall_all_at_k(&rankings, &correct_ids, &corpus_ids, 3);
        assert!((score - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_recall_all_at_k_all_found() {
        let rankings = vec![0, 1, 2, 3, 4];
        let correct_ids = HashSet::from(["doc0".to_string(), "doc1".to_string()]);
        let corpus_ids = vec![
            "doc0".to_string(),
            "doc1".to_string(),
            "doc2".to_string(),
            "doc3".to_string(),
            "doc4".to_string(),
        ];

        let score = recall_all_at_k(&rankings, &correct_ids, &corpus_ids, 3);
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_session_id_from_corpus_id_turn_level() {
        assert_eq!(session_id_from_corpus_id("sess_042_turn_3"), "sess_042");
        assert_eq!(session_id_from_corpus_id("sess_001"), "sess_001");
        assert_eq!(session_id_from_corpus_id("doc_xyz_turn_99"), "doc_xyz");
    }

    #[test]
    fn test_normalize_answer() {
        assert_eq!(normalize_answer("The quick, brown fox"), "quick brown fox");
        assert_eq!(
            normalize_answer("I went to the store and bought an apple"),
            "i went to store bought apple"
        );
    }

    #[test]
    fn test_f1_score_exact() {
        let score = f1_score("hello world", "hello world");
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_f1_score_partial() {
        let score = f1_score("the quick brown fox", "quick brown fox jumps");
        // normalize: "quick brown fox" vs "quick brown fox jumps"
        // common: quick, brown, fox = 3
        // pred len = 3, truth len = 4
        // precision = 3/3 = 1.0, recall = 3/4 = 0.75
        // f1 = 2 * 1.0 * 0.75 / (1.0 + 0.75) = 0.857
        assert!((score - 0.857142857).abs() < 1e-6);
    }

    #[test]
    fn test_f1_score_no_overlap() {
        let score = f1_score("hello world", "goodbye moon");
        assert!((score - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_benchmark_metrics_empty() {
        let metrics = BenchmarkMetrics::new(vec![5, 10]);
        let means = metrics.mean();
        assert!(means.is_empty());
    }

    #[test]
    fn test_benchmark_metrics_aggregation() {
        let mut metrics = BenchmarkMetrics::new(vec![3, 5]);

        let rankings = vec![0, 1, 2, 3, 4];
        let correct_ids = HashSet::from(["doc2".to_string()]);
        let corpus_ids = vec![
            "doc0".to_string(),
            "doc1".to_string(),
            "doc2".to_string(),
            "doc3".to_string(),
            "doc4".to_string(),
        ];

        metrics.add(&rankings, &correct_ids, &corpus_ids);

        let means = metrics.mean();
        // At K=3: doc2 is at index 2 (rank 2) → in top-3 → recall=1.0
        assert!((means["recall_any@3"] - 1.0).abs() < 1e-6);
        // At K=5: doc2 is at index 2 (rank 2) → in top-5 → recall=1.0
        assert!((means["recall_any@5"] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_evaluate_retrieval_all_ks() {
        let rankings = vec![0, 1, 2, 3, 4];
        let correct_ids = HashSet::from(["doc0".to_string(), "doc2".to_string()]);
        let corpus_ids = vec![
            "doc0".to_string(),
            "doc1".to_string(),
            "doc2".to_string(),
            "doc3".to_string(),
            "doc4".to_string(),
        ];

        let results = evaluate_retrieval(&rankings, &correct_ids, &corpus_ids, &[3, 5]);

        assert!((results["recall_any@3"] - 1.0).abs() < 1e-6);
        assert!((results["recall_any@5"] - 1.0).abs() < 1e-6);
        assert!((results["recall_all@3"] - 1.0).abs() < 1e-6); // both of 2 in top-3
        assert!((results["recall_all@5"] - 1.0).abs() < 1e-6); // both in top-5
    }
}
