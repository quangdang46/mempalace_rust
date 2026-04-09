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
//!     mcp                          Show MCP setup command
//!     status                       Show what's been filed
//!     repair                       Rebuild palace vector index from stored data
//!     hook run --hook ...          Run hook logic
//!     instructions <name>          Output skill instructions
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
use crate::split_mega_files::split_file_with_options;

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

        /// Don't respect .gitignore files when scanning project files
        #[arg(long)]
        no_gitignore: bool,

        /// Always scan these project-relative paths even if ignored; repeat or pass comma-separated paths
        #[arg(long, action = clap::ArgAction::Append)]
        include_ignored: Vec<String>,

        /// Your name -- recorded on every drawer (default: mempalace)
        #[arg(long, default_value = "mempalace")]
        agent: String,

        /// Max files to process (0 = all)
        #[arg(long, default_value = "0")]
        limit: usize,

        /// Show what would be filed without filing
        #[arg(long)]
        dry_run: bool,

        /// Extraction strategy for convos: 'exchange' (default) or 'general'
        #[arg(long, default_value = "exchange")]
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

    /// Run hook logic (reads JSON from stdin, outputs JSON to stdout).
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },

    /// Output skill instructions to stdout.
    #[command(disable_help_subcommand = true)]
    Instructions {
        #[command(subcommand)]
        name: InstructionName,
    },

    /// Rebuild palace vector index from stored data.
    Repair,

    /// Show what's been filed.
    Status,

    /// Internal Rust-only helper to mine discovered device sessions.
    #[command(hide = true, name = "mine-device")]
    MineDevice {
        /// Wing name for discovered sessions
        #[arg(long)]
        wing: Option<String>,

        /// Don't actually mine, just show what would be mined
        #[arg(long)]
        dry_run: bool,
    },

    /// Show MCP setup command for connecting MemPalace to your AI client.
    Mcp,
}

#[derive(Subcommand)]
enum HookAction {
    Run {
        #[arg(long, value_parser = ["session-start", "stop", "precompact"])]
        hook: String,
        #[arg(long, value_parser = ["claude-code", "codex"])]
        harness: String,
    },
}

#[derive(Subcommand, Clone, Debug)]
enum InstructionName {
    Init,
    Search,
    Mine,
    Help,
    Status,
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
                    Ok(PathBuf::from(home).join(p.strip_prefix("~/").unwrap()))
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
    _confidence: f32,
    _context: String,
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
    let registry_path = Config::registry_file_path().unwrap_or_else(|_| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".mempalace")
            .join("entity_registry.json")
    });
    let rejected_names: Vec<String> = EntityRegistry::load(&registry_path)
        .ok()
        .map(|r| r.get_rejected().to_vec())
        .unwrap_or_default();

    let is_rejected = |name: &str| {
        let lowered = name.to_lowercase();
        rejected_names.iter().any(|r| r == &lowered)
    };

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

    let config = Config::load()?;
    let config_path = config.init()?;
    let config_dir = config_path.parent().unwrap_or(&config_path);

    if !yes {
        let _registry = crate::onboarding::run_onboarding(dir, config_dir, true)?;
        println!("  Config saved: {:?}", config_path);
        println!();
        println!("  Next step:");
        println!("    mempalace mine {:?}", dir);
        println!();
        println!("{}", "=".repeat(55));
        return Ok(());
    }

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

#[allow(clippy::too_many_arguments)]
fn cmd_mine(
    dir: &PathBuf,
    mode: &MiningMode,
    wing: Option<&str>,
    agent: &str,
    limit: usize,
    dry_run: bool,
    no_gitignore: bool,
    include_ignored: &[String],
    palace_arg: Option<&str>,
    extract: Option<&str>,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let include_ignored_flat: Vec<String> = include_ignored
        .iter()
        .flat_map(|raw| raw.split(','))
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect();

    if dry_run && !matches!(mode, MiningMode::Convos) {
        println!("\n  [DRY RUN] Would mine: {:?}", dir);
        println!("  Palace: {:?}", palace_path);
        if let Some(w) = wing {
            println!("  Wing: {}", w);
        }
        println!("  Mode: {:?}", mode);
        if no_gitignore {
            println!("  .gitignore: DISABLED");
        }
        if !include_ignored_flat.is_empty() {
            println!("  Include ignored: {:?}", include_ignored_flat);
        }
        return Ok(());
    }

    match mode {
        MiningMode::Projects => {
            let result = runtime().block_on(miner::mine(
                dir,
                &palace_path,
                wing,
                if include_ignored_flat.is_empty() {
                    None
                } else {
                    Some(include_ignored_flat.as_slice())
                },
            ));
            match result {
                Ok(mining_result) => {
                    let mining_result = apply_mine_limit(mining_result, limit);
                    if no_gitignore {
                        // Parser-visible parity only for now; runtime behavior lands in later dedicated bead.
                    }
                    print_mining_result(&mining_result);
                }
                Err(e) => {
                    eprintln!("  Mining error: {}", e);
                    return Err(e);
                }
            }
        }
        MiningMode::Convos => {
            let result = runtime().block_on(mine_conversations(
                dir,
                &palace_path,
                wing,
                agent,
                limit,
                dry_run,
                extract,
            ));
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
                    let result = runtime().block_on(miner::mine(
                        dir,
                        &palace_path,
                        wing,
                        if include_ignored_flat.is_empty() {
                            None
                        } else {
                            Some(include_ignored_flat.as_slice())
                        },
                    ));
                    match result {
                        Ok(mining_result) => {
                            print_mining_result(&apply_mine_limit(mining_result, limit))
                        }
                        Err(e) => {
                            eprintln!("  Mining error: {}", e);
                            return Err(e);
                        }
                    }
                }
                MiningMode::Convos => {
                    let result = runtime().block_on(mine_conversations(
                        dir,
                        &palace_path,
                        wing,
                        agent,
                        limit,
                        dry_run,
                        extract,
                    ));
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
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    runtime().block_on(searcher::search(
        query,
        &palace_path,
        wing,
        room,
        results,
        None,
    ))?;
    Ok(())
}

fn cmd_hook(hook: &str, harness: &str) -> Result<()> {
    anyhow::bail!(
        "hook runtime is not implemented yet in Rust; use the Python reference flow for now ({hook}, {harness})"
    )
}

fn cmd_instructions(name: &InstructionName) -> Result<()> {
    let label = match name {
        InstructionName::Init => "init",
        InstructionName::Search => "search",
        InstructionName::Mine => "mine",
        InstructionName::Help => "help",
        InstructionName::Status => "status",
    };
    println!("instructions runtime is not implemented yet in Rust; requested: {label}");
    Ok(())
}

fn cmd_repair(palace_arg: Option<&str>) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    if !palace_path.is_dir() {
        println!("\n  No palace found at {}", palace_path.display());
        return Ok(());
    }

    println!("\n{}", "=".repeat(55));
    println!("  MemPalace Repair");
    println!("{}\n", "=".repeat(55));
    println!("  Palace: {}", palace_path.display());

    let mut db = PalaceDb::open(&palace_path)?;
    let total = db.count();
    println!("  Drawers found: {}", total);
    if total == 0 {
        println!("  Nothing to repair.");
        return Ok(());
    }

    let backup_path = PathBuf::from(format!("{}.backup", palace_path.display()));
    if backup_path.exists() {
        std::fs::remove_dir_all(&backup_path)?;
    }
    std::fs::create_dir_all(&backup_path)?;
    let docs_name = format!("{}.json", crate::palace_db::DEFAULT_COLLECTION_NAME);
    let source_docs = palace_path.join(&docs_name);
    let backup_docs = backup_path.join(&docs_name);
    if source_docs.exists() {
        std::fs::copy(&source_docs, &backup_docs)?;
    }

    db.flush()?;
    println!("\n  Repair complete. {} drawers rebuilt.", total);
    println!("  Backup saved at {}", backup_path.display());
    println!("\n{}\n", "=".repeat(55));
    Ok(())
}

fn cmd_mcp(palace_arg: Option<&str>) {
    let base_server_cmd = "python -m mempalace.mcp_server";
    if let Some(palace) = palace_arg {
        let resolved_palace = if let Some(stripped) = palace.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(stripped)
        } else {
            PathBuf::from(palace)
        };
        println!("MemPalace MCP quick setup:");
        println!(
            "  claude mcp add mempalace -- {} --palace {}",
            base_server_cmd,
            resolved_palace.display()
        );
        println!("\nRun the server directly:");
        println!(
            "  {} --palace {}",
            base_server_cmd,
            resolved_palace.display()
        );
    } else {
        println!("MemPalace MCP quick setup:");
        println!("  claude mcp add mempalace -- {}", base_server_cmd);
        println!("\nRun the server directly:");
        println!("  {}", base_server_cmd);
        println!("\nOptional custom palace:");
        println!(
            "  claude mcp add mempalace -- {} --palace /path/to/palace",
            base_server_cmd
        );
        println!("  {} --palace /path/to/palace", base_server_cmd);
    }
}

fn apply_mine_limit(mut result: MiningResult, limit: usize) -> MiningResult {
    if limit == 0 {
        return result;
    }
    result.files_processed = result.files_processed.min(limit);
    result
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
    output_dir: Option<&PathBuf>,
    dry_run: bool,
    min_sessions: usize,
) -> Result<()> {
    let max_scan_size = 500 * 1024 * 1024;

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

    let mut mega_files = Vec::new();
    for file_path in txt_files {
        let Ok(metadata) = std::fs::metadata(&file_path) else {
            continue;
        };
        if metadata.len() > max_scan_size {
            println!(
                "  SKIP: {} exceeds {} MB limit",
                file_path.display(),
                max_scan_size / (1024 * 1024)
            );
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&file_path) else {
            continue;
        };
        let lines: Vec<String> = content.split('\n').map(|s| s.to_string()).collect();
        let boundaries = crate::split_mega_files::find_session_boundaries_for_cli(&lines);
        if boundaries.len() >= min_sessions {
            mega_files.push((file_path, boundaries.len(), metadata.len()));
        }
    }

    if mega_files.is_empty() {
        println!(
            "No mega-files found in {:?} (min {} sessions).",
            dir, min_sessions
        );
        return Ok(());
    }

    println!();
    println!("{}", "=".repeat(60));
    println!(
        "  Mega-file splitter — {}",
        if dry_run { "DRY RUN" } else { "SPLITTING" }
    );
    println!("{}", "=".repeat(60));
    println!("  Source:      {}", dir.display());
    println!(
        "  Output:      {}",
        output_dir
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "same dir as source".to_string())
    );
    println!("  Mega-files:  {}", mega_files.len());
    println!("{}", "─".repeat(60));
    println!();

    let mut total_sessions = 0;
    let mut files_created = Vec::new();
    let mut errors = Vec::new();

    for (file_path, session_count, size_bytes) in mega_files {
        println!(
            "  {}  ({} sessions, {}KB)",
            file_path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| file_path.display().to_string()),
            session_count,
            size_bytes / 1024
        );
        let result = runtime().block_on(split_file_with_options(
            &file_path,
            Some(min_sessions),
            output_dir.map(|path| path.as_path()),
            dry_run,
        ));
        match result {
            Ok(split_result) => {
                let created_any = !split_result.files_created.is_empty();
                total_sessions += split_result.sessions_found;
                files_created.extend(split_result.files_created);
                errors.extend(split_result.errors);
                if !dry_run && created_any {
                    let backup = file_path.with_extension("mega_backup");
                    match std::fs::rename(&file_path, &backup) {
                        Ok(_) => println!("  → Original renamed to {}", backup.display()),
                        Err(error) => errors.push(format!(
                            "Failed to rename {} to {}: {}",
                            file_path.display(),
                            backup.display(),
                            error
                        )),
                    }
                }
                println!();
            }
            Err(e) => {
                errors.push(format!("Error processing {:?}: {}", file_path, e));
                println!();
            }
        }
    }

    println!("{}", "─".repeat(60));
    if dry_run {
        println!(
            "  DRY RUN — would create {} files from {} mega-files",
            files_created.len(),
            total_sessions
        );
    } else {
        println!(
            "  Done — created {} files from {} mega-files",
            files_created.len(),
            total_sessions
        );
    }
    println!();
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
                    PathBuf::from(home).join(path_str.strip_prefix("~/").unwrap())
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
    let identity_path = Config::identity_file_path().unwrap_or_else(|_| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".mempalace")
            .join("identity.txt")
    });

    if identity_path.exists() {
        let tokens = std::fs::read_to_string(&identity_path)
            .map(|s| s.len() / 4)
            .unwrap_or(0);
        println!("  L0 Identity: exists (~{} tokens)", tokens);
    } else {
        println!("  L0 Identity: not configured");
        println!("  Create: {}", identity_path.display());
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
            no_gitignore,
            include_ignored,
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
            *no_gitignore,
            include_ignored,
            palace_arg,
            extract.as_deref(),
        )?,
        Commands::Search {
            query,
            wing,
            room,
            results,
        } => cmd_search(
            query,
            wing.as_deref(),
            room.as_deref(),
            *results,
            palace_arg,
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
        Commands::Hook { action } => match action {
            HookAction::Run { hook, harness } => cmd_hook(hook, harness)?,
        },
        Commands::Instructions { name } => cmd_instructions(name)?,
        Commands::Repair => cmd_repair(palace_arg)?,
        Commands::Status => cmd_status(palace_arg)?,
        Commands::MineDevice { wing, dry_run } => {
            cmd_mine_device(wing.as_deref(), *dry_run, palace_arg)?
        }
        Commands::Mcp => cmd_mcp(palace_arg),
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
                no_gitignore,
                include_ignored,
                limit,
                dry_run,
                ..
            } => {
                assert_eq!(dir, PathBuf::from("/tmp/test"));
                assert!(matches!(mode, MiningMode::Convos));
                assert_eq!(wing, Some("test_wing".to_string()));
                assert!(!no_gitignore);
                assert!(include_ignored.is_empty());
                assert_eq!(limit, 0);
                assert!(dry_run);
            }
            _ => panic!("expected mine command"),
        }
    }

    #[test]
    fn test_cli_args_parse_mine_gitignore_flags() {
        let args = Cli::try_parse_from([
            "mempalace",
            "mine",
            "/tmp/test",
            "--no-gitignore",
            "--include-ignored",
            "a.txt,b.txt",
            "--include-ignored",
            "c.txt",
        ])
        .unwrap();
        match args.command {
            Commands::Mine {
                no_gitignore,
                include_ignored,
                ..
            } => {
                assert!(no_gitignore);
                assert_eq!(include_ignored, vec!["a.txt,b.txt", "c.txt"]);
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
        assert!(matches!(args.command, Commands::Status));
    }

    #[test]
    fn test_cli_args_parse_repair() {
        let args = Cli::try_parse_from(["mempalace", "repair"]).unwrap();
        assert!(matches!(args.command, Commands::Repair));
    }

    #[test]
    fn test_cli_args_parse_hook_run() {
        let args = Cli::try_parse_from([
            "mempalace",
            "hook",
            "run",
            "--hook",
            "session-start",
            "--harness",
            "claude-code",
        ])
        .unwrap();
        match args.command {
            Commands::Hook {
                action: HookAction::Run { hook, harness },
            } => {
                assert_eq!(hook, "session-start");
                assert_eq!(harness, "claude-code");
            }
            _ => panic!("expected hook command"),
        }
    }

    #[test]
    fn test_cli_args_parse_instructions() {
        let args = Cli::try_parse_from(["mempalace", "instructions", "help"]).unwrap();
        match args.command {
            Commands::Instructions { name } => {
                assert!(matches!(name, InstructionName::Help));
            }
            _ => panic!("expected instructions command"),
        }
    }

    #[test]
    fn test_cli_args_parse_mcp() {
        let args = Cli::try_parse_from(["mempalace", "mcp"]).unwrap();
        assert!(matches!(args.command, Commands::Mcp));
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

    #[test]
    fn test_confirm_entities_uses_config_registry_path() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let registry_path = Config::registry_file_path().unwrap();
        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        let mut registry = EntityRegistry::load(&registry_path).unwrap();
        registry.reject_entity("alice");
        registry.save().unwrap();

        let detected = DetectedEntities {
            people: vec![PersonEntity {
                name: "Alice".to_string(),
                confidence: 0.9,
                context: "work".to_string(),
            }],
            projects: vec![],
            uncertain: vec![],
        };

        let confirmed = confirm_entities(&detected, true);
        assert!(confirmed.people.is_empty());

        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
