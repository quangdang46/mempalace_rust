use crate::constants::{CHUNK_OVERLAP, CHUNK_SIZE, MIN_CHUNK_SIZE};
use crate::palace_db::PalaceDb;
use crate::room_detector_local::{detect_rooms_from_folders, RoomMapping};
use chrono::Utc;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use walkdir::WalkDir;

const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

static READABLE_EXTENSIONS: &[&str] = &[
    ".txt",
    ".md",
    ".py",
    ".js",
    ".ts",
    ".jsx",
    ".tsx",
    ".json",
    ".yaml",
    ".yml",
    ".html",
    ".css",
    ".java",
    ".go",
    ".rs",
    ".rb",
    ".sh",
    ".csv",
    ".sql",
    ".toml",
    ".c",
    ".cc",
    ".cpp",
    ".cxx",
    ".h",
    ".hh",
    ".hpp",
    ".hxx",
    ".inl",
    ".ixx",
    ".php",
    ".blade",
    ".twig",
    ".vue",
    ".svelte",
    ".astro",
    ".dart",
    ".swift",
    ".kt",
    ".kts",
    ".scala",
    ".erb",
    ".bash",
    ".zsh",
    ".fish",
    ".ps1",
    ".psm1",
    ".conf",
    ".ini",
    ".cfg",
    ".properties",
    ".xml",
    ".rss",
    ".atom",
    ".jsonl",
    ".ndjson",
    ".lock",
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
    ".obsidian",
    ".vault",
    ".vite",
    ".parcel-cache",
    ".turbo",
    ".nuxt",
    "vendor",
    "vendored",
    "node_modules",
    "bower_components",
    ".sass-cache",
];

static SKIP_FILES: &[&str] = &[
    "entities.json",
    "mempalace.json",
    "mempalace.yaml",
    "mempalace.yml",
    "mempal.yaml",
    "mempal.yml",
    ".gitignore",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
];

/// Default cap on chunks produced from a single file.
///
/// A safety rail against pathological generated artifacts (lockfiles
/// not in `SKIP_FILES`, vendored data dumps, etc.). Originally 500 to
/// bound ONNX runtime `bad allocation` errors on Windows (upstream
/// mempalace `5488e7b`, #1296), but at `CHUNK_SIZE` (800 chars) that
/// capped legitimate long-form content (full-text scholarly editions,
/// novels — upstream #1455) at ~400 KB. The new default leaves two
/// orders of magnitude of safety margin against the original lockfile
/// case while not touching hand-written prose. Override via
/// `MEMPALACE_MAX_CHUNKS_PER_FILE` env var, the `--max-chunks-per-file`
/// CLI flag, or by constructing a `Miner` with `with_max_chunks_per_file`.
/// Set to 0 (from any source) to disable the cap entirely.
const MAX_CHUNKS_PER_FILE: usize = 50_000;

/// Skip reason returned by `Miner::mine_file` so callers can surface
/// chunk-cap drops in mine summaries independently of the residual
/// already-filed / unreadable / too-short bucket. Mirrors upstream
/// mempalace's `skip_reason` tuple field added in #1455.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// File produced more than the configured `MAX_CHUNKS_PER_FILE`.
    ChunkCap,
}

/// Resolve the effective per-file chunk cap.
///
/// Precedence: explicit `override_value` (e.g. the `--max-chunks-per-file`
/// CLI flag) > `MEMPALACE_MAX_CHUNKS_PER_FILE` env var > the module-level
/// `MAX_CHUNKS_PER_FILE` default. A sentinel value of `0` (from any
/// source) disables the cap entirely. Negative or non-numeric values
/// from the env var emit a stderr warning and fall back to the default,
/// matching upstream's behavior so a misconfigured
/// `MEMPALACE_MAX_CHUNKS_PER_FILE=-500` typo does not silently disable
/// the cap and OOM on a generated artifact (upstream mempalace #1455).
pub fn resolve_max_chunks_per_file(override_value: Option<usize>) -> usize {
    if let Some(v) = override_value {
        return v;
    }
    match std::env::var("MEMPALACE_MAX_CHUNKS_PER_FILE") {
        Ok(raw) => match raw.trim().parse::<i64>() {
            Ok(val) if val < 0 => {
                eprintln!(
                    "  ! WARNING: MEMPALACE_MAX_CHUNKS_PER_FILE={raw:?} is negative; using default {MAX_CHUNKS_PER_FILE}"
                );
                MAX_CHUNKS_PER_FILE
            }
            Ok(val) => val as usize,
            Err(_) => {
                eprintln!(
                    "  ! WARNING: MEMPALACE_MAX_CHUNKS_PER_FILE={raw:?} is not an integer; using default {MAX_CHUNKS_PER_FILE}"
                );
                MAX_CHUNKS_PER_FILE
            }
        },
        Err(_) => MAX_CHUNKS_PER_FILE,
    }
}

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

/// Split `value` into lowercased tokens bounded by `-`, `_`, `.` or `/`.
fn tokens(value: &str) -> HashSet<String> {
    value
        .to_lowercase()
        .split(['-', '_', '.', '/'])
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// Return true when `a` and `b` match as equal strings or as
/// separator-bounded tokens of each other.
///
/// Prevents incidental substring collisions (e.g., `"views" in "interviews"`)
/// that a raw `contains` check would produce, while preserving the intended
/// match for real tokens (e.g., `"frontend"` in `"frontend-app"`).
fn name_matches(a: &str, b: &str) -> bool {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    if a_lower == b_lower {
        return true;
    }
    tokens(&a_lower).contains(&b_lower) || tokens(&b_lower).contains(&a_lower)
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

/// Scan `project_dir` for readable files, returning paths to mine.
///
/// Skips symlinks (which could otherwise follow links to `/dev/urandom`,
/// recursive structures, etc.) and oversized files. Each skipped symlink is
/// logged to `stderr` with a `"  SKIP: <relative-path> (symlink)"` line so
/// callers can tell why a directory looks empty after walking (#1462).
/// Walk `project_dir` and return readable file paths, logging each
/// skipped symlink to `stderr` with a `"  SKIP: <relative-path> (symlink)"`
/// line so callers can tell why a directory looks empty after walking.
/// Mirrors upstream Python `scan_project` post-#1462. (#1462)
pub fn scan_project(
    project_dir: &Path,
    respect_gitignore: bool,
    include_ignored: Option<&[String]>,
) -> Vec<std::path::PathBuf> {
    scan_project_with_log(
        project_dir,
        respect_gitignore,
        include_ignored,
        &mut std::io::stderr(),
    )
}

/// Same as [`scan_project`] but routes the skipped-symlink diagnostic to an
/// arbitrary writer. Lets unit tests assert the log fires without having to
/// fork a subprocess to capture stderr.
fn scan_project_with_log<W: Write>(
    project_dir: &Path,
    respect_gitignore: bool,
    include_ignored: Option<&[String]>,
    skip_log: &mut W,
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
        let ft = entry.file_type();

        if ft.is_dir() {
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

        // Let regular files AND symlinks through. Walkdir's `is_file()`
        // returns `false` for symlinks-to-files under `follow_links(false)`,
        // so a bare `!is_file()` check would silently drop every symlink
        // before the diagnostic branch below can fire — that was the bug in
        // the initial port of upstream #1462.
        if !ft.is_file() && !ft.is_symlink() {
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

        // Skip symlinks — prevents following links to /dev/urandom, recursive
        // structures, etc. Log to `skip_log` with the path relative to the
        // scan root so a nested symlink in a deep subdirectory is unambiguous
        // and the log renders with forward slashes on every platform. stdout
        // stays clean for "Files: N" / "Drawers filed: N" markers callers
        // parse. Mirrors upstream Python `scan_project` post-#1462. (#1462)
        if ft.is_symlink() {
            let rel = path
                .strip_prefix(&project_path)
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

        files.push(path);
    }

    files.sort();
    files
}

#[derive(Debug, Default)]
pub struct MiningResult {
    pub files_processed: usize,
    pub chunks_created: usize,
    pub errors: Vec<String>,
    /// Files dropped because they exceeded the per-file chunk cap.
    /// Surfaced separately from any other skip path so a corpus audit
    /// can tell chunk-cap drops apart from "already filed / read error"
    /// (upstream mempalace #1455).
    pub files_skipped_chunk_cap: usize,
}

pub struct Miner {
    palace_db: PalaceDb,
    wing: String,
    rooms: Vec<RoomMapping>,
    /// Per-Miner override for the chunk cap. `None` falls back to
    /// `MEMPALACE_MAX_CHUNKS_PER_FILE` then the module default. `Some(0)`
    /// disables the cap entirely (upstream mempalace #1455).
    max_chunks_per_file: Option<usize>,
}

impl Miner {
    pub fn new(palace_path: &Path, wing: &str, rooms: Vec<RoomMapping>) -> anyhow::Result<Self> {
        let palace_db = PalaceDb::open(palace_path)?;
        Ok(Self {
            palace_db,
            wing: wing.to_string(),
            rooms,
            max_chunks_per_file: None,
        })
    }

    /// Override the per-file chunk cap for this `Miner`. `Some(0)`
    /// disables the cap entirely; `None` falls back to the env var
    /// (`MEMPALACE_MAX_CHUNKS_PER_FILE`) and then the module default.
    pub fn with_max_chunks_per_file(mut self, override_value: Option<usize>) -> Self {
        self.max_chunks_per_file = override_value;
        self
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
                if candidates
                    .iter()
                    .any(|candidate| name_matches(&part, candidate))
                {
                    return room.name.clone();
                }
            }
        }

        for room in &self.rooms {
            let room_name_lower = room.name.to_lowercase();
            if name_matches(&filename, &room_name_lower) {
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
            let mut end = std::cmp::min(start + CHUNK_SIZE, content.len());
            // Snap end to a valid UTF-8 char boundary
            while end > start && !content.is_char_boundary(end) {
                end -= 1;
            }

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
                        let mut new_start = actual_end.saturating_sub(CHUNK_OVERLAP);
                        while new_start < actual_end && !content.is_char_boundary(new_start) {
                            new_start += 1;
                        }
                        start = new_start;
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
                        let mut new_start = actual_end.saturating_sub(CHUNK_OVERLAP);
                        while new_start < actual_end && !content.is_char_boundary(new_start) {
                            new_start += 1;
                        }
                        start = new_start;
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
                let mut new_start = end.saturating_sub(CHUNK_OVERLAP);
                while new_start < end && !content.is_char_boundary(new_start) {
                    new_start += 1;
                }
                start = new_start;
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

    /// Mine one file. Returns `(chunks_added, skip_reason)`. `skip_reason`
    /// is `None` on success and on every non-chunk-cap skip path (already
    /// filed, unreadable, too short, chunker empty); it is
    /// `Some(SkipReason::ChunkCap)` when the per-file chunk cap aborted
    /// the file. Callers use the tag to surface a separate counter in
    /// the mine summary (upstream mempalace #1455).
    pub async fn mine_file(
        &mut self,
        filepath: &Path,
    ) -> anyhow::Result<(usize, Option<SkipReason>)> {
        let source_file = filepath.to_string_lossy().to_string();

        if self.palace_db.file_already_mined(&source_file, true) {
            return Ok((0, None));
        }

        let source_mtime = std::fs::metadata(filepath)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64().to_string());

        let content = match std::fs::read_to_string(filepath) {
            Ok(c) => c,
            Err(_) => return Ok((0, None)),
        };

        let content = content.trim();
        if content.len() < MIN_CHUNK_SIZE {
            return Ok((0, None));
        }

        let room = self.detect_room(filepath, content);
        let chunks = self.chunk_text(content, &source_file);

        if chunks.is_empty() {
            return Ok((0, None));
        }

        let effective_cap = resolve_max_chunks_per_file(self.max_chunks_per_file);
        if effective_cap > 0 && chunks.len() > effective_cap {
            // Skip notice goes to stderr (upstream mempalace #1455) so
            // `mpr mine ... > out.log 2> err.log` piping stays coherent:
            // degraded outcomes on stderr, progress on stdout. Raised
            // default (50,000) means hand-written long-form content no
            // longer trips the cap; the env var / CLI flag exist for
            // operators who need a lower bound on Windows ONNX builds.
            let display = filepath
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| source_file.clone());
            let truncated: String = display.chars().take(50).collect();
            eprintln!(
                "  ! [skip] {:<50} produced {} chunks (> {}); raise via --max-chunks-per-file or MEMPALACE_MAX_CHUNKS_PER_FILE (set 0 to disable), or add to SKIP_FILES if this is a generated artifact",
                truncated,
                chunks.len(),
                effective_cap
            );
            return Ok((0, Some(SkipReason::ChunkCap)));
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
        let normalize_version = crate::constants::NORMALIZE_VERSION.to_string();
        for (i, _) in drawer_ids.iter().enumerate() {
            let mut chunk_metadata = vec![
                ("wing", self.wing.as_str()),
                ("room", room.as_str()),
                ("source_file", source_file.as_str()),
                ("chunk_index", chunk_indexes[i].as_str()),
                ("added_by", "mempalace"),
                ("filed_at", filed_at.as_str()),
                ("normalize_version", normalize_version.as_str()),
            ];
            if let Some(source_mtime) = source_mtime.as_deref() {
                chunk_metadata.push(("source_mtime", source_mtime));
            }
            metadata.push(chunk_metadata);
        }
        let metadata_refs: Vec<&[(&str, &str)]> = metadata.iter().map(|v| v.as_slice()).collect();

        self.palace_db.add(&ids_and_docs, &metadata_refs)?;

        Ok((chunks_added, None))
    }

    pub async fn scan_and_mine(&mut self, project_dir: &Path) -> MiningResult {
        let file_paths = scan_project(project_dir, true, None);

        // Sequential processing (parallelization requires mutable borrow of palace_db)
        let mut files_processed = 0;
        let mut chunks_created = 0;
        let mut files_skipped_chunk_cap = 0;
        let mut errors = Vec::new();

        for filepath in file_paths {
            match self.mine_file(&filepath).await {
                Ok((count, skip_reason)) => {
                    if count > 0 {
                        files_processed += 1;
                        chunks_created += count;
                    }
                    if skip_reason == Some(SkipReason::ChunkCap) {
                        files_skipped_chunk_cap += 1;
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
            files_skipped_chunk_cap,
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
    mine_with_options(project_dir, palace_path, wing_override, exclude_patterns, None).await
}

/// Same as `mine` but accepts an explicit per-file chunk cap override.
/// `None` defers to `MEMPALACE_MAX_CHUNKS_PER_FILE` then the module default;
/// `Some(0)` disables the cap entirely (upstream mempalace #1455).
pub async fn mine_with_options(
    project_dir: &Path,
    palace_path: &Path,
    wing_override: Option<&str>,
    exclude_patterns: Option<&[String]>,
    max_chunks_per_file: Option<usize>,
) -> anyhow::Result<MiningResult> {
    let (wing, rooms) = load_config(project_dir)?;
    let wing = wing_override.unwrap_or(&wing);

    let rooms_to_use = if rooms.is_empty() {
        detect_rooms_from_folders(project_dir)
    } else {
        rooms
    };

    let mut miner = Miner::new(palace_path, wing, rooms_to_use)?
        .with_max_chunks_per_file(max_chunks_per_file);

    let file_paths = scan_project(project_dir, true, exclude_patterns);
    let mut files_processed = 0;
    let mut chunks_created = 0;
    let mut files_skipped_chunk_cap = 0;
    let mut errors = Vec::new();

    for filepath in file_paths {
        match miner.mine_file(&filepath).await {
            Ok((count, skip_reason)) => {
                if count > 0 {
                    files_processed += 1;
                    chunks_created += count;
                }
                if skip_reason == Some(SkipReason::ChunkCap) {
                    files_skipped_chunk_cap += 1;
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
        files_skipped_chunk_cap,
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
        let (first, skip) = miner.mine_file(&file).await.unwrap();
        assert!(first > 0);
        assert_eq!(skip, None);
        miner.palace_db.flush().unwrap();

        let remine = Miner::new(&palace, "wing", rooms).unwrap();
        assert!(remine
            .palace_db
            .file_already_mined(&file.to_string_lossy(), true));

        let mut remine = remine;
        let (second, skip) = remine.mine_file(&file).await.unwrap();
        assert_eq!(second, 0);
        assert_eq!(skip, None);
    }

    #[test]
    fn test_file_already_mined_without_mtime_fails_check_mtime() {
        let temp = tempfile::TempDir::new().unwrap();
        let palace = temp.path().join("palace");
        let file = temp.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let mut db = PalaceDb::open(&palace).unwrap();
        let file_path = file.to_string_lossy().to_string();
        db.add(
            &[("d1", "hello world")],
            &[&[
                ("source_file", file_path.as_str()),
                ("normalize_version", "2"),
            ]],
        )
        .unwrap();
        db.flush().unwrap();

        assert!(db.file_already_mined(file_path.as_str(), false));
        assert!(!db.file_already_mined(file_path.as_str(), true));
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
    fn test_name_matches_token_boundary() {
        // exact match
        assert!(name_matches("backend", "backend"));
        // case-insensitive equality
        assert!(name_matches("Backend", "BACKEND"));
        // separator-bounded token match
        assert!(name_matches("frontend-app", "frontend"));
        assert!(name_matches("auth_service", "auth"));
        assert!(name_matches("api.v1", "api"));
        // negative: substring without token boundary must NOT match
        assert!(!name_matches("interviews", "views"));
        assert!(!name_matches("background", "back"));
        assert!(!name_matches("authenticate", "auth"));
    }

    #[test]
    fn test_detect_room_does_not_misroute_substring() {
        // Regression for upstream mempalace ead2c5d: "views" ⊂ "interviews"
        // must not route every interviews/* file to a "views" room.
        let rooms = vec![RoomMapping {
            name: "views".to_string(),
            description: "View templates".to_string(),
            keywords: vec![],
        }];
        let miner = Miner::new(std::path::Path::new("/tmp"), "test", rooms).unwrap();

        let room = miner.detect_room(
            std::path::Path::new("/tmp/project/interviews/q1.py"),
            "interview question",
        );
        assert_eq!(room, "general");
    }

    #[test]
    fn test_detect_room_matches_separator_bounded_token() {
        let rooms = vec![RoomMapping {
            name: "frontend".to_string(),
            description: "Frontend code".to_string(),
            keywords: vec![],
        }];
        let miner = Miner::new(std::path::Path::new("/tmp"), "test", rooms).unwrap();

        let room = miner.detect_room(
            std::path::Path::new("/tmp/project/frontend-app/index.tsx"),
            "import React",
        );
        assert_eq!(room, "frontend");
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_project_skips_symlinks() {
        // Regression for upstream #1462: symlinks are skipped so the walker
        // never follows links to /dev/urandom, recursive directories, or
        // resources outside the scan root. The "Files: 0" outcome surfaces
        // as a stderr SKIP log so callers can distinguish "no files" from
        // "all the files were symlinks". Asserts the diagnostic actually
        // fires — relying on result-set exclusion alone passes against
        // dead-code symlink branches, which is how the initial port shipped.
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let canonical_root = root.canonicalize().unwrap();
        let real = root.join("real.py");
        std::fs::write(&real, "print('real')\n".repeat(20)).unwrap();
        std::os::unix::fs::symlink(&real, root.join("link.py")).unwrap();

        let mut log = Vec::new();
        let files = scan_project_with_log(root, false, None, &mut log);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["real.py"]);
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.contains("  SKIP: link.py (symlink)\n"),
            "expected SKIP diagnostic for link.py, got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_project_skips_nested_symlinks() {
        // Nested symlinks (e.g. inside a subdirectory) are reported with the
        // path relative to the scan root, not the leaf filename, so they can
        // be located unambiguously. See upstream `d7d9604` polish to #1462.
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let canonical_root = root.canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        let real = root.join("a/b/real.py");
        std::fs::write(&real, "print('real')\n".repeat(20)).unwrap();
        std::os::unix::fs::symlink(&real, root.join("a/b/link.py")).unwrap();

        let mut log = Vec::new();
        let files = scan_project_with_log(root, false, None, &mut log);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["a/b/real.py"]);
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.contains("  SKIP: a/b/link.py (symlink)\n"),
            "expected scan-root-relative SKIP path, got: {log:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_scan_project_skips_dangling_symlinks() {
        // A dangling symlink (target does not exist) must not panic the
        // walker nor surface in the result set. Mirrors upstream coverage
        // for the polished #1462 case.
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let canonical_root = root.canonicalize().unwrap();
        std::fs::write(root.join("real.py"), "print('real')\n".repeat(20)).unwrap();
        std::os::unix::fs::symlink(root.join("missing.py"), root.join("dangling.py")).unwrap();

        let mut log = Vec::new();
        let files = scan_project_with_log(root, false, None, &mut log);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["real.py"]);
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.contains("  SKIP: dangling.py (symlink)\n"),
            "expected SKIP diagnostic for dangling.py, got: {log:?}"
        );
    }

    #[test]
    fn test_scan_project_emits_no_skip_lines_when_no_symlinks_present() {
        // Negative control: a regular directory with only real files must
        // not emit any SKIP diagnostics on the writer.
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        std::fs::write(root.join("a.py"), "print('a')\n".repeat(20)).unwrap();
        std::fs::write(root.join("b.py"), "print('b')\n".repeat(20)).unwrap();

        let mut log = Vec::new();
        let files = scan_project_with_log(root, false, None, &mut log);
        assert_eq!(files.len(), 2);
        let log = String::from_utf8(log).unwrap();
        assert!(
            !log.contains("SKIP"),
            "no symlinks present, expected empty log, got: {log:?}"
        );
    }

    #[test]
    fn test_scan_project_respects_gitignore() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let canonical_root = root.canonicalize().unwrap();
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
                p.strip_prefix(&canonical_root)
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
        let canonical_root = root.canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), "*.csv\n").unwrap();
        std::fs::create_dir_all(root.join("subrepo")).unwrap();
        std::fs::write(root.join("subrepo/.gitignore"), "!keep.csv\n").unwrap();
        std::fs::write(root.join("drop.csv"), "a,b,c\n".repeat(20)).unwrap();
        std::fs::write(root.join("subrepo/keep.csv"), "a,b,c\n".repeat(20)).unwrap();

        let files = scan_project(root, true, None);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
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
        let canonical_root = root.canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), "data/\n").unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/stuff.csv"), "a,b,c\n".repeat(20)).unwrap();

        let files = scan_project(root, false, None);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
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
        let canonical_root = root.canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), "generated/\n").unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::write(root.join("generated/drop.py"), "print('drop')\n".repeat(20)).unwrap();
        std::fs::write(root.join("generated/keep.py"), "print('keep')\n".repeat(20)).unwrap();

        let includes = vec!["generated/keep.py".to_string()];
        let files = scan_project(root, true, Some(&includes));
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
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
        let canonical_root = root.canonicalize().unwrap();
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
                p.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec![".pytest_cache/cache.py"]);
    }

    #[test]
    fn test_scan_project_can_include_exact_file_without_known_extension() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let canonical_root = root.canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), "README\n").unwrap();
        std::fs::write(root.join("README"), "hello\n".repeat(20)).unwrap();

        let includes = vec!["README".to_string()];
        let files = scan_project(root, true, Some(&includes));
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["README"]);
    }

    #[test]
    fn test_scan_project_skip_dirs_still_apply_without_override() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let canonical_root = root.canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".pytest_cache")).unwrap();
        std::fs::write(
            root.join(".pytest_cache/cache.py"),
            "print('cache')\n".repeat(20),
        )
        .unwrap();
        std::fs::write(root.join("main.py"), "print('main')\n".repeat(20)).unwrap();

        let files = scan_project(root, false, None);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["main.py"]);
    }

    #[test]
    fn test_scan_project_skips_mempalace_config() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        let canonical_root = root.canonicalize().unwrap();
        std::fs::write(root.join("mempalace.json"), "{}\n").unwrap();
        std::fs::write(root.join("main.rs"), "fn main() {}\n".repeat(20)).unwrap();

        let files = scan_project(root, false, None);
        let rel: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(rel, vec!["main.rs"]);
    }

    #[tokio::test]
    async fn test_mine_reports_backend_room_metadata_for_project_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("project");
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(project.join("backend")).unwrap();
        std::fs::write(
            project.join("backend/auth.py"),
            "JWT authentication uses bearer tokens\n".repeat(30),
        )
        .unwrap();
        std::fs::write(
            project.join("mempalace.yaml"),
            "wing: project\nrooms:\n  - name: backend\n    description: Backend code\n    keywords: [JWT, authentication]\n",
        )
        .unwrap();

        let result = mine(&project, &palace, None, None).await.unwrap();
        assert_eq!(result.files_processed, 1);
        assert!(result.chunks_created > 0);

        let db = PalaceDb::open(&palace).unwrap();
        let entries = db.get_all(Some("project"), Some("backend"), 10);
        assert!(!entries.is_empty());

        let metadata = entries[0].metadatas.first().unwrap();
        assert_eq!(
            metadata.get("room").and_then(|v| v.as_str()),
            Some("backend")
        );
        assert!(metadata.get("chunk_index").is_some());
        assert!(metadata.get("filed_at").is_some());
        assert!(metadata.get("source_mtime").is_some());
    }

    #[test]
    fn test_skip_files_includes_lockfiles() {
        // Mirrors upstream mempalace 5488e7b: pnpm/yarn lockfiles
        // should be skipped just like package-lock.json.
        for name in ["package-lock.json", "pnpm-lock.yaml", "yarn.lock"] {
            assert!(
                SKIP_FILES.contains(&name),
                "expected SKIP_FILES to include {name}"
            );
        }
    }

    #[tokio::test]
    async fn test_mine_file_skips_when_chunks_exceed_cap() {
        // A file that would produce > cap chunks should be skipped with a
        // warning instead of triggering a worst-case batch through the
        // embedder. Mirrors upstream mempalace 5488e7b (#1296). Uses an
        // explicit low cap so the fixture stays small under the raised
        // default introduced by upstream #1455.
        let temp = tempfile::TempDir::new().unwrap();
        let palace = temp.path().join("palace");
        let file = temp.path().join("generated.csv");

        // Build a payload with enough non-overlapping chunks to exceed the
        // low test cap. Each chunk needs >= MIN_CHUNK_SIZE chars and the
        // chunker splits on `\n\n`, so we emit `test_cap + 50` blocks
        // separated by blank lines.
        let test_cap = 50usize;
        let block = "lorem ipsum dolor sit amet consectetur adipiscing elit ";
        let mut payload = String::new();
        for _ in 0..(test_cap + 50) {
            payload.push_str(&block.repeat(20));
            payload.push_str("\n\n");
        }
        std::fs::write(&file, &payload).unwrap();

        let rooms = vec![RoomMapping {
            name: "general".to_string(),
            description: "General".to_string(),
            keywords: vec![],
        }];
        let mut miner = Miner::new(&palace, "wing", rooms)
            .unwrap()
            .with_max_chunks_per_file(Some(test_cap));

        // Sanity: the chunker really would emit > cap before the cap
        // kicks in.
        let raw_chunks = miner.chunk_text(payload.trim(), "generated.csv");
        assert!(
            raw_chunks.len() > test_cap,
            "fixture should exceed cap; produced {}",
            raw_chunks.len()
        );

        let (added, skip) = miner.mine_file(&file).await.unwrap();
        assert_eq!(
            added, 0,
            "files exceeding chunk cap should not be filed (got {added})"
        );
        assert_eq!(
            skip,
            Some(SkipReason::ChunkCap),
            "skip_reason should be ChunkCap so callers can account chunk-cap drops separately (#1455)"
        );

        // No drawers should have been added to the palace.
        miner.palace_db.flush().unwrap();
        assert_eq!(miner.palace_db.count(), 0);
    }

    #[tokio::test]
    async fn test_mine_file_zero_cap_disables_check() {
        // Sentinel value 0 disables the cap entirely (upstream mempalace
        // #1455). A long-form file that would have tripped a low cap should
        // be filed when `with_max_chunks_per_file(Some(0))` is used.
        let temp = tempfile::TempDir::new().unwrap();
        let palace = temp.path().join("palace");
        let file = temp.path().join("novel.txt");

        let block = "lorem ipsum dolor sit amet consectetur adipiscing elit ";
        let mut payload = String::new();
        for _ in 0..60 {
            payload.push_str(&block.repeat(20));
            payload.push_str("\n\n");
        }
        std::fs::write(&file, &payload).unwrap();

        let rooms = vec![RoomMapping {
            name: "general".to_string(),
            description: "General".to_string(),
            keywords: vec![],
        }];
        let mut miner = Miner::new(&palace, "wing", rooms)
            .unwrap()
            .with_max_chunks_per_file(Some(0));

        let (added, skip) = miner.mine_file(&file).await.unwrap();
        assert!(
            added > 0,
            "cap=0 disables chunk-cap check; file should be filed"
        );
        assert_eq!(skip, None);
    }

    #[test]
    fn test_resolve_max_chunks_per_file_default() {
        // Unset the env var to exercise the module-default path
        // deterministically regardless of how the test binary was launched.
        // SAFETY: tests in this module are not parallelized against env var
        // reads here, and the var is restored after the assertion.
        let prev = std::env::var("MEMPALACE_MAX_CHUNKS_PER_FILE").ok();
        // SAFETY: Single-threaded test process; restore below.
        unsafe {
            std::env::remove_var("MEMPALACE_MAX_CHUNKS_PER_FILE");
        }
        assert_eq!(
            resolve_max_chunks_per_file(None),
            MAX_CHUNKS_PER_FILE,
            "unset env + no override should yield module default"
        );
        if let Some(v) = prev {
            unsafe {
                std::env::set_var("MEMPALACE_MAX_CHUNKS_PER_FILE", v);
            }
        }
    }

    #[test]
    fn test_resolve_max_chunks_per_file_override_wins() {
        // Explicit override beats env var (upstream mempalace #1455).
        let prev = std::env::var("MEMPALACE_MAX_CHUNKS_PER_FILE").ok();
        unsafe {
            std::env::set_var("MEMPALACE_MAX_CHUNKS_PER_FILE", "123");
        }
        assert_eq!(resolve_max_chunks_per_file(Some(999)), 999);
        // Sentinel 0 from the override path also wins.
        assert_eq!(resolve_max_chunks_per_file(Some(0)), 0);
        unsafe {
            if let Some(v) = prev {
                std::env::set_var("MEMPALACE_MAX_CHUNKS_PER_FILE", v);
            } else {
                std::env::remove_var("MEMPALACE_MAX_CHUNKS_PER_FILE");
            }
        }
    }

    #[test]
    fn test_chunk_text_multibyte_utf8_boundary() {
        let miner = Miner::new(std::path::Path::new("/tmp"), "test", vec![]).unwrap();

        let prefix = "a".repeat(CHUNK_SIZE - 1);
        let text = format!("{}this continues after the boundary", prefix);

        let chunks = miner.chunk_text(&text, "test.txt");
        assert!(!chunks.is_empty(), "should produce at least one chunk");

        for (chunk, _idx) in &chunks {
            assert!(chunk.is_char_boundary(0));
            assert!(chunk.is_char_boundary(chunk.len()));
        }
    }
}
