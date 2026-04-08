use crate::palace_db::PalaceDb;
use crate::room_detector_local::{detect_rooms_from_folders, RoomMapping};
use sha2::{Digest, Sha256};
use std::path::Path;
use walkdir::WalkDir;

const CHUNK_SIZE: usize = 800;
const CHUNK_OVERLAP: usize = 100;
const MIN_CHUNK_SIZE: usize = 50;

static READABLE_EXTENSIONS: &[&str] = &[
    ".txt", ".md", ".py", ".js", ".ts", ".jsx", ".tsx", ".json", ".yaml", ".yml", ".html", ".css",
    ".java", ".go", ".rs", ".rb", ".sh", ".csv", ".sql", ".toml",
];

static SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    "coverage",
    ".mempalace",
    ".target",
];

static SKIP_FILES: &[&str] = &[
    "mempalace.yaml",
    "mempalace.yml",
    "mempalace.json",
    "mempalace.lock",
    ".gitignore",
    "package-lock.json",
    "Cargo.lock",
];

#[derive(Debug)]
pub struct MiningResult {
    pub files_processed: usize,
    pub chunks_created: usize,
    pub errors: Vec<String>,
}

pub struct Miner {
    palace_db: PalaceDb,
    wing: String,
    rooms: Vec<RoomMapping>,
}

impl Miner {
    pub fn new(palace_path: &Path, wing: &str, rooms: Vec<RoomMapping>) -> anyhow::Result<Self> {
        let palace_db = PalaceDb::open(palace_path)?;
        Ok(Self {
            palace_db,
            wing: wing.to_string(),
            rooms,
        })
    }

    fn is_readable_file(path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            let ext_lower = format!(".{}", ext.to_string_lossy().to_lowercase());
            READABLE_EXTENSIONS.contains(&ext_lower.as_str())
        } else {
            false
        }
    }

    fn should_skip_dir(name: &std::ffi::OsStr) -> bool {
        if let Some(name_str) = name.to_str() {
            SKIP_DIRS.contains(&name_str)
        } else {
            false
        }
    }

    fn should_skip_file(name: &std::ffi::OsStr) -> bool {
        if let Some(name_str) = name.to_str() {
            SKIP_FILES.contains(&name_str)
        } else {
            false
        }
    }

    fn detect_room(&self, filepath: &Path, _content: &str) -> String {
        let project_path = filepath.parent().unwrap_or(filepath);
        if let Ok(relative) = filepath.strip_prefix(project_path) {
            let relative_str = relative.to_string_lossy().to_lowercase();
            let filename = filepath
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();

            for room in &self.rooms {
                let room_name_lower = room.name.to_lowercase();

                if relative_str.contains(&room_name_lower) || room_name_lower.contains(&filename) {
                    return room.name.clone();
                }
            }
        }

        "general".to_string()
    }

    fn chunk_text(&self, content: &str, _source_file: &str) -> Vec<(String, usize)> {
        let content = content.trim();
        if content.is_empty() {
            return vec![];
        }

        let mut chunks = Vec::new();
        let mut start = 0;
        let mut chunk_index = 0;

        while start < content.len() {
            let end = std::cmp::min(start + CHUNK_SIZE, content.len());

            if end < content.len() {
                let slice = &content[start..end];

                if let Some(newline_pos) = slice.rfind("\n\n") {
                    if newline_pos > CHUNK_SIZE / 2 {
                        let actual_end = start + newline_pos;
                        let chunk = content[start..actual_end].trim();
                        if chunk.len() >= MIN_CHUNK_SIZE {
                            chunks.push((chunk.to_string(), chunk_index));
                            chunk_index += 1;
                        }
                        start = actual_end + 2;
                        continue;
                    }
                }

                if let Some(newline_pos) = slice.rfind('\n') {
                    if newline_pos > CHUNK_SIZE / 2 {
                        let actual_end = start + newline_pos;
                        let chunk = content[start..actual_end].trim();
                        if chunk.len() >= MIN_CHUNK_SIZE {
                            chunks.push((chunk.to_string(), chunk_index));
                            chunk_index += 1;
                        }
                        start = actual_end + 1;
                        continue;
                    }
                }
            }

            let chunk = content[start..end].trim();
            if chunk.len() >= MIN_CHUNK_SIZE {
                chunks.push((chunk.to_string(), chunk_index));
                chunk_index += 1;
            }

            if end < content.len() {
                start = end.saturating_sub(CHUNK_OVERLAP);
            } else {
                break;
            }
        }

        chunks
    }

    fn generate_drawer_id(wing: &str, room: &str, source_file: &str, chunk_index: usize) -> String {
        let input = format!("{}_{}_{}_{}", source_file, wing, room, chunk_index);
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let result = hasher.finalize();
        let hex_str = hex::encode(&result[..4]);
        format!("drawer_{}_{}_{}_{}", wing, room, chunk_index, hex_str)
    }

    pub async fn mine_file(&mut self, filepath: &Path) -> anyhow::Result<usize> {
        let source_file = filepath.to_string_lossy().to_string();

        let content = match std::fs::read_to_string(filepath) {
            Ok(c) => c,
            Err(_) => return Ok(0),
        };

        let content = content.trim();
        if content.len() < MIN_CHUNK_SIZE {
            return Ok(0);
        }

        let room = self.detect_room(filepath, content);
        let chunks = self.chunk_text(content, &source_file);

        if chunks.is_empty() {
            return Ok(0);
        }

        let chunks_added = chunks.len();

        // Batch insert all chunks for this file in a single call
        let drawer_ids: Vec<String> = chunks
            .iter()
            .map(|(_chunk_content, chunk_index)| {
                Self::generate_drawer_id(&self.wing, &room, &source_file, *chunk_index)
            })
            .collect();

        let ids_and_docs: Vec<(&str, &str)> = drawer_ids
            .iter()
            .zip(chunks.iter())
            .map(|(id, (content, _))| (id.as_str(), content.as_str()))
            .collect();

        let mut metadata: Vec<Vec<(&str, &str)>> = Vec::new();
        for _ in &drawer_ids {
            metadata.push(vec![
                ("wing", self.wing.as_str()),
                ("room", room.as_str()),
                ("source_file", source_file.as_str()),
            ]);
        }
        let metadata_refs: Vec<&[(&str, &str)]> = metadata.iter().map(|v| v.as_slice()).collect();

        self.palace_db.add(&ids_and_docs, &metadata_refs)?;

        Ok(chunks_added)
    }

    pub async fn scan_and_mine(&mut self, project_dir: &Path) -> MiningResult {
        let file_paths: Vec<_> = WalkDir::new(project_dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.file_type().is_dir() {
                    !Self::should_skip_dir(e.file_name())
                } else if e.file_type().is_file() {
                    !Self::should_skip_file(e.file_name()) && Self::is_readable_file(e.path())
                } else {
                    false
                }
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.path().to_path_buf())
            .collect();

        // Sequential processing (parallelization requires mutable borrow of palace_db)
        let mut files_processed = 0;
        let mut chunks_created = 0;
        let mut errors = Vec::new();

        for filepath in file_paths {
            match self.mine_file(&filepath).await {
                Ok(count) => {
                    files_processed += 1;
                    chunks_created += count;
                }
                Err(e) => {
                    errors.push(format!("Error mining {:?}: {}", filepath, e));
                }
            }
        }

        // Flush once at end - critical for Windows performance
        self.palace_db.flush().ok();

        MiningResult {
            files_processed,
            chunks_created,
            errors,
        }
    }
}

#[derive(serde::Deserialize)]
struct Config {
    wing: String,
    rooms: Option<Vec<RoomMapping>>,
}

pub fn load_config(project_dir: &Path) -> anyhow::Result<(String, Vec<RoomMapping>)> {
    let config_paths = [
        project_dir.join("mempalace.json"),
        project_dir.join("mempalace.yaml"),
        project_dir.join("mempalace.yml"),
        project_dir.join("mempal.yaml"),
        project_dir.join("mempal.yml"),
    ];

    let config_path = config_paths
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("No mempalace config found in {:?}", project_dir))?;

    let content = std::fs::read_to_string(config_path)?;
    let config: Config = serde_json::from_str(&content)?;

    let rooms = config.rooms.unwrap_or_else(|| {
        vec![RoomMapping {
            name: "general".to_string(),
            description: "All project files".to_string(),
            keywords: vec![],
        }]
    });

    Ok((config.wing, rooms))
}

pub async fn mine(
    project_dir: &Path,
    palace_path: &Path,
    wing_override: Option<&str>,
    _exclude_patterns: Option<&[String]>,
) -> anyhow::Result<MiningResult> {
    let (wing, rooms) = load_config(project_dir)?;
    let wing = wing_override.unwrap_or(&wing);

    let rooms_to_use = if rooms.is_empty() {
        detect_rooms_from_folders(project_dir)
    } else {
        rooms
    };

    let mut miner = Miner::new(palace_path, wing, rooms_to_use)?;
    Ok(miner.scan_and_mine(project_dir).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_text_basic() {
        let miner = Miner::new(std::path::Path::new("/tmp"), "test", vec![]).unwrap();

        let text = "This is a test paragraph.\n\nThis is another paragraph.\n\nAnd another one here with enough content to be a chunk.";
        let chunks = miner.chunk_text(text, "test.txt");

        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_text_respects_min_size() {
        let miner = Miner::new(std::path::Path::new("/tmp"), "test", vec![]).unwrap();

        let text = "Short text";
        let chunks = miner.chunk_text(text, "test.txt");

        assert!(chunks.is_empty());
    }

    #[test]
    fn test_detect_room_fallback() {
        let rooms = vec![RoomMapping {
            name: "backend".to_string(),
            description: "Backend code".to_string(),
            keywords: vec!["backend".to_string()],
        }];
        let miner = Miner::new(std::path::Path::new("/tmp"), "test", rooms).unwrap();

        let room = miner.detect_room(std::path::Path::new("/tmp/unknown_file.txt"), "content");
        assert_eq!(room, "general");
    }

    #[test]
    fn test_is_readable_file() {
        assert!(Miner::is_readable_file(std::path::Path::new("test.py")));
        assert!(Miner::is_readable_file(std::path::Path::new("test.RS")));
        assert!(Miner::is_readable_file(std::path::Path::new("test.TXT")));
        assert!(!Miner::is_readable_file(std::path::Path::new("test.exe")));
        assert!(!Miner::is_readable_file(std::path::Path::new("test")));
    }

    #[test]
    fn test_generate_drawer_id() {
        let id1 = Miner::generate_drawer_id("wing1", "room1", "/path/file.rs", 0);
        let id2 = Miner::generate_drawer_id("wing1", "room1", "/path/file.rs", 0);
        let id3 = Miner::generate_drawer_id("wing1", "room1", "/path/file.rs", 1);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert!(id1.starts_with("drawer_wing1_room1_0_"));
    }
}
