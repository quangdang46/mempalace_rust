//! Compress file — port of upstream `compress-file.ts`.
//!
//! Compresses markdown files to reduce token usage while preserving structure.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Result of file compression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressFileResult {
    pub original_path: String,
    pub backup_path: String,
    pub original_size: usize,
    pub compressed_size: usize,
    pub reduction_pct: f64,
}

/// Compress a markdown file while preserving structure.
pub fn compress_markdown_file(file_path: &Path) -> Result<CompressFileResult> {
    let content = std::fs::read_to_string(file_path)?;
    let original_size = content.len();

    // Create backup
    let backup_path = file_path.with_extension("original.md");
    std::fs::write(&backup_path, &content)?;

    // Compress
    let compressed = compress_markdown(&content);
    let compressed_size = compressed.len();

    // Write compressed content
    std::fs::write(file_path, &compressed)?;

    let reduction_pct = if original_size > 0 {
        (1.0 - compressed_size as f64 / original_size as f64) * 100.0
    } else {
        0.0
    };

    Ok(CompressFileResult {
        original_path: file_path.to_string_lossy().to_string(),
        backup_path: backup_path.to_string_lossy().to_string(),
        original_size,
        compressed_size,
        reduction_pct,
    })
}

/// Compress markdown content.
pub fn compress_markdown(content: &str) -> String {
    let mut result = String::new();
    let mut in_code_block = false;
    let mut consecutive_blank = 0;

    for line in content.lines() {
        // Preserve code blocks verbatim
        if line.trim().starts_with("```") {
            in_code_block = !in_code_block;
            result.push_str(line);
            result.push('\n');
            consecutive_blank = 0;
            continue;
        }

        if in_code_block {
            result.push_str(line);
            result.push('\n');
            consecutive_blank = 0;
            continue;
        }

        // Collapse multiple blank lines
        if line.trim().is_empty() {
            consecutive_blank += 1;
            if consecutive_blank <= 2 {
                result.push('\n');
            }
            continue;
        }

        consecutive_blank = 0;

        // Preserve headings
        if line.starts_with('#') {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Compress list items (remove leading whitespace beyond 2 spaces)
        if line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('-') || trimmed.starts_with('*') || trimmed.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                result.push_str(line);
                result.push('\n');
                continue;
            }
        }

        // Trim trailing whitespace from regular lines
        result.push_str(line.trim_end());
        result.push('\n');
    }

    // Remove trailing newlines beyond 2
    while result.ends_with("\n\n\n") {
        result.pop();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_markdown_collapses_blanks() {
        let input = "Hello\n\n\n\nWorld";
        let output = compress_markdown(input);
        assert!(!output.contains("\n\n\n"));
    }

    #[test]
    fn test_compress_markdown_preserves_headings() {
        let input = "# Title\n\n## Section\n\nContent";
        let output = compress_markdown(input);
        assert!(output.contains("# Title"));
        assert!(output.contains("## Section"));
    }

    #[test]
    fn test_compress_markdown_preserves_code_blocks() {
        let input = "Text\n```\ncode\nwith\nblanks\n\n\n```\nMore text";
        let output = compress_markdown(input);
        assert!(output.contains("code\nwith\nblanks\n\n\n"));
    }

    #[test]
    fn test_compress_markdown_trims_whitespace() {
        let input = "Hello   \nWorld  \n";
        let output = compress_markdown(input);
        assert!(!output.contains("   \n"));
    }

    #[test]
    fn test_compress_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cf_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.md");
        let content = "# Title\n\n\n\nSome content\n\n\n\nMore content\n```\ncode block\n\n\n```\n";
        std::fs::write(&path, content).unwrap();

        let result = compress_markdown_file(&path).unwrap();
        assert!(result.original_size > result.compressed_size);
        assert!(result.reduction_pct > 0.0);

        // Backup exists
        assert!(Path::new(&result.backup_path).exists());
    }

    #[test]
    fn test_compress_file_result_serialization() {
        let result = CompressFileResult {
            original_path: "/tmp/test.md".to_string(),
            backup_path: "/tmp/test.original.md".to_string(),
            original_size: 1000,
            compressed_size: 500,
            reduction_pct: 50.0,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: CompressFileResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.reduction_pct, 50.0);
    }
}
