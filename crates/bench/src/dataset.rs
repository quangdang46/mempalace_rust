//! Dataset loader for LongMemEval benchmark data.
//!
//! Downloads from HuggingFace and parses the JSON format used by
//! https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// HuggingFace dataset identifier for LongMemEval cleaned data.
pub const LOMEMEVAL_DATASET: &str = "xiaowu0162/longmemeval-cleaned";
pub const LOMEMEVAL_FILE: &str = "longmemeval_s_cleaned.json";

/// A single turn in a conversation session.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Turn {
    pub role: String,
    pub content: String,
}

/// A single benchmark entry (question + haystack).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct BenchmarkEntry {
    pub question_id: String,
    pub question: String,
    #[serde(rename = "question_type")]
    pub question_type: String,
    #[serde(rename = "question_date", default)]
    pub question_date: Option<String>,
    pub answer: serde_json::Value,
    #[serde(rename = "answer_session_ids")]
    pub answer_session_ids: Vec<String>,
    #[serde(rename = "haystack_session_ids")]
    pub haystack_session_ids: Vec<String>,
    #[serde(rename = "haystack_dates")]
    pub haystack_dates: Vec<String>,
    #[serde(rename = "haystack_sessions")]
    pub haystack_sessions: Vec<Vec<Turn>>,
}

/// Granularity for corpus construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Granularity {
    /// One document per session (all user turns joined).
    Session,
    /// One document per user turn.
    Turn,
}

/// Load dataset from local JSON file.
pub fn load_from_file(path: &Path) -> Result<Vec<BenchmarkEntry>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {:?}", path))?;
    let entries: Vec<BenchmarkEntry> = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {:?}", path))?;
    Ok(entries)
}

/// Download dataset from HuggingFace to local cache.
///
/// Returns the path to the downloaded file.
pub async fn download_from_huggingface(cache_dir: &Path) -> Result<PathBuf> {
    let cache_dir = cache_dir.to_path_buf();

    let file_path = cache_dir.join(LOMEMEVAL_FILE);

    // Check if already downloaded
    if file_path.exists() {
        return Ok(file_path);
    }

    // Build HuggingFace raw URL
    let url = format!(
        "https://huggingface.co/datasets/{}/resolve/main/{}",
        LOMEMEVAL_DATASET, LOMEMEVAL_FILE
    );

    println!("Downloading {} ...", url);

    // Use reqwest async (needs runtime with async features)
    let response = reqwest::get(&url)
        .await
        .context("Failed to send HTTP request to HuggingFace")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "HuggingFace download failed with status {}: {}",
            response.status(),
            url
        );
    }

    let bytes = response
        .bytes()
        .await
        .context("Failed to read response bytes")?;

    // Use spawn_blocking for filesystem operations
    let file_path_clone = file_path.clone();
    let cache_dir_clone = cache_dir.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;

        std::fs::create_dir_all(&cache_dir_clone).context("Failed to create cache directory")?;

        let mut file =
            std::fs::File::create(&file_path_clone).context("Failed to create local file")?;
        file.write_all(&bytes)
            .context("Failed to write downloaded data")?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Filesystem task failed")??;

    println!("Cached at {:?}", file_path);

    Ok(file_path)
}

/// Build a session-level corpus from a benchmark entry.
///
/// Returns (documents, corpus_ids) — one document per session.
/// Document text = all user turns joined with newlines.
pub fn build_session_corpus(entry: &BenchmarkEntry) -> (Vec<String>, Vec<String>) {
    let mut documents = Vec::new();
    let mut corpus_ids = Vec::new();

    for (i, session) in entry.haystack_sessions.iter().enumerate() {
        let session_id = entry
            .haystack_session_ids
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("sess_{:03}", i));

        // Join all USER turns only (matching Python behavior)
        let user_content: Vec<&str> = session
            .iter()
            .filter(|turn| turn.role == "user")
            .map(|turn| turn.content.as_str())
            .collect();

        let document = user_content.join("\n");

        if !document.is_empty() {
            documents.push(document);
            corpus_ids.push(session_id);
        }
    }

    (documents, corpus_ids)
}

/// Build a turn-level corpus from a benchmark entry.
///
/// Returns (documents, corpus_ids) — one document per user turn.
pub fn build_turn_corpus(entry: &BenchmarkEntry) -> (Vec<String>, Vec<String>) {
    let mut documents = Vec::new();
    let mut corpus_ids = Vec::new();

    for (i, session) in entry.haystack_sessions.iter().enumerate() {
        let session_id = entry
            .haystack_session_ids
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("sess_{:03}", i));

        for (turn_idx, turn) in session.iter().enumerate().filter(|(_, t)| t.role == "user") {
            let corpus_id = format!("{}_turn_{}", session_id, turn_idx);
            documents.push(turn.content.clone());
            corpus_ids.push(corpus_id);
        }
    }

    (documents, corpus_ids)
}

// mr-d4k3: multilingual bench datasets (DE/FR/HI/IT/KO/RU).
//
// We don't ship 100MB multilingual haystacks in the repo; instead we
// provide a small canonical sample per language. Callers wire these
// into the same BenchmarkConfig used for the English LongMemEval run.
// The samples are designed to exercise the multilingual embedder path
// (paraphrase-multilingual-MiniLM-L12-v2) and verify that
// case-folding, tokenisation, and vector-space geometry all behave
// for non-Latin scripts.
pub const SUPPORTED_LANGUAGES: &[&str] = &["de", "fr", "hi", "it", "ko", "ru"];

/// A minimal multilingual sample: a question and a set of haystack
/// documents. Modeled on the LongMemEval entry shape so callers can
/// trivially convert to `BenchmarkEntry`.
#[derive(Debug, Clone)]
pub struct MultilingualSample {
    pub language: &'static str,
    pub question: &'static str,
    pub expected_answer: &'static str,
    pub haystack: Vec<(&'static str, &'static str)>, // (id, content)
}

fn de_sample() -> MultilingualSample {
    MultilingualSample {
        language: "de",
        question: "Welche Programmiersprache verwende ich am liebsten?",
        expected_answer: "Rust",
        haystack: vec![
            (
                "de_s0",
                "Ich arbeite seit drei Jahren mit Rust an meinem Memory-Palace-Projekt.",
            ),
            (
                "de_s1",
                "Heute habe ich ein neues Modul in Rust geschrieben und es funktioniert großartig.",
            ),
            (
                "de_s2",
                "Die Performance von Rust ist beeindruckend im Vergleich zu Python.",
            ),
        ],
    }
}

fn fr_sample() -> MultilingualSample {
    MultilingualSample {
        language: "fr",
        question: "Quel est mon langage de programmation préféré?",
        expected_answer: "Rust",
        haystack: vec![
            (
                "fr_s0",
                "Je travaille sur un projet Rust depuis six mois et j'adore ça.",
            ),
            (
                "fr_s1",
                "Aujourd'hui j'ai compilé un nouveau module Rust sans aucun warning.",
            ),
            ("fr_s2", "La gestion de la mémoire en Rust est élégante."),
        ],
    }
}

fn hi_sample() -> MultilingualSample {
    MultilingualSample {
        language: "hi",
        question: "मेरी पसंदीदा प्रोग्रामिंग भाषा कौन सी है?",
        expected_answer: "Rust",
        haystack: vec![
            (
                "hi_s0",
                "मैं पिछले दो साल से रस्ट प्रोग्रामिंग भाषा का उपयोग कर रहा हूँ।",
            ),
            ("hi_s1", "आज मैंने रस्ट में एक नया मॉड्यूल बनाया है।"),
            ("hi_s2", "रस्ट की मेमोरी प्रबंधन प्रणाली बहुत अच्छी है।"),
        ],
    }
}

fn it_sample() -> MultilingualSample {
    MultilingualSample {
        language: "it",
        question: "Qual è il mio linguaggio di programmazione preferito?",
        expected_answer: "Rust",
        haystack: vec![
            ("it_s0", "Lavoro con Rust da due anni ed è fantastico."),
            (
                "it_s1",
                "Oggi ho scritto un nuovo modulo in Rust e funziona benissimo.",
            ),
            (
                "it_s2",
                "La gestione della memoria in Rust è elegante e sicura.",
            ),
        ],
    }
}

fn ko_sample() -> MultilingualSample {
    MultilingualSample {
        language: "ko",
        question: "내가 가장 좋아하는 프로그래밍 언어는 무엇인가?",
        expected_answer: "Rust",
        haystack: vec![
            ("ko_s0", "저는 2년 동안 Rust로 작업해 왔습니다."),
            ("ko_s1", "오늘 Rust로 새 모듈을 만들었습니다."),
            ("ko_s2", "Rust의 메모리 관리는 매우 안전합니다."),
        ],
    }
}

fn ru_sample() -> MultilingualSample {
    MultilingualSample {
        language: "ru",
        question: "Какой мой любимый язык программирования?",
        expected_answer: "Rust",
        haystack: vec![
            (
                "ru_s0",
                "Я работаю с Rust уже два года и мне нравится этот язык.",
            ),
            (
                "ru_s1",
                "Сегодня я написал новый модуль на Rust, и он работает отлично.",
            ),
            ("ru_s2", "Управление памятью в Rust элегантно и безопасно."),
        ],
    }
}

/// Look up the multilingual sample for a language code. Returns
/// `None` for unsupported codes.
pub fn multilingual_sample(lang: &str) -> Option<MultilingualSample> {
    match lang {
        "de" => Some(de_sample()),
        "fr" => Some(fr_sample()),
        "hi" => Some(hi_sample()),
        "it" => Some(it_sample()),
        "ko" => Some(ko_sample()),
        "ru" => Some(ru_sample()),
        _ => None,
    }
}

/// True if `lang` is a supported multilingual sample code.
pub fn is_supported_language(lang: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&lang)
}

/// Convert a multilingual sample into a single-entry `BenchmarkEntry`
/// so the existing `run_benchmark` pipeline can score it without
/// modification. The single haystack session holds all sample turns.
pub fn sample_to_entry(sample: &MultilingualSample) -> BenchmarkEntry {
    let haystack_session_ids: Vec<String> = sample
        .haystack
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    let haystack_sessions: Vec<Vec<Turn>> = vec![sample
        .haystack
        .iter()
        .map(|(_, content)| Turn {
            role: "user".to_string(),
            content: content.to_string(),
        })
        .collect()];
    BenchmarkEntry {
        question_id: format!("{}-q1", sample.language),
        question: sample.question.to_string(),
        question_type: "multilingual-preference".to_string(),
        question_date: None,
        answer: serde_json::json!(sample.expected_answer),
        answer_session_ids: vec![haystack_session_ids[0].clone()],
        haystack_session_ids,
        haystack_dates: vec!["1970/01/01 (Thu) 00:00".to_string()],
        haystack_sessions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> BenchmarkEntry {
        let haystack_sessions = vec![
            vec![
                Turn {
                    role: "user".to_string(),
                    content: "I worked on the auth migration today".to_string(),
                },
                Turn {
                    role: "assistant".to_string(),
                    content: "How did it go?".to_string(),
                },
            ],
            vec![Turn {
                role: "user".to_string(),
                content: "I still remember the happy high school experiences".to_string(),
            }],
        ];

        BenchmarkEntry {
            question_id: "test-001".to_string(),
            question: "high school reunion".to_string(),
            question_type: "single-session-preference".to_string(),
            question_date: Some("2023/08/15 (Tue) 14:30".to_string()),
            answer: serde_json::json!("You were in debate team."),
            answer_session_ids: vec!["sess_001".to_string()],
            haystack_session_ids: vec!["sess_000".to_string(), "sess_001".to_string()],
            haystack_dates: vec![
                "2023/01/10 (Tue) 09:00".to_string(),
                "2023/06/22 (Thu) 16:45".to_string(),
            ],
            haystack_sessions,
        }
    }

    #[test]
    fn test_build_session_corpus() {
        let entry = sample_entry();
        let (docs, ids) = build_session_corpus(&entry);

        assert_eq!(docs.len(), 2);
        assert_eq!(ids, vec!["sess_000", "sess_001"]);
        assert_eq!(docs[0], "I worked on the auth migration today");
        assert_eq!(
            docs[1],
            "I still remember the happy high school experiences"
        );
    }

    #[test]
    fn test_build_session_corpus_skips_assistant() {
        let entry = sample_entry();
        let (docs, _) = build_session_corpus(&entry);

        // First session has assistant turn but we only take user turns
        assert!(docs[0].contains("auth migration"));
        assert!(!docs[0].contains("How did it go"));
    }

    #[test]
    fn test_build_turn_corpus() {
        let entry = sample_entry();
        let (docs, ids) = build_turn_corpus(&entry);

        // 1 user turn in first session + 1 in second = 2
        assert_eq!(docs.len(), 2);
        assert_eq!(ids[0], "sess_000_turn_0");
        assert_eq!(ids[1], "sess_001_turn_0");
    }

    #[test]
    fn test_load_from_file_not_found() {
        let result = load_from_file(Path::new("/nonexistent/file.json"));
        assert!(result.is_err());
    }

    // mr-d4k3: every supported language must yield a non-empty
    // sample, and the sample must convert to a BenchmarkEntry with
    // the same question/answer shape that the English pipeline uses.
    #[test]
    fn test_multilingual_samples_round_trip() {
        for &lang in SUPPORTED_LANGUAGES {
            let sample =
                multilingual_sample(lang).unwrap_or_else(|| panic!("missing sample for {lang}"));
            assert!(!sample.question.is_empty());
            assert!(!sample.expected_answer.is_empty());
            assert!(!sample.haystack.is_empty());
            let entry = sample_to_entry(&sample);
            assert_eq!(entry.question, sample.question);
            assert_eq!(entry.question_id, format!("{lang}-q1"));
            assert_eq!(entry.haystack_session_ids.len(), sample.haystack.len());
        }
        // Unsupported language returns None.
        assert!(multilingual_sample("xx").is_none());
        assert!(!is_supported_language("xx"));
        assert!(is_supported_language("de"));
    }
}
