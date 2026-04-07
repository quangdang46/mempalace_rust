//! MemPalace CLI - command-line interface for the memory palace system.
//!
//! Commands:
//!     init <dir>                  Detect rooms from folder structure
//!     split <dir>                 Split concatenated mega-files into per-session files
//!     mine <dir>                   Mine project files (default)
//!     mine <dir> --mode convos     Mine conversation exports
//!     search "query"               Find anything, exact words
//!     wake-up                      Show L0 + L1 wake-up context
//!     wake-up --wing my_app        Wake-up for a specific project
//!     status                       Show what's been filed
//!     compress                     Compress drawers using AAAK dialect

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::config::Config;
use crate::convo_miner::{mine_conversations, ConvoMiningResult};
use crate::dialect;
use crate::entity_detector::{detect_from_content, PersonEntity, ProjectEntity};
use crate::entity_registry::EntityRegistry;
use crate::layers::MemoryStack;
use crate::miner::{self, MiningResult};
use crate::palace_db::PalaceDb;
use crate::room_detector_local::detect_rooms_from_folders;
use crate::searcher;
use crate::split_mega_files::split_file;

// ---------------------------------------------------------------------------
// CLI Arguments
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "mempalace",
    about = "MemPalace - Give your AI a memory. No API key required.",
    long_about = None,
    infer_subcommands = true,
)]
struct Cli {
    /// Where the palace lives (default: from ~/.mempalace/config.json or ~/.mempalace/palace)
    #[arg(long)]
    palace: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Detect rooms from your folder structure and initialize config.
    Init {
        /// Project directory to set up
        dir: PathBuf,

        /// Auto-accept all detected entities (non-interactive)
        #[arg(long)]
        yes: bool,
    },

    /// Mine files into the palace.
    Mine {
        /// Directory to mine
        dir: PathBuf,

        /// Ingest mode: 'projects' (default) or 'convos' for chat exports
        #[arg(long, default_value = "projects")]
        mode: MiningMode,

        /// Wing name (default: directory name)
        #[arg(long)]
        wing: Option<String>,

        /// Your name -- recorded on every drawer (default: mempalace)
        #[arg(long, default_value = "mempalace")]
        agent: String,

        /// Max files to process (0 = all)
        #[arg(long)]
        limit: Option<usize>,

        /// Show what would be filed without filing
        #[arg(long)]
        dry_run: bool,

        /// Extraction strategy for convos: 'exchange' (default) or 'general'
        #[arg(long)]
        extract: Option<String>,
    },

    /// Find anything, exact words.
    Search {
        /// What to search for
        query: String,

        /// Limit to one project
        #[arg(long)]
        wing: Option<String>,

        /// Limit to one room
        #[arg(long)]
        room: Option<String>,

        /// Number of results
        #[arg(long, default_value = "5")]
        results: usize,

        /// Embedding model: "naive" (default, word overlap), "multilingual" for cross-lingual support
        #[arg(long)]
        embedding: Option<String>,
    },

    /// Show L0 + L1 wake-up context (~600-900 tokens).
    WakeUp {
        /// Wake-up for a specific project/wing
        #[arg(long)]
        wing: Option<String>,
    },

    /// Compress drawers using AAAK Dialect (~30x reduction).
    Compress {
        /// Wing to compress (default: all wings)
        #[arg(long)]
        wing: Option<String>,

        /// Preview compression without storing
        #[arg(long)]
        dry_run: bool,

        /// Entity config JSON (e.g. entities.json)
        #[arg(long)]
        config: Option<String>,
    },

    /// Split concatenated transcript mega-files into per-session files.
    Split {
        /// Directory containing transcript files
        dir: PathBuf,

        /// Write split files here (default: same directory as source files)
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Show what would be split without writing files
        #[arg(long)]
        dry_run: bool,

        /// Only split files containing at least N sessions (default: 2)
        #[arg(long, default_value = "2")]
        min_sessions: usize,
    },

    /// Show what's been filed.
    Status,

    /// Scan machine for AI tool sessions and mine them all.
    MineDevice {
        /// Wing name for discovered sessions
        #[arg(long)]
        wing: Option<String>,

        /// Don't actually mine, just show what would be mined
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Default, Debug)]
enum MiningMode {
    #[default]
    Projects,
    Convos,
    Auto,
}

impl std::str::FromStr for MiningMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "projects" | "project" => Ok(MiningMode::Projects),
            "convos" | "convo" | "conversations" => Ok(MiningMode::Convos),
            "auto" => Ok(MiningMode::Auto),
            _ => Err(format!(
                "Unknown mining mode: {s}. Use 'projects', 'convos', or 'auto'."
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Palace path resolution
// ---------------------------------------------------------------------------

fn resolve_palace_path(palace_arg: Option<&str>) -> Result<PathBuf> {
    let config = Config::load()?;
    match palace_arg {
        Some(p) => {
            if p.starts_with("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    Ok(PathBuf::from(home).join(&p[2..]))
                } else {
                    Ok(PathBuf::from(p))
                }
            } else {
                Ok(PathBuf::from(p))
            }
        }
        None => Ok(config.palace_path.clone()),
    }
}

// ---------------------------------------------------------------------------
// Entity detection helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Debug)]
struct DetectedEntities {
    people: Vec<PersonEntity>,
    projects: Vec<ProjectEntity>,
    uncertain: Vec<UncertainEntity>,
}

#[derive(Clone, Default, Debug)]
struct UncertainEntity {
    name: String,
    confidence: f32,
    context: String,
}

/// Scan directory for files that can be used for entity detection.
fn scan_and_detect_entities(dir: &PathBuf) -> DetectedEntities {
    let mut all_text = String::new();
    let mut count = 0;

    let extensions = ["txt", "md", "py", "js", "ts", "rs", "json", "yaml", "yml"];

    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .take(50)
    {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if extensions.contains(&ext_str.as_str()) {
                if let Ok(content) = std::fs::read_to_string(path) {
                    all_text.push_str(&content);
                    all_text.push('\n');
                    count += 1;
                }
            }
        }
    }

    if count == 0 {
        return DetectedEntities::default();
    }

    let result = detect_from_content(&all_text);

    DetectedEntities {
        people: result.people,
        projects: result.projects,
        uncertain: vec![],
    }
}

/// Confirm entities with registry integration.
/// - Filters out previously rejected entities
/// - In yes mode, accepts all non-rejected entities
/// - In interactive mode, could prompt for confirmations (stub for now)
fn confirm_entities(detected: &DetectedEntities, yes: bool) -> DetectedEntities {
    // Load registry to check rejected entities from default path
    let registry_path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mempalace")
        .join("registry.json");
    let rejected_names: Vec<String> = EntityRegistry::load(&registry_path)
        .ok()
        .map(|r| r.get_rejected().to_vec())
        .unwrap_or_default();

    let is_rejected = |name: &str| rejected_names.iter().any(|r| r == name);

    if yes {
        // Non-interactive: accept all non-rejected entities
        DetectedEntities {
            people: detected
                .people
                .iter()
                .filter(|p| !is_rejected(&p.name))
                .cloned()
                .collect(),
            projects: detected
                .projects
                .iter()
                .filter(|p| !is_rejected(&p.name))
                .cloned()
                .collect(),
            uncertain: detected
                .uncertain
                .iter()
                .filter(|p| !is_rejected(&p.name))
                .cloned()
                .collect(),
        }
    } else {
        // Interactive: accept non-rejected entities (stub - real UI would ask)
        detected.clone()
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

fn cmd_init(dir: &PathBuf, yes: bool) -> Result<()> {
    println!();
    println!("{}", "=".repeat(55));
    println!("  MemPalace Init");
    println!("{}", "=".repeat(55));

    // Pass 1: scan for entities
    println!("\n  Scanning for entities in: {:?}", dir);
    let detected = scan_and_detect_entities(dir);
    let total = detected.people.len() + detected.projects.len() + detected.uncertain.len();
    if total > 0 {
        println!("  Found {} entities", total);
        let confirmed = confirm_entities(&detected, yes);
        if !confirmed.people.is_empty() {
            println!(
                "  People: {:?}",
                confirmed
                    .people
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
        if !confirmed.projects.is_empty() {
            println!(
                "  Projects: {:?}",
                confirmed
                    .projects
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
    } else {
        println!("  No entities detected -- proceeding with directory-based rooms.");
    }

    // Pass 2: detect rooms from folder structure
    println!();
    println!("  Detecting rooms from folder structure...");
    let rooms = detect_rooms_from_folders(dir);
    for room in &rooms {
        println!("    ROOM: {}", room.name);
        println!("          {}", room.description);
    }

    // Pass 3: initialize config
    let config = Config::load()?;
    let config_path = config.init()?;
    println!();
    println!("  Config saved: {:?}", config_path);
    println!();
    println!("  Next step:");
    println!("    mempalace mine {:?}", dir);
    println!();
    println!("{}", "=".repeat(55));

    Ok(())
}

/// Auto-detect mining mode by scanning for conversation file patterns
fn detect_mining_mode(dir: &PathBuf) -> MiningMode {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return MiningMode::Projects,
    };

    let mut has_conversation_markers = 0;
    let mut has_project_markers = 0;

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Check for conversation markers
        let name_lower = name.to_lowercase();
        if name_lower.contains("conversation")
            || name_lower.contains("transcript")
            || name_lower.contains("chatgpt")
            || name_lower.contains("claude")
            || name_lower.ends_with(".jsonl")
            || name_lower.ends_with(".json")
        {
            has_conversation_markers += 1;
        }

        // Check for project markers
        if path.is_dir()
            && (name_lower == "src"
                || name_lower == "lib"
                || name_lower == "tests"
                || name_lower == "scripts"
                || name_lower == "bin")
        {
            has_project_markers += 1;
        } else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(
                ext.to_lowercase().as_str(),
                "rs" | "py" | "js" | "ts" | "go" | "java" | "txt" | "md"
            ) {
                has_project_markers += 1;
            }
        }
    }

    if has_conversation_markers > has_project_markers {
        MiningMode::Convos
    } else {
        MiningMode::Projects
    }
}

fn cmd_mine(
    dir: &PathBuf,
    mode: &MiningMode,
    wing: Option<&str>,
    _agent: &str,
    _limit: Option<usize>,
    dry_run: bool,
    palace_arg: Option<&str>,
    extract: Option<&str>,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    if dry_run {
        println!("\n  [DRY RUN] Would mine: {:?}", dir);
        println!("  Palace: {:?}", palace_path);
        if let Some(w) = wing {
            println!("  Wing: {}", w);
        }
        println!("  Mode: {:?}", mode);
        return Ok(());
    }

    match mode {
        MiningMode::Projects => {
            let result = runtime().block_on(miner::mine(dir, &palace_path, wing, None));
            match result {
                Ok(mining_result) => {
                    print_mining_result(&mining_result);
                }
                Err(e) => {
                    eprintln!("  Mining error: {}", e);
                    return Err(e);
                }
            }
        }
        MiningMode::Convos => {
            let result = runtime().block_on(mine_conversations(dir, &palace_path, wing, extract));
            match result {
                Ok(convo_result) => {
                    print_convo_result(&convo_result);
                }
                Err(e) => {
                    eprintln!("  Convo mining error: {}", e);
                    return Err(e);
                }
            }
        }
        MiningMode::Auto => {
            // Auto-detect mining mode by scanning for known patterns
            let detected = detect_mining_mode(dir);
            println!("  Auto-detected mode: {:?}", detected);
            match detected {
                MiningMode::Projects | MiningMode::Auto => {
                    let result = runtime().block_on(miner::mine(dir, &palace_path, wing, None));
                    match result {
                        Ok(mining_result) => print_mining_result(&mining_result),
                        Err(e) => {
                            eprintln!("  Mining error: {}", e);
                            return Err(e);
                        }
                    }
                }
                MiningMode::Convos => {
                    let result =
                        runtime().block_on(mine_conversations(dir, &palace_path, wing, extract));
                    match result {
                        Ok(convo_result) => print_convo_result(&convo_result),
                        Err(e) => {
                            eprintln!("  Convo mining error: {}", e);
                            return Err(e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn cmd_search(
    query: &str,
    wing: Option<&str>,
    room: Option<&str>,
    results: usize,
    palace_arg: Option<&str>,
    embedding: Option<&str>,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let config = Config::load().ok();
    let model = embedding.or_else(|| config.as_ref().map(|c| c.embedding_model.as_str()));
    runtime().block_on(searcher::search(
        query,
        &palace_path,
        wing,
        room,
        results,
        model,
    ))?;
    Ok(())
}

fn cmd_wakeup(wing: Option<&str>, palace_arg: Option<&str>) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let mut stack = MemoryStack::new(Some(palace_path.clone()), None);

    let text = runtime().block_on(stack.wake_up(wing));
    let tokens = text.len() / 4;

    println!("Wake-up text (~{} tokens):", tokens);
    println!("{}", "=".repeat(50));
    println!("{}", text);

    Ok(())
}

fn cmd_compress(
    wing: Option<&str>,
    dry_run: bool,
    config_path: Option<&str>,
    palace_arg: Option<&str>,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    // Try to load entity config if not provided
    let config_path = config_path.map(PathBuf::from).or_else(|| {
        let p1 = PathBuf::from("entities.json");
        if p1.exists() {
            Some(p1)
        } else {
            let p2 = palace_path.join("entities.json");
            if p2.exists() {
                Some(p2)
            } else {
                None
            }
        }
    });

    if let Some(ref cp) = config_path {
        if cp.exists() {
            if let Ok(content) = std::fs::read_to_string(cp) {
                if serde_json::from_str::<serde_json::Value>(&content).is_ok() {
                    println!("  Loaded entity config: {:?}", cp);
                }
            }
        }
    }

    // Connect to palace
    let Ok(palace_db) = PalaceDb::open(&palace_path) else {
        println!("\n  No palace found at {:?}", palace_path);
        println!("  Run: mempalace init <dir> then mempalace mine <dir>");
        return Ok(());
    };

    println!(
        "\n  Compressing drawers{}...",
        if let Some(ref w) = wing {
            format!(" in wing '{w}'")
        } else {
            String::new()
        }
    );
    println!();

    // Get all entries, optionally filtered by wing
    let all_results = palace_db.get_all(wing, None, 1000);
    let mut docs = Vec::new();
    let mut metas = Vec::new();
    let mut ids = Vec::new();

    for qr in &all_results {
        for (i, doc) in qr.documents.iter().enumerate() {
            let meta = qr.metadatas.get(i).cloned().unwrap_or_default();
            let id = qr.ids.get(i).cloned().unwrap_or_default();
            docs.push(doc.clone());
            metas.push(meta);
            ids.push(id);
        }
    }

    if docs.is_empty() {
        println!(
            "  No drawers found{}.",
            if let Some(ref w) = wing {
                format!(" in wing '{w}'")
            } else {
                String::new()
            }
        );
        return Ok(());
    }

    let total_original = docs.iter().map(|s| s.len()).sum::<usize>();
    let mut total_compressed = 0;
    let mut compressed_entries: Vec<(
        String,
        String,
        std::collections::HashMap<String, serde_json::Value>,
        dialect::CompressionStats,
    )> = Vec::new();

    for i in 0..docs.len() {
        let id = &ids[i];
        let doc = &docs[i];
        let meta = &metas[i];

        let compressed = dialect::compress(doc, &std::collections::HashMap::new());
        let stats = dialect::compression_stats(doc, &compressed);
        total_compressed += stats.compressed_tokens * 4;

        if dry_run {
            let wing_name = meta
                .get("wing")
                .and_then(|v: &serde_json::Value| v.as_str())
                .unwrap_or("?");
            let room_name = meta
                .get("room")
                .and_then(|v: &serde_json::Value| v.as_str())
                .unwrap_or("?");
            let source: String = meta
                .get("source_file")
                .and_then(|v: &serde_json::Value| v.as_str())
                .and_then(|s: &str| {
                    std::path::Path::new(s)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "?".to_string());
            println!("  [{wing_name}/{room_name}] {source}");
            println!(
                "    {}t -> {}t ({:.1}x)",
                stats.original_tokens, stats.compressed_tokens, stats.ratio
            );
            if !compressed.is_empty() {
                println!("    {compressed}");
            } else {
                println!("    <not implemented>");
            }
            println!();
        }

        compressed_entries.push((id.clone(), compressed, meta.clone(), stats));
    }

    // Store compressed versions (unless dry-run)
    if !dry_run {
        println!("  Stored {} compressed drawers.", compressed_entries.len());
    }

    // Summary
    let ratio = total_original as f64 / total_compressed.max(1) as f64;
    let orig_tokens = total_original / 4;
    let comp_tokens = total_compressed / 4;
    println!(
        "  Total: {}t -> {}t ({:.1}x compression)",
        orig_tokens, comp_tokens, ratio
    );
    if dry_run {
        println!("  (dry run -- nothing stored)");
    }

    Ok(())
}

fn cmd_split(
    dir: &PathBuf,
    _output_dir: Option<&PathBuf>,
    _dry_run: bool,
    min_sessions: usize,
) -> Result<()> {
    println!();
    println!("  Splitting transcript files in: {:?}", dir);
    println!();

    // Find .txt files that could be mega-files
    let txt_files: Vec<_> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "txt")
                .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    if txt_files.is_empty() {
        println!("  No .txt files found in {:?}", dir);
        return Ok(());
    }

    let mut total_sessions = 0;
    let mut files_created = Vec::new();
    let mut errors = Vec::new();

    for file_path in txt_files {
        let result = runtime().block_on(split_file(&file_path, Some(min_sessions)));
        match result {
            Ok(split_result) => {
                total_sessions += split_result.sessions_found;
                files_created.extend(split_result.files_created);
                errors.extend(split_result.errors);
            }
            Err(e) => {
                errors.push(format!("Error processing {:?}: {}", file_path, e));
            }
        }
    }

    println!();
    println!("  Sessions found: {}", total_sessions);
    if files_created.is_empty() {
        println!("  No files created.");
    } else {
        println!("  Files created:");
        for f in &files_created {
            println!("    {}", f);
        }
    }
    if !errors.is_empty() {
        println!("  Errors:");
        for e in &errors {
            println!("    {}", e);
        }
    }

    Ok(())
}

/// Platform-specific session discovery
fn discover_ai_sessions() -> Vec<(String, PathBuf)> {
    let mut sessions = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    // Claude Code sessions (~/.claude/)
    if let Some(ref h) = home {
        let claude_dir = h.join(".claude");
        if claude_dir.exists() {
            sessions.push(("Claude Code".to_string(), claude_dir));
        }
        // Codex sessions (~/.codex/sessions/)
        let codex_dir = h.join(".codex").join("sessions");
        if codex_dir.exists() {
            sessions.push(("Codex".to_string(), codex_dir));
        }
        // OpenCode sessions
        let opencode_dir = h.join(".opencode");
        if opencode_dir.exists() {
            sessions.push(("OpenCode".to_string(), opencode_dir));
        }
    }

    // Platform-specific paths
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = home {
            let app_support = h.join("Library").join("Application Support");
            for tool in &["Claude", "ChatGPT", "SoulForge"] {
                let path = app_support.join(tool);
                if path.exists() {
                    sessions.push((tool.to_string(), path));
                }
            }
        }
    }

    sessions
}

fn cmd_mine_device(wing: Option<&str>, dry_run: bool, palace_arg: Option<&str>) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    println!();
    println!("{}", "=".repeat(55));
    println!("  MemPalace: Mine Device");
    println!("{}", "=".repeat(55));

    let sessions = discover_ai_sessions();

    if sessions.is_empty() {
        println!("  No AI tool sessions found.");
        return Ok(());
    }

    println!("  Discovered sessions:");
    for (tool, path) in &sessions {
        println!("    {}: {}", tool, path.display());
    }
    println!();

    if dry_run {
        println!("  [DRY RUN] Skipping actual mining.");
        return Ok(());
    }

    let mut total_mined = 0;
    for (tool, path) in &sessions {
        let wing_name = wing.unwrap_or(tool);
        println!("  Mining {} sessions into wing '{}'...", tool, wing_name);
        let result = runtime().block_on(miner::mine(path, &palace_path, Some(wing_name), None));
        match result {
            Ok(r) => {
                println!(
                    "    {} files, {} chunks",
                    r.files_processed, r.chunks_created
                );
                total_mined += r.chunks_created;
            }
            Err(e) => {
                println!("    Error: {}", e);
            }
        }
    }

    println!();
    println!("  Total chunks mined: {}", total_mined);
    Ok(())
}

fn cmd_status(palace_arg: Option<&str>) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    // Expand path for display
    let display_path =
        if palace_path.starts_with("~/") || palace_path.to_string_lossy().contains("~") {
            if let Ok(home) = std::env::var("HOME") {
                let path_str = palace_path.to_string_lossy();
                if path_str.starts_with("~/") {
                    PathBuf::from(home).join(&path_str[2..])
                } else {
                    PathBuf::from(path_str.replace("~", &home))
                }
            } else {
                palace_path.clone()
            }
        } else {
            palace_path.clone()
        };

    println!();
    println!("{}", "=".repeat(55));
    println!("  MemPalace Status");
    println!("{}", "=".repeat(55));
    println!("  Palace: {}", display_path.display());

    let config = Config::load()?;
    println!("  Topic wings: {:?}", config.topic_wings);

    match PalaceDb::open(&palace_path) {
        Ok(db) => {
            let count = db.count();
            println!("  Total drawers: {}", count);
        }
        Err(e) => {
            println!("  Palace not yet initialized: {}", e);
            println!("  Run: mempalace init <dir> then mempalace mine <dir>");
        }
    }

    // Show identity info
    let identity_path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mempalace")
        .join("identity.txt");

    if identity_path.exists() {
        let tokens = std::fs::read_to_string(&identity_path)
            .map(|s| s.len() / 4)
            .unwrap_or(0);
        println!("  L0 Identity: exists (~{} tokens)", tokens);
    } else {
        println!("  L0 Identity: not configured");
        println!("  Create: ~/.mempalace/identity.txt");
    }

    println!("{}", "=".repeat(55));

    Ok(())
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

fn print_mining_result(result: &MiningResult) {
    println!();
    println!("  Mining complete!");
    println!("    Files processed: {}", result.files_processed);
    println!("    Chunks created: {}", result.chunks_created);
    if !result.errors.is_empty() {
        println!("    Errors ({}):", result.errors.len());
        for e in &result.errors {
            println!("      {}", e);
        }
    }
}

fn print_convo_result(result: &ConvoMiningResult) {
    println!();
    println!("  Convo mining complete!");
    println!("    Files processed: {}", result.files_processed);
    println!("    Conversations mined: {}", result.conversations_mined);
    println!("    Chunks created: {}", result.chunks_created);
    if !result.errors.is_empty() {
        println!("    Errors ({}):", result.errors.len());
        for e in &result.errors {
            println!("      {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime helper
// ---------------------------------------------------------------------------

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("failed to create tokio runtime")
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let palace_arg = cli.palace.as_deref();

    match &cli.command {
        Commands::Init { dir, yes } => cmd_init(dir, *yes)?,
        Commands::Mine {
            dir,
            mode,
            wing,
            agent,
            limit,
            dry_run,
            extract,
        } => cmd_mine(
            dir,
            mode,
            wing.as_deref(),
            agent,
            *limit,
            *dry_run,
            palace_arg,
            extract.as_deref(),
        )?,
        Commands::Search {
            query,
            wing,
            room,
            results,
            embedding,
        } => cmd_search(
            query,
            wing.as_deref(),
            room.as_deref(),
            *results,
            palace_arg,
            embedding.as_deref(),
        )?,
        Commands::WakeUp { wing } => cmd_wakeup(wing.as_deref(), palace_arg)?,
        Commands::Compress {
            wing,
            dry_run,
            config,
        } => cmd_compress(wing.as_deref(), *dry_run, config.as_deref(), palace_arg)?,
        Commands::Split {
            dir,
            output_dir,
            dry_run,
            min_sessions,
        } => cmd_split(dir, output_dir.as_ref(), *dry_run, *min_sessions)?,
        Commands::Status {} => cmd_status(palace_arg)?,
        Commands::MineDevice { wing, dry_run } => {
            cmd_mine_device(wing.as_deref(), *dry_run, palace_arg)?
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mining_mode_parsing() {
        assert!(matches!(
            "projects".parse::<MiningMode>(),
            Ok(MiningMode::Projects)
        ));
        assert!(matches!(
            "convos".parse::<MiningMode>(),
            Ok(MiningMode::Convos)
        ));
        assert!(matches!(
            "conversations".parse::<MiningMode>(),
            Ok(MiningMode::Convos)
        ));
        assert!("invalid".parse::<MiningMode>().is_err());
    }

    #[test]
    fn test_cli_args_parse_init() {
        let args = Cli::try_parse_from(["mempalace", "init", "/tmp/test", "--yes"]).unwrap();
        match args.command {
            Commands::Init { dir, yes } => {
                assert_eq!(dir, PathBuf::from("/tmp/test"));
                assert!(yes);
            }
            _ => panic!("expected init command"),
        }
    }

    #[test]
    fn test_cli_args_parse_mine() {
        let args = Cli::try_parse_from([
            "mempalace",
            "mine",
            "/tmp/test",
            "--mode",
            "convos",
            "--wing",
            "test_wing",
            "--dry-run",
        ])
        .unwrap();
        match args.command {
            Commands::Mine {
                dir,
                mode,
                wing,
                dry_run,
                ..
            } => {
                assert_eq!(dir, PathBuf::from("/tmp/test"));
                assert!(matches!(mode, MiningMode::Convos));
                assert_eq!(wing, Some("test_wing".to_string()));
                assert!(dry_run);
            }
            _ => panic!("expected mine command"),
        }
    }

    #[test]
    fn test_cli_args_parse_search() {
        let args = Cli::try_parse_from([
            "mempalace",
            "search",
            "rust async",
            "--wing",
            "tech",
            "--room",
            "backend",
            "--results",
            "10",
        ])
        .unwrap();
        match args.command {
            Commands::Search {
                query,
                wing,
                room,
                results,
                embedding: _,
            } => {
                assert_eq!(query, "rust async");
                assert_eq!(wing, Some("tech".to_string()));
                assert_eq!(room, Some("backend".to_string()));
                assert_eq!(results, 10);
            }
            _ => panic!("expected search command"),
        }
    }

    #[test]
    fn test_cli_args_parse_wakeup() {
        let args = Cli::try_parse_from(["mempalace", "wake-up", "--wing", "myapp"]).unwrap();
        match args.command {
            Commands::WakeUp { wing } => {
                assert_eq!(wing, Some("myapp".to_string()));
            }
            _ => panic!("expected wake-up command"),
        }
    }

    #[test]
    fn test_cli_args_parse_compress() {
        let args = Cli::try_parse_from([
            "mempalace",
            "compress",
            "--wing",
            "myapp",
            "--dry-run",
            "--config",
            "entities.json",
        ])
        .unwrap();
        match args.command {
            Commands::Compress {
                wing,
                dry_run,
                config,
            } => {
                assert_eq!(wing, Some("myapp".to_string()));
                assert!(dry_run);
                assert_eq!(config, Some("entities.json".to_string()));
            }
            _ => panic!("expected compress command"),
        }
    }

    #[test]
    fn test_cli_args_parse_split() {
        let args = Cli::try_parse_from([
            "mempalace",
            "split",
            "/tmp/chats",
            "--output-dir",
            "/tmp/split",
            "--dry-run",
            "--min-sessions",
            "3",
        ])
        .unwrap();
        match args.command {
            Commands::Split {
                dir,
                output_dir,
                dry_run,
                min_sessions,
            } => {
                assert_eq!(dir, PathBuf::from("/tmp/chats"));
                assert_eq!(output_dir, Some(PathBuf::from("/tmp/split")));
                assert!(dry_run);
                assert_eq!(min_sessions, 3);
            }
            _ => panic!("expected split command"),
        }
    }

    #[test]
    fn test_cli_args_parse_status() {
        let args = Cli::try_parse_from(["mempalace", "status"]).unwrap();
        assert!(matches!(args.command, Commands::Status {}));
    }

    #[test]
    fn test_cli_args_with_palace_override() {
        let args =
            Cli::try_parse_from(["mempalace", "--palace", "/custom/palace", "status"]).unwrap();
        assert_eq!(args.palace, Some("/custom/palace".to_string()));
    }

    #[test]
    fn test_scan_and_detect_entities_empty_dir() {
        let temp = tempfile::TempDir::new().unwrap();
        let result = scan_and_detect_entities(&temp.path().to_path_buf());
        assert!(result.people.is_empty());
        assert!(result.projects.is_empty());
    }

    #[test]
    fn test_confirm_entities_passes_through() {
        let detected = DetectedEntities::default();
        let confirmed = confirm_entities(&detected, false);
        assert!(confirmed.people.is_empty());
    }
}
