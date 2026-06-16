//! onboarding.rs — MemPalace first-run setup.
//!
//! Guides users through initial configuration:
//!   1. Mode selection (work / personal / combo)
//!   2. People registration (names, relationships)
//!   3. Project registration
//!   4. Wing configuration
//!   5. Auto-detect additional people from files
//!
//! Seeds the entity_registry with confirmed data so MemPalace knows your world
//! from minute one — before a single session is indexed.

use crate::entity_detector::{detect_entities, scan_for_detection, PersonEntity};
use crate::entity_registry::{EntityRegistry, COMMON_ENGLISH_WORDS};
use crate::llm_client::{default_model, get_provider};
use crate::llm_refine::{refine_entities, DetectedEntities, EntityEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Non-interactive mode detection
// ---------------------------------------------------------------------------

/// Check if we're running in non-interactive mode.
/// Respects MEMPALACE_NONINTERACTIVE environment variable.
pub fn is_non_interactive() -> bool {
    std::env::var("MEMPALACE_NONINTERACTIVE")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

/// Check if stdin appears to be interactive (has input available).
pub fn is_interactive() -> bool {
    // Non-interactive env var takes precedence
    if is_non_interactive() {
        return false;
    }
    // On Unix, check if stdin is a tty via termios, or just check if stdin is readable
    // Simple heuristic: if stdin read doesn't immediately EOF, assume interactive
    // The real check would need atty crate, but we avoid the dependency
    true // Default to interactive, let stdin read failures handle non-interactive
}

/// Safe prompt that returns default in non-interactive mode.
/// Takes a prompt message and a default value to return when non-interactive.
pub fn prompt_or_default<T: Clone + ToString>(prompt: &str, default: T) -> T {
    if is_interactive() {
        print!("{}", prompt);
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                return default;
            }
        }
        default
    } else {
        eprintln!(
            "[non-interactive mode] Using default: {}",
            default.to_string()
        );
        default
    }
}

/// Prompt for string input with default in non-interactive mode.
pub fn prompt_string(prompt: &str, default: &str) -> String {
    if is_interactive() {
        print!("{} [{}]: ", prompt, default);
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                return default.to_string();
            }
            trimmed.to_string()
        } else {
            default.to_string()
        }
    } else {
        eprintln!("[non-interactive mode] Using default: {}", default);
        default.to_string()
    }
}

// ---------------------------------------------------------------------------
// Default wing taxonomies by mode
// ---------------------------------------------------------------------------

pub const DEFAULT_WINGS_WORK: &[&str] = &["projects", "clients", "team", "decisions", "research"];

pub const DEFAULT_WINGS_PERSONAL: &[&str] = &[
    "family",
    "health",
    "creative",
    "reflections",
    "relationships",
];

pub const DEFAULT_WINGS_COMBO: &[&str] = &[
    "family",
    "work",
    "health",
    "creative",
    "projects",
    "reflections",
];

#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Mode {
    Work,
    Personal,
    Combo,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Work => "work",
            Mode::Personal => "personal",
            Mode::Combo => "combo",
        }
    }

    pub fn default_wings(&self) -> Vec<String> {
        match self {
            Mode::Work => DEFAULT_WINGS_WORK.iter().map(|s| s.to_string()).collect(),
            Mode::Personal => DEFAULT_WINGS_PERSONAL
                .iter()
                .map(|s| s.to_string())
                .collect(),
            Mode::Combo => DEFAULT_WINGS_COMBO.iter().map(|s| s.to_string()).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Bootstrap file generation
// ---------------------------------------------------------------------------

/// Person entry from onboarding questions.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PersonEntry {
    pub name: String,
    pub relationship: String,
    pub context: String,
}

fn hr() {
    println!("\n{}", "─".repeat(58));
}

fn header(text: &str) {
    println!("\n{}", "=".repeat(58));
    println!("  {}", text);
    println!("{}", "=".repeat(58));
}

fn ask(prompt: &str, default: Option<&str>) -> String {
    if let Some(default) = default {
        prompt_string(prompt, default)
    } else if is_interactive() {
        print!("  {}: ", prompt);
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            input.trim().to_string()
        } else {
            String::new()
        }
    } else {
        String::new()
    }
}

fn yn(prompt: &str, default_yes: bool) -> bool {
    if !is_interactive() {
        return default_yes;
    }

    let default = if default_yes { "Y/n" } else { "y/N" };
    print!("  {} [{}]: ", prompt, default);
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_ok() {
        let trimmed = input.trim().to_lowercase();
        if trimmed.is_empty() {
            return default_yes;
        }
        trimmed.starts_with('y')
    } else {
        default_yes
    }
}

/// Generate AAAK entity registry + critical facts bootstrap from onboarding data.
/// These files teach the AI about the user's world from session one.
pub fn generate_aaak_bootstrap(
    people: &[PersonEntry],
    projects: &[String],
    wings: &[String],
    mode: Mode,
    config_dir: &Path,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    let mempalace_dir = if config_dir.as_os_str().is_empty() {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".mempalace"))
            .unwrap_or_else(|_| PathBuf::from(".mempalace"))
    } else {
        config_dir.to_path_buf()
    };
    if !mempalace_dir.exists() {
        std::fs::create_dir_all(&mempalace_dir)?;
    }

    // Build AAAK entity codes (first 3 letters of name, uppercase)
    let mut entity_codes: HashMap<String, String> = HashMap::new();
    for p in people {
        let mut code = p.name[..3.min(p.name.len())].to_uppercase();
        // Handle collisions
        while entity_codes.values().any(|c| c == &code) {
            let len = (code.len() + 1).min(p.name.len());
            code = p.name[..len].to_uppercase();
        }
        entity_codes.insert(p.name.clone(), code);
    }

    // AAAK entity registry
    let mut registry_lines = vec![
        "# AAAK Entity Registry".to_string(),
        "# Auto-generated by mempalace init. Update as needed.".to_string(),
        String::new(),
        "## People".to_string(),
    ];
    for p in people {
        let code = entity_codes.get(&p.name).cloned().unwrap_or_default();
        if p.relationship.is_empty() {
            registry_lines.push(format!("  {}={}", code, p.name));
        } else {
            registry_lines.push(format!("  {}={} ({})", code, p.name, p.relationship));
        }
    }

    if !projects.is_empty() {
        registry_lines.push(String::new());
        registry_lines.push("## Projects".to_string());
        for proj in projects {
            let code = proj[..4.min(proj.len())].to_uppercase();
            registry_lines.push(format!("  {}={}", code, proj));
        }
    }

    registry_lines.extend(vec![
        String::new(),
        "## AAAK Quick Reference".to_string(),
        "  Symbols: ♡=love ★=importance ⚠=warning →=relationship |=separator".to_string(),
        "  Structure: KEY:value | GROUP(details) | entity.attribute".to_string(),
        "  Read naturally — expand codes, treat *markers* as emotional context.".to_string(),
    ]);

    let registry_path = mempalace_dir.join("aaak_entities.md");
    std::fs::write(&registry_path, registry_lines.join("\n"))?;

    // Critical facts bootstrap
    let personal_people: Vec<_> = people.iter().filter(|p| p.context == "personal").collect();
    let work_people: Vec<_> = people.iter().filter(|p| p.context == "work").collect();

    let mut facts_lines = vec![
        "# Critical Facts (bootstrap — will be enriched after mining)".to_string(),
        String::new(),
    ];

    if !personal_people.is_empty() {
        facts_lines.push("## People (personal)".to_string());
        for p in &personal_people {
            let code = entity_codes.get(&p.name).cloned().unwrap_or_default();
            if p.relationship.is_empty() {
                facts_lines.push(format!("- **{}** ({})", p.name, code));
            } else {
                facts_lines.push(format!("- **{}** ({}) — {}", p.name, code, p.relationship));
            }
        }
        facts_lines.push(String::new());
    }

    if !work_people.is_empty() {
        facts_lines.push("## People (work)".to_string());
        for p in &work_people {
            let code = entity_codes.get(&p.name).cloned().unwrap_or_default();
            if p.relationship.is_empty() {
                facts_lines.push(format!("- **{}** ({})", p.name, code));
            } else {
                facts_lines.push(format!("- **{}** ({}) — {}", p.name, code, p.relationship));
            }
        }
        facts_lines.push(String::new());
    }

    if !projects.is_empty() {
        facts_lines.push("## Projects".to_string());
        for proj in projects {
            facts_lines.push(format!("- **{}**", proj));
        }
        facts_lines.push(String::new());
    }

    facts_lines.extend(vec![
        "## Palace".to_string(),
        format!("Wings: {}", wings.join(", ")),
        format!("Mode: {}", mode.as_str()),
        String::new(),
        "*This file will be enriched by palace_facts.py after mining.*".to_string(),
    ]);

    let facts_path = mempalace_dir.join("critical_facts.md");
    std::fs::write(&facts_path, facts_lines.join("\n"))?;

    Ok((registry_path, facts_path))
}

// ---------------------------------------------------------------------------
// Quick setup (non-interactive)
// ---------------------------------------------------------------------------

/// Programmatic setup without interactive prompts.
/// Used in tests and CLI with --non-interactive flag.
pub fn quick_setup(
    config_dir: &Path,
    mode: Mode,
    people: Vec<(String, String, String)>,
    projects: Vec<String>,
    aliases: Option<HashMap<String, String>>,
) -> anyhow::Result<EntityRegistry> {
    let registry_path = config_dir.join("entity_registry.json");
    let mut registry = EntityRegistry::load(&registry_path)?;

    let people_refs: Vec<(&str, &str, &str)> = people
        .iter()
        .map(|(n, c, r)| (n.as_str(), c.as_str(), r.as_str()))
        .collect();

    let projects_refs: Vec<&str> = projects.iter().map(|s| s.as_str()).collect();

    let alias_refs = aliases.as_ref().map(|map| {
        map.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<HashMap<&str, &str>>()
    });

    registry.seed(mode.as_str(), people_refs, projects_refs, alias_refs)?;
    Ok(registry)
}

// ---------------------------------------------------------------------------
// Auto-detect from files
// ---------------------------------------------------------------------------

/// Scan directory for additional entity candidates using entity_detector.
pub fn auto_detect_from_directory(
    directory: &Path,
    known_people: &[PersonEntity],
) -> Vec<PersonEntity> {
    let known_names: std::collections::HashSet<String> =
        known_people.iter().map(|p| p.name.to_lowercase()).collect();

    let files = scan_for_detection(directory, 10);
    let detection = detect_entities(&files, 10, None);
    detection
        .people
        .into_iter()
        .filter(|p| !known_names.contains(&p.name.to_lowercase()) && p.confidence >= 0.7)
        .collect()
}

// ---------------------------------------------------------------------------
// Manifest + git author scanning (mr-4fqp)
// ---------------------------------------------------------------------------

/// Manifest filenames that can carry an `authors` field we mine for entity
/// candidates. Each parser knows how to extract a list of names from its
/// file format.
const MANIFEST_FILES: &[&str] = &[
    "pyproject.toml",
    "Cargo.toml",
    "package.json",
    "go.mod",
];

/// Scan nearby project manifests for author fields and add their authors
/// to the candidate entity set (#mr-4fqp).
///
/// Each manifest is read best-effort — if a file is missing, malformed, or
/// has no authors, we silently move on. A returned name always has a
/// non-empty `name`; the `context` field identifies the source manifest
/// (e.g. `Cargo.toml`).
pub fn scan_manifest_authors(directory: &Path) -> Vec<PersonEntity> {
    let mut out: Vec<PersonEntity> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for manifest in MANIFEST_FILES {
        let path = directory.join(manifest);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let extracted = match *manifest {
            "pyproject.toml" => extract_authors_pyproject(&text),
            "Cargo.toml" => extract_authors_cargo_toml(&text),
            "package.json" => extract_authors_package_json(&text),
            "go.mod" => extract_authors_go_mod(&text),
            _ => Vec::new(),
        };
        for name in extracted {
            let key = name.to_lowercase();
            if seen.insert(key) {
                out.push(PersonEntity {
                    name,
                    confidence: 0.9,
                    context: (*manifest).to_string(),
                });
            }
        }
    }

    out
}

fn extract_authors_pyproject(text: &str) -> Vec<String> {
    // Heuristic: regex-collect `name = "..."` lines inside [tool.poetry.authors]
    // or [[authors]] arrays. Cheap and good-enough for first-run seeding.
    let mut names = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("name") {
            continue;
        }
        if let Some(eq) = trimmed.find('=') {
            let value = trimmed[eq + 1..].trim();
            if let Some(name) = unquote(value) {
                // Skip emails like "Name <a@b.com>" → keep just "Name"
                let cleaned = name.split('<').next().unwrap_or(&name).trim().to_string();
                if !cleaned.is_empty() {
                    names.push(cleaned);
                }
            }
        }
    }
    names
}

fn extract_authors_cargo_toml(text: &str) -> Vec<String> {
    // Cargo workspace `authors = ["Alice <a@b>", "Bob"]` lines.
    let mut names = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("authors") {
            continue;
        }
        if let Some(eq) = trimmed.find('=') {
            let value = trimmed[eq + 1..].trim();
            // Drop surrounding array brackets and quotes; split on comma.
            let stripped = value.trim_matches(|c: char| c == '[' || c == ']' || c == ',');
            for piece in stripped.split(',') {
                if let Some(p) = unquote(piece.trim()) {
                    let cleaned = p.split('<').next().unwrap_or(&p).trim().to_string();
                    if !cleaned.is_empty() {
                        names.push(cleaned);
                    }
                }
            }
        }
    }
    names
}

fn extract_authors_package_json(text: &str) -> Vec<String> {
    // Look for `"author": "Name <email>"` or `"authors": [...]` / `"contributors": [...]`.
    // We do a tiny state machine: track whether we are inside one of those
    // keys, then collect quoted strings.
    let mut names = Vec::new();
    let mut in_target = false;
    for line in text.lines() {
        let lower = line.to_lowercase();
        if lower.contains("\"author\"")
            || lower.contains("\"authors\"")
            || lower.contains("\"contributors\"")
        {
            in_target = true;
        }
        if !in_target {
            continue;
        }
        // Pull every "..." token from the line.
        let bytes = line.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'"' {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != b'"' {
                    end += 1;
                }
                if end > start {
                    if let Ok(s) = std::str::from_utf8(&bytes[start..end]) {
                        let cleaned = s.split('<').next().unwrap_or(s).trim().to_string();
                        if !cleaned.is_empty() {
                            names.push(cleaned);
                        }
                    }
                }
                i = end + 1;
            } else {
                i += 1;
            }
        }
        // Reset when we leave the section.
        if line.trim_end().ends_with('}') {
            in_target = false;
        }
    }
    names
}

fn extract_authors_go_mod(text: &str) -> Vec<String> {
    // `go.mod` itself rarely has authors, but projects sometimes keep an
    // `AUTHORS` block in a comment. Look for lines starting with `# Name`.
    let mut names = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let after_hash = trimmed.strip_prefix('#').unwrap_or("").trim();
        if after_hash.is_empty() {
            continue;
        }
        // Heuristic: a single token or a "Name <email>" form, no spaces.
        if after_hash.contains('<') {
            let cleaned = after_hash.split('<').next().unwrap_or(after_hash).trim();
            if !cleaned.is_empty() {
                names.push(cleaned.to_string());
            }
        }
    }
    names
}

fn unquote(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return Some(trimmed[1..trimmed.len() - 1].to_string());
    }
    if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return Some(trimmed[1..trimmed.len() - 1].to_string());
    }
    None
}

/// Run `git log --format='%aN <%aE>'` and collect unique author display
/// names as candidate entities. Silently returns an empty Vec if `git` is
/// missing, the directory is not a repo, or the command fails — entity
/// seeding is a best-effort signal, not a hard prerequisite.
pub fn scan_git_authors(directory: &Path) -> Vec<PersonEntity> {
    let output = match std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .arg("log")
        .arg("--format=%aN <%aE>")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<PersonEntity> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let name = line.split('<').next().unwrap_or(line).trim().to_string();
        if name.is_empty() {
            continue;
        }
        let key = name.to_lowercase();
        if seen.insert(key) {
            out.push(PersonEntity {
                name,
                confidence: 0.8,
                context: "git".to_string(),
            });
        }
    }
    out
}

/// Convenience: combine manifest + git author signals into one de-duped
/// candidate set. Entries from manifests are listed first (higher
/// confidence) so the prompt can show them in priority order.
pub fn scan_project_authors(directory: &Path) -> Vec<PersonEntity> {
    let mut all: Vec<PersonEntity> = scan_manifest_authors(directory);
    let known: std::collections::HashSet<String> =
        all.iter().map(|p| p.name.to_lowercase()).collect();
    for git_author in scan_git_authors(directory) {
        if !known.contains(&git_author.name.to_lowercase()) {
            all.push(git_author);
        }
    }
    all
}

// ---------------------------------------------------------------------------
// Claude Code conversation dir scan (mr-l9o5)
// ---------------------------------------------------------------------------

/// Scan `~/.claude/projects/` for conversation directories. Each subdir
/// represents one project; the dir name encodes the project (e.g.
/// `myrepo` or `mypath-to-myrepo` for repos whose path contains a dash).
/// We return the dir names as candidate project entities.
pub fn scan_claude_projects(projects_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(projects_dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(name) = entry.file_name().to_str() {
                let name = name.trim();
                if !name.is_empty() && !name.starts_with('.') {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// Resolve `~/.claude/projects/` to an absolute path. Returns `None` if
/// `HOME` (or `USERPROFILE` on Windows) is not set.
pub fn default_claude_projects_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home).join(".claude").join("projects"));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Some(PathBuf::from(profile).join(".claude").join("projects"));
    }
    None
}

pub fn warn_ambiguous(people: &[PersonEntry]) -> Vec<String> {
    people
        .iter()
        .filter(|p| COMMON_ENGLISH_WORDS.contains(&p.name.to_lowercase().as_str()))
        .map(|p| p.name.clone())
        .collect()
}

fn ask_projects(mode: Mode) -> Vec<String> {
    if mode == Mode::Personal {
        return Vec::new();
    }

    hr();
    println!(
        "\n  What are your main projects? (These help MemPalace distinguish project\n  names from person names — e.g. \"Lantern\" the project vs. \"Lantern\" the word.)\n\n  Type 'done' when finished.\n"
    );

    let mut projects = Vec::new();
    loop {
        let proj = ask("Project", None);
        let trimmed = proj.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("done") {
            break;
        }
        projects.push(trimmed.to_string());
    }
    projects
}

fn ask_wings(mode: Mode) -> Vec<String> {
    let defaults = mode.default_wings();
    hr();
    println!(
        "\n  Wings are the top-level categories in your memory palace.\n\n  Suggested wings for {} mode:\n    {}\n\n  Press enter to keep these, or type your own comma-separated list.\n",
        mode.as_str(),
        defaults.join(", ")
    );

    let custom = ask("Wings", None);
    if custom.trim().is_empty() {
        defaults
    } else {
        custom
            .split(',')
            .map(|w| w.trim())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_string())
            .collect()
    }
}

fn save_wing_config(config_dir: &Path, wings: &[String]) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(config_dir)?;
    let wing_config_path = config_dir.join("wing_config.json");
    let payload = serde_json::json!({
        "default_wing": "wing_general",
        "wings": wings
            .iter()
            .map(|wing| {
                let key = format!("wing_{}", wing.to_lowercase().replace(' ', "_"));
                (
                    key,
                    serde_json::json!({
                        "type": "topic",
                        "keywords": [wing],
                    }),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>(),
    });
    std::fs::write(&wing_config_path, serde_json::to_string_pretty(&payload)?)?;
    Ok(wing_config_path)
}

/// Collect corpus text for LLM refinement context.
/// Reads a sample of text files to provide contextual signals for entity classification.
fn collect_corpus_for_refinement(directory: &Path) -> String {
    const MAX_FILES: usize = 20;
    const MAX_LINES_PER_FILE: usize = 50;
    const PROSE_EXTENSIONS: &[&str] = &["md", "txt", "rst", "markdown"];

    let mut corpus_lines = Vec::new();
    let mut files_read = 0;

    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.filter_map(Result::ok) {
            if files_read >= MAX_FILES {
                break;
            }
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if PROSE_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            for line in content.lines().take(MAX_LINES_PER_FILE) {
                                let trimmed = line.trim();
                                if !trimmed.is_empty() {
                                    corpus_lines.push(format!("{}: {}", path.display(), trimmed));
                                }
                            }
                            files_read += 1;
                        }
                    }
                }
            }
        }
    }

    corpus_lines.join("\n")
}

#[allow(clippy::too_many_arguments)]
pub fn run_onboarding(
    directory: &Path,
    config_dir: &Path,
    auto_detect: bool,
    use_llm: bool,
    llm_provider: Option<&str>,
    llm_model: Option<&str>,
    llm_endpoint: Option<&str>,
    llm_api_key: Option<&str>,
    _accept_external_llm: bool,
) -> anyhow::Result<EntityRegistry> {
    let mode = prompt_mode();
    let (mut people, aliases) = prompt_people(mode);
    let projects = ask_projects(mode);
    let wings = ask_wings(mode);

    if auto_detect
        && yn(
            "\nScan your files for additional names we might have missed?",
            true,
        )
    {
        let dir_input = ask("Directory to scan", Some(&directory.to_string_lossy()));
        let scan_dir = PathBuf::from(dir_input);
        let known_people: Vec<PersonEntity> = people
            .iter()
            .map(|p| PersonEntity {
                name: p.name.clone(),
                confidence: 1.0,
                context: p.context.clone(),
            })
            .collect();

        let detected = auto_detect_from_directory(&scan_dir, &known_people);
        if !detected.is_empty() {
            hr();
            println!("\n  Found {} additional name candidates:\n", detected.len());
            for entity in detected {
                println!(
                    "    {:20} confidence={:.0}%  ({})",
                    entity.name,
                    entity.confidence * 100.0,
                    entity
                        .context
                        .split(';')
                        .next()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("no strong signals")
                );
                if ask("    Add as (p)erson or (s)kip?", None)
                    .trim()
                    .eq_ignore_ascii_case("p")
                {
                    let rel = ask(&format!("    Relationship/role for {}?", entity.name), None);
                    let ctx = if mode == Mode::Personal {
                        "personal".to_string()
                    } else if mode == Mode::Work {
                        "work".to_string()
                    } else {
                        let raw = ask("    Context — (p)ersonal or (w)ork?", None);
                        if raw.trim().to_lowercase().starts_with('w') {
                            "work".to_string()
                        } else {
                            "personal".to_string()
                        }
                    };
                    people.push(PersonEntry {
                        name: entity.name,
                        relationship: rel,
                        context: ctx,
                    });
                }
            }
        }
    }

    let ambiguous = warn_ambiguous(&people);
    if !ambiguous.is_empty() {
        hr();
        println!(
            "\n  Heads up — these names are also common English words:\n    {}\n\n  MemPalace will check the context before treating them as person names.\n  For example: \"I picked up Riley\" → person.\n               \"Have you ever tried\" → adverb.\n",
            ambiguous.join(", ")
        );
    }

    // LLM refinement: reclassify uncertain entities using an LLM
    if use_llm {
        let provider_name = llm_provider.unwrap_or("ollama");
        let model = llm_model.unwrap_or_else(|| default_model(provider_name));
        let endpoint = llm_endpoint.map(|s| s.to_string());
        let api_key = llm_api_key.map(|s| s.to_string());

        match get_provider(provider_name, model, endpoint.clone(), api_key.clone(), 120) {
            Ok(provider) => {
                println!(
                    "\n  Refining entity classifications with LLM ({}/{})...",
                    provider_name, model
                );
                // Collect corpus text for context
                let corpus_text = collect_corpus_for_refinement(directory);
                // Convert onboarding DetectedEntities to llm_refine DetectedEntities
                let llm_detected = DetectedEntities {
                    people: people
                        .iter()
                        .map(|p| EntityEntry {
                            name: p.name.clone(),
                            entry_type: "person".to_string(),
                            signals: vec![],
                        })
                        .collect(),
                    projects: projects
                        .iter()
                        .map(|n| EntityEntry {
                            name: n.clone(),
                            entry_type: "project".to_string(),
                            signals: vec![],
                        })
                        .collect(),
                    topics: vec![],
                    uncertain: vec![],
                };
                let result = refine_entities(
                    &llm_detected,
                    &corpus_text,
                    provider.as_ref(),
                    25,
                    true,
                    true,
                    None,
                );
                if result.reclassified > 0 {
                    println!(
                        "  LLM reclassified {} entities ({} dropped, {} errors)",
                        result.reclassified,
                        result.dropped,
                        result.errors.len()
                    );
                } else if !result.errors.is_empty() {
                    println!(
                        "  LLM refinement completed with {} errors",
                        result.errors.len()
                    );
                }
            }
            Err(e) => {
                println!(
                    "\n  LLM provider error: {}. Continuing without refinement.",
                    e
                );
            }
        }
    }

    let people_tuples = people
        .iter()
        .map(|p| (p.name.clone(), p.context.clone(), p.relationship.clone()))
        .collect();
    let registry = quick_setup(
        config_dir,
        mode,
        people_tuples,
        projects.clone(),
        Some(aliases),
    )?;
    let (aaak_path, facts_path) =
        generate_aaak_bootstrap(&people, &projects, &wings, mode, config_dir)?;
    let _ = save_wing_config(config_dir, &wings)?;

    header("Setup Complete");
    println!();
    println!("  {}", registry.summary());
    println!("\n  Wings: {}", wings.join(", "));
    println!("\n  Registry saved to: {}", registry.path().display());
    println!("  AAAK entity registry: {}", aaak_path.display());
    println!("  Critical facts bootstrap: {}", facts_path.display());
    println!("\n  Your AI will know your world from the first session.");
    println!();

    Ok(registry)
}

// ---------------------------------------------------------------------------
// CLI helpers (for interactive mode)
// ---------------------------------------------------------------------------

/// Prompt user for mode selection.
pub fn prompt_mode() -> Mode {
    if !is_interactive() {
        eprintln!("[non-interactive mode] Using default mode: Combo");
        return Mode::Combo;
    }

    println!("\n============================================================");
    println!("  Welcome to MemPalace");
    println!("============================================================");
    println!();
    println!("  MemPalace is a personal memory system. To work well, it needs");
    println!("  to know a little about your world — who the people are,");
    println!("  what the projects are, and how you want your memory organized.");
    println!();
    println!("  This takes about 2 minutes. You can always update it later.");
    println!();
    println!("  How are you using MemPalace?");
    println!();
    println!("    [1]  Work     — notes, projects, clients, colleagues, decisions");
    println!("    [2]  Personal — diary, family, health, relationships, reflections");
    println!("    [3]  Both     — personal and professional mixed");
    println!();

    loop {
        print!("  Your choice [1/2/3]: ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            match input.trim() {
                "1" => return Mode::Work,
                "2" => return Mode::Personal,
                "3" => return Mode::Combo,
                _ => {}
            }
        }
        println!("  Please enter 1, 2, or 3.");
    }
}

/// Prompt user for people and aliases.
pub fn prompt_people(mode: Mode) -> (Vec<PersonEntry>, HashMap<String, String>) {
    let mut people = Vec::new();
    let mut aliases = HashMap::new();

    if mode == Mode::Personal || mode == Mode::Combo {
        hr();
        println!(
            "\n  Personal world — who are the important people in your life?\n\n  Format: name, relationship (e.g. \"Riley, daughter\" or just \"Devon\")\n  For nicknames, you'll be asked separately.\n  Type 'done' when finished.\n"
        );
        loop {
            let entry = ask("Person", None);
            let trimmed = entry.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("done") {
                break;
            }
            let mut parts = trimmed.splitn(2, ',').map(|s| s.trim());
            let name = parts.next().unwrap_or_default().to_string();
            let relationship = parts.next().unwrap_or_default().to_string();
            if !name.is_empty() {
                let nick = ask(&format!("Nickname for {}? (or enter to skip)", name), None);
                if !nick.trim().is_empty() {
                    aliases.insert(nick.trim().to_string(), name.clone());
                }
                people.push(PersonEntry {
                    name,
                    relationship,
                    context: "personal".to_string(),
                });
            }
        }
    }

    if mode == Mode::Work || mode == Mode::Combo {
        hr();
        println!(
            "\n  Work world — who are the colleagues, clients, or collaborators\n  you'd want to find in your notes?\n\n  Format: name, role (e.g. \"Ben, co-founder\" or just \"Sarah\")\n  Type 'done' when finished.\n"
        );
        loop {
            let entry = ask("Person", None);
            let trimmed = entry.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("done") {
                break;
            }
            let mut parts = trimmed.splitn(2, ',').map(|s| s.trim());
            let name = parts.next().unwrap_or_default().to_string();
            let relationship = parts.next().unwrap_or_default().to_string();
            if !name.is_empty() {
                people.push(PersonEntry {
                    name,
                    relationship,
                    context: "work".to_string(),
                });
            }
        }
    }

    (people, aliases)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_mode_as_str() {
        assert_eq!(Mode::Work.as_str(), "work");
        assert_eq!(Mode::Personal.as_str(), "personal");
        assert_eq!(Mode::Combo.as_str(), "combo");
    }

    #[test]
    fn test_mode_default_wings() {
        assert!(!Mode::Work.default_wings().is_empty());
        assert!(!Mode::Personal.default_wings().is_empty());
        assert!(!Mode::Combo.default_wings().is_empty());
        assert!(Mode::Work.default_wings().contains(&"projects".to_string()));
        assert!(Mode::Personal
            .default_wings()
            .contains(&"family".to_string()));
        let combo = Mode::Combo.default_wings();
        assert!(combo.contains(&"family".to_string()));
        assert!(combo.contains(&"work".to_string()));
    }

    #[test]
    fn test_quick_setup() {
        let temp_dir = TempDir::new().unwrap();
        let people = vec![
            (
                "Alice".to_string(),
                "personal".to_string(),
                "friend".to_string(),
            ),
            (
                "Bob".to_string(),
                "work".to_string(),
                "colleague".to_string(),
            ),
        ];
        let projects = vec!["ProjectX".to_string()];

        let result = quick_setup(temp_dir.path(), Mode::Personal, people, projects, None).unwrap();

        assert_eq!(result.mode(), "personal");
        assert!(result.people().contains_key("Alice"));
        assert!(result.projects().contains(&"ProjectX".to_string()));
        assert!(temp_dir.path().join("entity_registry.json").exists());
    }

    #[test]
    fn test_quick_setup_empty() {
        let temp_dir = TempDir::new().unwrap();
        let result = quick_setup(
            temp_dir.path(),
            Mode::Personal,
            Vec::new(),
            Vec::new(),
            None,
        )
        .unwrap();

        assert_eq!(result.mode(), "personal");
        assert!(result.people().is_empty());
        assert!(result.projects().is_empty());
    }

    #[test]
    fn test_generate_aaak_bootstrap() {
        let temp_dir = TempDir::new().unwrap();
        let people = vec![PersonEntry {
            name: "Alice".to_string(),
            relationship: "friend".to_string(),
            context: "personal".to_string(),
        }];
        let projects = vec!["TestProject".to_string()];
        let wings = vec!["projects".to_string()];

        let (registry_path, facts_path) =
            generate_aaak_bootstrap(&people, &projects, &wings, Mode::Personal, temp_dir.path())
                .unwrap();

        assert!(registry_path.exists());
        assert!(facts_path.exists());

        let registry_content = std::fs::read_to_string(&registry_path).unwrap();
        assert!(registry_content.contains("ALI=Alice"));
        assert!(registry_content.contains("# AAAK Entity Registry"));

        let facts_content = std::fs::read_to_string(&facts_path).unwrap();
        assert!(facts_content.contains("Alice"));
        assert!(facts_content.contains("# Critical Facts"));
    }

    #[test]
    fn test_generate_aaak_bootstrap_collision() {
        let temp_dir = TempDir::new().unwrap();
        let people = vec![
            PersonEntry {
                name: "Alice".to_string(),
                relationship: "friend".to_string(),
                context: "work".to_string(),
            },
            PersonEntry {
                name: "Alison".to_string(),
                relationship: "coworker".to_string(),
                context: "work".to_string(),
            },
        ];

        let (registry_path, _) = generate_aaak_bootstrap(
            &people,
            &[],
            &["work".to_string()],
            Mode::Work,
            temp_dir.path(),
        )
        .unwrap();
        let registry_content = std::fs::read_to_string(registry_path).unwrap();
        assert!(registry_content.contains("ALI=Alice"));
        assert!(registry_content.contains("ALIS=Alison"));
    }

    #[test]
    fn test_generate_aaak_bootstrap_no_relationship() {
        let temp_dir = TempDir::new().unwrap();
        let people = vec![PersonEntry {
            name: "Bob".to_string(),
            relationship: "".to_string(),
            context: "work".to_string(),
        }];

        let (registry_path, _) = generate_aaak_bootstrap(
            &people,
            &[],
            &["work".to_string()],
            Mode::Work,
            temp_dir.path(),
        )
        .unwrap();
        let registry_content = std::fs::read_to_string(registry_path).unwrap();
        assert!(registry_content.contains("BOB=Bob"));
    }

    #[test]
    fn test_generate_aaak_bootstrap_empty_people_and_projects() {
        let temp_dir = TempDir::new().unwrap();

        let (registry_path, facts_path) = generate_aaak_bootstrap(
            &[],
            &[],
            &["general".to_string()],
            Mode::Personal,
            temp_dir.path(),
        )
        .unwrap();

        assert!(registry_path.exists());
        assert!(facts_path.exists());
        let registry_content = std::fs::read_to_string(registry_path).unwrap();
        let facts_content = std::fs::read_to_string(facts_path).unwrap();
        assert!(registry_content.contains("## People"));
        assert!(facts_content.contains("## Palace"));
        assert!(facts_content.contains("Mode: personal"));
    }

    #[test]
    fn test_generate_aaak_bootstrap_respects_config_dir() {
        let temp_dir = TempDir::new().unwrap();
        let people = vec![PersonEntry {
            name: "Riley".to_string(),
            relationship: "daughter".to_string(),
            context: "personal".to_string(),
        }];

        let (registry_path, facts_path) = generate_aaak_bootstrap(
            &people,
            &[],
            &["family".to_string()],
            Mode::Personal,
            temp_dir.path(),
        )
        .unwrap();

        assert_eq!(registry_path.parent(), Some(temp_dir.path()));
        assert_eq!(facts_path.parent(), Some(temp_dir.path()));
    }

    #[test]
    fn test_auto_detect_from_directory() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        std::fs::write(
            dir.join("notes.txt"),
            "Alice said hi. Alice said hi. Alice said hi. Alice laughed. Alice laughed. Alice laughed.",
        )
        .unwrap();
        std::fs::write(
            dir.join("README.md"),
            "Alice asked about the plan. Alice asked about the plan. Alice asked about the plan.",
        )
        .unwrap();
        std::fs::write(
            dir.join("guide.rst"),
            "Alice smiled today. Alice smiled today. Alice smiled today.",
        )
        .unwrap();

        let known_people = vec![PersonEntity {
            name: "Charlie".to_string(),
            confidence: 0.9,
            context: "known".to_string(),
        }];

        let detected = auto_detect_from_directory(dir, &known_people);
        assert!(detected.iter().any(|p| p.name == "Alice"));
    }

    #[test]
    fn test_scan_manifest_authors_extracts_cargo_and_package_json() {
        // mr-4fqp: manifest authors must surface as candidate entities so
        // the onboarding prompt can pre-fill the people list from
        // pyproject.toml / Cargo.toml / package.json / go.mod.
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        std::fs::write(
            dir.join("Cargo.toml"),
            r#"[package]
name = "demo"
version = "0.1.0"
authors = ["Alice Kim <alice@example.com>", "Bob Stone"]
edition = "2021"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{
  "name": "demo",
  "author": "Carol Lee <carol@example.com>",
  "contributors": ["Dan"]
}
"#,
        )
        .unwrap();

        let candidates = scan_manifest_authors(dir);
        let names: Vec<String> = candidates.iter().map(|p| p.name.clone()).collect();
        assert!(
            names.iter().any(|n| n.contains("Alice")),
            "expected Alice from Cargo.toml, got {names:?}"
        );
        assert!(
            names.iter().any(|n| n.contains("Bob")),
            "expected Bob from Cargo.toml, got {names:?}"
        );
        assert!(
            names.iter().any(|n| n.contains("Carol")),
            "expected Carol from package.json, got {names:?}"
        );

        // Each candidate must carry a source-manifest context.
        for c in &candidates {
            assert!(
                ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"]
                    .contains(&c.context.as_str()),
                "unexpected context {}",
                c.context
            );
        }
    }

    #[test]
    fn test_scan_git_authors_returns_empty_for_non_repo() {
        // mr-4fqp: a non-git directory must NOT panic; it must return an
        // empty candidate set so onboarding can fall back to manifests.
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();
        // No git init — git log will exit non-zero and we must not error.
        let candidates = scan_git_authors(dir);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_scan_claude_projects_lists_subdirs() {
        // mr-l9o5: a fake `~/.claude/projects/` must yield each subdir
        // name as a candidate project entity. Hidden dirs and stray
        // files must be ignored.
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();
        std::fs::create_dir(dir.join("mempalace_rust")).unwrap();
        std::fs::create_dir(dir.join("side-project")).unwrap();
        std::fs::create_dir(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("stray-file.txt"), "x").unwrap();

        let names = scan_claude_projects(dir);
        assert!(names.contains(&"mempalace_rust".to_string()));
        assert!(names.contains(&"side-project".to_string()));
        assert!(!names.iter().any(|n| n.starts_with('.')));
        assert!(!names.iter().any(|n| n.contains('.')));
    }

    #[test]
    fn test_auto_detect_prefers_prose_files() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();
        std::fs::write(dir.join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.join("b.md"), "beta").unwrap();
        std::fs::write(dir.join("c.rst"), "gamma").unwrap();
        std::fs::write(dir.join("code.rs"), "fn main() {} ").unwrap();

        let files = scan_for_detection(dir, 10);
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|p| {
            matches!(
                p.extension().and_then(|ext| ext.to_str()),
                Some("txt") | Some("md") | Some("rst")
            )
        }));
    }

    #[test]
    fn test_warn_ambiguous_flags_common_words() {
        let people = vec![
            PersonEntry {
                name: "Grace".to_string(),
                relationship: "friend".to_string(),
                context: "personal".to_string(),
            },
            PersonEntry {
                name: "Riley".to_string(),
                relationship: "daughter".to_string(),
                context: "personal".to_string(),
            },
        ];
        let result = warn_ambiguous(&people);
        assert!(result.contains(&"Grace".to_string()));
        assert!(!result.contains(&"Riley".to_string()));
    }

    #[test]
    fn test_warn_ambiguous_empty_list() {
        assert!(warn_ambiguous(&[]).is_empty());
    }

    #[test]
    fn test_warn_ambiguous_multiple_hits() {
        let people = vec![
            PersonEntry {
                name: "Grace".to_string(),
                relationship: "friend".to_string(),
                context: "personal".to_string(),
            },
            PersonEntry {
                name: "May".to_string(),
                relationship: "aunt".to_string(),
                context: "personal".to_string(),
            },
            PersonEntry {
                name: "Joy".to_string(),
                relationship: "sister".to_string(),
                context: "personal".to_string(),
            },
        ];

        let result = warn_ambiguous(&people);
        assert!(result.contains(&"Grace".to_string()));
        assert!(result.contains(&"May".to_string()));
        assert!(result.contains(&"Joy".to_string()));
    }

    #[test]
    fn test_quick_setup_preserves_aliases() {
        let temp_dir = TempDir::new().unwrap();
        let people = vec![(
            "Alice".to_string(),
            "personal".to_string(),
            "daughter".to_string(),
        )];
        let aliases = HashMap::from([("Ali".to_string(), "Alice".to_string())]);

        let registry = quick_setup(
            temp_dir.path(),
            Mode::Personal,
            people,
            Vec::new(),
            Some(aliases),
        )
        .unwrap();

        assert!(registry.people().contains_key("Alice"));
        assert!(registry.people().contains_key("Ali"));
    }
}
