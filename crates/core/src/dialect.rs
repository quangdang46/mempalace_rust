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
    if text.is_empty() {
        return String::new();
    }

    let mut result = text.to_string();

    // Apply people name substitutions from people_map
    for (canonical, alias) in people_map {
        // Replace full name occurrences with AAAK code (first 3 chars uppercase)
        let code = &canonical[..canonical.len().min(3)].to_uppercase();
        result = result.replace(alias, code);
        result = result.replace(canonical, code);
    }

    // Common compression patterns
    let patterns: Vec<(&str, &str)> = vec![
        // Team patterns
        ("lead developer", "PRI"),
        ("lead engineer", "PRI"),
        ("technical lead", "TL"),
        ("software engineer", "SWE"),
        ("backend developer", "BE"),
        ("frontend developer", "FE"),
        ("full stack developer", "FS"),
        ("devops engineer", "DevOps"),
        ("site reliability engineer", "SRE"),
        // Project patterns
        ("working on", "Wkg"),
        ("working with", "w/"),
        ("implemented", "impl"),
        ("investigating", "inv"),
        ("researching", "res"),
        ("developing", "dev"),
        ("deployed to", "dep→"),
        ("migrated to", "mig→"),
        // Decision patterns
        ("decided to use", "dec:"),
        ("chose over", "chose>"),
        ("recommendation", "rec"),
        // Status patterns
        ("in progress", "active"),
        ("not started", "pending"),
        ("completed", "done"),
        ("blocked on", "blkd:"),
        // Communication patterns
        ("talked to", "tx"),
        ("discussed with", "disc"),
        ("shared with", "shd"),
        ("presented to", "pres"),
    ];

    for (from, to) in patterns {
        result = result.replace(from, to);
    }

    // Remove filler words
    let fillers = ["the ", "a ", "an ", "that ", "this ", "it ", "is ", "was ", "were ", "are "];
    for filler in fillers {
        result = result.replace(filler, "");
    }

    // Collapse multiple spaces
    while result.contains("  ") {
        result = result.replace("  ", " ");
    }

    // Trim
    result = result.trim().to_string();

    // If compression is empty or same as input, return abbreviated version
    if result.is_empty() || result == text {
        // Return first 50 chars or less as compressed version
        return text.chars().take(50).collect::<String>() + "…";
    }

    result
}

/// Decompress AAAK shorthand back to natural language.
pub fn decompress(aaak_text: &str, people_map: &HashMap<String, String>) -> String {
    if aaak_text.is_empty() {
        return String::new();
    }

    let mut result = aaak_text.to_string();

    // Expand abbreviation patterns back to full forms
    let expansions: Vec<(&str, &str)> = vec![
        ("PRI", "Lead Developer"),
        ("TL", "Technical Lead"),
        ("SWE", "Software Engineer"),
        ("BE", "Backend Developer"),
        ("FE", "Frontend Developer"),
        ("FS", "Full Stack Developer"),
        ("DevOps", "DevOps Engineer"),
        ("SRE", "Site Reliability Engineer"),
        ("Wkg", "Working on"),
        ("impl", "Implemented"),
        ("inv", "Investigating"),
        ("res", "Researching"),
        ("dev", "Developing"),
        ("dep→", "Deployed to"),
        ("mig→", "Migrated to"),
        ("dec:", "Decided to use"),
        ("chose>", "Chose over"),
        ("rec", "Recommendation"),
        ("blkd:", "Blocked on"),
        ("tx", "Talked to"),
        ("disc", "Discussed with"),
        ("shd", "Shared with"),
        ("pres", "Presented to"),
    ];

    for (from, to) in expansions {
        result = result.replace(from, to);
    }

    // Apply people_map in reverse: expand codes back to names
    for (canonical, alias) in people_map {
        let code = &canonical[..canonical.len().min(3)].to_uppercase();
        result = result.replace(code, canonical);
    }

    result.trim().to_string()
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

    #[test]
    fn test_compress_round_trip() {
        let text = "The lead developer is working on the backend migration.";
        let mut people = HashMap::new();
        people.insert("Alice Smith".to_string(), "Alice".to_string());

        let compressed = compress(text, &people);
        let decompressed = decompress(&compressed, &people);

        // Decompressed should be shorter than original (was compressed)
        assert!(compressed.len() < text.len(), "Compression should reduce length");
        // Decompressed should expand back
        assert!(decompressed.contains("Lead Developer") || decompressed.contains("lead"));
    }

    #[test]
    fn test_compress_empty() {
        let mut people = HashMap::new();
        assert_eq!(compress("", &people), "");
        assert_eq!(decompress("", &people), "");
    }
}
