use crate::general_extractor::{extract_memories, MemoryType};
use crate::normalize::normalize;
use crate::palace_db::PalaceDb;
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const CONVO_EXTENSIONS: &[&str] = &["txt", "md", "json", "jsonl"];
const MIN_CHUNK_SIZE: usize = 30;
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".venv",
    "venv",
    "env",
    "target",
    "dist",
    "build",
    ".next",
    "coverage",
    ".mempalace",
    ".cache",
    ".tox",
    ".nox",
    ".idea",
    ".vscode",
    ".ipynb_checkpoints",
    ".eggs",
    "htmlcov",
];

const TOPIC_KEYWORDS: &[(&str, &[&str])] = &[
    (
        "technical",
        &[
            "code", "python", "function", "bug", "error", "api", "database", "server", "deploy",
            "git", "test", "debug", "refactor",
        ],
    ),
    (
        "architecture",
        &[
            "architecture",
            "design",
            "pattern",
            "structure",
            "schema",
            "interface",
            "module",
            "component",
            "service",
            "layer",
        ],
    ),
    (
        "planning",
        &[
            "plan",
            "roadmap",
            "milestone",
            "deadline",
            "priority",
            "sprint",
            "backlog",
            "scope",
            "requirement",
            "spec",
        ],
    ),
    (
        "decisions",
        &[
            "decided",
            "chose",
            "picked",
            "switched",
            "migrated",
            "replaced",
            "trade-off",
            "alternative",
            "option",
            "approach",
        ],
    ),
    (
        "problems",
        &[
            "problem",
            "issue",
            "broken",
            "failed",
            "crash",
            "stuck",
            "workaround",
            "fix",
            "solved",
            "resolved",
        ],
    ),
];

#[derive(Debug)]
pub struct ConvoMiningResult {
    pub files_processed: usize,
    pub conversations_mined: usize,
    pub chunks_created: usize,
    pub files_skipped: usize,
    pub room_counts: Vec<(String, usize)>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Chunk {
    content: String,
    chunk_index: usize,
    memory_type: Option<String>,
}

pub async fn mine_conversations(
    directory: &Path,
    palace_path: &Path,
    wing: Option<&str>,
    agent: &str,
    limit: usize,
    dry_run: bool,
    extract: Option<&str>,
) -> anyhow::Result<ConvoMiningResult> {
    let convo_path = directory.expanduser();
    let wing = wing
        .map(|value| value.to_string())
        .unwrap_or_else(|| sanitize_wing_name(&convo_path));
    let extract_mode = extract.unwrap_or("exchange");

    let mut files = scan_convos(&convo_path);
    if limit > 0 {
        files.truncate(limit);
    }
    let mut db = if dry_run {
        None
    } else {
        Some(PalaceDb::open(palace_path)?)
    };

    let mut files_processed = 0;
    let mut conversations_mined = 0;
    let mut chunks_created = 0;
    let mut files_skipped = 0;
    let mut room_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut errors = Vec::new();

    println!();
    println!("{}", "=".repeat(55));
    println!("  MemPalace Mine — Conversations");
    println!("{}", "=".repeat(55));
    println!("  Wing:    {}", wing);
    println!("  Source:  {}", convo_path.display());
    println!("  Files:   {}", files.len());
    println!("  Palace:  {}", palace_path.display());
    if dry_run {
        println!("  DRY RUN — nothing will be filed");
    }
    println!("{}", "-".repeat(55));
    println!();

    for (index, filepath) in files.iter().enumerate() {
        let source_file = filepath.to_string_lossy().to_string();
        if !dry_run
            && db
                .as_ref()
                .map(|db| db.file_already_mined(&source_file, false))
                .unwrap_or(false)
        {
            files_skipped += 1;
            continue;
        }

        let raw = match std::fs::read_to_string(filepath) {
            Ok(content) => content,
            Err(error) => {
                errors.push(format!("Error reading {:?}: {}", filepath, error));
                continue;
            }
        };

        let normalized = match normalize(filepath, &raw) {
            Ok(content) => content,
            Err(error) => {
                errors.push(format!("Error normalizing {:?}: {}", filepath, error));
                continue;
            }
        };

        if normalized.trim().len() < MIN_CHUNK_SIZE {
            continue;
        }

        let chunks = if extract_mode == "general" {
            extract_general_chunks(&normalized)
        } else {
            chunk_exchanges(&normalized)
        };

        if chunks.is_empty() {
            continue;
        }

        let room = if extract_mode == "general" {
            None
        } else {
            Some(detect_convo_room(&normalized))
        };

        if dry_run {
            if extract_mode == "general" {
                let mut counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for chunk in &chunks {
                    let key = chunk
                        .memory_type
                        .clone()
                        .unwrap_or_else(|| "general".to_string());
                    *counts.entry(key.clone()).or_insert(0) += 1;
                    *room_counts.entry(key).or_insert(0) += 1;
                }
                let mut sorted_counts: Vec<(String, usize)> = counts.into_iter().collect();
                sorted_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                let types_str = sorted_counts
                    .into_iter()
                    .map(|(kind, count)| format!("{}:{}", kind, count))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "    [DRY RUN] {} → {} memories ({})",
                    filepath
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                    chunks.len(),
                    types_str
                );
            } else if let Some(room_name) = room.as_ref() {
                *room_counts.entry(room_name.clone()).or_insert(0) += 1;
                println!(
                    "    [DRY RUN] {} → room:{} ({} drawers)",
                    filepath
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                    room_name,
                    chunks.len()
                );
            }
            files_processed += 1;
            conversations_mined += 1;
            chunks_created += chunks.len();
            continue;
        }

        if extract_mode != "general" {
            if let Some(room_name) = room.as_ref() {
                *room_counts.entry(room_name.clone()).or_insert(0) += 1;
            }
        }

        let source_mtime = std::fs::metadata(filepath)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64().to_string());
        let filed_at = Utc::now().to_rfc3339();
        let chunk_rooms: Vec<String> = chunks
            .iter()
            .map(|chunk| {
                chunk
                    .memory_type
                    .clone()
                    .or_else(|| room.clone())
                    .unwrap_or_else(|| "general".to_string())
            })
            .collect();

        let ids_and_docs: Vec<(String, String)> = chunks
            .iter()
            .zip(chunk_rooms.iter())
            .map(|(chunk, chunk_room)| {
                (
                    generate_drawer_id(&wing, chunk_room, &source_file, chunk.chunk_index),
                    chunk.content.clone(),
                )
            })
            .collect();

        let ids_and_docs_ref: Vec<(&str, &str)> = ids_and_docs
            .iter()
            .map(|(id, content)| (id.as_str(), content.as_str()))
            .collect();

        let chunk_indexes: Vec<String> = chunks
            .iter()
            .map(|chunk| chunk.chunk_index.to_string())
            .collect();
        let mut metadata = Vec::new();
        for index in 0..chunks.len() {
            let mut fields = vec![
                ("wing", wing.as_str()),
                ("room", chunk_rooms[index].as_str()),
                ("source_file", source_file.as_str()),
                ("chunk_index", chunk_indexes[index].as_str()),
                ("added_by", agent),
                ("filed_at", filed_at.as_str()),
                ("ingest_mode", "convos"),
                ("extract_mode", extract_mode),
            ];
            if let Some(mtime) = source_mtime.as_deref() {
                fields.push(("source_mtime", mtime));
            }
            metadata.push(fields);
        }
        let metadata_refs: Vec<&[(&str, &str)]> = metadata.iter().map(|m| m.as_slice()).collect();

        db.as_mut()
            .unwrap()
            .add(&ids_and_docs_ref, &metadata_refs)?;
        for chunk_room in &chunk_rooms {
            if extract_mode == "general" {
                *room_counts.entry(chunk_room.clone()).or_insert(0) += 1;
            }
        }
        files_processed += 1;
        conversations_mined += 1;
        chunks_created += chunks.len();
        println!(
            "  ✓ [{:4}/{}] {:50} +{}",
            index + 1,
            files.len(),
            filepath
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| {
                    if name.len() > 50 {
                        name[..50].to_string()
                    } else {
                        format!("{:<50}", name)
                    }
                })
                .unwrap_or_default(),
            chunks.len()
        );
    }

    if let Some(db) = db.as_mut() {
        db.flush()?;
    }

    let mut room_counts_vec: Vec<(String, usize)> = room_counts.into_iter().collect();
    room_counts_vec.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    println!();
    println!("{}", "=".repeat(55));
    println!("  Done.");
    println!("  Files processed: {}", files_processed);
    println!("  Files skipped (already filed): {}", files_skipped);
    println!("  Drawers filed: {}", chunks_created);
    if !room_counts_vec.is_empty() {
        println!();
        println!("  By room:");
        for (room, count) in &room_counts_vec {
            println!("    {:20} {} files", room, count);
        }
    }
    println!();
    println!("  Next: mempalace search \"what you're looking for\"");
    println!("{}", "=".repeat(55));
    println!();

    Ok(ConvoMiningResult {
        files_processed,
        conversations_mined,
        chunks_created,
        files_skipped,
        room_counts: room_counts_vec,
        errors,
    })
}

fn sanitize_wing_name(path: &Path) -> String {
    path.file_name()
        .map(|name| {
            name.to_string_lossy()
                .to_lowercase()
                .replace([' ', '-'], "_")
        })
        .unwrap_or_else(|| "convos".to_string())
}

trait ExpandUser {
    fn expanduser(&self) -> PathBuf;
}

impl ExpandUser for Path {
    fn expanduser(&self) -> PathBuf {
        let text = self.to_string_lossy();
        if let Some(stripped) = text.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(stripped)
        } else {
            self.to_path_buf()
        }
    }
}

fn generate_drawer_id(wing: &str, room: &str, source_file: &str, chunk_index: usize) -> String {
    let input = format!("{}{}", source_file, chunk_index);
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let hex = hex::encode(hasher.finalize());
    format!("drawer_{}_{}_{}", wing, room, &hex[..24])
}

/// Scan `convo_dir` for conversation files, returning paths to mine.
///
/// Skips symlinks (which could otherwise follow links to recursive structures
/// or `/dev/urandom`) and oversized files. Each skipped symlink is logged to
/// `stderr` with a `"  SKIP: <relative-path> (symlink)"` line so callers can
/// tell why a directory looks empty after walking (#1462).
fn scan_convos(convo_dir: &Path) -> Vec<PathBuf> {
    scan_convos_with_log(convo_dir, &mut std::io::stderr())
}

/// Same as [`scan_convos`] but routes the skipped-symlink diagnostic to an
/// arbitrary writer. Lets unit tests assert the log fires without having to
/// fork a subprocess to capture stderr.
fn scan_convos_with_log<W: Write>(convo_dir: &Path, skip_log: &mut W) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in WalkDir::new(convo_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !entry.file_type().is_dir() || !SKIP_DIRS.contains(&name.as_ref())
        })
        .filter_map(|entry| entry.ok())
    {
        let ft = entry.file_type();
        // Let regular files AND symlinks through. Walkdir's `is_file()`
        // returns `false` for symlinks-to-files under `follow_links(false)`,
        // so a bare `!is_file()` check would silently drop every symlink
        // before the diagnostic branch below can fire.
        if !ft.is_file() && !ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.ends_with(".meta.json") {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if !CONVO_EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        // Skip symlinks — prevents following recursive/bogus links. Log to
        // `skip_log` with the path relative to the scan root so the
        // diagnostic is unambiguous and renders with forward slashes on
        // every platform. Runs AFTER the extension filter to match upstream
        // Python `scan_convos` ordering — a `.png` symlink is silently
        // dropped at the extension gate rather than logged. (#1462)
        if ft.is_symlink() {
            let rel = path
                .strip_prefix(convo_dir)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            let _ = writeln!(skip_log, "  SKIP: {rel} (symlink)");
            continue;
        }
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_SIZE {
            continue;
        }
        files.push(path.to_path_buf());
    }
    files
}

fn chunk_exchanges(content: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = content.split('\n').collect();
    let quote_lines = lines
        .iter()
        .filter(|line| line.trim().starts_with('>'))
        .count();
    if quote_lines >= 3 {
        chunk_by_exchange(&lines)
    } else {
        chunk_by_paragraph(content)
    }
}

fn chunk_by_exchange(lines: &[&str]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.trim().starts_with('>') {
            let user_turn = line.trim().to_string();
            index += 1;

            let mut ai_lines = Vec::new();
            while index < lines.len() {
                let next_line = lines[index];
                let trimmed = next_line.trim();
                if trimmed.starts_with('>') || trimmed.starts_with("---") {
                    break;
                }
                if !trimmed.is_empty() {
                    ai_lines.push(trimmed.to_string());
                }
                index += 1;
            }

            let ai_response = ai_lines.into_iter().take(8).collect::<Vec<_>>().join(" ");
            let chunk_content = if ai_response.is_empty() {
                user_turn
            } else {
                format!("{}\n{}", user_turn, ai_response)
            };

            if chunk_content.trim().len() > MIN_CHUNK_SIZE {
                chunks.push(Chunk {
                    content: chunk_content,
                    chunk_index: chunks.len(),
                    memory_type: None,
                });
            }
        } else {
            index += 1;
        }
    }

    chunks
}

fn chunk_by_paragraph(content: &str) -> Vec<Chunk> {
    let paragraphs: Vec<String> = content
        .split("\n\n")
        .map(|paragraph| paragraph.trim().to_string())
        .filter(|paragraph| !paragraph.is_empty())
        .collect();

    if paragraphs.len() <= 1 && content.lines().count() > 20 {
        return content
            .lines()
            .collect::<Vec<_>>()
            .chunks(25)
            .filter_map(|group| {
                let text = group.join("\n").trim().to_string();
                if text.len() > MIN_CHUNK_SIZE {
                    Some(text)
                } else {
                    None
                }
            })
            .enumerate()
            .map(|(chunk_index, content)| Chunk {
                content,
                chunk_index,
                memory_type: None,
            })
            .collect();
    }

    paragraphs
        .into_iter()
        .filter(|paragraph| paragraph.len() > MIN_CHUNK_SIZE)
        .enumerate()
        .map(|(chunk_index, content)| Chunk {
            content,
            chunk_index,
            memory_type: None,
        })
        .collect()
}

fn detect_convo_room(content: &str) -> String {
    let lower = content
        .chars()
        .take(3000)
        .collect::<String>()
        .to_lowercase();
    let mut best_room = "general";
    let mut best_score = 0;
    for (room, keywords) in TOPIC_KEYWORDS {
        let score = keywords
            .iter()
            .filter(|keyword| lower.contains(**keyword))
            .count();
        if score > best_score {
            best_score = score;
            best_room = room;
        }
    }
    best_room.to_string()
}

fn extract_general_chunks(content: &str) -> Vec<Chunk> {
    extract_memories(content, 0.3)
        .into_iter()
        .enumerate()
        .map(|(chunk_index, classification)| Chunk {
            content: classification.text,
            chunk_index,
            memory_type: Some(memory_type_name(&classification.memory_type).to_string()),
        })
        .collect()
}

fn memory_type_name(memory_type: &MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Decision => "decision",
        MemoryType::Preference => "preference",
        MemoryType::Milestone => "milestone",
        MemoryType::Problem => "problem",
        MemoryType::Emotional => "emotional",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::searcher::search_memories;

    #[test]
    fn test_chunk_exchanges_exchange_chunking() {
        let content = "> What is memory?\nMemory is persistence of information over time.\n\n> Why does it matter?\nIt enables continuity across sessions and conversations.\n\n> How do we build it?\nWith structured storage and retrieval mechanisms.\n";
        let chunks = chunk_exchanges(content);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|chunk| chunk.chunk_index < chunks.len()));
    }

    #[test]
    fn test_chunk_exchanges_paragraph_fallback() {
        let content = format!(
            "{}\n\n{}\n\n{}",
            "This is a long paragraph about memory systems. ".repeat(10),
            "This is another paragraph about storage. ".repeat(10),
            "And a third paragraph about retrieval. ".repeat(10)
        );
        let chunks = chunk_exchanges(&content);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn test_chunk_exchanges_line_group_fallback() {
        let content = (0..60)
            .map(|index| format!("Line {}: some content that is meaningful", index))
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = chunk_exchanges(&content);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_detect_convo_room() {
        let content = "Let me debug this python function and fix the code error in the api";
        assert_eq!(detect_convo_room(content), "technical");
    }

    #[test]
    fn test_scan_convos() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("chat.txt"), "hello").unwrap();
        std::fs::write(temp.path().join("notes.md"), "world").unwrap();
        std::fs::write(temp.path().join("chat.meta.json"), "{}").unwrap();
        std::fs::write(temp.path().join("image.png"), "fake").unwrap();
        std::fs::create_dir_all(temp.path().join(".git")).unwrap();
        std::fs::write(temp.path().join(".git/config.txt"), "git stuff").unwrap();

        let files = scan_convos(temp.path());
        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"chat.txt".to_string()));
        assert!(names.contains(&"notes.md".to_string()));
        assert!(!names.contains(&"chat.meta.json".to_string()));
        assert!(!names.contains(&"config.txt".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_convos_skips_symlinks() {
        // Regression for upstream #1462: scan_convos drops symlinked files
        // so the walker can't recurse into bogus link targets. The stderr
        // SKIP log surfaces the skip with a path relative to the scan root.
        // Asserts the diagnostic actually fires — relying on result-set
        // exclusion alone passes against dead-code symlink branches, which
        // is how the initial port shipped.
        let temp = tempfile::TempDir::new().unwrap();
        let real = temp.path().join("real.md");
        std::fs::write(&real, "hello world").unwrap();
        std::os::unix::fs::symlink(&real, temp.path().join("link.md")).unwrap();

        let mut log = Vec::new();
        let files = scan_convos_with_log(temp.path(), &mut log);
        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["real.md".to_string()]);
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.contains("  SKIP: link.md (symlink)\n"),
            "expected SKIP diagnostic for link.md, got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_convos_skips_dangling_symlinks() {
        // A dangling symlink in the convo dir must not panic the walker nor
        // surface in the result set. Mirrors upstream coverage for #1462's
        // polished dangling-link path.
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("real.md"), "hello world").unwrap();
        std::os::unix::fs::symlink(
            temp.path().join("missing.md"),
            temp.path().join("dangling.md"),
        )
        .unwrap();

        let mut log = Vec::new();
        let files = scan_convos_with_log(temp.path(), &mut log);
        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["real.md".to_string()]);
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.contains("  SKIP: dangling.md (symlink)\n"),
            "expected SKIP diagnostic for dangling.md, got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_convos_does_not_log_extension_filtered_symlinks() {
        // A symlink whose name doesn't match `CONVO_EXTENSIONS` is silently
        // dropped at the extension gate — it must NOT surface in the SKIP
        // log, matching upstream Python ordering (extension → symlink-log
        // → size).
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("real.md"), "hello world").unwrap();
        std::os::unix::fs::symlink("real.md", temp.path().join("link.png")).unwrap();

        let mut log = Vec::new();
        let files = scan_convos_with_log(temp.path(), &mut log);
        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["real.md".to_string()]);
        let log = String::from_utf8(log).unwrap();
        assert!(
            !log.contains("link.png"),
            "extension-filtered symlink leaked into SKIP log: {log:?}"
        );
    }

    #[test]
    fn test_scan_convos_skips_python_parity_dirs() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join(".mempalace")).unwrap();
        std::fs::create_dir_all(temp.path().join("coverage")).unwrap();
        std::fs::create_dir_all(temp.path().join(".idea")).unwrap();
        std::fs::write(temp.path().join(".mempalace/chat.txt"), "hello").unwrap();
        std::fs::write(temp.path().join("coverage/chat.txt"), "hello").unwrap();
        std::fs::write(temp.path().join(".idea/chat.txt"), "hello").unwrap();
        std::fs::write(temp.path().join("root.txt"), "hello").unwrap();

        let files = scan_convos(temp.path());
        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["root.txt".to_string()]);
    }

    #[tokio::test]
    async fn test_mine_conversations_exchange_mode() {
        let temp = tempfile::TempDir::new().unwrap();
        let convo_dir = temp.path();
        let palace = temp.path().join("palace");
        std::fs::write(
            convo_dir.join("chat.txt"),
            "> What is memory?\nMemory is persistence.\n\n> Why does it matter?\nIt enables continuity.\n\n> How do we build it?\nWith structured storage.\n",
        )
        .unwrap();

        let result = mine_conversations(
            convo_dir,
            &palace,
            Some("test_convos"),
            "mempalace",
            0,
            false,
            Some("exchange"),
        )
        .await
        .unwrap();
        assert_eq!(result.files_processed, 1);
        assert!(result.chunks_created >= 2);

        let db = PalaceDb::open(&palace).unwrap();
        let entries = db.get_all(Some("test_convos"), None, 10);
        assert!(!entries.is_empty());
        let meta = entries[0].metadatas.first().unwrap();
        assert_eq!(
            meta.get("ingest_mode").and_then(|v| v.as_str()),
            Some("convos")
        );
        assert_eq!(
            meta.get("extract_mode").and_then(|v| v.as_str()),
            Some("exchange")
        );

        let search = search_memories(
            "memory persistence",
            &palace,
            Some("test_convos"),
            None,
            3,
            None,
        )
        .await
        .unwrap();
        assert!(!search.results.is_empty());
        assert!(search
            .results
            .iter()
            .any(|result| result.text.to_lowercase().contains("memory is persistence")));
    }

    #[tokio::test]
    async fn test_mine_conversations_general_mode() {
        let temp = tempfile::TempDir::new().unwrap();
        let convo_dir = temp.path();
        let palace = temp.path().join("palace");
        std::fs::write(
            convo_dir.join("chat.txt"),
            "We decided to use Postgres because it handles concurrent writes better than SQLite.\n\nI always use snake_case for variable names.\n",
        )
        .unwrap();

        let result = mine_conversations(
            convo_dir,
            &palace,
            Some("test_convos"),
            "mempalace",
            0,
            false,
            Some("general"),
        )
        .await
        .unwrap();
        assert_eq!(result.files_processed, 1);
        assert!(result.chunks_created >= 1);

        let db = PalaceDb::open(&palace).unwrap();
        let entries = db.get_all(Some("test_convos"), None, 10);
        assert!(!entries.is_empty());
        assert!(entries.iter().any(|entry| {
            entry
                .metadatas
                .first()
                .and_then(|meta| meta.get("room"))
                .and_then(|value| value.as_str())
                .map(|room| room == "decision" || room == "preference")
                .unwrap_or(false)
        }));
    }

    #[tokio::test]
    async fn test_mine_conversations_dry_run_reports_counts_without_writes() {
        let temp = tempfile::TempDir::new().unwrap();
        let convo_dir = temp.path();
        let palace = temp.path().join("palace");
        std::fs::write(
            convo_dir.join("chat.txt"),
            "> What is memory?\nMemory is persistence.\n\n> Why does it matter?\nIt enables continuity.\n",
        )
        .unwrap();

        let result = mine_conversations(
            convo_dir,
            &palace,
            Some("test_convos"),
            "mempalace",
            0,
            true,
            Some("exchange"),
        )
        .await
        .unwrap();
        assert_eq!(result.files_processed, 1);
        assert!(result.chunks_created >= 1);

        let db = PalaceDb::open(&palace).unwrap();
        assert_eq!(db.count(), 0);
    }
}
