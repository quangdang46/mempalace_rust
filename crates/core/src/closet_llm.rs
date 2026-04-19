//! closet_llm.rs — LLM-powered AAAK closet regeneration.
//!
//! Regenerates AAAK closet entries via an OpenAI-compatible API.
//! Runs in the background with exponential backoff on retries.
//!
//! Usage:
//!     mpr regenerate-closets [--wing X] [--dry-run]

use crate::config::Config;
use crate::palace_db::PalaceDb;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Default endpoint (Ollama/local inference).
const DEFAULT_ENDPOINT: &str = "http://localhost:11434/api/generate";
const MAX_RETRIES: u32 = 5;
const INITIAL_DELAY_MS: u64 = 1000;

/// AAAK prompt template for structured regeneration.
const REGENERATE_PROMPT: &str = r#"You are a memory archivist. Regenerate the following memory entry in AAAK shorthand (dialect.py format).

Context: {context}

Requirements:
- Keep ALL verbatim quotes exactly as written
- Use AAAK abbreviations for entity names
- Include source attribution
- Output ONLY the AAAK compressed form, no explanation"#;

/// Regeneration statistics.
#[derive(Debug, Clone, Serialize)]
pub struct RegenerateStats {
    pub wings_processed: usize,
    pub entries_regenerated: usize,
    pub errors: usize,
}

/// Result of a successful LLM generation.
#[derive(Debug, Clone, Deserialize)]
struct LlmResponse {
    response: String,
}

pub use self::RegenerateError as Error;

#[derive(Debug, thiserror::Error)]
pub enum RegenerateError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("LLM returned non-200: {code} — {message}")]
    NonOk { code: u16, message: String },

    #[error("LLM returned invalid JSON: {0}")]
    InvalidJson(serde_json::Error),

    #[error("LLM response empty")]
    Empty,

    #[error("Palace error: {0}")]
    Palace(String),
}

/// Regenerate closets for a wing (or all wings).
pub fn regenerate_closets(
    palace_path: Option<&Path>,
    wing: Option<&str>,
    dry_run: bool,
    endpoint: Option<&str>,
) -> anyhow::Result<RegenerateStats> {
    let config = Config::load()?;
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());
    let palace_db = PalaceDb::open(palace_path).map_err(|e| anyhow::anyhow!("{e}"))?;

    let endpoint = endpoint.unwrap_or(DEFAULT_ENDPOINT);

    println!("\n{}", "=".repeat(55));
    println!("  MemPalace Closet Regenerator");
    println!("{}", "=".repeat(55));
    println!("  Palace: {}", palace_path.display());
    println!("  Endpoint: {}", endpoint);
    if let Some(w) = wing {
        println!("  Wing: {}", w);
    }
    println!("  Mode: {}", if dry_run { "DRY RUN" } else { "LIVE" });
    println!("{}", "=".repeat(55));

    let all_entries = palace_db.get_all(wing, None, usize::MAX);
    let mut entries_by_wing: std::collections::HashMap<String, Vec<_>> =
        std::collections::HashMap::new();
    for entry in &all_entries {
        let wing_name = entry
            .metadatas
            .first()
            .and_then(|m| m.get("wing"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        entries_by_wing
            .entry(wing_name)
            .or_default()
            .push(entry.clone());
    }

    let mut total_regenerated = 0usize;
    let mut total_errors = 0usize;

    for (wing_name, entries) in &entries_by_wing {
        println!("\n  Processing wing: {}", wing_name);
        for entry in entries {
            let content = entry.documents.first().cloned().unwrap_or_default();
            if content.is_empty() {
                continue;
            }

            if dry_run {
                println!(
                    "    [DRY RUN] Would regenerate: {}",
                    &content[..content.len().min(80)]
                );
                total_regenerated += 1;
            } else {
                match regenerate_entry(&content, endpoint) {
                    Ok(regenerated) => {
                        if !regenerated.is_empty() {
                            println!(
                                "    OK: {} → {}",
                                &content[..content.len().min(40)],
                                &regenerated[..regenerated.len().min(40)]
                            );
                            total_regenerated += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("    ERROR: {}", e);
                        total_errors += 1;
                    }
                }
            }
        }
    }

    println!("\n{}", "=".repeat(55));
    if dry_run {
        println!("  [DRY RUN] No changes written.");
    }
    println!(
        "  Done. Regenerated: {}, Errors: {}",
        total_regenerated, total_errors
    );
    println!("{}", "=".repeat(55));

    Ok(RegenerateStats {
        wings_processed: entries_by_wing.len(),
        entries_regenerated: total_regenerated,
        errors: total_errors,
    })
}

fn regenerate_entry(content: &str, endpoint: &str) -> Result<String, RegenerateError> {
    let prompt = REGENERATE_PROMPT.replace("{context}", content);

    let client = reqwest::blocking::Client::new();
    let mut delay_ms = INITIAL_DELAY_MS;

    for attempt in 0..MAX_RETRIES {
        let response = client
            .post(endpoint)
            .json(&serde_json::json!({
                "model": "llama3",
                "prompt": prompt,
                "stream": false,
            }))
            .timeout(std::time::Duration::from_secs(30))
            .send();

        match response {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let parsed: LlmResponse = resp.json().map_err(RegenerateError::Http)?;
                    if parsed.response.trim().is_empty() {
                        return Err(RegenerateError::Empty);
                    }
                    return Ok(parsed.response.trim().to_string());
                }

                if status.as_u16() == 429 || status.as_u16() == 503 && attempt < MAX_RETRIES - 1 {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    delay_ms *= 2;
                    continue;
                }

                return Err(RegenerateError::NonOk {
                    code: status.as_u16(),
                    message: resp.text().unwrap_or_default(),
                });
            }
            Err(e) => {
                if attempt < MAX_RETRIES - 1 {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    delay_ms *= 2;
                    continue;
                }
                return Err(RegenerateError::Http(e));
            }
        }
    }

    Err(RegenerateError::Empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regenerate_prompt_includes_context() {
        let content = "Alice is the lead developer.";
        let prompt = REGENERATE_PROMPT.replace("{context}", content);
        assert!(prompt.contains(content));
    }

    #[test]
    fn test_regenerate_stats_debug() {
        let stats = RegenerateStats {
            wings_processed: 2,
            entries_regenerated: 10,
            errors: 1,
        };
        let debug = format!("{:?}", stats);
        assert!(debug.contains("2"));
    }
}
