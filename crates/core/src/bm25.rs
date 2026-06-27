//! BM25 ranking algorithm for search result reranking.
//!
//! BM25 (Best Matching 25) is a ranking function used by search engines
//! to estimate the relevance of documents to a given search query.
//!
//! This implementation uses the standard BM25 formula with configurable parameters.

#![doc(hidden)]

use std::collections::HashMap;

use crate::search::cjk_segmenter::segment_cjk;

/// BM25 ranking parameters.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
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

    /// Tokenize text into terms (whitespace-based with CJK-aware segmentation).
    ///
    /// When the text contains CJK characters (Han, Hiragana, Katakana, Hangul),
    /// the tokenizer also runs the CJK script segmenter which splits CJK script
    /// runs at script boundaries (e.g. "中文hello" → ["中文", "hello"]).
    /// This improves BM25 recall on mixed-script queries common in multilingual
    /// codebases and documentation.
    fn tokenize(text: &str) -> Vec<String> {
        let lower = text.to_lowercase();
        let mut tokens: Vec<String> = Vec::new();

        // First pass: whitespace splitting (covers Latin/Other/whitespace-separated text)
        for word in lower.split_whitespace() {
            // If the word contains CJK characters, use the CJK segmenter
            // so that Han/Hiragana/Katakana/Hangul runs produce distinct
            // tokens instead of being treated as one opaque string.
            if crate::search::cjk_segmenter::has_cjk(word) {
                tokens.extend(segment_cjk(word));
            } else {
                tokens.push(word.to_string());
            }
        }

        tokens
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

    /// Persist BM25 index to a JSON file for faster startup on subsequent runs.
    ///
    /// Saves doc_freqs, total_docs, avg_doc_length, and params.
    /// Returns error if serialization fails or file cannot be written.
    pub fn persist_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        #[derive(serde::Serialize)]
        struct PersistedBm25 {
            params: Bm25Params,
            doc_freqs: Vec<(String, usize)>,
            total_docs: usize,
            avg_doc_length: f64,
        }

        let persisted = PersistedBm25 {
            params: self.params.clone(),
            doc_freqs: self
                .doc_freqs
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            total_docs: self.total_docs,
            avg_doc_length: self.avg_doc_length,
        };

        let json = serde_json::to_string_pretty(&persisted)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    /// Load BM25 index from a JSON file.
    ///
    /// Returns `Ok(None)` if the file doesn't exist (index needs building).
    /// Returns `Ok(Some(scorer))` if loaded successfully.
    /// Returns `Err` on file read/parse errors.
    pub fn load_from_file(path: &std::path::Path) -> std::io::Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }

        #[derive(serde::Deserialize)]
        struct PersistedBm25 {
            params: Bm25Params,
            doc_freqs: Vec<(String, usize)>,
            total_docs: usize,
            avg_doc_length: f64,
        }

        let content = std::fs::read_to_string(path)?;
        let persisted: PersistedBm25 = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let doc_freqs: HashMap<String, usize> = persisted.doc_freqs.into_iter().collect();

        Ok(Some(Self {
            params: persisted.params,
            doc_freqs,
            total_docs: persisted.total_docs,
            avg_doc_length: persisted.avg_doc_length,
        }))
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
