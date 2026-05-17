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
use serde_json::json;
use std::path::{Path, PathBuf};
use std::{env, fs, io, sync::LazyLock};

use crate::config::Config;
use crate::convo_miner::{mine_conversations, ConvoMiningResult};
use crate::dialect;
use crate::entity_registry::EntityRegistry;
use crate::layers::MemoryStack;
use crate::mine_palace_lock::{self, MineAlreadyRunning};
use crate::miner::{self, MiningResult};
use crate::palace_db::PalaceDb;
use crate::room_detector_local::{detect_rooms_from_folders, RoomMapping};
use crate::searcher;
use crate::split_mega_files::split_file_with_options;
use crate::sweeper::{sweep, sweep_directory};

// ---------------------------------------------------------------------------
// Environment Variables
// ---------------------------------------------------------------------------

#[allow(dead_code)]
static VERBOSE: LazyLock<bool> = LazyLock::new(|| {
    env::var("MEMPAL_VERBOSE")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
});

// ---------------------------------------------------------------------------
// CLI Arguments
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "mpr",
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

        /// DEPRECATED — LLM-assisted entity refinement is now ON by default.
        /// This flag is preserved for backward compatibility;
        /// pass --no-llm to opt out instead.
        #[arg(long, action = clap::ArgAction::SetTrue)]
        llm: bool,

        /// Disable LLM-assisted entity refinement. Run init in heuristics-only mode.
        #[arg(long, action = clap::ArgAction::SetTrue)]
        no_llm: bool,

        /// LLM provider (default: ollama).
        #[arg(long, default_value = "ollama")]
        llm_provider: Option<String>,

        /// Model name for the chosen provider (default: gemma4:e4b for Ollama).
        #[arg(long, default_value = "gemma4:e4b")]
        llm_model: Option<String>,

        /// Provider endpoint URL. Default for Ollama: http://localhost:11434.
        #[arg(long)]
        llm_endpoint: Option<String>,

        /// API key for the provider.
        #[arg(long)]
        llm_api_key: Option<String>,

        /// Bypass interactive consent prompt for external LLM.
        #[arg(long, action = clap::ArgAction::SetTrue)]
        accept_external_llm: bool,

        /// Automatically run mine after initialization completes.
        #[arg(long, action = clap::ArgAction::SetTrue)]
        auto_mine: bool,

        #[arg(long)]
        lang: Option<String>,
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

        /// Your name -- recorded on every drawer (default: mpr)
        #[arg(long, default_value = "mpr")]
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

        #[arg(long, action = clap::ArgAction::SetTrue)]
        redetect_origin: bool,
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

        /// Enable BM25 reranking for better relevance
        #[arg(long)]
        bm25: bool,
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
    #[command(subcommand)]
    Repair(RepairCommands),

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

    /// Run MemPalace MCP server (stdio transport).
    Serve {
        /// Read-only mode (blocks mutations).
        #[arg(long)]
        read_only: bool,
    },

    /// Re-ingest a file or directory of mined drawers into the palace (idempotent).
    Sweep {
        /// File or directory to sweep into the palace.
        target: PathBuf,
        #[arg(long)]
        palace: Option<String>,
    },
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

#[derive(Subcommand)]
enum RepairCommands {
    /// Scan for corrupt/unfetchable drawer IDs
    Scan {
        /// Only scan this wing
        #[arg(long)]
        wing: Option<String>,
    },
    /// Delete corrupt IDs (requires --confirm)
    Prune {
        /// Actually delete (otherwise dry run)
        #[arg(long)]
        confirm: bool,
    },
    /// Rebuild the palace index
    Rebuild,
    /// Clean up stale PID file from interrupted mine operations
    CleanupPid,
}

#[derive(Subcommand, Clone, Debug)]
enum InstructionName {
    Init,
    Search,
    Mine,
    Help,
    Status,
}

const SAVE_INTERVAL: usize = 15;
const STOP_BLOCK_REASON: &str = "AUTO-SAVE checkpoint. Save key topics, decisions, quotes, and code from this session to your memory system. Organize into appropriate categories. Use verbatim quotes where possible. Continue conversation after saving.";
const PRECOMPACT_BLOCK_REASON: &str = "COMPACTION IMMINENT. Save ALL topics, decisions, quotes, code, and important context from this session to your memory system. Be thorough — after compaction, detailed context will be lost. Organize into appropriate categories. Use verbatim quotes where possible. Save everything, then allow compaction to proceed.";
// Instruction markdown is embedded at compile time so the binary works
// regardless of where it's installed. The previous version computed a
// runtime path from `env!("CARGO_MANIFEST_DIR")` which baked the build
// machine's source tree into the released binary (e.g.
// `/home/runner/work/mempalace_rust/mempalace_rust/...`), so a packaged
// binary failed `mpr instructions <topic>` with "file not found".
const INSTRUCTION_INIT: &str = include_str!("../../../instructions/init.md");
const INSTRUCTION_SEARCH: &str = include_str!("../../../instructions/search.md");
const INSTRUCTION_MINE: &str = include_str!("../../../instructions/mine.md");
const INSTRUCTION_HELP: &str = include_str!("../../../instructions/help.md");
const INSTRUCTION_STATUS: &str = include_str!("../../../instructions/status.md");

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

type DetectedEntities = crate::entity_detector::DetectionResult;

/// Scan directory for files that can be used for entity detection.
fn scan_and_detect_entities(dir: &Path) -> DetectedEntities {
    // Load locale patterns from config if languages are configured
    let _config = crate::Config::load().ok();
    let _locale_patterns = if let Some(ref cfg) = _config {
        if !cfg.languages.is_empty() {
            // Use the first language in the list
            let locale_code = &cfg.languages[0];
            crate::entity_detector::load_locale_patterns(locale_code)
        } else {
            None
        }
    } else {
        None
    };

    // For now, we still use the project_scanner which doesn't support locale patterns yet
    // TODO: Integrate locale patterns into project_scanner
    crate::project_scanner::discover_entities(dir, 10)
}

/// Confirm entities with registry integration.
/// Python parity currently keeps registry persistence separate from this CLI-side
/// confirmation helper, so this function simply returns detected entities.
fn confirm_entities(detected: &DetectedEntities, yes: bool) -> DetectedEntities {
    let _ = yes;
    detected.clone()
}

fn save_detected_entities(dir: &Path, detected: &DetectedEntities) -> Result<PathBuf> {
    let entities_path = dir.join("entities.json");
    let content = serde_json::to_string_pretty(detected)?;
    std::fs::write(&entities_path, content)?;
    Ok(entities_path)
}

fn merge_detected_into_registry(detected: &DetectedEntities) -> Result<PathBuf> {
    let registry_path = Config::registry_file_path()?;
    let mut registry = EntityRegistry::load(&registry_path)?;
    registry.merge_detected_entities(&detected.people, &detected.projects)?;
    Ok(registry_path)
}

fn save_project_config(
    project_dir: &Path,
    project_name: &str,
    rooms: &[RoomMapping],
) -> Result<PathBuf> {
    let config_path = project_dir.join("mempalace.json");
    let config = json!({
        "wing": project_name,
        "rooms": rooms,
    });
    fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(config_path)
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn cmd_init(
    dir: &PathBuf,
    yes: bool,
    _use_llm: bool,
    _llm_provider: Option<&str>,
    _llm_model: Option<&str>,
    _llm_endpoint: Option<&str>,
    _llm_api_key: Option<&str>,
    _accept_external_llm: bool,
    _auto_mine: bool,
    _lang: Option<&str>,
) -> Result<()> {
    println!();
    println!("{}", "=".repeat(55));
    println!("  MemPalace Init");
    println!("{}", "=".repeat(55));

    let mut config = Config::load()?;

    // Canonicalize the target directory for comparison
    let target_dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());

    // Idempotency check: if palace already exists at target dir, handle gracefully
    let existing_palace_path =
        std::fs::canonicalize(&config.palace_path).unwrap_or_else(|_| config.palace_path.clone());

    // Check if target is the same as existing palace
    if target_dir == existing_palace_path {
        // Check if it's a valid palace
        let palace_db_path = target_dir.join(format!(
            "{}.json",
            crate::palace_db::DEFAULT_COLLECTION_NAME
        ));
        let is_valid_palace = palace_db_path.exists();

        if is_valid_palace {
            println!();
            println!("  Palace already exists at: {}", target_dir.display());
            println!();

            if yes {
                // In non-interactive mode, skip re-initialization
                println!("  Skipping re-initialization (--yes set).");
                println!("  Use 'mpr status' to check palace status.");
                println!("  Use 'mpr mine' to add new content.");
                println!("{}", "=".repeat(55));
                return Ok(());
            }

            println!("  This palace contains existing data.");
            println!("  Options:");
            println!("    1. Keep existing palace and exit (recommended)");
            println!("    2. Re-scan entities (doesn't affect existing drawers)");
            println!("    3. Force re-initialization (WARNING: may affect configuration)");
            println!();
            println!("  Your choice [1]: ");

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let choice = input.trim();

            match choice {
                "2" => {
                    println!("  Re-scanning entities only...");
                    // Continue with entity detection but skip full init
                    let config_path = config.init()?;

                    if let Some(lang_val) = _lang {
                        let languages: Vec<String> =
                            lang_val.split(',').map(|s| s.trim().to_string()).collect();
                        let mut config = Config::load()?;
                        config.languages = languages;
                        config.save()?;
                    }

                    let _config_dir = config_path.parent().unwrap_or(&config_path);
                    let detected = scan_and_detect_entities(dir);
                    let total =
                        detected.people.len() + detected.projects.len() + detected.uncertain.len();
                    if total > 0 {
                        println!("  Found {} entities", total);
                        let confirmed = confirm_entities(&detected, yes);
                        if !confirmed.people.is_empty() || !confirmed.projects.is_empty() {
                            let entities_path = save_detected_entities(dir, &confirmed)?;
                            println!("  Entities saved: {}", entities_path.display());
                            let registry_path = merge_detected_into_registry(&confirmed)?;
                            println!("  Registry updated: {}", registry_path.display());
                        }
                    }
                    println!("  Entity re-scan complete.");
                    println!("{}", "=".repeat(55));
                    return Ok(());
                }
                "3" => {
                    println!(
                        "  WARNING: Force re-initialization may affect existing configuration."
                    );
                    println!("  Existing drawers will NOT be deleted, but config may change.");
                    println!();
                    if !yes {
                        println!("  Continue? [y/N]: ");
                        let mut confirm = String::new();
                        io::stdin().read_line(&mut confirm)?;
                        if !confirm.trim().to_lowercase().starts_with('y') {
                            println!("  Cancelled.");
                            println!("{}", "=".repeat(55));
                            return Ok(());
                        }
                    }
                    println!("  Proceeding with re-initialization...");
                    // Continue with normal init flow
                }
                _ => {
                    println!("  Keeping existing palace.");
                    println!("  Use 'mpr status' to check palace status.");
                    println!("  Use 'mpr mine' to add new content.");
                    println!("{}", "=".repeat(55));
                    return Ok(());
                }
            }
        }
    }

    // Set the palace path to the target directory
    config.palace_path = target_dir.clone();
    config.save()?;

    let config_path = config.init()?;

    if let Some(lang_val) = _lang {
        let languages: Vec<String> = lang_val.split(',').map(|s| s.trim().to_string()).collect();
        config.languages = languages;
        config.save()?;
    }

    let config_dir = config_path.parent().unwrap_or(&config_path);

    if !yes {
        let _registry = crate::onboarding::run_onboarding(dir, config_dir, true)?;

        // Detect rooms from folder structure so `mpr mine` has something to read.
        let rooms = detect_rooms_from_folders(dir);
        let project_name = target_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase().replace([' ', '-'], "_"))
            .unwrap_or_else(|| "project".to_string());
        let project_config_path = save_project_config(dir, &project_name, &rooms)?;

        println!("  Project config saved: {:?}", project_config_path);
        println!("  Global config saved: {:?}", config_path);
        println!();
        println!("  Next step:");
        println!("    mpr mine {:?}", dir);
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
        if !confirmed.people.is_empty() || !confirmed.projects.is_empty() {
            let entities_path = save_detected_entities(dir, &confirmed)?;
            println!("  Entities saved: {}", entities_path.display());
            let registry_path = merge_detected_into_registry(&confirmed)?;
            println!("  Registry updated: {}", registry_path.display());
        }
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

    // Pass 2.5: estimate mining scope
    println!();
    println!("  Estimating mining scope...");
    let scope_estimate = estimate_mining_scope(dir)?;
    println!(
        "  ~{} files (~{} MB) would be mined into this palace.",
        scope_estimate.file_count, scope_estimate.size_mb
    );

    let project_name = target_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase().replace([' ', '-'], "_"))
        .unwrap_or_else(|| "project".to_string());
    let project_config_path = save_project_config(dir, &project_name, &rooms)?;

    // Pass 3: initialize config
    println!();
    println!("  Project config saved: {:?}", project_config_path);
    println!("  Global config saved: {:?}", config_path);

    if !yes && !_auto_mine {
        println!();
        println!("  Mine this directory now? [Y/n]");
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();
        if input == "n" || input == "no" {
            println!("  Skipped. Run manually: mpr mine {:?}", dir);
            println!();
            println!("{}", "=".repeat(55));
            return Ok(());
        }
    }

    println!();
    println!("  Next step:");
    println!("    mpr mine {:?}", dir);
    println!();
    println!("{}", "=".repeat(55));

    if _auto_mine {
        println!();
        println!("  Running mine automatically (--auto-mine set)...");
        if let Err(err) = cmd_mine(
            dir,
            &MiningMode::Auto,
            None,
            "mpr",
            0,
            false,
            false,
            &[],
            None,
            None,
            false,
        ) {
            eprintln!("Warning: auto-mine failed: {}", err);
        }
    }

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
    redetect_origin: bool,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    // Acquire PID guard to prevent concurrent mine operations
    let mut pid_guard = crate::mine_pid_guard::MinePidGuard::new(&palace_path);
    if let Err(e) = pid_guard.acquire() {
        match e {
            crate::mine_pid_guard::PidGuardError::AlreadyRunning { pid, timestamp } => {
                eprintln!("  Error: Mine operation already in progress");
                eprintln!("  PID: {}", pid);
                eprintln!("  Started at: {}", timestamp);
                eprintln!("  If you believe this is stale, run: mpr repair --cleanup-pid");
                return Err(anyhow::anyhow!("Mine operation already in progress"));
            }
            _ => return Err(e.into()),
        }
    }

    // Check for shutdown request
    crate::signal_handler::check_shutdown()?;

    if redetect_origin {
        let origin_result = crate::corpus_origin::resolve_corpus_origin(&palace_path, None);
        let origin_path = palace_path.join(".mempalace").join("origin.json");
        if let Some(parent) = origin_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&origin_result)?;
        std::fs::write(&origin_path, json)?;
        println!(
            "Re-detected corpus origin: likely_ai_dialogue={}, confidence={:.2}",
            origin_result.likely_ai_dialogue, origin_result.confidence
        );
    }

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
    bm25: bool,
    palace_arg: Option<&str>,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let response = runtime().block_on(searcher::search_memories_with_rerank(
        query,
        &palace_path,
        wing,
        room,
        results,
        None,
        bm25,
    ))?;
    searcher::print_search_response(&response);
    Ok(())
}

fn cmd_hook(hook: &str, harness: &str) -> Result<()> {
    run_hook(hook, harness)
}

fn cmd_instructions(name: &InstructionName) -> Result<()> {
    let label = match name {
        InstructionName::Init => "init",
        InstructionName::Search => "search",
        InstructionName::Mine => "mine",
        InstructionName::Help => "help",
        InstructionName::Status => "status",
    };
    run_instructions(label)?;
    Ok(())
}

fn hook_state_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mempalace")
        .join("hook_state")
}

fn precompact_state_file(session_id: &str) -> PathBuf {
    hook_state_dir().join(format!("{session_id}_precompact_blocked_at"))
}

fn sanitize_session_id(session_id: &str) -> String {
    let sanitized: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn count_human_messages(transcript_path: &str) -> usize {
    let path = PathBuf::from(transcript_path);
    let Ok(content) = fs::read_to_string(path) else {
        return 0;
    };

    content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| {
            let message = entry.get("message");
            if let Some(message) = message {
                if message.get("role").and_then(|v| v.as_str()) == Some("user") {
                    match message.get("content") {
                        Some(serde_json::Value::String(content)) => {
                            !content.contains("<command-message>")
                        }
                        Some(serde_json::Value::Array(blocks)) => {
                            let joined = blocks
                                .iter()
                                .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join(" ");
                            !joined.contains("<command-message>")
                        }
                        _ => true,
                    }
                } else {
                    false
                }
            } else {
                entry.get("type").and_then(|v| v.as_str()) == Some("event_msg")
                    && entry
                        .get("payload")
                        .and_then(|v| v.get("type"))
                        .and_then(|v| v.as_str())
                        == Some("user_message")
                    && !entry
                        .get("payload")
                        .and_then(|v| v.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .contains("<command-message>")
            }
        })
        .count()
}

fn log_hook(message: &str) {
    let state_dir = hook_state_dir();
    if fs::create_dir_all(&state_dir).is_err() {
        return;
    }
    let log_path = state_dir.join("hook.log");
    let timestamp = chrono::Local::now().format("%H:%M:%S");
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .and_then(|mut f| {
            std::io::Write::write_all(&mut f, format!("[{timestamp}] {message}\n").as_bytes())
        });
}

fn maybe_auto_ingest(async_mode: bool) {
    let Some(mempal_dir) = std::env::var_os("MEMPAL_DIR") else {
        return;
    };
    let mempal_dir = PathBuf::from(mempal_dir);
    if !mempal_dir.is_dir() {
        return;
    }
    let log_path = hook_state_dir().join("hook.log");
    let _ = fs::create_dir_all(hook_state_dir());
    let stdout = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    let stderr = stdout.as_ref().and_then(|_| {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok()
    });

    let mut cmd = std::process::Command::new(
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mpr")),
    );
    cmd.arg("mine").arg(&mempal_dir);
    if let Some(out) = stdout {
        cmd.stdout(out);
    }
    if let Some(err) = stderr {
        cmd.stderr(err);
    }

    if async_mode {
        let _ = cmd.spawn();
    } else {
        let _ = cmd.status();
    }
}

fn parse_harness_input(
    data: &serde_json::Value,
    harness: &str,
) -> Result<(String, bool, String, String)> {
    if !matches!(harness, "claude-code" | "codex") {
        anyhow::bail!("Unknown harness: {harness}");
    }
    Ok((
        sanitize_session_id(
            data.get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
        ),
        data.get("stop_hook_active")
            .and_then(|v| v.as_bool())
            .or_else(|| {
                data.get("stop_hook_active")
                    .and_then(|v| v.as_str())
                    .map(|v| matches!(v, "true" | "1" | "yes"))
            })
            .unwrap_or(false),
        data.get("transcript_path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        data.get("trigger")
            .and_then(|v| v.as_str())
            .unwrap_or("auto")
            .to_ascii_lowercase(),
    ))
}

fn emit_json(value: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn hook_session_start_response(
    data: &serde_json::Value,
    harness: &str,
) -> Result<serde_json::Value> {
    let (session_id, _, _, _) = parse_harness_input(data, harness)?;
    log_hook(&format!("SESSION START for session {session_id}"));
    fs::create_dir_all(hook_state_dir())?;
    Ok(json!({}))
}

fn hook_stop_response(data: &serde_json::Value, harness: &str) -> Result<serde_json::Value> {
    let (session_id, stop_hook_active, transcript_path, _) = parse_harness_input(data, harness)?;
    if stop_hook_active {
        return Ok(json!({}));
    }

    let exchange_count = count_human_messages(&transcript_path);
    let state_dir = hook_state_dir();
    let _ = fs::create_dir_all(&state_dir);
    let last_save_file = state_dir.join(format!("{session_id}_last_save"));
    let last_save = fs::read_to_string(&last_save_file)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let since_last = exchange_count.saturating_sub(last_save);
    log_hook(&format!(
        "Session {session_id}: {exchange_count} exchanges, {since_last} since last save"
    ));

    if since_last >= SAVE_INTERVAL && exchange_count > 0 {
        let _ = fs::write(&last_save_file, exchange_count.to_string());
        log_hook(&format!("TRIGGERING SAVE at exchange {exchange_count}"));
        maybe_auto_ingest(true);
        Ok(json!({"decision":"block","reason":STOP_BLOCK_REASON}))
    } else {
        Ok(json!({}))
    }
}

fn hook_precompact_response(data: &serde_json::Value, harness: &str) -> Result<serde_json::Value> {
    let (session_id, _, transcript_path, trigger) = parse_harness_input(data, harness)?;
    log_hook(&format!(
        "PRE-COMPACT triggered for session {session_id} (trigger={trigger})"
    ));

    if trigger == "manual" {
        log_hook("PRE-COMPACT manual trigger -- allowing compaction");
        let _ = fs::remove_file(precompact_state_file(&session_id));
        return Ok(json!({}));
    }

    let exchange_count = if transcript_path.is_empty() {
        0
    } else {
        count_human_messages(&transcript_path)
    };
    let state_file = precompact_state_file(&session_id);
    let _ = fs::create_dir_all(hook_state_dir());
    let last_blocked_at = fs::read_to_string(&state_file)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok());

    if let Some(last_blocked_at) = last_blocked_at {
        if exchange_count <= last_blocked_at {
            log_hook(&format!(
                "PRE-COMPACT already blocked at exchange {last_blocked_at} (now {exchange_count}) -- allowing compaction to prevent deadlock"
            ));
            let _ = fs::remove_file(&state_file);
            return Ok(json!({}));
        }
    }

    let _ = fs::write(&state_file, exchange_count.to_string());
    maybe_auto_ingest(false);
    Ok(json!({"decision":"block","reason":PRECOMPACT_BLOCK_REASON}))
}

fn run_hook(hook_name: &str, harness: &str) -> Result<()> {
    let data: serde_json::Value =
        serde_json::from_reader(io::stdin()).unwrap_or_else(|_| json!({}));
    let response = match hook_name {
        "session-start" => hook_session_start_response(&data, harness)?,
        "stop" => hook_stop_response(&data, harness)?,
        "precompact" => hook_precompact_response(&data, harness)?,
        _ => anyhow::bail!("Unknown hook: {hook_name}"),
    };
    emit_json(response)
}

fn run_instructions(name: &str) -> Result<()> {
    let content = match name {
        "init" => INSTRUCTION_INIT,
        "search" => INSTRUCTION_SEARCH,
        "mine" => INSTRUCTION_MINE,
        "help" => INSTRUCTION_HELP,
        "status" => INSTRUCTION_STATUS,
        _ => anyhow::bail!(
            "Unknown instructions: {name}. Available: init, search, mine, help, status"
        ),
    };
    print!("{content}");
    Ok(())
}

fn cmd_repair(cmd: &RepairCommands, palace_arg: Option<&str>) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    match cmd {
        RepairCommands::Scan { wing } => {
            crate::repair::scan_palace(Some(palace_path.as_path()), wing.as_deref())?;
        }
        RepairCommands::Prune { confirm } => {
            crate::repair::prune_corrupt(Some(palace_path.as_path()), *confirm)?;
        }
        RepairCommands::Rebuild => {
            crate::repair::rebuild_index(Some(palace_path.as_path()))?;
        }
        RepairCommands::CleanupPid => {
            crate::repair::cleanup_pid(Some(palace_path.as_path()))?;
        }
    }

    Ok(())
}

fn cmd_mcp(palace_arg: Option<&str>) {
    let base_server_cmd = "mpr serve";
    if let Some(palace) = palace_arg {
        let resolved_palace = if let Some(stripped) = palace.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(stripped)
        } else {
            PathBuf::from(palace)
        };
        let custom_server_cmd = format!("mpr --palace {} serve", resolved_palace.display());
        println!("MemPalace MCP quick setup:");
        println!("  claude mcp add mpr -- {}", custom_server_cmd);
        println!("\nRun the server directly:");
        println!("  {}", custom_server_cmd);
    } else {
        println!("MemPalace MCP quick setup:");
        println!("  claude mcp add mpr -- {}", base_server_cmd);
        println!("\nRun the server directly:");
        println!("  {}", base_server_cmd);
        println!("\nOptional custom palace:");
        println!("  claude mcp add mpr -- mpr --palace /path/to/palace serve");
        println!("  mpr --palace /path/to/palace serve");
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

    let mut people_map = std::collections::HashMap::new();
    if let Some(ref cp) = config_path {
        if cp.exists() {
            if let Ok(content) = std::fs::read_to_string(cp) {
                if serde_json::from_str::<serde_json::Value>(&content).is_ok() {
                    println!("  Loaded entity config: {:?}", cp);
                }
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(entities) = parsed.get("entities").and_then(|v| v.as_object()) {
                        for (name, code) in entities {
                            if let Some(code) = code.as_str() {
                                people_map.insert(name.clone(), code.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Connect to palace
    let Ok(palace_db) = PalaceDb::open(&palace_path) else {
        println!("\n  No palace found at {:?}", palace_path);
        println!("  Run: mpr init <dir> then mpr mine <dir>");
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

    let mut total_original_tokens = 0;
    let mut total_compressed_tokens = 0;
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

        let compressed = dialect::compress_with_metadata(doc, &people_map, Some(meta));
        let stats = dialect::compression_stats(doc, &compressed);
        total_original_tokens += stats.original_tokens_est;
        total_compressed_tokens += stats.summary_tokens_est;

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
                stats.original_tokens_est, stats.summary_tokens_est, stats.size_ratio
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
        let mut compressed_db = PalaceDb::open_collection(
            &palace_path,
            crate::palace_db::DEFAULT_COMPRESSED_COLLECTION_NAME,
        )?;
        let upserts = compressed_entries
            .iter()
            .map(|(id, compressed, meta, stats)| {
                let mut comp_meta = meta.clone();
                comp_meta.insert(
                    "compression_ratio".to_string(),
                    serde_json::json!(stats.size_ratio),
                );
                comp_meta.insert(
                    "original_tokens".to_string(),
                    serde_json::json!(stats.original_tokens_est),
                );
                (id.clone(), compressed.clone(), comp_meta)
            })
            .collect::<Vec<_>>();
        compressed_db.upsert_documents(&upserts)?;
        compressed_db.flush()?;
        println!(
            "  Stored {} compressed drawers in 'mpr_compressed' collection.",
            compressed_entries.len()
        );
    }

    // Summary
    let ratio = total_original_tokens as f64 / total_compressed_tokens.max(1) as f64;
    println!(
        "  Total: {}t -> {}t ({:.1}x compression)",
        total_original_tokens, total_compressed_tokens, ratio
    );
    if dry_run {
        println!("  (dry run -- nothing stored)");
    }

    Ok(())
}

fn cmd_sweep(target: &Path, palace_arg: Option<&str>) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    let stats = if target.is_dir() {
        sweep_directory(target, Some(&palace_path))?
    } else {
        sweep(target, Some(&palace_path))?
    };

    println!("  Sweep complete");
    println!("    drawers added: {}", stats.drawers_added);
    println!("    already present: {}", stats.drawers_already_present);
    println!("    skipped: {}", stats.drawers_skipped);

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
            println!("  Run: mpr init <dir> then mpr mine <dir>");
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
    println!("    Files skipped: {}", result.files_skipped);
    println!("    Conversations mined: {}", result.conversations_mined);
    println!("    Chunks created: {}", result.chunks_created);
    if !result.room_counts.is_empty() {
        println!("    By room:");
        for (room, count) in &result.room_counts {
            println!("      {}: {}", room, count);
        }
    }
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
    // Setup signal handler for graceful shutdown
    let _signal_guard = crate::signal_handler::setup_signal_handler();

    let cli = Cli::parse();
    let palace_arg = cli.palace.as_deref();

    match &cli.command {
        Commands::Init {
            dir,
            yes,
            llm: _,
            no_llm,
            llm_provider,
            llm_model,
            llm_endpoint,
            llm_api_key,
            accept_external_llm,
            auto_mine,
            lang,
        } => {
            let use_llm = !no_llm;
            cmd_init(
                dir,
                *yes,
                use_llm,
                llm_provider.as_deref(),
                llm_model.as_deref(),
                llm_endpoint.as_deref(),
                llm_api_key.as_deref(),
                *accept_external_llm,
                *auto_mine,
                lang.as_deref(),
            )?
        }
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
            redetect_origin,
        } => {
            let palace_path = resolve_palace_path(palace_arg)?;
            if let Err(MineAlreadyRunning { pid }) =
                mine_palace_lock::mine_palace_lock(&palace_path)
            {
                eprintln!(
                    "  Error: another mpr mine process (PID {}) already running for this palace",
                    pid
                );
                std::process::exit(1);
            }
            cmd_mine(
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
                *redetect_origin,
            )?
        }
        Commands::Search {
            query,
            wing,
            room,
            results,
            bm25,
        } => cmd_search(
            query,
            wing.as_deref(),
            room.as_deref(),
            *results,
            *bm25,
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
        Commands::Repair(ref cmd) => cmd_repair(cmd, palace_arg)?,
        Commands::Status => cmd_status(palace_arg)?,
        Commands::MineDevice { wing, dry_run } => {
            cmd_mine_device(wing.as_deref(), *dry_run, palace_arg)?
        }
        Commands::Mcp => cmd_mcp(palace_arg),
        Commands::Serve { read_only } => {
            crate::mcp_server::run_server(palace_arg, *read_only)?;
        }
        Commands::Sweep { target, palace } => cmd_sweep(target, palace.as_deref())?,
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper Functions
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct MiningScopeEstimate {
    file_count: usize,
    size_mb: f64,
}

fn estimate_mining_scope(dir: &PathBuf) -> Result<MiningScopeEstimate> {
    use walkdir::WalkDir;

    let mut file_count = 0;
    let mut total_bytes = 0u64;

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
        "coverage",
        ".terraform",
        "vendor",
        "target",
        ".mempalace",
        ".cache",
        ".pytest_cache",
        ".ruff_cache",
    ];

    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();

        // Skip directories
        if path.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if SKIP_DIRS.contains(&dir_name) {
                continue;
            }
        }

        // Only count readable files
        if path.is_file() {
            // Skip common non-content files
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                if matches!(
                    ext_str.as_str(),
                    "lock"
                        | "pyc"
                        | "pyo"
                        | "dll"
                        | "so"
                        | "dylib"
                        | "exe"
                        | "bin"
                        | "class"
                        | "jar"
                        | "war"
                        | "zip"
                        | "tar"
                        | "gz"
                        | "rar"
                        | "7z"
                ) {
                    continue;
                }
            }

            if let Ok(metadata) = path.metadata() {
                file_count += 1;
                total_bytes += metadata.len();
            }
        }
    }

    let size_mb = total_bytes as f64 / (1024.0 * 1024.0);

    Ok(MiningScopeEstimate {
        file_count,
        size_mb,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        cmd_compress, cmd_init, cmd_mine, confirm_entities, count_human_messages,
        detect_mining_mode, hook_precompact_response, hook_session_start_response,
        hook_stop_response, merge_detected_into_registry, parse_harness_input, run_instructions,
        save_detected_entities, scan_and_detect_entities, Cli, Commands, DetectedEntities,
        HookAction, InstructionName, MiningMode, INSTRUCTION_HELP, INSTRUCTION_INIT,
        INSTRUCTION_MINE, INSTRUCTION_SEARCH, INSTRUCTION_STATUS, PRECOMPACT_BLOCK_REASON,
        SAVE_INTERVAL, STOP_BLOCK_REASON,
    };
    use crate::config::Config;
    use crate::entity_detector::{PersonEntity, ProjectEntity};
    use crate::entity_registry::EntityRegistry;
    use crate::palace_db::PalaceDb;
    use crate::test_env_lock;
    use clap::Parser;
    use serde_json::json;
    use std::path::PathBuf;

    fn expect_init(args: Cli) -> (PathBuf, bool) {
        assert!(matches!(args.command, Commands::Init { .. }));
        if let Commands::Init { dir, yes, .. } = args.command {
            (dir, yes)
        } else {
            (PathBuf::new(), false)
        }
    }

    fn expect_mine(args: Cli) -> Commands {
        assert!(matches!(args.command, Commands::Mine { .. }));
        match args.command {
            command @ Commands::Mine { .. } => command,
            _ => Commands::Status,
        }
    }

    fn expect_search(args: Cli) -> (String, Option<String>, Option<String>, usize) {
        assert!(matches!(args.command, Commands::Search { .. }));
        if let Commands::Search {
            query,
            wing,
            room,
            results,
            bm25: _,
        } = args.command
        {
            (query, wing, room, results)
        } else {
            (String::new(), None, None, 0)
        }
    }

    fn expect_wakeup(args: Cli) -> Option<String> {
        assert!(matches!(args.command, Commands::WakeUp { .. }));
        if let Commands::WakeUp { wing } = args.command {
            wing
        } else {
            None
        }
    }

    fn expect_compress(args: Cli) -> (Option<String>, bool, Option<String>) {
        assert!(matches!(args.command, Commands::Compress { .. }));
        if let Commands::Compress {
            wing,
            dry_run,
            config,
        } = args.command
        {
            (wing, dry_run, config)
        } else {
            (None, false, None)
        }
    }

    fn expect_split(args: Cli) -> (PathBuf, Option<PathBuf>, bool, usize) {
        assert!(matches!(args.command, Commands::Split { .. }));
        if let Commands::Split {
            dir,
            output_dir,
            dry_run,
            min_sessions,
        } = args.command
        {
            (dir, output_dir, dry_run, min_sessions)
        } else {
            (PathBuf::new(), None, false, 0)
        }
    }

    fn expect_instructions(args: Cli) -> InstructionName {
        assert!(matches!(args.command, Commands::Instructions { .. }));
        if let Commands::Instructions { name } = args.command {
            name
        } else {
            InstructionName::Help
        }
    }

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
        let (dir, yes) = expect_init(args);
        assert_eq!(dir, PathBuf::from("/tmp/test"));
        assert!(yes);
    }

    #[test]
    fn test_cmd_init_writes_project_config_for_mining() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should not be poisoned");
        let temp_dir = tempfile::tempdir().unwrap();
        let project_dir = temp_dir.path().join("Sample Project");
        let src_dir = project_dir.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::create_dir_all(temp_dir.path().join("xdg")).unwrap();

        std::env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("xdg"));
        cmd_init(
            &project_dir,
            true,
            false,
            None,
            None,
            None,
            None,
            false,
            false,
            None,
        )
        .unwrap();
        std::env::remove_var("XDG_CONFIG_HOME");

        let config_path = project_dir.join("mempalace.json");
        assert!(config_path.exists());
        let (wing, rooms) = crate::miner::load_config(&project_dir).unwrap();
        assert_eq!(wing, "sample_project");
        assert!(rooms.iter().any(|room| room.name == "src"));
    }

    #[test]
    fn test_cmd_init_interactive_writes_project_config() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should not be poisoned");
        let temp_dir = tempfile::tempdir().unwrap();
        let project_dir = temp_dir.path().join("Interactive Project");
        let src_dir = project_dir.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::create_dir_all(temp_dir.path().join("xdg")).unwrap();

        std::env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("xdg"));
        std::env::set_var("MEMPALACE_NONINTERACTIVE", "1");
        let result = cmd_init(
            &project_dir,
            false,
            false,
            None,
            None,
            None,
            None,
            false,
            false,
            None,
        );
        std::env::remove_var("MEMPALACE_NONINTERACTIVE");
        std::env::remove_var("XDG_CONFIG_HOME");
        result.unwrap();

        let config_path = project_dir.join("mempalace.json");
        assert!(
            config_path.exists(),
            "interactive init should still write mempalace.json so `mpr mine` works"
        );
        let (wing, rooms) = crate::miner::load_config(&project_dir).unwrap();
        assert_eq!(wing, "interactive_project");
        assert!(rooms.iter().any(|room| room.name == "src"));
    }

    #[test]
    fn test_cli_args_parse_mine() {
        let args = Cli::try_parse_from([
            "mpr",
            "mine",
            "/tmp/test",
            "--mode",
            "convos",
            "--wing",
            "test_wing",
            "--dry-run",
        ])
        .unwrap();
        match expect_mine(args) {
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
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_cli_args_parse_mine_gitignore_flags() {
        let args = Cli::try_parse_from([
            "mpr",
            "mine",
            "/tmp/test",
            "--no-gitignore",
            "--include-ignored",
            "a.txt,b.txt",
            "--include-ignored",
            "c.txt",
        ])
        .unwrap();
        match expect_mine(args) {
            Commands::Mine {
                no_gitignore,
                include_ignored,
                ..
            } => {
                assert!(no_gitignore);
                assert_eq!(include_ignored, vec!["a.txt,b.txt", "c.txt"]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_cli_args_parse_search() {
        let args = Cli::try_parse_from([
            "mpr",
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
        let (query, wing, room, results) = expect_search(args);
        assert_eq!(query, "rust async");
        assert_eq!(wing, Some("tech".to_string()));
        assert_eq!(room, Some("backend".to_string()));
        assert_eq!(results, 10);
    }

    #[test]
    fn test_cli_args_parse_wakeup() {
        let args = Cli::try_parse_from(["mempalace", "wake-up", "--wing", "myapp"]).unwrap();
        let wing = expect_wakeup(args);
        assert_eq!(wing, Some("myapp".to_string()));
    }

    #[test]
    fn test_cli_args_parse_compress() {
        let args = Cli::try_parse_from([
            "mpr",
            "compress",
            "--wing",
            "myapp",
            "--dry-run",
            "--config",
            "entities.json",
        ])
        .unwrap();
        let (wing, dry_run, config) = expect_compress(args);
        assert_eq!(wing, Some("myapp".to_string()));
        assert!(dry_run);
        assert_eq!(config, Some("entities.json".to_string()));
    }

    #[test]
    fn test_cli_args_parse_split() {
        let args = Cli::try_parse_from([
            "mpr",
            "split",
            "/tmp/chats",
            "--output-dir",
            "/tmp/split",
            "--dry-run",
            "--min-sessions",
            "3",
        ])
        .unwrap();
        let (dir, output_dir, dry_run, min_sessions) = expect_split(args);
        assert_eq!(dir, PathBuf::from("/tmp/chats"));
        assert_eq!(output_dir, Some(PathBuf::from("/tmp/split")));
        assert!(dry_run);
        assert_eq!(min_sessions, 3);
    }

    #[test]
    fn test_cli_args_parse_status() {
        let args = Cli::try_parse_from(["mempalace", "status"]).unwrap();
        assert!(matches!(args.command, Commands::Status));
    }

    #[test]
    fn test_cli_args_parse_repair() {
        let args = Cli::try_parse_from(["mpr", "repair", "scan"]).unwrap();
        assert!(matches!(args.command, Commands::Repair(_)));
    }

    #[test]
    fn test_cli_args_parse_hook_run() {
        let args = Cli::try_parse_from([
            "mpr",
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
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_cli_args_hook_requires_subcommand() {
        let err = Cli::try_parse_from(["mempalace", "hook"]).err().unwrap();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn test_cli_args_parse_instructions() {
        let args = Cli::try_parse_from(["mempalace", "instructions", "help"]).unwrap();
        let name = expect_instructions(args);
        assert!(matches!(name, InstructionName::Help));
    }

    #[test]
    fn test_cli_args_instructions_requires_subcommand() {
        let err = Cli::try_parse_from(["mempalace", "instructions"])
            .err()
            .unwrap();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn test_run_instructions_known_name_succeeds() {
        run_instructions("help").expect("known instructions name should succeed");
    }

    #[test]
    fn test_run_instructions_invalid_name_errors() {
        let err = run_instructions("nonexistent")
            .expect_err("invalid instructions name should error")
            .to_string();
        assert!(err.contains("Unknown instructions: nonexistent"));
        assert!(err.contains("Available:"));
    }

    #[test]
    fn test_instruction_content_is_embedded_not_runtime_path() {
        // Regression: the released binary previously embedded the build
        // machine's `CARGO_MANIFEST_DIR` as a runtime path, so packaged
        // binaries failed with "Instructions file not found:
        // /home/runner/work/mempalace_rust/...". After the fix all five
        // instruction bodies are baked into the binary via include_str!.
        for name in ["init", "search", "mine", "help", "status"] {
            assert!(
                !INSTRUCTION_INIT.is_empty()
                    && !INSTRUCTION_SEARCH.is_empty()
                    && !INSTRUCTION_MINE.is_empty()
                    && !INSTRUCTION_HELP.is_empty()
                    && !INSTRUCTION_STATUS.is_empty(),
                "all embedded instructions must have content"
            );
            run_instructions(name).unwrap_or_else(|e| {
                panic!("instructions for `{name}` should succeed but errored: {e}")
            });
        }
    }

    #[test]
    fn test_run_hook_session_start_outputs_empty_json() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        std::env::set_var("HOME", temp_dir.path());
        let response =
            hook_session_start_response(&json!({"session_id": "run-test"}), "claude-code")
                .expect("session-start hook should succeed");
        assert_eq!(response, json!({}));
        std::env::remove_var("HOME");
    }

    #[test]
    fn test_run_hook_stop_blocks_at_interval() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        std::env::set_var("HOME", temp_dir.path());
        let transcript = temp_dir.path().join("t.jsonl");
        let lines = (0..SAVE_INTERVAL)
            .map(|i| format!(r#"{{"message":{{"role":"user","content":"msg {i}"}}}}"#))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&transcript, lines).expect("should write transcript");
        let response = hook_stop_response(
            &json!({
                "session_id": "test",
                "stop_hook_active": false,
                "transcript_path": transcript.display().to_string(),
            }),
            "claude-code",
        )
        .expect("stop hook should succeed");
        assert_eq!(response["decision"], "block");
        assert_eq!(response["reason"], STOP_BLOCK_REASON);
        std::env::remove_var("HOME");
    }

    #[test]
    fn test_run_hook_stop_passthrough_when_active_string() {
        let response = hook_stop_response(
            &json!({
                "session_id": "test",
                "stop_hook_active": "true",
                "transcript_path": "/nonexistent.jsonl",
            }),
            "claude-code",
        )
        .expect("stop hook should passthrough when active string is true");
        assert_eq!(response, json!({}));
    }

    #[test]
    fn test_parse_harness_input_reads_trigger() {
        let (session_id, stop_hook_active, transcript_path, trigger) = parse_harness_input(
            &json!({
                "session_id": "run-test",
                "stop_hook_active": false,
                "transcript_path": "/tmp/session.jsonl",
                "trigger": "manual"
            }),
            "claude-code",
        )
        .expect("parse_harness_input should succeed");

        assert_eq!(session_id, "run-test");
        assert!(!stop_hook_active);
        assert_eq!(transcript_path, "/tmp/session.jsonl");
        assert_eq!(trigger, "manual");
    }

    #[test]
    fn test_count_human_messages_supports_codex_format() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        let transcript = temp_dir.path().join("codex.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"hello"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"<command-message>status</command-message>"}}"#,
                "\n"
            ),
        )
        .expect("should write codex transcript");

        assert_eq!(
            count_human_messages(transcript.to_str().expect("path should be utf-8")),
            1
        );
    }

    #[test]
    fn test_run_hook_precompact_blocks() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        std::env::set_var("HOME", temp_dir.path());
        let transcript = temp_dir.path().join("precompact.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                r#"{"message":{"role":"user","content":"first"}}"#,
                "\n",
                r#"{"message":{"role":"assistant","content":"ok"}}"#,
                "\n"
            ),
        )
        .expect("should write transcript");
        let response = hook_precompact_response(
            &json!({
                "session_id": "run-test",
                "transcript_path": transcript.display().to_string(),
                "trigger": "auto"
            }),
            "claude-code",
        )
        .expect("precompact hook should succeed");
        assert_eq!(response["decision"], "block");
        assert_eq!(response["reason"], PRECOMPACT_BLOCK_REASON);
        std::env::remove_var("HOME");
    }

    #[test]
    fn test_run_hook_precompact_manual_trigger_passes_through() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        std::env::set_var("HOME", temp_dir.path());
        let response = hook_precompact_response(
            &json!({"session_id": "run-test", "trigger": "manual"}),
            "claude-code",
        )
        .expect("manual precompact should succeed");
        assert_eq!(response, json!({}));
        std::env::remove_var("HOME");
    }

    #[test]
    fn test_run_hook_precompact_deadlock_guard_allows_refire() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        std::env::set_var("HOME", temp_dir.path());
        let transcript = temp_dir.path().join("precompact.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                r#"{"message":{"role":"user","content":"first"}}"#,
                "\n",
                r#"{"message":{"role":"assistant","content":"ok"}}"#,
                "\n"
            ),
        )
        .expect("should write transcript");
        let payload = json!({
            "session_id": "run-test",
            "transcript_path": transcript.display().to_string(),
            "trigger": "auto"
        });

        let first = hook_precompact_response(&payload, "claude-code")
            .expect("first precompact should succeed");
        assert_eq!(first["decision"], "block");

        let second = hook_precompact_response(&payload, "claude-code")
            .expect("second precompact should succeed");
        assert_eq!(second, json!({}));
        std::env::remove_var("HOME");
    }

    #[test]
    fn test_run_hook_precompact_new_human_message_rearms_block() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        std::env::set_var("HOME", temp_dir.path());
        let transcript = temp_dir.path().join("precompact.jsonl");
        std::fs::write(
            &transcript,
            r#"{"message":{"role":"user","content":"first"}}"#,
        )
        .expect("should write initial transcript");
        let payload = json!({
            "session_id": "run-test",
            "transcript_path": transcript.display().to_string(),
            "trigger": "auto"
        });

        let first = hook_precompact_response(&payload, "claude-code")
            .expect("first precompact should succeed");
        assert_eq!(first["decision"], "block");

        let second = hook_precompact_response(&payload, "claude-code")
            .expect("second precompact should succeed");
        assert_eq!(second, json!({}));

        std::fs::write(
            &transcript,
            concat!(
                r#"{"message":{"role":"user","content":"first"}}"#,
                "\n",
                r#"{"message":{"role":"assistant","content":"ok"}}"#,
                "\n",
                r#"{"message":{"role":"user","content":"second"}}"#,
                "\n"
            ),
        )
        .expect("should write updated transcript");

        let third = hook_precompact_response(&payload, "claude-code")
            .expect("third precompact should succeed");
        assert_eq!(third["decision"], "block");
        std::env::remove_var("HOME");
    }

    #[test]
    fn test_cli_args_parse_mcp() {
        let args = Cli::try_parse_from(["mempalace", "mcp"]).expect("mcp args should parse");
        assert!(matches!(args.command, Commands::Mcp));
    }

    #[test]
    fn test_cli_args_parse_compress_defaults() {
        let args =
            Cli::try_parse_from(["mempalace", "compress"]).expect("compress defaults should parse");
        let (wing, dry_run, config) = expect_compress(args);
        assert_eq!(wing, None);
        assert!(!dry_run);
        assert_eq!(config, None);
    }

    #[test]
    fn test_cli_args_with_palace_override() {
        let args = Cli::try_parse_from(["mempalace", "--palace", "/custom/palace", "status"])
            .expect("palace override should parse");
        assert_eq!(args.palace, Some("/custom/palace".to_string()));
    }

    #[test]
    fn test_scan_and_detect_entities_empty_dir() {
        let temp = tempfile::TempDir::new().expect("tempdir should be created");
        let result = scan_and_detect_entities(temp.path());
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
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        let xdg_root = temp_dir
            .path()
            .to_str()
            .expect("tempdir path should be utf-8");
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let registry_path = Config::registry_file_path().expect("registry path should resolve");
        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent).expect("registry parent should be created");
        }

        let mut registry =
            EntityRegistry::load(&registry_path).expect("registry should load from config path");
        registry.reject_entity("alice");
        registry.save().expect("registry should save");

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
        assert_eq!(confirmed.people.len(), 1);
        assert_eq!(confirmed.people[0].name, "Alice");

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_save_detected_entities_writes_entities_json() {
        let temp = tempfile::TempDir::new().expect("tempdir should be created");
        let detected = DetectedEntities {
            people: vec![PersonEntity {
                name: "Alice".to_string(),
                confidence: 0.9,
                context: "git author".to_string(),
            }],
            projects: vec![ProjectEntity {
                name: "signal-app".to_string(),
                confidence: 0.95,
                context: "Cargo.toml".to_string(),
            }],
            uncertain: vec![],
        };

        let path = save_detected_entities(temp.path(), &detected).expect("entities should save");
        let content = std::fs::read_to_string(path).expect("entities file should be readable");
        assert!(content.contains("\"Alice\""));
        assert!(content.contains("\"signal-app\""));
    }

    #[test]
    fn test_merge_detected_into_registry_uses_config_path() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

        let detected = DetectedEntities {
            people: vec![PersonEntity {
                name: "Alice".to_string(),
                confidence: 0.9,
                context: "git author".to_string(),
            }],
            projects: vec![ProjectEntity {
                name: "signal-app".to_string(),
                confidence: 0.95,
                context: "Cargo.toml".to_string(),
            }],
            uncertain: vec![],
        };

        let registry_path =
            merge_detected_into_registry(&detected).expect("registry should be updated");
        let registry = EntityRegistry::load(&registry_path).expect("registry should load");
        assert!(registry.people().contains_key("Alice"));
        assert!(registry
            .projects()
            .iter()
            .any(|project| project.eq_ignore_ascii_case("signal-app")));

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_detect_mining_mode_prefers_convos_for_chat_exports() {
        let temp = tempfile::TempDir::new().expect("tempdir should be created");
        std::fs::write(temp.path().join("conversation.jsonl"), "{}\n{}")
            .expect("conversation export should be written");
        std::fs::write(temp.path().join("chatgpt-export.json"), "{}")
            .expect("chatgpt export should be written");

        assert!(matches!(
            detect_mining_mode(&temp.path().to_path_buf()),
            MiningMode::Convos
        ));
    }

    #[test]
    fn test_detect_mining_mode_prefers_projects_for_source_trees() {
        let temp = tempfile::TempDir::new().expect("tempdir should be created");
        std::fs::create_dir_all(temp.path().join("src")).expect("src dir should be created");
        std::fs::create_dir_all(temp.path().join("tests")).expect("tests dir should be created");
        std::fs::write(temp.path().join("README.md"), "project docs")
            .expect("readme should be written");
        std::fs::write(temp.path().join("src").join("main.rs"), "fn main() {}\n")
            .expect("main.rs should be written");

        assert!(matches!(
            detect_mining_mode(&temp.path().to_path_buf()),
            MiningMode::Projects
        ));
    }

    #[test]
    fn test_cmd_mine_flattens_include_ignored_for_project_mode_dry_run() {
        let result = cmd_mine(
            &PathBuf::from("/tmp/project"),
            &MiningMode::Projects,
            None,
            "mpr",
            0,
            true,
            false,
            &["a.txt,b.txt".to_string(), "c.txt".to_string()],
            Some("/tmp/palace"),
            Some("exchange"),
            false,
        );

        if let Err(e) = &result {
            eprintln!("Error: {:?}", e);
        }
        assert!(result.is_ok());
    }

    #[test]
    fn test_cmd_compress_stores_lossy_summaries_in_compressed_collection() {
        let temp = tempfile::TempDir::new().expect("tempdir should be created");
        let palace = temp.path().join("palace");
        let mut db = PalaceDb::open(&palace).expect("palace db should open");
        db.add(
            &[(
                "doc-1",
                "Alice decided to use GraphQL instead of REST for the backend API.",
            )],
            &[&[
                ("wing", "project"),
                ("room", "backend"),
                ("source_file", "notes/decision.txt"),
            ]],
        )
        .expect("seed drawer should be added");
        db.flush().expect("seed db should flush");

        let entities = temp.path().join("entities.json");
        std::fs::write(&entities, r#"{"entities":{"Alice":"ALC"}}"#)
            .expect("entities config should be written");

        cmd_compress(
            None,
            false,
            entities.to_str(),
            Some(palace.to_str().expect("palace path should be utf-8")),
        )
        .expect("compress command should succeed");

        let compressed = PalaceDb::open_collection(
            &palace,
            crate::palace_db::DEFAULT_COMPRESSED_COLLECTION_NAME,
        )
        .expect("compressed db should open");
        let entries = compressed.get_all(None, None, 10);
        assert_eq!(entries.len(), 1);
        let result = &entries[0];
        assert!(result.documents[0].starts_with("project|backend|?|decision"));
        assert!(result.documents[0].contains("ALC") || result.documents[0].contains("0:"));
        let meta = &result.metadatas[0];
        assert!(meta.contains_key("compression_ratio"));
        assert!(meta.contains_key("original_tokens"));

        let query_hits = compressed
            .query_sync("GraphQL backend", Some("project"), Some("backend"), 5)
            .expect("compressed collection should be queryable");
        assert_eq!(query_hits.len(), 1);
        assert!(
            query_hits[0].documents[0].contains("graphql")
                || query_hits[0].documents[0].contains("GraphQL")
        );
    }
}
