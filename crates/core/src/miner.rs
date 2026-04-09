use crate::constants::{CHUNK_OVERLAP, CHUNK_SIZE, MIN_CHUNK_SIZE};
use crate::palace_db::PalaceDb;
use crate::room_detector_local::{detect_rooms_from_folders, RoomMapping};
use chrono::Utc;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use walkdir::WalkDir;

const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

static READABLE_EXTENSIONS: &[&str] = &[
    ".txt", ".md", ".py", ".js", ".ts", ".jsx", ".tsx", ".json", ".yaml", ".yml", ".html", ".css",
    ".java", ".go", ".rs", ".rb", ".sh", ".csv", ".sql", ".toml", ".c", ".cc", ".cpp", ".cxx",
    ".h", ".hh", ".hpp", ".hxx", ".inl", ".ixx",
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
    ".ruff_cache",
    ".mypy_cache",
    ".pytest_cache",
    ".cache",
    ".tox",
    ".nox",
    ".idea",
    ".vscode",
    ".ipynb_checkpoints",
    ".eggs",
    "htmlcov",
    "target",
];

static SKIP_FILES: &[&str] = &[
    "mempalace.yaml",
    "mempalace.yml",
    "mempal.yaml",
    "mempal.yml",
    ".gitignore",
    "package-lock.json",
];

#[derive(Debug, Clone)]
struct GitignoreRule {
    pattern: String,
    anchored: bool,
    dir_only: bool,
    negated: bool,
}

#[derive(Debug, Clone)]
struct GitignoreMatcher {
    base_dir: std::path::PathBuf,
    rules: Vec<GitignoreRule>,
}

impl GitignoreMatcher {
    fn from_dir(dir_path: &Path) -> Option<Self> {
        let gitignore_path = dir_path.join(".gitignore");
        if !gitignore_path.is_file() {
            return None;
        }

        let content = std::fs::read_to_string(&gitignore_path).ok()?;
        let mut rules = Vec::new();

        for raw_line in content.lines() {
            let mut line = raw_line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            if line.starts_with("\\#") || line.starts_with("\\!") {
                line = line[1..].to_string();
            } else if line.starts_with('#') {
                continue;
            }

            let negated = line.starts_with('!');
            if negated {
                line = line[1..].to_string();
            }

            let anchored = line.starts_with('/');
            if anchored {
                line = line.trim_start_matches('/').to_string();
            }

            let dir_only = line.ends_with('/');
            if dir_only {
                line = line.trim_end_matches('/').to_string();
            }

            if line.is_empty() {
                continue;
            }

            rules.push(GitignoreRule {
                pattern: line,
                anchored,
                dir_only,
                negated,
            });
        }

        if rules.is_empty() {
            None
        } else {
            Some(Self {
                base_dir: dir_path.to_path_buf(),
                rules,
            })
        }
    }

    fn matches(&self, path: &Path, is_dir: bool) -> Option<bool> {
        let relative = path.strip_prefix(&self.base_dir).ok()?.to_string_lossy();
        let relative = relative.replace('\\', "/").trim_matches('/').to_string();
        if relative.is_empty() {
            return None;
        }

        let mut ignored = None;
        for rule in &self.rules {
            if self.rule_matches(rule, &relative, is_dir) {
                ignored = Some(!rule.negated);
            }
        }
        ignored
    }

    fn rule_matches(&self, rule: &GitignoreRule, relative: &str, is_dir: bool) -> bool {
        let parts: Vec<&str> = relative.split('/').collect();
        let pattern_parts: Vec<&str> = rule.pattern.split('/').collect();

        if rule.dir_only {
            let target_parts = if is_dir {
                parts.clone()
            } else if parts.is_empty() {
                Vec::new()
            } else {
                parts[..parts.len().saturating_sub(1)].to_vec()
            };

            if target_parts.is_empty() {
                return false;
            }

            if rule.anchored || pattern_parts.len() > 1 {
                return Self::match_from_root(&target_parts, &pattern_parts);
            }

            return target_parts
                .iter()
                .any(|part| glob_matches(&rule.pattern, part));
        }

        if rule.anchored || pattern_parts.len() > 1 {
            return Self::match_from_root(&parts, &pattern_parts);
        }

        parts.iter().any(|part| glob_matches(&rule.pattern, part))
    }

    fn match_from_root(target_parts: &[&str], pattern_parts: &[&str]) -> bool {
        fn rec(target: &[&str], pattern: &[&str], ti: usize, pi: usize) -> bool {
            if pi == pattern.len() {
                return true;
            }
            if ti == target.len() {
                return pattern[pi..].iter().all(|p| *p == "**");
            }

            let current = pattern[pi];
            if current == "**" {
                return rec(target, pattern, ti, pi + 1) || rec(target, pattern, ti + 1, pi);
            }

            if !glob_matches(current, target[ti]) {
                return false;
            }

            rec(target, pattern, ti + 1, pi + 1)
        }

        rec(target_parts, pattern_parts, 0, 0)
    }
}

fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            _ => regex.push(ch),
        }
    }
    regex.push('$');
    Regex::new(&regex)
        .map(|re| re.is_match(candidate))
        .unwrap_or(false)
}

fn load_gitignore_matcher(
    dir_path: &Path,
    cache: &mut HashMap<std::path::PathBuf, Option<GitignoreMatcher>>,
) -> Option<GitignoreMatcher> {
    if let Some(existing) = cache.get(dir_path) {
        return existing.clone();
    }

    let matcher = GitignoreMatcher::from_dir(dir_path);
    cache.insert(dir_path.to_path_buf(), matcher.clone());
    matcher
}

fn is_gitignored(path: &Path, matchers: &[GitignoreMatcher], is_dir: bool) -> bool {
    let mut ignored = false;
    for matcher in matchers {
        if let Some(decision) = matcher.matches(path, is_dir) {
            ignored = decision;
        }
    }
    ignored
}

fn has_gitignored_ancestor(
    path: &Path,
    project_path: &Path,
    matchers: &[GitignoreMatcher],
) -> bool {
    let Ok(relative) = path.strip_prefix(project_path) else {
        return false;
    };

    let mut current = project_path.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    if components.len() <= 1 {
        return false;
    }

    for component in &components[..components.len() - 1] {
        current.push(component.as_os_str());
        if is_gitignored(&current, matchers, true) {
            return true;
        }
    }

    false
}

fn normalize_include_paths(include_ignored: Option<&[String]>) -> HashSet<String> {
    include_ignored
        .unwrap_or(&[])
        .iter()
        .flat_map(|raw| raw.split(','))
        .map(|raw| raw.trim().trim_matches('/').replace('\\', "/"))
        .filter(|s| !s.is_empty())
        .collect()
}

fn relative_posix(path: &Path, project_path: &Path) -> Option<String> {
    path.strip_prefix(project_path).ok().map(|p| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_matches('/')
            .to_string()
    })
}

fn is_exact_force_include(
    path: &Path,
    project_path: &Path,
    include_paths: &HashSet<String>,
) -> bool {
    relative_posix(path, project_path)
        .map(|relative| include_paths.contains(&relative))
        .unwrap_or(false)
}

fn is_force_included(path: &Path, project_path: &Path, include_paths: &HashSet<String>) -> bool {
    let Some(relative) = relative_posix(path, project_path) else {
        return false;
    };
    if relative.is_empty() {
        return false;
    }

    include_paths.iter().any(|include_path| {
        relative == *include_path
            || relative.starts_with(&format!("{include_path}/"))
            || include_path.starts_with(&format!("{relative}/"))
    })
}

pub fn scan_project(
    project_dir: &Path,
    respect_gitignore: bool,
    include_ignored: Option<&[String]>,
) -> Vec<std::path::PathBuf> {
    let project_path = match project_dir.canonicalize() {
        Ok(path) => path,
        Err(_) => project_dir.to_path_buf(),
    };

    let include_paths = normalize_include_paths(include_ignored);
    let mut files = Vec::new();
    let mut active_matchers: Vec<GitignoreMatcher> = Vec::new();
    let mut matcher_cache: HashMap<std::path::PathBuf, Option<GitignoreMatcher>> = HashMap::new();

    for entry in WalkDir::new(&project_path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path().to_path_buf();

        if entry.file_type().is_dir() {
            if respect_gitignore {
                active_matchers.retain(|matcher| {
                    path == matcher.base_dir || path.starts_with(&matcher.base_dir)
                });
                if let Some(current_matcher) = load_gitignore_matcher(&path, &mut matcher_cache) {
                    active_matchers.push(current_matcher);
                }
            }
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        let Some(parent) = path.parent() else {
            continue;
        };
        let Some(file_name) = path.file_name() else {
            continue;
        };

        let force_include = is_force_included(&path, &project_path, &include_paths);
        let exact_force_include = is_exact_force_include(&path, &project_path, &include_paths);

        let dir_components: Vec<_> = parent
            .strip_prefix(&project_path)
            .ok()
            .into_iter()
            .flat_map(|p| p.components())
            .collect();

        if !force_include
            && dir_components.iter().any(|component| {
                let name = component.as_os_str();
                Miner::should_skip_dir(name)
            })
        {
            continue;
        }

        if !force_include && Miner::should_skip_file(file_name) {
            continue;
        }

        if !exact_force_include && !Miner::is_readable_file(&path) {
            continue;
        }

        if respect_gitignore && !force_include && is_gitignored(&path, &active_matchers, false) {
            continue;
        }

        if respect_gitignore
            && !force_include
            && has_gitignored_ancestor(&path, &project_path, &active_matchers)
        {
            continue;
        }

        if path.is_symlink() {
            continue;
        }

        let Ok(metadata) = path.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_SIZE {
            continue;
        }

        files.push(path);
    }

    files.sort();
    files
}

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
            SKIP_DIRS.contains(&name_str) || name_str.ends_with(".egg-info")
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
        let filename = filepath
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let path_parts: Vec<String> = filepath
            .parent()
            .into_iter()
            .flat_map(|parent| parent.components())
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect();

        for part in path_parts {
            for room in &self.rooms {
                let mut candidates = vec![room.name.to_lowercase()];
                candidates.extend(room.keywords.iter().map(|keyword| keyword.to_lowercase()));
                if candidates.iter().any(|candidate| {
                    part == *candidate || candidate.contains(&part) || part.contains(candidate)
                }) {
                    return room.name.clone();
                }
            }
        }

        for room in &self.rooms {
            let room_name_lower = room.name.to_lowercase();
            if room_name_lower.contains(&filename) || filename.contains(&room_name_lower) {
                return room.name.clone();
            }
        }

        let content_lower = _content
            .chars()
            .take(2000)
            .collect::<String>()
            .to_lowercase();
        let mut best_room = None;
        let mut best_score = 0;
        for room in &self.rooms {
            let score: usize = room
                .keywords
                .iter()
                .chain(std::iter::once(&room.name))
                .map(|keyword| content_lower.matches(&keyword.to_lowercase()).count())
                .sum();
            if score > best_score {
                best_score = score;
                best_room = Some(room.name.clone());
            }
        }

        if best_score > 0 {
            if let Some(room) = best_room {
                return room;
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
                        start = actual_end.saturating_sub(CHUNK_OVERLAP);
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
                        start = actual_end.saturating_sub(CHUNK_OVERLAP);
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
        let input = format!("{}{}", source_file, chunk_index);
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let result = hasher.finalize();
        let hex_str = hex::encode(result);
        format!("drawer_{}_{}_{}", wing, room, &hex_str[..24])
    }

    pub async fn mine_file(&mut self, filepath: &Path) -> anyhow::Result<usize> {
        let source_file = filepath.to_string_lossy().to_string();

        if self.palace_db.file_already_mined(&source_file, true) {
            return Ok(0);
        }

        let source_mtime = std::fs::metadata(filepath)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64().to_string());

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

        let filed_at = Utc::now().to_rfc3339();
        let chunk_indexes: Vec<String> = chunks
            .iter()
            .map(|(_, chunk_index)| chunk_index.to_string())
            .collect();
        let mut metadata: Vec<Vec<(&str, &str)>> = Vec::new();
        for (i, _) in drawer_ids.iter().enumerate() {
            let mut chunk_metadata = vec![
                ("wing", self.wing.as_str()),
                ("room", room.as_str()),
                ("source_file", source_file.as_str()),
                ("chunk_index", chunk_indexes[i].as_str()),
                ("added_by", "mempalace"),
                ("filed_at", filed_at.as_str()),
            ];
            if let Some(source_mtime) = source_mtime.as_deref() {
                chunk_metadata.push(("source_mtime", source_mtime));
            }
            metadata.push(chunk_metadata);
        }
        let metadata_refs: Vec<&[(&str, &str)]> = metadata.iter().map(|v| v.as_slice()).collect();

        self.palace_db.add(&ids_and_docs, &metadata_refs)?;

        Ok(chunks_added)
    }

    pub async fn scan_and_mine(&mut self, project_dir: &Path) -> MiningResult {
        let file_paths = scan_project(project_dir, true, None);

        // Sequential processing (parallelization requires mutable borrow of palace_db)
        let mut files_processed = 0;
        let mut chunks_created = 0;
        let mut errors = Vec::new();

        for filepath in file_paths {
            match self.mine_file(&filepath).await {
                Ok(count) => {
                    if count > 0 {
                        files_processed += 1;
                        chunks_created += count;
                    }
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
    let config: Config = match config_path.extension().and_then(|ext| ext.to_str()) {
        Some("yaml") | Some("yml") => serde_yaml::from_str(&content)?,
        _ => serde_json::from_str(&content)?,
    };

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
    exclude_patterns: Option<&[String]>,
) -> anyhow::Result<MiningResult> {
    let (wing, rooms) = load_config(project_dir)?;
    let wing = wing_override.unwrap_or(&wing);

    let rooms_to_use = if rooms.is_empty() {
        detect_rooms_from_folders(project_dir)
    } else {
        rooms
    };

    let mut miner = Miner::new(palace_path, wing, rooms_to_use)?;

    let file_paths = scan_project(project_dir, true, exclude_patterns);
    let mut files_processed = 0;
    let mut chunks_created = 0;
    let mut errors = Vec::new();

    for filepath in file_paths {
        match miner.mine_file(&filepath).await {
            Ok(count) => {
                if count > 0 {
                    files_processed += 1;
                    chunks_created += count;
                }
            }
            Err(e) => errors.push(format!("Error mining {:?}: {}", filepath, e)),
        }
    }

    miner.palace_db.flush().ok();

    Ok(MiningResult {
        files_processed,
        chunks_created,
        errors,
    })
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
        assert!(id1.starts_with("drawer_wing1_room1_"));
        assert_eq!(id1.len(), "drawer_wing1_room1_".len() + 24);
    }

    #[tokio::test]
    async fn test_mine_file_skips_when_source_mtime_matches() {
        let temp = tempfile::TempDir::new().unwrap();
        let palace = temp.path().join("palace");
        let file = temp.path().join("app.py");
        std::fs::write(&file, "print('hello')\n".repeat(40)).unwrap();

        let rooms = vec![RoomMapping {
            name: "general".to_string(),
            description: "General".to_string(),
            keywords: vec![],
        }];

        let mut miner = Miner::new(&palace, "wing", rooms.clone()).unwrap();
        let first = miner.mine_file(&file).await.unwrap();
        assert!(first > 0);
        miner.palace_db.flush().unwrap();

        let remine = Miner::new(&palace, "wing", rooms).unwrap();
        assert!(remine
            .palace_db
            .file_already_mined(&file.to_string_lossy(), true));

        let mut remine = remine;
        let second = remine.mine_file(&file).await.unwrap();
        assert_eq!(second, 0);
    }

    #[test]
    fn test_detect_room_matches_room_keywords_and_path_segments() {
        let rooms = vec![RoomMapping {
            name: "backend".to_string(),
            description: "Backend code".to_string(),
            keywords: vec!["authentication".to_string(), "jwt".to_string()],
        }];
        let miner = Miner::new(std::path::Path::new("/tmp"), "test", rooms).unwrap();

        let room = miner.detect_room(
            std::path::Path::new("/tmp/project/backend/auth.py"),
            "JWT authentication uses bearer tokens",
        );
        assert_eq!(room, "backend");
    }

    #[test]
    fn test_scan_project_respects_gitignore() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "ignored.py\ngenerated/\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::write(root.join("src/app.py"), "print('hello')\n".repeat(20)).unwrap();
        std::fs::write(root.join("ignored.py"), "print('ignore')\n".repeat(20)).unwrap();
        std::fs::write(
            root.join("generated/artifact.py"),
            "print('artifact')\n".repeat(20),
        )
        .unwrap();

        let files = scan_project(root, true, None);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["src/app.py"]);
    }

    #[test]
    fn test_scan_project_nested_gitignore_override() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "*.csv\n").unwrap();
        std::fs::create_dir_all(root.join("subrepo")).unwrap();
        std::fs::write(root.join("subrepo/.gitignore"), "!keep.csv\n").unwrap();
        std::fs::write(root.join("drop.csv"), "a,b,c\n".repeat(20)).unwrap();
        std::fs::write(root.join("subrepo/keep.csv"), "a,b,c\n".repeat(20)).unwrap();

        let files = scan_project(root, true, None);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["subrepo/keep.csv"]);
    }

    #[test]
    fn test_scan_project_does_not_reinclude_file_from_ignored_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "generated/\n!generated/keep.py\n").unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::write(root.join("generated/drop.py"), "print('drop')\n".repeat(20)).unwrap();
        std::fs::write(root.join("generated/keep.py"), "print('keep')\n".repeat(20)).unwrap();

        let files = scan_project(root, true, None);
        assert!(files.is_empty());
    }

    #[test]
    fn test_scan_project_can_disable_gitignore() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "data/\n").unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/stuff.csv"), "a,b,c\n".repeat(20)).unwrap();

        let files = scan_project(root, false, None);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["data/stuff.csv"]);
    }

    #[test]
    fn test_scan_project_can_include_specific_ignored_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "generated/\n").unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::write(root.join("generated/drop.py"), "print('drop')\n".repeat(20)).unwrap();
        std::fs::write(root.join("generated/keep.py"), "print('keep')\n".repeat(20)).unwrap();

        let includes = vec!["generated/keep.py".to_string()];
        let files = scan_project(root, true, Some(&includes));
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["generated/keep.py"]);
    }

    #[test]
    fn test_scan_project_include_override_beats_skip_dirs() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join(".pytest_cache")).unwrap();
        std::fs::write(
            root.join(".pytest_cache/cache.py"),
            "print('cache')\n".repeat(20),
        )
        .unwrap();

        let includes = vec![".pytest_cache".to_string()];
        let files = scan_project(root, false, Some(&includes));
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec![".pytest_cache/cache.py"]);
    }
}
