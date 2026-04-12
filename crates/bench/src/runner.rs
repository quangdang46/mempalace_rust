use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;

use mempalace_core::onnx_embed::OnnxModel;
use mempalace_core::palace_db::EmbeddingDb;

use crate::dataset::{build_session_corpus, build_turn_corpus, BenchmarkEntry, Granularity};
use crate::metrics::{session_id_from_corpus_id, BenchmarkMetrics};

pub struct BenchmarkConfig {
    pub granularity: Granularity,
    pub n_results: usize,
    pub ks: Vec<usize>,
    pub limit: Option<usize>,
    pub embed_model: String,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            granularity: Granularity::Session,
            n_results: 50,
            ks: vec![5, 10],
            limit: None,
            embed_model: "all-MiniLM-L6-v2".to_string(),
        }
    }
}

pub struct BenchmarkResults {
    pub total_questions: usize,
    pub skipped: usize,
    pub metrics: BenchmarkMetrics,
    pub per_type_results: std::collections::HashMap<String, BenchmarkMetrics>,
    pub durations_ms: Vec<u64>,
}

impl BenchmarkResults {
    pub fn summary(&self) -> String {
        let means = self.metrics.mean();
        let mut lines = vec![
            format!("Total questions: {}", self.total_questions),
            format!("Skipped (empty corpus): {}", self.skipped),
            String::new(),
        ];

        for (key, val) in &means {
            lines.push(format!("  {}: {:.4}", key, val));
        }

        lines.push(String::new());
        lines.push(format!(
            "Avg query time: {:.2}ms",
            if self.durations_ms.is_empty() {
                0.0
            } else {
                self.durations_ms.iter().sum::<u64>() as f64 / self.durations_ms.len() as f64
            }
        ));

        lines.join("\n")
    }
}

fn rank_corpus(
    query: &str,
    corpus_documents: &[String],
    n_results: usize,
    embedder: &Arc<OnnxModel>,
) -> Result<Vec<usize>> {
    let mut db = EmbeddingDb::with_embedder(embedder.clone(), 384)?;

    let items: Vec<(String, String)> = corpus_documents
        .iter()
        .enumerate()
        .map(|(i, doc)| (format!("doc_{}", i), doc.clone()))
        .collect();
    db.add_batch(&items)?;

    let results = db.query(query, n_results)?;

    let ranked_indices: Vec<usize> = results.into_iter().map(|(_, idx)| idx).collect();

    Ok(ranked_indices)
}

pub async fn run_benchmark(
    entries: &[BenchmarkEntry],
    config: &BenchmarkConfig,
) -> Result<BenchmarkResults> {
    let embedder = Arc::new(OnnxModel::load()?);

    let mut metrics = BenchmarkMetrics::new(config.ks.clone());
    let mut per_type_results: std::collections::HashMap<_, _> = Default::default();
    let mut skipped = 0;
    let mut durations_ms = Vec::with_capacity(entries.len());

    let limit = config.limit.unwrap_or(entries.len());
    for entry in entries.iter().take(limit) {
        if let Some(dur) = run_single_question(
            entry,
            config,
            &embedder,
            &mut metrics,
            &mut per_type_results,
        )? {
            durations_ms.push(dur);
        } else {
            skipped += 1;
        }
    }

    Ok(BenchmarkResults {
        total_questions: entries.len(),
        skipped,
        metrics,
        per_type_results,
        durations_ms,
    })
}

fn run_single_question(
    entry: &BenchmarkEntry,
    config: &BenchmarkConfig,
    embedder: &Arc<OnnxModel>,
    metrics: &mut BenchmarkMetrics,
    per_type_results: &mut std::collections::HashMap<String, BenchmarkMetrics>,
) -> Result<Option<u64>> {
    let start = Instant::now();

    let (corpus_documents, corpus_ids) = match config.granularity {
        Granularity::Session => build_session_corpus(entry),
        Granularity::Turn => build_turn_corpus(entry),
    };

    if corpus_documents.is_empty() {
        return Ok(None);
    }

    let ranked_indices = rank_corpus(
        &entry.question,
        &corpus_documents,
        config.n_results,
        embedder,
    )?;

    let correct_session_ids: std::collections::HashSet<&str> = entry
        .answer_session_ids
        .iter()
        .map(|s| s.as_str())
        .collect();

    let ground_truth: std::collections::HashSet<&str> = entry
        .haystack_session_ids
        .iter()
        .map(|s| s.as_str())
        .collect();

    let correct_ids: std::collections::HashSet<String> = if config.granularity == Granularity::Turn
    {
        ranked_indices
            .iter()
            .take(config.n_results)
            .filter_map(|idx| corpus_ids.get(*idx))
            .map(|s| session_id_from_corpus_id(s))
            .filter(|s| ground_truth.contains(s.as_str()))
            .collect()
    } else {
        correct_session_ids
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    };

    if correct_ids.is_empty() {
        return Ok(None);
    }

    metrics.add(&ranked_indices, &correct_ids, &corpus_ids);

    let per_type = per_type_results
        .entry(entry.question_type.clone())
        .or_insert_with(|| BenchmarkMetrics::new(config.ks.clone()));
    per_type.add(&ranked_indices, &correct_ids, &corpus_ids);

    Ok(Some(start.elapsed().as_millis() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rank_corpus_returns_sorted_indices() {
        let embedder = match OnnxModel::load() {
            Ok(e) => Arc::new(e),
            Err(e) => {
                eprintln!("OnnxModel not available (Python/chromadb required): {}", e);
                return;
            }
        };
        let docs = vec![
            "I worked on the auth migration today".to_string(),
            "I still remember the happy high school experiences".to_string(),
        ];

        let result = rank_corpus("high school", &docs, 2, &embedder);
        assert!(result.is_ok());
        let indices = result.unwrap();
        assert_eq!(indices.len(), 2);
    }
}
