use crate::general_extractor::{extract_memories, MemoryType};
use crate::normalize::normalize;
use crate::palace_db::PalaceDb;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use walkdir::WalkDir;

const MIN_CHUNK_SIZE: usize = 30;

const CONVO_EXTENSIONS: &[&str] = &[".txt", ".md", ".json", ".jsonl"];

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    ".mempalace",
];

const TOPIC_KEYWORDS: &[(&str, &[&str])] = &[
    ("technical", &["code", "python", "function", "bug", "error", "api", "database", "server", "deploy", "git", "test", "debug", "refactor"]),
    ("architecture", &["architecture", "design", "pattern", "structure", "schema", "interface", "module", "component", "service", "layer"]),
    ("planning", &["plan", "roadmap", "milestone", "deadline", "priority", "sprint", "backlog", "scope", "requirement", "spec"]),
    ("decisions", &["decided", "chose", "picked", "switched", "migrated", "replaced", "trade-off", "alternative", "option", "approach"]),
    ("problems", &["problem", "issue", "broken", "failed", "crash", "stuck", "workaround", "fix", "solved", "resolved"]),
];

fn is_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

fn has_convo_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| CONVO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn chunk_exchanges(content: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = content.lines().collect();
    let quote_count = lines.iter().filter(|l| l.trim().starts_with('>')).count();

    if quote_count >= 3 {
        chunk_by_exchange(&lines)
    } else {
        chunk_by_paragraph(content)
    }
}

fn chunk_by_exchange(lines: &[&str]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().starts_with('>') {
            let user_turn = line.trim();
            i += 1;

            let mut ai_lines = Vec::new();
            while i < lines.len() {
                let next_line = lines[i];
                if next_line.trim().starts_with('>') || next_line.trim().starts_with("---") {
                    break;
                }
                if !next_line.trim().is_empty() {
                    ai_lines.push(next_line.trim().to_string());
                }
                i += 1;
            }

            let ai_response: String = ai_lines.iter().take(8).map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
            let content = if ai_response.is_empty() {
                user_turn.to_string()
            } else {
                format!("{}\n{}", user_turn, ai_response)
            };

            if content.len() > MIN_CHUNK_SIZE {
                chunks.push(Chunk {
                    content,
                    chunk_index: chunks.len(),
                    memory_type: None,
                });
            }
        } else {
            i += 1;
        }
    }

    chunks
}

fn chunk_by_paragraph(content: &str) -> Vec<Chunk> {
    let paragraphs: Vec<&str> = content.split("\n\n").filter(|p| !p.trim().is_empty()).collect();

    if paragraphs.len() <= 1 && content.lines().count() > 20 {
        let lines: Vec<&str> = content.lines().collect();
        let mut chunks = Vec::new();
        for i in (0..lines.len()).step_by(25) {
            let end = (i + 25).min(lines.len());
            let group = lines[i..end].join("\n").trim().to_string();
            if group.len() > MIN_CHUNK_SIZE {
                chunks.push(Chunk {
                    content: group,
                    chunk_index: chunks.len(),
                    memory_type: None,
                });
            }
        }
        return chunks;
    }

    paragraphs
        .iter()
        .filter(|para| para.len() > MIN_CHUNK_SIZE)
        .enumerate()
        .map(|(idx, para)| Chunk {
            content: para.to_string(),
            chunk_index: idx,
            memory_type: None,
        })
        .collect()
}

fn detect_convo_room(content: &str) -> String {
    let content_lower = &content.chars().take(3000).collect::<String>().to_lowercase();
    let mut scores: HashMap<&str, usize> = HashMap::new();

    for (room, keywords) in TOPIC_KEYWORDS {
        let score = keywords.iter().filter(|kw| content_lower.contains(*kw)).count();
        if score > 0 {
            scores.insert(room, score);
        }
    }

    scores
        .iter()
        .max_by_key(|(_, score)| *score)
        .map(|(room, _)| (*room).to_string())
        .unwrap_or_else(|| "general".to_string())
}

#[derive(Debug, Clone)]
struct Chunk {
    content: String,
    chunk_index: usize,
    memory_type: Option<MemoryType>,
}

pub async fn mine_conversations(
    directory: &Path,
    palace_path: &Path,
    wing: Option<&str>,
    extract: Option<&str>,
) -> anyhow::Result<ConvoMiningResult> {
    let db = Arc::new(Mutex::new(PalaceDb::open(palace_path)?));
    let wing_name = wing.unwrap_or_else(|| {
        directory
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("convos")
            .to_lowercase()
            .replace(' ', "_")
            .replace('-', "_")
    });

    let extract_mode = extract.unwrap_or("exchange");

    let files = scan_conversation_files(directory);
    let mut result = ConvoMiningResult {
        files_processed: 0,
        conversations_mined: 0,
        chunks_created: 0,
        errors: vec![],
    };

    let mut room_counts: HashMap<String, usize> = HashMap::new();

    for filepath in &files {
        match process_file(filepath, &db, wing_name, extract_mode, &mut room_counts).await {
            Ok(chunks) => {
                result.files_processed += 1;
                result.conversations_mined += 1;
                result.chunks_created += chunks;
            }
            Err(e) => {
                result.errors.push(format!("{}: {}", filepath.display(), e));
            }
        }
    }

    Ok(result)
}

fn scan_conversation_files(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();

    for entry in WalkDir::new(directory)
        .max_depth(10)
        .into_iter()
        .filter_entry(|e| !is_skip_dir(e.file_name().to_str().unwrap_or("")))
        .filter_map(|e| e.ok())
    {
        let path = entry.path().to_path_buf();
        if path.is_file() && has_convo_extension(&path) {
            files.push(path);
        }
    }

    files
}

async fn process_file(
    filepath: &Path,
    db: &Arc<Mutex<PalaceDb>>,
    wing: &str,
    extract_mode: &str,
    room_counts: &mut HashMap<String, usize>,
) -> anyhow::Result<usize> {
    let content = std::fs::read_to_string(filepath)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", filepath.display(), e))?;
    let content = normalize(filepath, &content)?;

    if content.trim().len() < MIN_CHUNK_SIZE {
        return Ok(0);
    }

    let chunks = if extract_mode == "general" {
        let classifications = extract_memories(&content, 0.3);
        classifications
            .into_iter()
            .map(|c| Chunk {
                content: c.text,
                chunk_index: 0,
                memory_type: Some(c.memory_type),
            })
            .collect()
    } else {
        chunk_exchanges(&content)
            .into_iter()
            .map(|c| Chunk {
                content: c.content,
                chunk_index: c.chunk_index,
                memory_type: None,
            })
            .collect()
    };

    if chunks.is_empty() {
        return Ok(0);
    }

    let room = if extract_mode != "general" {
        detect_convo_room(&content)
    } else {
        "general".to_string()
    };

    let source_file = filepath.to_string_lossy().to_string();
    let mut db_guard = db.lock().await;
    let mut new_chunks = 0;

    for chunk in chunks {
        let chunk_room = chunk
            .memory_type
            .as_ref()
            .map(|mt| match mt {
                MemoryType::Decision => "decision",
                MemoryType::Preference => "preference",
                MemoryType::Milestone => "milestone",
                MemoryType::Problem => "problem",
                MemoryType::Emotional => "emotional",
            })
            .unwrap_or(&room)
            .to_string();

        let chunk_index = chunk.chunk_index;
        let drawer_id = format!(
            "drawer_{}_{}_{}_{}",
            wing,
            chunk_room,
            hash_string(&source_file),
            chunk_index
        );

        let metadata = vec![
            ("wing", wing),
            ("room", &chunk_room),
            ("source_file", &source_file),
            ("chunk_index", &chunk_index.to_string()),
            ("added_by", "mempalace"),
            ("ingest_mode", "convos"),
            ("extract_mode", extract_mode),
        ];

        db_guard.add(
            &[(&drawer_id, &chunk.content)],
            &[&metadata],
        )?;

        *room_counts.entry(chunk_room).or_insert(0) += 1;
        new_chunks += 1;
    }

    Ok(new_chunks)
}

fn hash_string(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:x}", hasher.finish())[..16].to_string()
}

#[derive(Debug)]
pub struct ConvoMiningResult {
    pub files_processed: usize,
    pub conversations_mined: usize,
    pub chunks_created: usize,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_exchanges_simple() {
        let content = "> Hello\nHow are you?\n> What is Rust?\nIt is a programming language.";
        let chunks = chunk_exchanges(content);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_detect_convo_room() {
        let content = "We decided to use Python for the API. The bug in the database connection was fixed.";
        let room = detect_convo_room(content);
        assert!(["decisions", "problems", "technical"].contains(&room.as_str()));
    }

    #[test]
    fn test_has_convo_extension() {
        assert!(has_convo_extension(Path::new("chat.json")));
        assert!(has_convo_extension(Path::new("transcript.md")));
        assert!(!has_convo_extension(Path::new("code.rs")));
    }

    #[test]
    fn test_chunk_by_paragraph() {
        let content = "First paragraph here.\n\nSecond paragraph here.";
        let chunks = chunk_by_paragraph(content);
        assert!(chunks.len() >= 1);
    }
}
