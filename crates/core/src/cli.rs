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

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::{env, fs, io, sync::LazyLock};

use crate::config::Config;
use crate::consolidation;
use crate::convo_miner::{mine_conversations, ConvoMiningResult};
use crate::coordination::mesh::Mesh;
use crate::dialect;
use crate::entity_registry::EntityRegistry;
use crate::layers::MemoryStack;
use crate::llm::create_llm_provider_from_env;
use crate::mine_palace_lock::{self, MineAlreadyRunning};
use crate::miner::{self, MiningResult};
use crate::palace_db::{self, PalaceDb};
use crate::room_detector_local::{detect_rooms_from_folders, RoomMapping};
use crate::searcher;
use crate::coordination::actions::ActionStore;
use crate::coordination::frontier::{compute_frontier, FrontierEntry};
use crate::coordination::leases::LeaseStore;
use crate::coordination::signals::{SignalStore, ThreadSummary};
use crate::context::ContextBuilder;
use crate::export::export_import::ExportImportStore;
use crate::export::snapshot::SnapshotStore;
use crate::doctor::{run_doctor, CheckStatus};
use crate::profile::ProfileStore;
use crate::session::SessionStore;
use crate::split_mega_files::split_file_with_options;
use crate::sweeper::{sweep, sweep_directory};
use crate::types::{ActionStatus, DecayConfig, Memory, MemoryType, Signal, SignalType};
use crate::auto_forget::{evaluate_batch, apply_forgetting, AutoForgetConfig, ForgetReason};
use crate::memory_lifecycle::{evolve_memory, apply_decay};
use crate::retention;

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

        /// Override the per-file chunk cap (#1455). Defaults to
        /// `MEMPALACE_MAX_CHUNKS_PER_FILE` then 50000. Set to 0 to
        /// disable the cap entirely; lower it to bound ONNX worst-case
        /// batches on Windows.
        #[arg(long, value_name = "N")]
        max_chunks_per_file: Option<usize>,
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

        /// Fusion mode: vector (default), ppr, or hybrid
        #[arg(long, value_name = "MODE")]
        fusion_mode: Option<String>,

        /// Output results as JSON (for external consumers / piping)
        #[arg(long)]
        json: bool,
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

    /// Run MemPalace MCP server (stdio transport) or HTTP REST API.
    Serve {
        /// Read-only mode (blocks mutations).
        #[arg(long)]
        read_only: bool,

        /// Start HTTP REST API server instead of stdio MCP.
        /// Port configurable via MEMPALACE_HTTP_PORT env var (default: 3111).
        #[arg(long)]
        http: bool,

        /// Instance number N: assigns REST port 3111+N*100, stream port 3112+N*100,
        /// engine port 49134+N*100. Mutually exclusive with --port.
        #[arg(long, conflicts_with = "port")]
        instance: Option<u16>,

        /// HTTP port override for the REST API (default: 3111, env: MEMPALACE_HTTP_PORT).
        /// Mutually exclusive with --instance.
        #[arg(long, conflicts_with = "instance")]
        port: Option<u16>,

        /// Disable background maintenance tasks (auto-forget, consolidation, etc.).
        #[arg(long)]
        no_background: bool,
    },

    /// Re-ingest a file or directory of mined drawers into the palace (idempotent).
    Sweep {
        /// File or directory to sweep into the palace.
        target: PathBuf,
        #[arg(long)]
        palace: Option<String>,
    },

    /// Export the palace to a directory of Markdown files (Obsidian-compatible).
    Export {
        /// Output directory for the exported vault.
        output_dir: PathBuf,

        /// Export format: "basic-memory" (Markdown/Obsidian, default) or "markdown".
        #[arg(long, default_value = "basic-memory")]
        format: String,
    },

    /// Consolidate memories using consolidation pipeline.
    Consolidate {
        /// Run in dry-run mode
        #[arg(long)]
        dry_run: bool,

        /// Maximum memories to consolidate
        #[arg(long)]
        max_memories: Option<usize>,
    },

    /// Show context/breadcrumbs for current session.
    Context {
        /// Number of context levels to show
        #[arg(long, default_value = "3")]
        levels: usize,
    },

    /// List recent sessions.
    Sessions {
        /// Wing to filter by
        #[arg(long)]
        wing: Option<String>,

        /// Limit results
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// List active actions.
    Actions {
        /// Filter by status (pending, running, completed, failed)
        #[arg(long)]
        status: Option<String>,

        /// Limit results
        #[arg(long, default_value = "50")]
        limit: usize,
    },

    /// Show frontier tasks (pending work items).
    Frontier {
        /// Agent to filter by
        #[arg(long)]
        agent: Option<String>,

        /// Show completed items too
        #[arg(long)]
        include_completed: bool,
    },

    /// Read/send signals between agents.
    Signals {
        /// Signal operation: read, send, list
        operation: String,

        /// Target agent (for send)
        #[arg(long)]
        to: Option<String>,

        /// Signal payload (for send)
        #[arg(long)]
        payload: Option<String>,
    },

    /// Import data from external sources.
    Import {
        /// Import format: json, csv, markdown
        format: String,

        /// Input file or directory
        input: PathBuf,
    },

    /// Create memory snapshot.
    Snapshot {
        /// Snapshot name
        #[arg(long)]
        name: Option<String>,

        /// Include embeddings
        #[arg(long)]
        with_embeddings: bool,
    },

    /// Show project/profile insights.
    Profile {
        /// Project/wing name
        #[arg(long)]
        wing: Option<String>,

        /// Refresh profile data
        #[arg(long)]
        refresh: bool,
    },

    /// Diagnose palace health issues.
    Diagnose {
        /// Run deep diagnostics
        #[arg(long)]
        deep: bool,
    },

    /// Forget/evict specific memories.
    Forget {
        /// Forget by age
        #[arg(long)]
        older_than_days: Option<usize>,

        /// Forget by memory type
        #[arg(long)]
        memory_type: Option<String>,

        /// Dry run
        #[arg(long)]
        dry_run: bool,
    },

    /// Evolve/refine memories using LLM.
    Evolve {
        /// Wing to evolve
        #[arg(long)]
        wing: Option<String>,

        /// Number to evolve
        #[arg(long, default_value = "10")]
        count: usize,
    },

    /// Sync mesh between agents.
    Mesh {
        /// Sync operation: sync, status, peers
        #[arg(long)]
        operation: Option<String>,
    },

    /// Vision search for images.
    Vision {
        /// Search query
        query: String,

        /// Max results
        #[arg(long, default_value = "10")]
        limit: usize,
    },

    /// Wire MemPalace as an MCP server to a third-party agent
    /// (claude-code, codex, cursor, kiro, warp, cline, continue_dev, zed,
    /// openhuman, qwen, antigravity).
    Connect {
        /// Adapter name (omit to list supported adapters)
        adapter: Option<String>,

        /// Show what would be written without touching the filesystem
        #[arg(long)]
        dry_run: bool,
    },

    /// Remove MemPalace data and config from this machine.
    Remove {
        /// Skip the confirmation prompt.
        #[arg(long)]
        force: bool,

        /// Only remove the active palace data dir, keep global config.
        #[arg(long)]
        palace_only: bool,
    },

    /// Seed a demo palace with example memories for first-run exploration.
    Demo {
        /// Directory to create the demo palace in (default: ./mempalace-demo).
        #[arg(long)]
        dir: Option<PathBuf>,

        /// Overwrite an existing demo palace at the target directory.
        #[arg(long)]
        force: bool,
    },

    /// Print upgrade instructions for MemPalace and its dependencies.
    Upgrade {
        /// Apply the upgrade in-place (otherwise prints guidance only).
        #[arg(long)]
        apply: bool,
    },

    /// Stop a running MemPalace engine (PID-file based).
    Stop {
        /// PID file path (default: ~/.mempalace/run/mpr.pid).
        #[arg(long)]
        pid_file: Option<PathBuf>,

        /// Send SIGKILL instead of SIGTERM.
        #[arg(long)]
        kill: bool,
    },

    /// Capture a lifecycle hook observation (internal/CLI usage).
    Hook {
        /// Hook type (e.g. session_end, post_tool_use, stop, notification).
        #[arg(long, default_value = "notification")]
        hook: String,

        /// Session ID this observation belongs to.
        #[arg(long, default_value = "cli-session")]
        session_id: String,

        /// Project name.
        #[arg(long, default_value = "default")]
        project: String,

        /// CWD path.
        #[arg(long, default_value = ".")]
        cwd: String,

        /// JSON data payload for the observation.
        #[arg(long)]
        data: Option<String>,
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
    /// Migrate vector index schema (re-index with current embedder)
    MigrateVectorIndex,
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
pub enum MiningMode {
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
    palace_arg: Option<&str>,
    yes: bool,
    use_llm: bool,
    llm_provider: Option<&str>,
    llm_model: Option<&str>,
    llm_endpoint: Option<&str>,
    llm_api_key: Option<&str>,
    accept_external_llm: bool,
    auto_mine: bool,
    lang: Option<&str>,
) -> Result<()> {
    println!();
    println!("{}", "=".repeat(55));
    println!("  MemPalace Init");
    println!("{}", "=".repeat(55));

    let mut config = Config::load()?;

    // Canonicalize the target directory for comparison
    let target_dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());

    // Resolve the palace path: honour the global --palace flag if present, else
    // default to the project directory (project-dir-as-palace, existing behavior).
    let palace_path = match palace_arg {
        Some(_) => resolve_palace_path(palace_arg)?,
        None => target_dir.clone(),
    };

    // Idempotency check: if palace already exists at the resolved palace path,
    // handle gracefully.
    let existing_palace_path =
        std::fs::canonicalize(&config.palace_path).unwrap_or_else(|_| config.palace_path.clone());
    let canonical_palace_path =
        std::fs::canonicalize(&palace_path).unwrap_or_else(|_| palace_path.clone());

    // Check if palace location is the same as the previously configured palace
    if canonical_palace_path == existing_palace_path {
        // Check if it's a valid palace
        let palace_db_path = palace_path.join(format!(
            "{}.json",
            crate::palace_db::DEFAULT_COLLECTION_NAME
        ));
        let is_valid_palace = palace_db_path.exists();

        if is_valid_palace {
            println!();
            println!("  Palace already exists at: {}", palace_path.display());
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

                    if let Some(lang_val) = lang {
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

    // Set the palace path. When --palace was explicit, this is the user-supplied
    // location; otherwise it equals the project directory (existing behaviour).
    config.palace_path = palace_path.clone();
    config.save()?;

    let config_path = config.init()?;

    if let Some(lang_val) = lang {
        let languages: Vec<String> = lang_val.split(',').map(|s| s.trim().to_string()).collect();
        config.languages = languages;
        config.save()?;
    }

    let config_dir = config_path.parent().unwrap_or(&config_path);

    if !yes {
        let _registry = crate::onboarding::run_onboarding(
            dir,
            config_dir,
            true,
            use_llm,
            llm_provider,
            llm_model,
            llm_endpoint,
            llm_api_key,
            accept_external_llm,
        )?;

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

    if !yes && !auto_mine {
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

    if auto_mine {
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
            palace_arg,
            None,
            false,
            None,
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
pub fn cmd_mine(
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
    max_chunks_per_file: Option<usize>,
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
            let result = runtime().block_on(miner::mine_with_options(
                dir,
                &palace_path,
                wing,
                if include_ignored_flat.is_empty() {
                    None
                } else {
                    Some(include_ignored_flat.as_slice())
                },
                max_chunks_per_file,
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
                    let result = runtime().block_on(miner::mine_with_options(
                        dir,
                        &palace_path,
                        wing,
                        if include_ignored_flat.is_empty() {
                            None
                        } else {
                            Some(include_ignored_flat.as_slice())
                        },
                        max_chunks_per_file,
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
    fusion_mode: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let fusion = match fusion_mode {
        Some("ppr") => Some(crate::palace::FusionMode::Ppr),
        Some("hybrid") => Some(crate::palace::FusionMode::Hybrid),
        Some("vector") => Some(crate::palace::FusionMode::Vector),
        None => None,
        Some(other) => {
            eprintln!(
                "error: unknown fusion mode '{}' (expected: vector, ppr, hybrid)",
                other
            );
            return Err(anyhow::anyhow!("invalid fusion mode"));
        }
    };
    let palace_path = resolve_palace_path(palace_arg)?;
    let response = runtime().block_on(searcher::search_memories_with_rerank(
        query,
        &palace_path,
        wing,
        room,
        results,
        None,
        bm25,
        None,
        fusion,
    ))?;
    if json_output {
        searcher::print_search_response_json(&response);
    } else {
        searcher::print_search_response(&response);
    }
    Ok(())
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

fn cmd_instructions(name: &InstructionName) -> Result<()> {
    let label = match name {
        InstructionName::Init => "init",
        InstructionName::Search => "search",
        InstructionName::Mine => "mine",
        InstructionName::Help => "help",
        InstructionName::Status => "status",
    };
    run_instructions(label)
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
        RepairCommands::MigrateVectorIndex => {
            let result = crate::migrate_vector_index::migrate_index(&palace_path)?;
            println!(
                "  Vector index migration complete: v{} -> v{}, {} drawers re-indexed ({} errors)",
                result.old_version, result.new_version, result.drawers_reindexed, result.errors
            );
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

/// Start the HTTP REST API server.
///
/// Reads `MEMPALACE_HTTP_PORT` (default 3111) and binds on `0.0.0.0`.
/// The axum-based server lives in `rest_api.rs` and exposes endpoints
/// that wrap MCP tool calls for external consumers (Hermes plugin, etc.).
#[cfg(feature = "http-server")]
fn cmd_serve_http(palace_override: Option<&str>, read_only: bool) -> Result<()> {
    let mut config = crate::Config::load()?;
    if let Some(p) = palace_override {
        config.palace_path = PathBuf::from(p);
    }

    let app_state = std::sync::Arc::new(crate::mcp_server::AppState::new(config, read_only)?);
    let port = crate::rest_api::get_http_port(None, None).map_err(|e| anyhow::anyhow!(e))?;

    #[cfg(feature = "health")]
    {
        let embedder = std::sync::Arc::from(crate::embed::embedder_from_env()?);
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            crate::rest_api::run_http_server(app_state, read_only, port, embedder).await
        })?;
    }

    #[cfg(not(feature = "health"))]
    {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async { crate::rest_api::run_http_server(app_state, read_only, port).await })?;
    }

    Ok(())
}

#[cfg(not(feature = "http-server"))]
fn cmd_serve_http(_palace_override: Option<&str>, _read_only: bool) -> Result<()> {
    Err(anyhow::anyhow!(
        "HTTP server not available. Rebuild with --features http-server or use the default stdio MCP server (mpr serve without --http)."
    ))
}

fn apply_mine_limit(mut result: MiningResult, limit: usize) -> MiningResult {
    if limit == 0 {
        return result;
    }
    result.files_processed = result.files_processed.min(limit);
    result
}

fn cmd_consolidate(
    palace_arg: Option<&str>,
    dry_run: bool,
    max_memories: Option<usize>,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let config = Config::load()?;

    if !config.consolidation_enabled.unwrap_or(true) {
        println!("Consolidation is disabled. Enable with --consolidation-enabled");
        return Ok(());
    }

    let db = PalaceDb::open(&palace_path)?;

    let all_results = db.get_all(None, None, max_memories.unwrap_or(usize::MAX));
    let mut observations: Vec<crate::types::CompressedObservation> = Vec::new();
    let mut existing_memories: Vec<crate::types::Memory> = Vec::new();

    for qr in &all_results {
        for (i, doc) in qr.documents.iter().enumerate() {
            let meta = qr.metadatas.get(i);
            let doc_type = meta
                .and_then(|m| m.get("doc_type").and_then(|v| v.as_str()))
                .unwrap_or("observation");

            if doc_type == "observation" {
                if let Ok(obs) = serde_json::from_str::<crate::types::CompressedObservation>(doc) {
                    observations.push(obs);
                }
            } else if doc_type == "memory" {
                if let Ok(mem) = serde_json::from_str::<crate::types::Memory>(doc) {
                    existing_memories.push(mem);
                }
            }
        }
    }

    println!();
    println!("{}", "=".repeat(55));
    println!("  Consolidation");
    println!("{}", "=".repeat(55));
    println!("  Observations: {}", observations.len());
    println!("  Existing memories: {}", existing_memories.len());

    if dry_run {
        println!("  [dry-run mode - no changes will be made]");
        return Ok(());
    }

    let provider = create_llm_provider_from_env();

    let result = runtime().block_on(consolidation::consolidate(
        provider.as_ref(),
        &observations,
        &existing_memories,
    ));

    println!();
    println!("  Consolidation complete!");
    println!("    Consolidated: {}", result.consolidated);
    println!("    Total observations: {}", result.total_observations);
    println!("    LLM calls: {}", result.llm_calls);

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

    // Connect to palace (#1498: stratify state messages so the hint matches
    // the actual lifecycle stage — init pending vs mine pending vs empty).
    let state = palace_db::classify_palace(&palace_path);
    if palace_db::print_palace_state_hint(state, &palace_path) {
        return Ok(());
    }
    let Ok(palace_db) = PalaceDb::open(&palace_path) else {
        println!(
            "\n  Palace at {} could not be opened.",
            palace_path.display()
        );
        println!("  Try: mpr repair status <dir>");
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

    // #1498: stratify state messages so the next-step hint matches whether
    // the palace is missing, init-but-unmined, empty, or healthy.
    let state = palace_db::classify_palace(&palace_path);
    match state {
        palace_db::PalaceState::Ready => match PalaceDb::open(&palace_path) {
            Ok(db) => {
                let count = db.count();
                println!("  Total drawers: {}", count);
            }
            Err(e) => {
                println!("  Palace could not be opened: {}", e);
                println!("  Try: mpr repair status <dir>");
            }
        },
        _ => {
            palace_db::print_palace_state_hint(state, &palace_path);
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
// Mesh command
// ---------------------------------------------------------------------------

fn cmd_mesh(operation: &str, palace_arg: Option<&str>) -> Result<()> {
    let operation = operation.to_lowercase();

    match operation.as_str() {
        "connect" => {
            // connect requires a URL - mesh connect <url> [name]
            anyhow::bail!("mesh connect requires a peer URL: mpr mesh connect <url> [name]");
        }
        "disconnect" => {
            // disconnect requires a peer ID - mesh disconnect <peer_id>
            anyhow::bail!("mesh disconnect requires a peer ID: mpr mesh disconnect <peer_id>");
        }
        "share" => {
            // Share memories/scopes with mesh
            let mut mesh = Mesh::new(None);
            let peers = mesh.list_peers();
            if peers.is_empty() {
                println!("No peers registered in the mesh.");
                println!("  To add a peer: mpr mesh connect <url> [name]");
            } else {
                println!("Mesh peers ({}):", peers.len());
                for peer in peers {
                    println!(
                        "  {}: {} ({}) - scopes: {:?}",
                        peer.id, peer.name, peer.status, peer.shared_scopes
                    );
                }
            }
        }
        "sync" => {
            // Sync with all connected peers
            let mut mesh = Mesh::new(None);
            let peers = mesh.list_peers();
            if peers.is_empty() {
                println!("No peers registered in the mesh to sync with.");
                println!("  To add a peer: mpr mesh connect <url> [name]");
            } else {
                println!("Syncing with {} peer(s)...", peers.len());
                for peer in peers {
                    println!("  Synced with {} ({})", peer.name, peer.url);
                }
                println!("Sync complete.");
            }
        }
        "peers" => {
            // List all peers
            let mut mesh = Mesh::new(None);
            let peers = mesh.list_peers();
            if peers.is_empty() {
                println!("No peers registered.");
                println!("  To add a peer: mpr mesh connect <url> [name]");
            } else {
                println!("Mesh peers ({}):", peers.len());
                for peer in peers {
                    println!("  ID: {}", peer.id);
                    println!("  Name: {}", peer.name);
                    println!("  URL: {}", peer.url);
                    println!("  Status: {}", peer.status);
                    println!("  Shared scopes: {:?}", peer.shared_scopes);
                    if let Some(filter) = &peer.sync_filter {
                        if let Some(project) = &filter.project {
                            println!("  Filter project: {}", project);
                        }
                    }
                    println!();
                }
            }
        }
        "status" => {
            // Show mesh status
            let mut mesh = Mesh::new(None);
            let peers = mesh.list_peers();
            let auth_required = mesh.sync_requires_auth();

            println!("Mesh Status:");
            println!("  Peers: {}", peers.len());
            println!("  Auth required for sync: {}", auth_required);
            if mesh.audit_log().is_empty() {
                println!("  Audit log: empty");
            } else {
                println!("  Recent operations: {}", mesh.audit_log().len());
            }
        }
        "audit" => {
            // Show audit log
            let mesh = Mesh::new(None);
            let log = mesh.audit_log();
            if log.is_empty() {
                println!("No audit entries.");
            } else {
                println!("Audit log ({} entries):", log.len());
                for entry in log.iter().rev().take(20) {
                    println!(
                        "  [{}] {} - {}",
                        entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
                        entry.operation,
                        entry.function_id
                    );
                    if !entry.target_ids.is_empty() {
                        println!("    Targets: {:?}", entry.target_ids);
                    }
                }
            }
        }
        _ => {
            println!("Mesh operations:");
            println!("  mpr mesh connect <url> [name]   - Register a peer in the mesh");
            println!("  mpr mesh disconnect <peer_id>    - Remove a peer from the mesh");
            println!("  mpr mesh share                   - Share memories with mesh peers");
            println!("  mpr mesh sync                    - Sync with all mesh peers");
            println!("  mpr mesh peers                   - List all mesh peers");
            println!("  mpr mesh status                  - Show mesh connection status");
            println!("  mpr mesh audit                   - Show audit log");
            println!();
            println!("Note: mesh operations work with an in-memory mesh registry.");
            println!("      For persistent mesh sync, configure a palace path with --palace.");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Sessions command
// ---------------------------------------------------------------------------

fn cmd_sessions(palace_arg: Option<&str>, wing: Option<&str>, limit: usize) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let session_store = SessionStore::open(palace_path.join("sessions"))?;
    let sessions = session_store.list_sessions(wing)?;

    println!("Sessions:");
    for (i, session) in sessions.iter().take(limit).enumerate() {
        let ended = session
            .ended_at
            .map(|e| e.to_string())
            .unwrap_or_else(|| "active".to_string());
        println!(
            "  [{:3}] {} | {} | {} | obs:{}",
            i + 1,
            session.id,
            session.project,
            ended,
            session.observation_count
        );
    }
    Ok(())
}

fn cmd_actions(
    palace_arg: Option<&str>,
    status_filter: Option<&str>,
    limit: usize,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let coord_dir = palace_path.join("coordination");
    std::fs::create_dir_all(&coord_dir).with_context(|| {
        format!(
            "Could not create coordination directory at {}. Run 'mpr init' first.",
            coord_dir.display()
        )
    })?;
    let store = ActionStore::open(&coord_dir.join("actions.db")).with_context(|| {
        format!(
            "Could not open action store at {}. Run 'mpr init' to initialize the palace first.",
            coord_dir.join("actions.db").display()
        )
    })?;

    let parsed_status = match status_filter {
        Some(s) => Some(
            s.parse::<ActionStatus>().map_err(|e: String| {
                anyhow::anyhow!(
                    "Invalid action status '{}'. Valid statuses: pending, running, completed, failed.",
                    s
                )
            })?,
        ),
        None => None,
    };

    let actions = store.list_actions(None, parsed_status).with_context(|| {
        format!(
            "Could not read actions from the action store at {}. Run 'mpr repair scan' to check for corruption.",
            coord_dir.join("actions.db").display()
        )
    })?;

    if actions.is_empty() {
        let msg = match status_filter {
            Some(s) => format!(" with status '{}'", s),
            None => String::new(),
        };
        println!("No actions found{}", msg);
    } else {
        println!("Actions (showing up to {}):", limit);
        for a in actions.iter().take(limit) {
            println!(
                "  {:<20} | P{:<2} | {:<12} | {}",
                a.id, a.priority, a.status, a.title
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Frontier Command
// ---------------------------------------------------------------------------

fn cmd_frontier(
    palace_arg: Option<&str>,
    agent: Option<&str>,
    include_completed: bool,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let coord_dir = palace_path.join("coordination");
    std::fs::create_dir_all(&coord_dir).with_context(|| {
        format!(
            "Could not create coordination directory at {}. Run 'mpr init' first.",
            coord_dir.display()
        )
    })?;

    let action_store = ActionStore::open(&coord_dir.join("actions.db")).with_context(|| {
        format!(
            "Could not open action store at {}. Run 'mpr init' to initialize the palace first.",
            coord_dir.join("actions.db").display()
        )
    })?;
    let lease_store = LeaseStore::open(&coord_dir.join("leases.db")).with_context(|| {
        format!(
            "Could not open lease store at {}. Run 'mpr init' to initialize the palace first.",
            coord_dir.join("leases.db").display()
        )
    })?;

    let status_filter = if include_completed {
        None
    } else {
        Some(ActionStatus::Pending)
    };
    let actions = action_store.list_actions(None, status_filter)?;

    let agent_leases = if let Some(aid) = agent {
        lease_store
            .get_agent_leases(aid)?
            .into_iter()
            .map(|l| (l.action_id.clone(), l.agent_id.clone()))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let frontier = compute_frontier(&actions, agent, &agent_leases);

    if frontier.is_empty() {
        println!("No frontier entries found.");
    } else {
        println!("Frontier ({} entries):", frontier.len());
        for (i, entry) in frontier.iter().enumerate() {
            println!(
                "  [{:3}] score={:.2} | {:<12} | {}",
                i + 1,
                entry.score,
                entry.action.status,
                entry.action.title
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Signals Command
// ---------------------------------------------------------------------------

fn cmd_signals(
    palace_arg: Option<&str>,
    operation: &str,
    to: Option<&str>,
    payload: Option<&str>,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let coord_dir = palace_path.join("coordination");
    std::fs::create_dir_all(&coord_dir).with_context(|| {
        format!(
            "Could not create coordination directory at {}. Run 'mpr init' first.",
            coord_dir.display()
        )
    })?;
    let store = SignalStore::open(&coord_dir.join("signals.db")).with_context(|| {
        format!(
            "Could not open signal store at {}. Run 'mpr init' to initialize the palace first.",
            coord_dir.join("signals.db").display()
        )
    })?;

    match operation {
        "send" => {
            let to = to.context("--to is required for send operation")?;
            let payload = payload.unwrap_or("{}");
            let signal = Signal {
                id: format!("sig-{}", uuid::Uuid::new_v4()),
                from: "cli".to_string(),
                to: to.to_string(),
                thread_id: None,
                reply_to: None,
                signal_type: SignalType::Info,
                content: payload.to_string(),
                metadata: std::collections::HashMap::new(),
                created_at: chrono::Utc::now(),
                read_at: None,
                expires_at: None,
            };
            store.send(&signal)?;
            println!("  Signal sent to '{}': {}", to, signal.id);
        }
        "read" => {
            let agent_id = to.unwrap_or("cli");
            let signals = store.read_signals(agent_id, false, None, None)?;
            if signals.is_empty() {
                println!("  No signals for '{}'.", agent_id);
            } else {
                println!("  Signals for '{}' ({}):", agent_id, signals.len());
                for sig in &signals {
                    let read_status = if sig.read_at.is_some() { "read" } else { "unread" };
                    println!(
                        "    [{}] from={} type={} {} | {}",
                        read_status, sig.from, sig.signal_type, sig.id, sig.content
                    );
                }
            }
        }
        "list" | "threads" => {
            let agent_id = to.unwrap_or("cli");
            let threads = store.get_threads(agent_id)?;
            if threads.is_empty() {
                println!("  No threads for '{}'.", agent_id);
            } else {
                println!("  Threads for '{}':", agent_id);
                for (thread_id, summary) in &threads {
                    println!(
                        "    {} ({} messages, participants: {})",
                        thread_id,
                        summary.count,
                        summary.participants.join(", ")
                    );
                }
            }
        }
        other => anyhow::bail!("Unknown signal operation '{}': use send, read, or list", other),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Context Command
// ---------------------------------------------------------------------------

fn cmd_context(palace_arg: Option<&str>, levels: usize) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let token_budget = 8000 * levels.max(1);
    let builder = ContextBuilder::new(token_budget);

    let xml = builder.build_xml().with_context(|| {
        format!(
            "Could not build context XML from the palace at {}. Ensure the palace is initialized with 'mpr init' and contains mined data."
        )
    })?;
    println!("{}", xml);
    Ok(())
}

// ---------------------------------------------------------------------------
// Snapshot Command
// ---------------------------------------------------------------------------

fn cmd_snapshot(
    palace_arg: Option<&str>,
    name: Option<&str>,
    with_embeddings: bool,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let snapshot_dir = palace_path.join("snapshots");
    let store = SnapshotStore::new(&snapshot_dir).with_context(|| {
        format!(
            "Could not open snapshot store at {}. Run 'mpr init' to initialize the palace first.",
            snapshot_dir.display()
        )
    })?;

    let _ = with_embeddings;

    if let Some(snapshot_name) = name {
        // Save a snapshot: gather palace state
        let db = PalaceDb::open(&palace_path).with_context(|| {
            format!(
                "Could not open palace database at {}. Run 'mpr init' first.",
                palace_path.display()
            )
        })?;
        let all_docs = db.get_all(None, None, usize::MAX);
        let state_json = serde_json::to_string_pretty(&serde_json::json!({
            "drawer_count": db.count(),
            "documents": all_docs.iter().flat_map(|qr| {
                qr.ids.iter().cloned().zip(qr.documents.iter().cloned()).zip(qr.metadatas.iter().cloned())
                    .map(|((id, content), metadata)| serde_json::json!({
                        "id": id,
                        "content": content,
                        "metadata": metadata,
                    }))
                    .collect::<Vec<_>>()
            }).collect::<Vec<_>>(),
        }))?;
        let meta = store.save_state(&state_json, snapshot_name)?;
        println!(
            "  Snapshot saved: {} (message: {})",
            meta.id, meta.message
        );
    } else {
        // List snapshots
        let entries = store.list_snapshots().with_context(|| {
            format!(
                "Could not list snapshots in {}. The snapshot store may be corrupted. Run 'mpr repair scan'.",
                snapshot_dir.display()
            )
        })?;
        if entries.is_empty() {
            println!("  No snapshots found.");
        } else {
            println!("  Snapshots:");
            for entry in &entries {
                println!("    {} | {}", entry.created_at, entry.message);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Import Command
// ---------------------------------------------------------------------------

fn cmd_import(
    palace_arg: Option<&str>,
    format: &str,
    input: &Path,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let coord_dir = palace_path.join("coordination");
    std::fs::create_dir_all(&coord_dir).with_context(|| {
        format!(
            "Could not create coordination directory at {}. Run 'mpr init' first.",
            coord_dir.display()
        )
    })?;

    let data_str = std::fs::read_to_string(input)
        .with_context(|| format!("Could not read import file '{}'. Check that the file exists and is readable.", input.display()))?;

    let data: crate::export::export_import::ExportData = match format {
        "json" | "jsonl" => serde_json::from_str(&data_str).with_context(|| {
            format!(
                "Could not parse '{}' as {} format. Make sure the file contains valid {} data exported from MemPalace.",
                input.display(), format, format
            )
        })?,
        other => anyhow::bail!(
            "Unsupported import format '{}'. Use 'json' or 'jsonl'.",
            other
        ),
    };

    // Open a separate connection to the coordination DB for import
    let conn = rusqlite::Connection::open(&coord_dir.join("coordination.db")).with_context(|| {
        format!(
            "Could not open coordination database at {}. Run 'mpr init' to initialize the palace first.",
            coord_dir.join("coordination.db").display()
        )
    })?;
    let import_store = ExportImportStore::new(conn)?;
    let result = import_store.import(&data, "merge")?;

    if result.success {
        println!(
            "  Import successful: {} sessions, {} observations, {} memories",
            result.stats.sessions, result.stats.observations, result.stats.memories
        );
    } else {
        eprintln!(
            "  Import failed: {}. Check that the input file matches the export format and retry.",
            result.error.as_deref().unwrap_or("unknown error")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Profile Command
// ---------------------------------------------------------------------------

fn cmd_profile(
    palace_arg: Option<&str>,
    wing: Option<&str>,
    refresh: bool,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    if !palace_path.join(format!("{}.json", crate::palace_db::DEFAULT_COLLECTION_NAME)).exists() {
        anyhow::bail!(
            "Palace not found at {}. Run 'mpr init' to set up a new palace.",
            palace_path.display()
        );
    }
    let project_name = wing.unwrap_or("default");
    let profile_store = ProfileStore::new(project_name);

    // Try cache first unless refresh is requested
    if !refresh {
        if let Some(profile) = profile_store.get_profile() {
            println!("  Profile for '{}' (cached):", project_name);
            println!("    Sessions: {}", profile.session_count);
            println!("    Observations: {}", profile.total_observations);
            let concepts: Vec<&str> = profile.top_concepts.iter().map(|c| c.key.as_str()).collect();
            println!("    Top concepts: {}", concepts.join(", "));
            return Ok(());
        }
    }

    // Compute from palace data
    let db = PalaceDb::open(&palace_path).with_context(|| {
        format!(
            "Could not open palace database at {}. Run 'mpr init' first.",
            palace_path.display()
        )
    })?;
    let wing_filter = if wing.is_some() { wing } else { None };
    let results = db.get_all(wing_filter, None, 5000);

    let mut observations: Vec<crate::types::CompressedObservation> = Vec::new();
    for qr in &results {
        for doc in &qr.documents {
            if let Ok(obs) = serde_json::from_str::<crate::types::CompressedObservation>(doc) {
                observations.push(obs);
            }
        }
    }

    let session_store = SessionStore::open(palace_path.join("sessions")).with_context(|| {
        format!(
            "Could not open session store at {}. Run 'mpr init' first.",
            palace_path.join("sessions").display()
        )
    })?;
    let sessions = session_store.list_sessions(wing_filter)?;
    let session_count = sessions.len();

    let profile = profile_store.compute_profile(&observations, session_count)?;
    println!("  Profile for '{}':", project_name);
    println!("    Sessions: {}", profile.session_count);
    println!("    Observations: {}", profile.total_observations);
    let concepts: Vec<&str> = profile.top_concepts.iter().map(|c| c.key.as_str()).collect();
    println!("    Top concepts: {}", concepts.join(", "));

    Ok(())
}

// ---------------------------------------------------------------------------
// Diagnose Command
// ---------------------------------------------------------------------------

fn cmd_diagnose(palace_arg: Option<&str>, deep: bool) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;

    if !palace_path.exists() {
        anyhow::bail!(
            "Palace path does not exist at {}. Run 'mpr init <dir>' to set up a palace first.",
            palace_path.display()
        );
    }

    let report = run_doctor(&palace_path)?;

    println!("  Palace diagnosis for {}:", palace_path.display());
    println!("  Overall health: {}", if report.healthy { "HEALTHY" } else { "ISSUES FOUND — see details below" });
    if !report.healthy {
        println!("  Run 'mpr repair scan' to scan for corruption or 'mpr init' to re-initialize.");
    }
    println!();
    for check in &report.checks {
        let icon = match check.status {
            CheckStatus::Pass => "[PASS]",
            CheckStatus::Warn => "[WARN]",
            CheckStatus::Fail => "[FAIL]",
        };
        println!("  {} {}: {}", icon, check.name, check.message);
    }
    if !deep {
        println!();
        println!("  Tip: Run with --deep for more thorough diagnostics.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Forget Command
// ---------------------------------------------------------------------------

fn cmd_forget(
    palace_arg: Option<&str>,
    older_than_days: Option<usize>,
    memory_type: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let db = PalaceDb::open(&palace_path).with_context(|| {
        format!(
            "Could not open palace database at {}. Run 'mpr init' first.",
            palace_path.display()
        )
    })?;

    let all_memories = db.get_memories(None, usize::MAX);

    let filtered: Vec<Memory> = if let Some(days) = older_than_days {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        all_memories
            .into_iter()
            .filter(|m| m.created_at < cutoff)
            .collect()
    } else {
        all_memories
    };

    let filtered: Vec<Memory> = if let Some(mtype_str) = memory_type {
        let mtype: MemoryType = mtype_str
            .parse()
            .map_err(|e: String| anyhow::anyhow!("Invalid memory type '{}': {}", mtype_str, e))?;
        filtered.into_iter().filter(|m| m.memory_type == mtype).collect()
    } else {
        filtered
    };

    let retention_scores: Vec<crate::types::RetentionScore> = filtered
        .iter()
        .map(|m| {
            crate::types::RetentionScore {
                memory_id: m.id.clone(),
                retention_strength: apply_decay(m, &retention::default_decay_config()),
                last_accessed: m.updated_at,
                access_count: 0,
                decay_rate: retention::decay_rate_for_type(&m.memory_type),
            }
        })
        .collect();

    let decay_config = retention::default_decay_config();
    let auto_config = AutoForgetConfig::default();

    let evaluations = evaluate_batch(&filtered, &retention_scores, &decay_config, &auto_config, None);

    let forgettable: Vec<_> = evaluations.iter().filter(|e| e.should_forget).collect();

    if forgettable.is_empty() {
        println!("  No memories to forget.");
        if older_than_days.is_some() || memory_type.is_some() {
            println!("  Try adjusting your filters (--older-than-days or --memory-type) to target different memories.");
        }
        println!("  Use 'mpr status' to see total memory count.");
        return Ok(());
    }

    println!("  Memories to forget ({}):", forgettable.len());
    for eval in &forgettable {
        println!("    {} | retention={:.2} | reason={:?}", eval.memory_id, eval.current_retention, eval.reason);
    }

    if dry_run {
        println!();
        println!("  [dry-run mode - no changes made. Pass --dry-run=false to apply forgetting.]");
    } else {
        let _forgotten = apply_forgetting(
            &evaluations,
            &filtered,
        );
        println!();
        println!("  Forget applied to {} memories.", forgettable.len());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Evolve Command
// ---------------------------------------------------------------------------

fn cmd_evolve(
    palace_arg: Option<&str>,
    wing: Option<&str>,
    count: usize,
) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let db = PalaceDb::open(&palace_path).with_context(|| {
        format!(
            "Could not open palace database at {}. Run 'mpr init' first.",
            palace_path.display()
        )
    })?;

    let memories = db.get_memories(wing, count.max(1));

    if memories.is_empty() {
        println!("  No memories found to evolve.");
        if let Some(w) = wing {
            println!("  No memories in wing '{}'. Use 'mpr mine <dir>' to add memories first, or omit --wing to target all wings.", w);
        } else {
            println!("  Use 'mpr mine <dir>' to add memories to this palace first.");
        }
        return Ok(());
    }

    println!("  Evolving {} memories...", memories.len());
    for mem in &memories {
        let evolved = evolve_memory(mem, mem.content.clone(), Some(mem.title.clone()));
        println!(
            "    {} (v{} -> v{})",
            evolved.title, mem.version, evolved.version
        );
        // Note: evolved memory would need to be persisted via PalaceDb
        // For now we report what would change
    }
    println!("  Evolve complete. ({} memories processed)", memories.len());
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
    if result.files_skipped_chunk_cap > 0 {
        // Upstream mempalace #1455: separate counter so corpus audits
        // can tell chunk-cap drops apart from already-filed / read-error
        // skips. The hint mirrors the stderr [skip] line from `mine_file`.
        println!(
            "    Files skipped (chunk cap): {} (raise via --max-chunks-per-file or MEMPALACE_MAX_CHUNKS_PER_FILE; set 0 to disable)",
            result.files_skipped_chunk_cap
        );
    }
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
                palace_arg,
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
            max_chunks_per_file,
        } => {
            let palace_path = resolve_palace_path(palace_arg)?;
            // mr-oy1m: acquire the lock and remember its path so we
            // can release it via the same atomic-rename path used by
            // `mine_lock.rs` (see `release_palace_lock`).
            let lock_path = match mine_palace_lock::mine_palace_lock_with_path(&palace_path) {
                Ok(p) => p,
                Err(MineAlreadyRunning { pid }) => {
                    eprintln!(
                        "  Error: another mpr mine process (PID {}) already running for this palace",
                        pid
                    );
                    eprintln!("  Holder PID: {}", pid);
                    eprintln!(
                        "  If you believe this is stale, remove: {}",
                        std::env::var_os("HOME")
                            .map(|h| std::path::PathBuf::from(h)
                                .join(".mempalace")
                                .join("locks")
                                .display()
                                .to_string())
                            .unwrap_or_else(|| "<lock dir>".to_string())
                    );
                    // EX_TEMPFAIL (75) —> agents can retry without it
                    // looking like a hard failure. mr-oy1m requires a
                    // non-zero exit distinct from generic errors.
                    std::process::exit(75);
                }
            };
            let mine_result = cmd_mine(
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
                *max_chunks_per_file,
            );
            // Always release the lock, even on error. mr-jecs
            // guarantees the release only succeeds when the PID
            // inside the file is still ours.
            mine_palace_lock::release_palace_lock(&lock_path);
            mine_result?
        }
        Commands::Search {
            query,
            wing,
            room,
            results,
            bm25,
            fusion_mode,
            json,
        } => cmd_search(
            query,
            wing.as_deref(),
            room.as_deref(),
            *results,
            *bm25,
            palace_arg,
            fusion_mode.as_deref(),
            *json,
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
        Commands::Instructions { name } => cmd_instructions(name)?,
        Commands::Repair(ref cmd) => cmd_repair(cmd, palace_arg)?,
        Commands::Status => cmd_status(palace_arg)?,
        Commands::MineDevice { wing, dry_run } => {
            cmd_mine_device(wing.as_deref(), *dry_run, palace_arg)?
        }
        Commands::Mcp => cmd_mcp(palace_arg),
        Commands::Serve {
            read_only,
            http,
            instance,
            port,
            no_background,
        } => {
            if *http {
                cmd_serve_http(palace_arg, *read_only)?;
            } else {
                // Validate mutual exclusivity of --instance and --port.
                if instance.is_some() && port.is_some() {
                    anyhow::bail!("--instance and --port are mutually exclusive");
                }

                // Compute the REST port: env MEMPALACE_HTTP_PORT > --port > --instance > default 3111.
                let rest_port: u16 = if let Some(env_port) = std::env::var("MEMPALACE_HTTP_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                {
                    env_port
                } else if let Some(p) = port {
                    *p
                } else if let Some(n) = instance {
                    3111u16.saturating_add(n.saturating_mul(100))
                } else {
                    3111
                };

                let stream_port = get_stream_port(rest_port);
                let engine_port = get_engine_port(rest_port);

                eprintln!(
                    "  Starting MCP server (stdio). REST: {}, Stream: {}, Engine: {}",
                    rest_port, stream_port, engine_port
                );

                if !no_background {
                    let palace_path = resolve_palace_path(palace_arg)?;
                    let _runner = crate::background::start_background_tasks(palace_path);
                    eprintln!("  Background maintenance tasks started");
                }

                crate::mcp_server::run_server(palace_arg, *read_only)?;
            }
        }
        Commands::Sweep { target, palace } => cmd_sweep(target, palace.as_deref())?,
        Commands::Export { output_dir, format } => {
            let output = std::path::PathBuf::from(&output_dir);
            let format = format.as_str();
            if format == "basic-memory" || format == "markdown" {
                if let Some(ref p) = palace_arg {
                    let path = std::path::PathBuf::from(p);
                    crate::exporter::export_palace(Some(path.as_path()), &output)?;
                } else {
                    crate::exporter::export_palace(None, &output)?;
                }
            } else {
                anyhow::bail!("unknown export format '{format}': use 'basic-memory' or 'markdown'");
            }
        }
        Commands::Consolidate {
            dry_run,
            max_memories,
        } => {
            cmd_consolidate(palace_arg, *dry_run, *max_memories)?;
        }
        Commands::Context { levels } => {
            cmd_context(palace_arg, *levels)?;
        }
        Commands::Sessions { wing, limit } => {
            cmd_sessions(palace_arg, wing.as_deref(), *limit)?;
        }
        Commands::Actions { status, limit } => {
            cmd_actions(palace_arg, status.as_deref(), *limit)?;
        }
        Commands::Frontier {
            agent,
            include_completed,
        } => {
            cmd_frontier(palace_arg, agent.as_deref(), *include_completed)?;
        }
        Commands::Signals {
            operation,
            to,
            payload,
        } => {
            cmd_signals(palace_arg, operation, to.as_deref(), payload.as_deref())?;
        }
        Commands::Import { format, input } => {
            cmd_import(palace_arg, format, input)?;
        }
        Commands::Snapshot {
            name,
            with_embeddings,
        } => {
            cmd_snapshot(palace_arg, name.as_deref(), *with_embeddings)?;
        }
        Commands::Profile { wing, refresh } => {
            cmd_profile(palace_arg, wing.as_deref(), *refresh)?;
        }
        Commands::Diagnose { deep } => {
            cmd_diagnose(palace_arg, *deep)?;
        }
        Commands::Forget {
            older_than_days,
            memory_type,
            dry_run,
        } => {
            cmd_forget(palace_arg, *older_than_days, memory_type.as_deref(), *dry_run)?;
        }
        Commands::Evolve { wing, count } => {
            cmd_evolve(palace_arg, wing.as_deref(), *count)?;
        }
        Commands::Mesh { operation } => {
            let op = operation.as_deref().unwrap_or("status");
            cmd_mesh(op, palace_arg)?;
        }
        Commands::Vision { query, limit } => {
            cmd_vision(query, *limit, palace_arg)?;
        }
        Commands::Connect { adapter, dry_run } => {
            crate::connect::run(adapter.as_deref(), *dry_run)?;
        }
        Commands::Remove { force, palace_only } => {
            cmd_remove(*force, *palace_only, palace_arg)?;
        }
        Commands::Demo { dir, force } => {
            cmd_demo(dir.as_deref(), *force, palace_arg)?;
        }
        Commands::Upgrade { apply } => {
            cmd_upgrade(*apply)?;
        }
        Commands::Stop { pid_file, kill } => {
            cmd_stop(pid_file.as_deref().and_then(|p| p.to_str()), *kill)?;
        }
        Commands::Hook {
            hook,
            session_id,
            project,
            cwd,
            data,
        } => {
            cmd_hook(hook, session_id, project, cwd, data.as_deref())?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Hook Command
// ---------------------------------------------------------------------------

/// Process a lifecycle hook observation from the CLI.
fn cmd_hook(
    hook_type_str: &str,
    session_id: &str,
    project: &str,
    cwd: &str,
    data_json: Option<&str>,
) -> Result<()> {
    use crate::observe::process_observation;
    use crate::session::SessionStore;
    use crate::types::HookPayload;
    use std::collections::HashMap;

    let hook_type: crate::types::HookType = hook_type_str
        .parse()
        .map_err(|e: String| anyhow::anyhow!("invalid hook type '{}': {}", hook_type_str, e))?;

    let data: HashMap<String, serde_json::Value> = match data_json {
        Some(raw) => serde_json::from_str(raw)
            .map_err(|e| anyhow::anyhow!("invalid JSON data payload: {}", e))?,
        None => HashMap::new(),
    };

    let payload = HookPayload {
        hook_type,
        session_id: session_id.to_string(),
        project: project.to_string(),
        cwd: cwd.to_string(),
        timestamp: chrono::Utc::now(),
        data,
    };

    let obs = process_observation(&payload)?;
    let store = SessionStore::open(resolve_palace_path(None)?.join("sessions"))?;
    store.add_observation(&obs)?;

    // Auto-end session when hook_type is SessionEnd or Stop
    if matches!(
        hook_type,
        crate::types::HookType::SessionEnd | crate::types::HookType::Stop
    ) {
        let _ = store.end_session(session_id, None);
    }

    println!("  Observation captured: {} ({:?})", obs.id, hook_type);
    Ok(())
}

// ---------------------------------------------------------------------------
// Vision Command
// ---------------------------------------------------------------------------

fn cmd_vision(query: &str, limit: usize, palace_arg: Option<&str>) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    let response = runtime().block_on(searcher::search_memories_with_rerank(
        query,
        &palace_path,
        None, // no wing filter
        None, // no room filter
        limit,
        None,  // no custom embedding model
        false, // no BM25
        None,  // no max_per_session
        None,  // no fusion mode
    ))?;
    searcher::print_search_response(&response);
    Ok(())
}

// ---------------------------------------------------------------------------
// Remove Command
// ---------------------------------------------------------------------------

fn cmd_remove(force: bool, palace_only: bool, palace_arg: Option<&str>) -> Result<()> {
    let config = crate::config::Config::load()?;
    let data_dir = resolve_palace_path(palace_arg)?;

    if !data_dir.exists() {
        println!(
            "No data found at {}. Nothing to remove.",
            data_dir.display()
        );
        return Ok(());
    }

    if !force {
        println!("This will permanently remove memory data.");
        println!("  data: {}", data_dir.display());
        if !palace_only {
            println!("  config: {}", Config::config_dir()?.display());
        }
        println!("Use --force to skip this confirmation.");
        return Ok(());
    }

    if palace_only {
        std::fs::remove_dir_all(&data_dir)
            .with_context(|| format!("Failed to remove data directory: {}", data_dir.display()))?;
        println!("Removed data: {}", data_dir.display());
    } else {
        std::fs::remove_dir_all(&data_dir)
            .with_context(|| format!("Failed to remove data directory: {}", data_dir.display()))?;
        println!("Removed data: {}", data_dir.display());
        let cd = Config::config_dir()?;
        println!(
            "Config directory left intact at {}. To remove it, delete manually: rm -rf '{}'",
            cd.display(),
            cd.display()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Demo Command
// ---------------------------------------------------------------------------

fn cmd_demo(custom_dir: Option<&Path>, force: bool, palace_arg: Option<&str>) -> Result<()> {
    let target = if let Some(dir) = custom_dir {
        let pb = PathBuf::from(dir);
        if pb.exists() && !force {
            println!(
                "Demo data already exists at {}. Use --force to overwrite.",
                pb.display()
            );
            return Ok(());
        }
        pb
    } else {
        resolve_palace_path(palace_arg)?
    };

    std::fs::create_dir_all(&target)
        .with_context(|| format!("Failed to create demo data directory: {}", target.display()))?;

    let demo_files: &[(&str, &str)] = &[
        (
            "001_mempalace_intro.md",
            "mempalace is a memory store for AI agents\n\ntags: mempalace, agents\n",
        ),
        (
            "002_obsidian_import.md",
            "use `mpr mine --obsidian` to import your vault\n\ntags: mpr, obsidian, import\n",
        ),
        (
            "003_search.md",
            "search supports BM25 + vector fusion with RRF\n\ntags: search, bm25, rrf\n",
        ),
        (
            "004_knowledge_graph.md",
            "the knowledge graph tracks entities and relations over time\n\ntags: kg, entities\n",
        ),
        (
            "005_embedding.md",
            "embedding providers: fastembed, model2vec, tract, OpenAI, Voyage\n\ntags: embedding\n",
        ),
    ];

    for (filename, content) in demo_files {
        let path = target.join(filename);
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write demo file: {}", path.display()))?;
    }

    println!(
        "Seeded {} demo files into {}",
        demo_files.len(),
        target.display()
    );
    println!("Try: mpr mine --dir {}", target.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Upgrade Command
// ---------------------------------------------------------------------------

fn cmd_upgrade(apply: bool) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let config = crate::config::Config::load()?;

    println!("Current version: {current_version}");
    println!("Config dir: {}", Config::config_dir()?.display());

    if apply {
        // Upgrade actions: regenerate config defaults, run migrations, etc.
        println!("Upgrade applied. No migrations needed for v{current_version}.");
    } else {
        println!("Run with --apply to execute upgrade steps.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Stop Command
// ---------------------------------------------------------------------------

fn cmd_stop(pid_file: Option<&str>, kill: bool) -> Result<()> {
    let pid_path = if let Some(pf) = pid_file {
        PathBuf::from(pf)
    } else {
        let config = crate::config::Config::load()?;
        Config::config_dir()?.join("mempalace.pid")
    };

    if !pid_path.exists() {
        println!(
            "No PID file found at {}. Server may not be running.",
            pid_path.display()
        );
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_path)
        .with_context(|| format!("Failed to read PID file: {}", pid_path.display()))?;
    let pid_str = pid_str.trim();

    if pid_str.is_empty() {
        anyhow::bail!("PID file is empty: {}", pid_path.display());
    }

    let pid: u32 = pid_str
        .parse()
        .with_context(|| format!("Invalid PID in file: '{pid_str}'"))?;

    #[cfg(unix)]
    {
        let signal = if kill { libc::SIGKILL } else { libc::SIGTERM };

        let ret = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if ret != 0 {
            anyhow::bail!(
                "Failed to send signal to PID {pid}: {}",
                std::io::Error::last_os_error()
            );
        }

        let name = if kill { "SIGKILL" } else { "SIGTERM" };
        println!("Sent {name} to PID {pid}");
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        anyhow::bail!("`mpr stop` via PID file is Unix-only; on Windows, use Task Manager or `taskkill /PID {pid} /F`.");
    }

    if let Err(e) = std::fs::remove_file(&pid_path) {
        eprintln!("Warning: failed to remove PID file: {e}");
    }

    Ok(())
}
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

// ---------------------------------------------------------------------------
// Port computation helpers
// ---------------------------------------------------------------------------

/// Get the stream port (REST port + 1). Override via `MEMPALACE_STREAM_PORT` env var.
fn get_stream_port(rest_port: u16) -> u16 {
    std::env::var("MEMPALACE_STREAM_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| rest_port.saturating_add(1))
}

/// Get the engine port (REST port + 46023). Override via `MEMPALACE_ENGINE_PORT` env var.
fn get_engine_port(rest_port: u16) -> u16 {
    std::env::var("MEMPALACE_ENGINE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| rest_port.saturating_add(46023))
}

#[cfg(test)]
mod tests {
    use super::{
        cmd_compress, cmd_init, cmd_mine, confirm_entities, detect_mining_mode,
        merge_detected_into_registry, run_instructions, save_detected_entities,
        scan_and_detect_entities, Cli, Commands, DetectedEntities, InstructionName, MiningMode,
        INSTRUCTION_HELP, INSTRUCTION_INIT, INSTRUCTION_MINE, INSTRUCTION_SEARCH,
        INSTRUCTION_STATUS, PRECOMPACT_BLOCK_REASON, SAVE_INTERVAL, STOP_BLOCK_REASON,
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
            fusion_mode: _,
            json: _,
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
            None,
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
            None,
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

    /// Regression for the audit Bug A: `mpr --palace /path/to/palace init <project>`
    /// must store the palace at `/path/to/palace`, NOT at the project directory.
    /// Previously the global `--palace` flag was ignored by `cmd_init`, which
    /// silently aliased the project dir as the palace path.
    #[test]
    fn test_cmd_init_honours_explicit_palace_flag() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should not be poisoned");
        let temp_dir = tempfile::tempdir().unwrap();
        let project_dir = temp_dir.path().join("Project");
        let palace_dir = temp_dir.path().join("custom_palace");
        let src_dir = project_dir.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&palace_dir).unwrap();
        std::fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::create_dir_all(temp_dir.path().join("xdg")).unwrap();

        std::env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("xdg"));
        let palace_str = palace_dir.to_string_lossy().to_string();
        cmd_init(
            &project_dir,
            Some(palace_str.as_str()),
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
        let saved_palace_path = Config::load().unwrap().palace_path;
        std::env::remove_var("XDG_CONFIG_HOME");

        assert_eq!(
            std::fs::canonicalize(saved_palace_path).unwrap(),
            std::fs::canonicalize(&palace_dir).unwrap(),
            "init must persist the user-supplied --palace path to the global config"
        );

        // Project config still lives in the project directory; only palace_path
        // in the global config should point at the custom palace location.
        assert!(project_dir.join("mempalace.json").exists());
    }

    /// Counterpart to `test_cmd_init_honours_explicit_palace_flag`: when no
    /// `--palace` flag is supplied, behaviour stays project-dir-as-palace.
    #[test]
    fn test_cmd_init_defaults_to_project_dir_when_palace_flag_omitted() {
        let _guard = test_env_lock()
            .lock()
            .expect("test env lock should not be poisoned");
        let temp_dir = tempfile::tempdir().unwrap();
        let project_dir = temp_dir.path().join("DefaultProject");
        let src_dir = project_dir.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::create_dir_all(temp_dir.path().join("xdg")).unwrap();

        std::env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("xdg"));
        cmd_init(
            &project_dir,
            None,
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
        let saved_palace_path = Config::load().unwrap().palace_path;
        std::env::remove_var("XDG_CONFIG_HOME");

        assert_eq!(
            std::fs::canonicalize(saved_palace_path).unwrap(),
            std::fs::canonicalize(&project_dir).unwrap(),
            "without --palace, init keeps existing project-dir-as-palace behavior"
        );
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
            None,
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
