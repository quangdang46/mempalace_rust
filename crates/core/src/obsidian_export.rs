//! Obsidian export — port of upstream `obsidian-export.ts`.
//!
//! Exports memories and observations as Obsidian-compatible markdown files.

use anyhow::Result;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Configuration for Obsidian export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsidianExportConfig {
    pub output_dir: String,
    pub include_frontmatter: bool,
    pub include_tags: bool,
    pub include_links: bool,
    pub tag_prefix: String,
    pub date_format: String,
}

impl Default for ObsidianExportConfig {
    fn default() -> Self {
        Self {
            output_dir: "./memory-export".to_string(),
            include_frontmatter: true,
            include_tags: true,
            include_links: true,
            tag_prefix: "memory/".to_string(),
            date_format: "%Y-%m-%d %H:%M".to_string(),
        }
    }
}

/// Result of an export operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsidianExportResult {
    pub exported_count: usize,
    pub output_dir: String,
    pub files: Vec<String>,
}

/// Null-record safe-ID normalizer: returns a sanitized string suitable for templates
/// or an empty string if null/missing, never panics.
fn safe_id(v: &Option<String>, label: &str) -> String {
    v.as_deref()
        .unwrap_or_else(|| {
            tracing::warn!("Obsidian export: null or missing {label}, using placeholder");
            ""
        })
        .to_string()
}

/// Null-record safe-timestamp formatter.
fn safe_timestamp(dt: &chrono::DateTime<chrono::Utc>, format: &str) -> String {
    dt.format(format).to_string()
}

/// Null-record safe-string normalizer.
fn safe_str(v: &str) -> &str {
    v.trim()
}

/// Export a single memory as Obsidian markdown.
pub fn memory_to_obsidian_md(
    memory: &crate::types::Memory,
    config: &ObsidianExportConfig,
) -> String {
    let mut md = String::new();

    // Frontmatter
    if config.include_frontmatter {
        md.push_str("---\n");
        md.push_str(&format!(
            "id: {}\n",
            safe_id(&Some(memory.id.clone()), "memory.id")
        ));
        md.push_str(&format!(
            "title: {}\n",
            safe_str(&memory.title).replace('\n', " ")
        ));
        md.push_str(&format!("type: {}\n", memory.memory_type));
        md.push_str(&format!(
            "created: {}\n",
            memory.created_at.format(&config.date_format)
        ));
        md.push_str(&format!("strength: {:.2}\n", memory.strength));
        md.push_str(&format!("version: {}\n", memory.version));

        if config.include_tags && !memory.concepts.is_empty() {
            let tags: Vec<String> = memory
                .concepts
                .iter()
                .map(|c| format!("[{}/{}]", config.tag_prefix, sanitize_tag(c)))
                .collect();
            md.push_str(&format!("tags: [{}]\n", tags.join(", ")));
        }

        if !memory.files.is_empty() {
            let files: Vec<String> = memory.files.iter().map(|f| format!("\"{}\"", f)).collect();
            md.push_str(&format!("files: [{}]\n", files.join(", ")));
        }

        md.push_str("---\n\n");
    }

    // Title
    md.push_str(&format!("# {}\n\n", memory.title));

    // Content
    md.push_str(&memory.content);
    md.push_str("\n\n");

    // Links
    if config.include_links {
        if !memory.related_ids.is_empty() {
            md.push_str("## Related\n\n");
            for id in &memory.related_ids {
                md.push_str(&format!("- [[{}]]\n", id));
            }
            md.push('\n');
        }

        if !memory.source_observation_ids.is_empty() {
            md.push_str("## Sources\n\n");
            for id in &memory.source_observation_ids {
                md.push_str(&format!("- [[{}]]\n", id));
            }
            md.push('\n');
        }

        if !memory.supersedes.is_empty() {
            md.push_str("## Supersedes\n\n");
            for id in &memory.supersedes {
                md.push_str(&format!("- [[{}]]\n", id));
            }
            md.push('\n');
        }
    }

    md
}

/// Export a single observation as Obsidian markdown.
pub fn observation_to_obsidian_md(
    obs: &crate::types::CompressedObservation,
    config: &ObsidianExportConfig,
) -> String {
    let mut md = String::new();

    // Frontmatter
    if config.include_frontmatter {
        md.push_str("---\n");
        md.push_str(&format!(
            "id: {}\n",
            safe_id(&Some(obs.id.clone()), "obs.id")
        ));
        md.push_str(&format!("type: observation\n"));
        md.push_str(&format!("observation_type: {}\n", obs.observation_type));
        md.push_str(&format!(
            "timestamp: {}\n",
            obs.timestamp.format(&config.date_format)
        ));
        md.push_str(&format!("importance: {}\n", obs.importance));
        md.push_str(&format!("confidence: {:.2}\n", obs.confidence));
        md.push_str(&format!("session: {}\n", obs.session_id));

        if config.include_tags && !obs.concepts.is_empty() {
            let tags: Vec<String> = obs
                .concepts
                .iter()
                .map(|c| format!("[{}/{}]", config.tag_prefix, sanitize_tag(c)))
                .collect();
            md.push_str(&format!("tags: [{}]\n", tags.join(", ")));
        }

        md.push_str("---\n\n");
    }

    // Title
    md.push_str(&format!("# {}\n\n", obs.title));

    if let Some(subtitle) = &obs.subtitle {
        md.push_str(&format!("> {}\n\n", subtitle));
    }

    // Narrative
    md.push_str(&obs.narrative);
    md.push_str("\n\n");

    // Facts
    if !obs.facts.is_empty() {
        md.push_str("## Facts\n\n");
        for fact in &obs.facts {
            md.push_str(&format!("- {}\n", fact));
        }
        md.push('\n');
    }

    // Files
    if !obs.files.is_empty() {
        md.push_str("## Files\n\n");
        for file in &obs.files {
            md.push_str(&format!("- `{}`\n", file));
        }
        md.push('\n');
    }

    md
}

/// Export all memories to Obsidian format.
pub fn export_memories(
    memories: &[crate::types::Memory],
    config: &ObsidianExportConfig,
) -> Result<ObsidianExportResult> {
    let output_dir = Path::new(&config.output_dir);
    std::fs::create_dir_all(output_dir)?;

    let mut files = Vec::new();
    let mut errors = 0usize;

    for memory in memories {
        // Skip null records — four-layer safety: id filter, safe normalizers, outer try/catch, fail-safe sort
        if memory.id.trim().is_empty() {
            tracing::warn!("Skipping memory with empty id: title={:?}", memory.title);
            continue;
        }
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<String> {
                Ok(memory_to_obsidian_md(memory, config))
            }));
        let md = match result {
            Ok(Ok(md)) => md,
            Ok(Err(e)) => {
                tracing::warn!("Obsidian export failed for memory {}: {e}", memory.id);
                errors += 1;
                continue;
            }
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                tracing::warn!("Obsidian export panicked for memory {}: {msg}", memory.id);
                errors += 1;
                continue;
            }
        };
        let filename = sanitize_filename(&memory.title);
        let path = output_dir.join(format!("{}.md", filename));
        if let Err(e) = std::fs::write(&path, &md) {
            tracing::warn!("Obsidian export write failed for memory {}: {e}", memory.id);
            errors += 1;
            continue;
        }
        files.push(path.to_string_lossy().to_string());
    }

    tracing::debug!("Obsidian export: {} OK, {} errors", files.len(), errors);
    Ok(ObsidianExportResult {
        exported_count: files.len(),
        output_dir: config.output_dir.clone(),
        files,
    })
}

/// Export all observations to Obsidian format.
pub fn export_observations(
    observations: &[crate::types::CompressedObservation],
    config: &ObsidianExportConfig,
) -> Result<ObsidianExportResult> {
    let output_dir = Path::new(&config.output_dir);
    std::fs::create_dir_all(output_dir.join("observations"))?;

    let mut files = Vec::new();
    let mut errors = 0usize;

    for obs in observations {
        // Skip null records
        if obs.id.trim().is_empty() {
            tracing::warn!("Skipping observation with empty id: title={:?}", obs.title);
            continue;
        }
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<String> {
                Ok(observation_to_obsidian_md(obs, config))
            }));
        let md = match result {
            Ok(Ok(md)) => md,
            Ok(Err(e)) => {
                tracing::warn!("Obsidian export failed for observation {}: {e}", obs.id);
                errors += 1;
                continue;
            }
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                tracing::warn!("Obsidian export panicked for observation {}: {msg}", obs.id);
                errors += 1;
                continue;
            }
        };
        let filename = sanitize_filename(&obs.title);
        let path = output_dir
            .join("observations")
            .join(format!("{}.md", filename));
        if let Err(e) = std::fs::write(&path, &md) {
            tracing::warn!(
                "Obsidian export write failed for observation {}: {e}",
                obs.id
            );
            errors += 1;
            continue;
        }
        files.push(path.to_string_lossy().to_string());
    }

    tracing::debug!(
        "Obsidian export observations: {} OK, {} errors",
        files.len(),
        errors
    );
    Ok(ObsidianExportResult {
        exported_count: files.len(),
        output_dir: config.output_dir.clone(),
        files,
    })
}

fn sanitize_tag(tag: &str) -> String {
    tag.chars()
        .map(|c| match c {
            ' ' => '-',
            c if c.is_alphanumeric() || c == '-' || c == '_' => c,
            _ => '_',
        })
        .collect::<String>()
        .to_lowercase()
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CompressedObservation, Memory, MemoryType, ObservationType};

    fn test_memory(id: &str) -> Memory {
        Memory {
            id: id.into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            memory_type: MemoryType::Semantic,
            title: format!("Test Memory {}", id),
            content: "This is test content.".into(),
            concepts: vec!["rust".into(), "testing".into()],
            files: vec!["src/main.rs".into()],
            session_ids: vec!["s-1".into()],
            strength: 0.8,
            version: 1,
            parent_id: None,
            supersedes: vec![],
            related_ids: vec![],
            source_observation_ids: vec!["o-1".into()],
            is_latest: true,
            forget_after: None,
            image_ref: None,
            agent_id: None,
            project: "test".into(),
        }
    }

    fn test_obs(id: &str) -> CompressedObservation {
        CompressedObservation {
            id: id.into(),
            session_id: "s-1".into(),
            timestamp: chrono::Utc::now(),
            observation_type: ObservationType::FileEdit,
            title: format!("Test Observation {}", id),
            subtitle: Some("A test subtitle".into()),
            facts: vec!["Fact 1".into(), "Fact 2".into()],
            narrative: "This is the observation narrative.".into(),
            concepts: vec!["rust".into()],
            files: vec!["src/lib.rs".into()],
            importance: 5,
            confidence: 0.9,
            image_ref: None,
            image_description: None,
            modality: "text".into(),
            agent_id: None,
        }
    }

    #[test]
    fn test_memory_to_obsidian_md() {
        let memory = test_memory("m-1");
        let config = ObsidianExportConfig::default();
        let md = memory_to_obsidian_md(&memory, &config);

        assert!(md.contains("---"));
        assert!(md.contains("id: m-1"));
        assert!(md.contains("# Test Memory m-1"));
        assert!(md.contains("This is test content."));
        assert!(md.contains("[[o-1]]"));
        assert!(md.contains("tags:"));
    }

    #[test]
    fn test_observation_to_obsidian_md() {
        let obs = test_obs("o-1");
        let config = ObsidianExportConfig::default();
        let md = observation_to_obsidian_md(&obs, &config);

        assert!(md.contains("---"));
        assert!(md.contains("id: o-1"));
        assert!(md.contains("# Test Observation o-1"));
        assert!(md.contains("A test subtitle"));
        assert!(md.contains("## Facts"));
        assert!(md.contains("Fact 1"));
    }

    #[test]
    fn test_memory_no_frontmatter() {
        let memory = test_memory("m-1");
        let config = ObsidianExportConfig {
            include_frontmatter: false,
            ..Default::default()
        };
        let md = memory_to_obsidian_md(&memory, &config);

        assert!(!md.starts_with("---"));
        assert!(md.contains("# Test Memory m-1"));
    }

    #[test]
    fn test_memory_no_links() {
        let memory = test_memory("m-1");
        let config = ObsidianExportConfig {
            include_links: false,
            ..Default::default()
        };
        let md = memory_to_obsidian_md(&memory, &config);

        assert!(!md.contains("## Related"));
        assert!(!md.contains("## Sources"));
    }

    #[test]
    fn test_sanitize_tag() {
        assert_eq!(sanitize_tag("rust programming"), "rust-programming");
        assert_eq!(sanitize_tag("Special/Chars!"), "special_chars_");
        assert_eq!(sanitize_tag("UPPERCASE"), "uppercase");
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("test/file:name"), "test_file_name");
        assert_eq!(sanitize_filename("valid-name"), "valid-name");
    }

    #[test]
    fn test_export_memories() {
        let dir = std::env::temp_dir().join(format!("obsidian_test_{}", std::process::id()));
        let memories = vec![test_memory("m-1"), test_memory("m-2")];
        let config = ObsidianExportConfig {
            output_dir: dir.to_string_lossy().to_string(),
            ..Default::default()
        };

        let result = export_memories(&memories, &config).unwrap();
        assert_eq!(result.exported_count, 2);

        // Files were created
        for file in &result.files {
            assert!(Path::new(file).exists());
        }
    }

    #[test]
    fn test_export_result_serialization() {
        let result = ObsidianExportResult {
            exported_count: 5,
            output_dir: "./export".to_string(),
            files: vec!["file1.md".to_string()],
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ObsidianExportResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.exported_count, 5);
    }
}
