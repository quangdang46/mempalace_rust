/// Cross-encoder reranker for search results.
///
/// Ported from mempalace's reranker.ts:
/// - Uses ms-marco-MiniLM-L-6-v2 ONNX model via tract-onnx
/// - Input format: "{query} [SEP] {title} {narrative}".truncate(512)
/// - Returns top_k results sorted by rerank score (descending)
/// - Feature flag: rerank-cross-encoder, lazy model loading
use serde::{Deserialize, Serialize};

/// Input for reranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankInput {
    pub id: String,
    pub title: String,
    pub content: String,
    pub initial_score: f64,
}

/// Reranked result with new score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankResult {
    pub id: String,
    pub rerank_score: f64,
    pub initial_score: f64,
}

/// Maximum input length for the reranker model.
pub const MAX_INPUT_LENGTH: usize = 512;

/// Default number of top results to return after reranking.
pub const DEFAULT_TOP_K: usize = 20;

/// Format input text for the cross-encoder model.
///
/// Format: "{query} [SEP] {title} {narrative}".truncate(512)
pub fn format_rerank_input(query: &str, title: &str, content: &str) -> String {
    let sep = " [SEP] ";
    let full = format!("{query}{sep}{title} {content}");
    if full.len() <= MAX_INPUT_LENGTH {
        full
    } else {
        // Truncate content to fit
        let prefix_len = query.len() + sep.len() + title.len() + 1;
        let content_max = MAX_INPUT_LENGTH.saturating_sub(prefix_len);
        format!(
            "{}{}{} {}",
            query,
            sep,
            title,
            &content[..content_max.min(content.len())]
        )
    }
}

/// Rerank results using provided scores.
///
/// The actual model scoring is delegated to a callback function,
/// allowing both tract-onnx (feature-gated) and mock implementations.
pub fn rerank_with_scores<F>(
    query: &str,
    inputs: &[RerankInput],
    mut score_fn: F,
    top_k: usize,
) -> Vec<RerankResult>
where
    F: FnMut(&str) -> f64,
{
    let mut results: Vec<RerankResult> = inputs
        .iter()
        .map(|input| {
            let formatted = format_rerank_input(query, &input.title, &input.content);
            let rerank_score = score_fn(&formatted);
            RerankResult {
                id: input.id.clone(),
                rerank_score,
                initial_score: input.initial_score,
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.rerank_score
            .partial_cmp(&a.rerank_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    results.truncate(top_k);
    results
}

/// Mock reranker for testing — returns scores based on ID hash.
pub fn mock_score_fn(text: &str) -> f64 {
    // Simple hash-based mock score between 0 and 1
    let hash: u64 = text.bytes().enumerate().fold(0u64, |acc, (i, b)| {
        acc.wrapping_add((b as u64).wrapping_mul((i as u64).wrapping_add(1)))
    });
    (hash % 1000) as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_rerank_input_short() {
        let input = format_rerank_input("query", "title", "content");
        assert!(input.contains("[SEP]"));
        assert!(input.starts_with("query"));
    }

    #[test]
    fn test_format_rerank_input_truncates() {
        let long_content = "x".repeat(1000);
        let input = format_rerank_input("query", "title", &long_content);
        assert!(input.len() <= MAX_INPUT_LENGTH);
        assert!(input.contains("[SEP]"));
    }

    #[test]
    fn test_rerank_sorts_by_score() {
        let inputs = vec![
            RerankInput {
                id: "a".to_string(),
                title: "low".to_string(),
                content: "".to_string(),
                initial_score: 0.9,
            },
            RerankInput {
                id: "b".to_string(),
                title: "high".to_string(),
                content: "".to_string(),
                initial_score: 0.5,
            },
        ];

        // Use deterministic scores: "a" → 0.1, "b" → 0.9
        let scores = |text: &str| -> f64 {
            if text.contains("high") {
                0.9
            } else {
                0.1
            }
        };

        let results = rerank_with_scores("query", &inputs, scores, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "b"); // Higher rerank score first
        assert!(results[0].rerank_score > results[1].rerank_score);
    }

    #[test]
    fn test_rerank_respects_top_k() {
        let inputs: Vec<RerankInput> = (0..10)
            .map(|i| RerankInput {
                id: format!("id-{i}"),
                title: format!("title-{i}"),
                content: "".to_string(),
                initial_score: 0.5,
            })
            .collect();

        let results = rerank_with_scores("query", &inputs, mock_score_fn, 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_mock_score_fn_range() {
        let score = mock_score_fn("test input");
        assert!(score >= 0.0 && score < 1.0);
    }
}
