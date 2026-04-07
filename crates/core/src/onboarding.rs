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

use crate::entity_detector::{detect_from_content, PersonEntity};
use crate::entity_registry::EntityRegistry;
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
pub struct PersonEntry {
    pub name: String,
    pub relationship: String,
    pub context: String,
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
    // If config_dir is a file path (e.g., registry.json), use its parent
    let base_dir = if config_dir.is_file() {
        config_dir.parent().unwrap_or(config_dir)
    } else {
        config_dir
    };
    let mempalace_dir = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".mempalace"))
        .unwrap_or_else(|_| base_dir.to_path_buf());
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
    config_path: &Path,
    mode: Mode,
    people: Vec<(String, String, String)>,
    projects: Vec<String>,
    _aliases: Option<HashMap<String, String>>,
) -> anyhow::Result<HashMap<String, String>> {
    let mut registry = EntityRegistry::load(config_path)?;

    let people_refs: Vec<(&str, &str, &str)> = people
        .iter()
        .map(|(n, c, r)| (n.as_str(), c.as_str(), r.as_str()))
        .collect();

    let projects_refs: Vec<&str> = projects.iter().map(|s| s.as_str()).collect();

    // Aliases seeding handled separately if needed
    registry.seed(mode.as_str(), people_refs, projects_refs, None)?;

    let summary = registry.summary();
    tracing::info!("Registry seeded:\n{}", summary);

    // Generate bootstrap files
    let people_entries: Vec<PersonEntry> = people
        .into_iter()
        .map(|(name, context, relationship)| PersonEntry {
            name,
            context,
            relationship,
        })
        .collect();

    let (registry_path, facts_path) = generate_aaak_bootstrap(
        &people_entries,
        &projects,
        &mode.default_wings(),
        mode,
        config_path,
    )?;

    let mut result = HashMap::new();
    result.insert(
        "registry_path".to_string(),
        registry_path.to_string_lossy().to_string(),
    );
    result.insert(
        "facts_path".to_string(),
        facts_path.to_string_lossy().to_string(),
    );
    result.insert("summary".to_string(), summary);

    Ok(result)
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

    // Collect text from readable files
    let mut all_text = String::new();
    for entry in walkdir::WalkDir::new(directory)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                if matches!(
                    ext_str.as_str(),
                    "txt" | "md" | "py" | "js" | "ts" | "rs" | "go"
                ) {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        all_text.push_str(&content);
                        all_text.push('\n');
                    }
                }
            }
        }
    }

    let detection = detect_from_content(&all_text);
    detection
        .people
        .into_iter()
        .filter(|p| !known_names.contains(&p.name.to_lowercase()) && p.confidence >= 0.7)
        .collect()
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

/// Prompt user for people (non-interactive stub for testing).
pub fn prompt_people(_mode: Mode) -> Vec<PersonEntry> {
    // Interactive prompts require a full CLI — this is a placeholder
    // that returns empty vec. Use quick_setup() for programmatic use.
    Vec::new()
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
    }

    #[test]
    fn test_quick_setup() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("registry.json");

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

        let result = quick_setup(&config_path, Mode::Personal, people, projects, None).unwrap();

        assert!(result.contains_key("registry_path"));
        assert!(result.contains_key("facts_path"));
        assert!(result.contains_key("summary"));
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
    fn test_auto_detect_from_directory() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        // Write test files
        std::fs::write(dir.join("notes.txt"), "Alice and Bob worked on ProjectX.").unwrap();

        let known_people = vec![PersonEntity {
            name: "Charlie".to_string(),
            confidence: 0.9,
            context: "known".to_string(),
        }];

        // Just verify it runs without error
        let _detected = auto_detect_from_directory(dir, &known_people);
        // Detection results depend on entity_detector confidence thresholds
    }
}
