//! BM25 ranking algorithm for search result reranking.
//!
//! BM25 (Best Matching 25) is a ranking function used by search engines
//! to estimate the relevance of documents to a given search query.
//!
//! This implementation uses the standard BM25 formula with configurable parameters.

use std::collections::HashMap;

/// BM25 ranking parameters.
#[derive(Debug, Clone)]
pub struct Bm25Params {
    /// k1 parameter - term frequency saturation (default: 1.2)
    pub k1: f64,
    /// b parameter - length normalization (default: 0.75)
    pub b: f64,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

/// BM25 scorer for reranking search results.
pub struct Bm25Scorer {
    params: Bm25Params,
    // Document frequencies: how many documents contain each term
    doc_freqs: HashMap<String, usize>,
    // Total number of documents
    total_docs: usize,
    // Average document length
    avg_doc_length: f64,
}

impl Bm25Scorer {
    /// Create a new BM25 scorer from a corpus of documents.
    pub fn new(documents: &[String], params: Bm25Params) -> Self {
        let mut doc_freqs: HashMap<String, usize> = HashMap::new();
        let mut total_length = 0.0;

        for doc in documents {
            let terms = Self::tokenize(doc);
            let unique_terms: std::collections::HashSet<&str> =
                terms.iter().map(|s| s.as_str()).collect();

            for term in unique_terms {
                *doc_freqs.entry(term.to_string()).or_insert(0) += 1;
            }

            total_length += terms.len() as f64;
        }

        let total_docs = documents.len();
        let avg_doc_length = if total_docs > 0 {
            total_length / total_docs as f64
        } else {
            0.0
        };

        Self {
            params,
            doc_freqs,
            total_docs,
            avg_doc_length,
        }
    }

    /// Tokenize text into terms (simple whitespace-based tokenization).
    fn tokenize(text: &str) -> Vec<String> {
        text.to_lowercase()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    }

    /// Calculate BM25 score for a document given a query.
    pub fn score(&self, document: &str, query: &str) -> f64 {
        if self.total_docs == 0 || self.avg_doc_length == 0.0 {
            return 0.0;
        }

        let doc_terms = Self::tokenize(document);
        let query_terms = Self::tokenize(query);
        let doc_length = doc_terms.len() as f64;

        // Count term frequencies in document
        let mut term_freqs: HashMap<&str, usize> = HashMap::new();
        for term in &doc_terms {
            *term_freqs.entry(term).or_insert(0) += 1;
        }

        let mut score = 0.0;

        for term in &query_terms {
            let tf = *term_freqs.get(term.as_str()).unwrap_or(&0) as f64;
            let df = *self.doc_freqs.get(term).unwrap_or(&0) as f64;

            if df == 0.0 {
                continue;
            }

            // IDF component
            let idf = ((self.total_docs as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();

            // TF normalization
            let tf_normalized = (tf * (self.params.k1 + 1.0))
                / (tf
                    + self.params.k1
                        * (1.0 - self.params.b
                            + self.params.b * (doc_length / self.avg_doc_length)));

            score += idf * tf_normalized;
        }

        score
    }

    /// Rerank a list of documents based on a query.
    pub fn rerank(&self, documents: &[String], query: &str) -> Vec<(usize, f64)> {
        let mut scored: Vec<(usize, f64)> = documents
            .iter()
            .enumerate()
            .map(|(idx, doc)| (idx, self.score(doc, query)))
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bm25_basic() {
        let documents = vec![
            "the quick brown fox jumps over the lazy dog".to_string(),
            "the quick brown dog jumps over the lazy fox".to_string(),
            "the lazy dog sleeps".to_string(),
        ];

        let scorer = Bm25Scorer::new(&documents, Bm25Params::default());

        // Query should match first document best
        let query = "quick fox";
        let scores = scorer.rerank(&documents, query);

        assert!(!scores.is_empty());
        // First document should have highest score (contains both "quick" and "fox")
        assert_eq!(scores[0].0, 0);
    }

    #[test]
    fn test_bm25_tokenization() {
        let text = "The Quick Brown FOX";
        let terms = Bm25Scorer::tokenize(text);

        assert_eq!(terms, vec!["the", "quick", "brown", "fox"]);
    }

    #[test]
    fn test_bm25_empty_corpus() {
        let documents: Vec<String> = vec![];
        let scorer = Bm25Scorer::new(&documents, Bm25Params::default());

        let score = scorer.score("test document", "test query");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_bm25_no_match() {
        let documents = vec!["the quick brown fox".to_string()];

        let scorer = Bm25Scorer::new(&documents, Bm25Params::default());

        // Query with no matching terms
        let score = scorer.score("the quick brown fox", "xyz abc");
        assert_eq!(score, 0.0);
    }
}
