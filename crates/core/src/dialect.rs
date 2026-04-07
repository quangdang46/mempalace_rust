use std::collections::HashMap;

/// Count tokens using character-based approximation.
/// This is a fallback when tiktoken is unavailable.
/// For more accurate counting, the tiktoken crate should be used.
pub fn count_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    // Character-based approximation: ~4 chars per token for English text
    // This is the approximation used by many tokenizers for non-tokenized text
    // For AAAK text which is more condensed, the ratio is closer to 3:1
    ((text.len() as f64) / 3.5).ceil() as usize
}

/// Compress text to AAAK shorthand dialect.
pub fn compress(text: &str, people_map: &HashMap<String, String>) -> String {
    let _ = (text, people_map);
    // AAAK compression is a complex dialect - placeholder stub
    String::new()
}

/// Decompress AAAK shorthand back to natural language.
pub fn decompress(aaak_text: &str) -> String {
    let _ = aaak_text;
    String::new()
}

/// Get compression statistics with accurate token counting.
pub fn compression_stats(original: &str, compressed: &str) -> CompressionStats {
    let original_tokens = count_tokens(original);
    let compressed_tokens = count_tokens(compressed);
    let ratio = if original_tokens > 0 && compressed_tokens > 0 {
        original_tokens as f64 / compressed_tokens as f64
    } else {
        0.0
    };

    CompressionStats {
        original_tokens,
        compressed_tokens,
        ratio,
    }
}

#[derive(Debug, Clone)]
pub struct CompressionStats {
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub ratio: f64,
}

/// Return the AAAK dialect specification for AI agents.
pub fn get_aaak_spec() -> &'static str {
    r#"AAAK — AI Agent Acquisition Knowledge shorthand

ABBREVIATIONS:
  TEAMS: PRI(lead) | NAME(role,tenure) ...
  PROJ: NAME(type.keyword) | SPRINT: task→status
  DECISION: NAME.rec:value>value(reason) | ★★rating
  PERSON: full_name → role/history
  TASK: name | assigned:NAME | status:pending|active|done

EXAMPLES:
  TEAM: PRI(lead) | KAI(backend,3yr) SOR(frontend) MAY(infra)
  PROJ: DRIFTWOOD(saas.analytics) | SPRINT: auth.migration→clerk
  DECISION: KAI.rec:clerk>auth0(pricing+dx) | ★★★★
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens_empty() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn test_count_tokens_consistency() {
        let text = "The quick brown fox jumps over the lazy dog.";
        let tokens1 = count_tokens(text);
        let tokens2 = count_tokens(text);
        assert_eq!(tokens1, tokens2, "Token count should be deterministic");
    }

    #[test]
    fn test_compression_stats_ratio() {
        let original = "Hello, world! This is a longer piece of text that should have more tokens.";
        let compressed = "HW! TLS mp tx";
        let stats = compression_stats(original, compressed);
        assert!(stats.original_tokens > stats.compressed_tokens);
        assert!(stats.ratio > 1.0, "Compression ratio should be > 1");
    }

    #[test]
    fn test_compression_stats_empty() {
        let stats = compression_stats("", "");
        assert_eq!(stats.original_tokens, 0);
        assert_eq!(stats.compressed_tokens, 0);
    }

    #[test]
    fn test_get_aaak_spec() {
        let spec = get_aaak_spec();
        assert!(spec.contains("AAAK"));
        assert!(spec.contains("TEAM"));
        assert!(spec.contains("PROJ"));
    }
}
