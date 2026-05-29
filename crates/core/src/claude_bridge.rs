//! Claude bridge — port of upstream `claude-bridge.ts`.
//!
//! Syncs memory state to/from Claude Code's native MEMORY.md file.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Configuration for the Claude bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeBridgeConfig {
    pub enabled: bool,
    pub project_path: Option<String>,
    pub memory_file_path: Option<String>,
    pub line_budget: usize,
}

impl Default for ClaudeBridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            project_path: None,
            memory_file_path: None,
            line_budget: 200,
        }
    }
}

/// Parsed sections from MEMORY.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedMemory {
    pub sections: HashMap<String, String>,
    pub raw: String,
    pub line_count: usize,
}

/// Parse MEMORY.md content into sections.
pub fn parse_memory_md(content: &str) -> ParsedMemory {
    let mut sections = HashMap::new();
    let mut current_section = String::new();
    let mut current_content = Vec::new();

    for line in content.lines() {
        if line.starts_with("## ") {
            if !current_section.is_empty() {
                sections.insert(current_section.clone(), current_content.join("\n").trim().to_string());
            }
            current_section = line[3..].trim().to_string();
            current_content.clear();
        } else {
            current_content.push(line.to_string());
        }
    }
    if !current_section.is_empty() {
        sections.insert(current_section, current_content.join("\n").trim().to_string());
    }

    ParsedMemory {
        sections,
        raw: content.to_string(),
        line_count: content.lines().count(),
    }
}

/// Serialize memories to MEMORY.md format.
pub fn serialize_to_memory_md(
    memories: &[crate::types::Memory],
    project_summary: &str,
    line_budget: usize,
) -> String {
    let mut lines = Vec::new();
    lines.push("# Agent Memory (auto-synced by mempalace)".to_string());
    lines.push(String::new());

    if !project_summary.is_empty() {
        lines.push("## Project Summary".to_string());
        lines.push(project_summary.to_string());
        lines.push(String::new());
    }

    lines.push("## Key Memories".to_string());
    lines.push(String::new());

    let mut sorted: Vec<_> = memories.iter()
        .filter(|m| m.is_latest)
        .collect();
    sorted.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap_or(std::cmp::Ordering::Equal));

    for mem in sorted {
        if lines.len() >= line_budget - 2 {
            break;
        }
        lines.push(format!("### {}", mem.title));
        for cl in mem.content.lines() {
            if lines.len() >= line_budget - 1 {
                break;
            }
            lines.push(cl.to_string());
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

/// Read MEMORY.md from disk.
pub fn read_memory_file(path: &Path) -> Result<ParsedMemory> {
    let content = std::fs::read_to_string(path)?;
    Ok(parse_memory_md(&content))
}

/// Write MEMORY.md to disk.
pub fn write_memory_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Sync memories to MEMORY.md.
pub fn sync_to_claude(
    config: &ClaudeBridgeConfig,
    memories: &[crate::types::Memory],
    project_summary: &str,
) -> Result<usize> {
    if !config.enabled {
        return Err(anyhow::anyhow!("Claude bridge not enabled"));
    }
    let path = config.memory_file_path.as_ref()
        .ok_or_else(|| anyhow::anyhow!("memory_file_path not configured"))?;

    let md = serialize_to_memory_md(memories, project_summary, config.line_budget);
    write_memory_file(Path::new(path), &md)?;
    Ok(md.lines().count())
}

/// Read from Claude's MEMORY.md.
pub fn read_from_claude(config: &ClaudeBridgeConfig) -> Result<ParsedMemory> {
    if !config.enabled {
        return Err(anyhow::anyhow!("Claude bridge not enabled"));
    }
    let path = config.memory_file_path.as_ref()
        .ok_or_else(|| anyhow::anyhow!("memory_file_path not configured"))?;

    if !Path::new(path).exists() {
        return Ok(ParsedMemory {
            sections: HashMap::new(),
            raw: String::new(),
            line_count: 0,
        });
    }

    read_memory_file(Path::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_md() {
        let content = "# Agent Memory\n\n## Project Summary\nTest project\n\n## Key Memories\n### Auth\nUses JWT\n";
        let parsed = parse_memory_md(content);
        assert_eq!(parsed.sections.len(), 2);
        assert!(parsed.sections.contains_key("Project Summary"));
        assert!(parsed.sections.contains_key("Key Memories"));
    }

    #[test]
    fn test_parse_empty_memory_md() {
        let content = "";
        let parsed = parse_memory_md(content);
        assert!(parsed.sections.is_empty());
        assert_eq!(parsed.line_count, 0);
    }

    #[test]
    fn test_serialize_to_memory_md() {
        use crate::types::{Memory, MemoryType};
        let memories = vec![Memory {
            id: "m-1".into(), created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
            memory_type: MemoryType::Semantic, title: "Auth uses JWT".into(),
            content: "The project uses JWT for auth".into(),
            concepts: vec!["auth".into()], files: vec![], session_ids: vec![],
            strength: 0.9, version: 1, parent_id: None, supersedes: vec![],
            related_ids: vec![], source_observation_ids: vec![], is_latest: true,
            forget_after: None, image_ref: None, agent_id: None,
            project: "test".into(),
        }];
        let md = serialize_to_memory_md(&memories, "Test project", 200);
        assert!(md.contains("Auth uses JWT"));
        assert!(md.contains("Test project"));
    }

    #[test]
    fn test_serialize_respects_line_budget() {
        use crate::types::{Memory, MemoryType};
        let memories = vec![Memory {
            id: "m-1".into(), created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
            memory_type: MemoryType::Semantic, title: "Test".into(),
            content: "line1\nline2\nline3\nline4\nline5".into(),
            concepts: vec![], files: vec![], session_ids: vec![],
            strength: 0.9, version: 1, parent_id: None, supersedes: vec![],
            related_ids: vec![], source_observation_ids: vec![], is_latest: true,
            forget_after: None, image_ref: None, agent_id: None,
            project: "test".into(),
        }];
        let md = serialize_to_memory_md(&memories, "", 10);
        assert!(md.lines().count() <= 10);
    }

    #[test]
    fn test_read_write_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cb_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("MEMORY.md");

        let content = "# Test\n\n## Section\nContent\n";
        write_memory_file(&path, content).unwrap();
        let parsed = read_memory_file(&path).unwrap();
        assert!(parsed.sections.contains_key("Section"));
    }

    #[test]
    fn test_sync_disabled() {
        let config = ClaudeBridgeConfig::default();
        let result = sync_to_claude(&config, &[], "");
        assert!(result.is_err());
    }
}
