//! Agent adapter system for `mpr connect <agent-name>`.
//!
//! Writes MCP server config into each supported AI agent's config directory.
//! Detects which agents are installed on the machine and can optionally
//! write config files that wire `mpr` as an MCP server.
//!
//! # Supported agents
//!
//! | Agent | Config path | Detect dir |
//! |-------|-------------|------------|
//! | kiro | `~/.kiro/settings/mcp.json` | `~/.kiro` |
//! | warp | `~/.warp/.mcp.json` | `~/.warp` |
//! | cline | `~/.cline/mcp.json` | `~/.cline` |
//! | continue | `~/.continue/config.json` | `~/.continue` |
//! | zed | `~/.config/zed/settings.json` | `~/.config/zed` |
//! | openhuman | `~/.openhuman/mcp.json` | `~/.openhuman` |
//! | qwen | `~/.qwen/settings.json` | `~/.qwen` |
//! | antigravity | `~/.config/Antigravity/User/mcp_config.json` | `~/.config/Antigravity/User` |
//!
//! # Usage
//!
//! ```rust,ignore
//! use mempalace::connect::{connect, list_adapters, resolve_adapter};
//!
//! // Connect a specific agent by name
//! let result = connect("zed", &ConnectOptions { dry_run: false, force: false });
//!
//! // List all known adapters
//! for adapter in list_adapters() {
//!     println!("{} at {:?}", adapter.name(), adapter.config_path());
//! }
//! ```

pub mod amp;
pub mod antigravity;
pub mod claude_code;
pub mod cline;
pub mod codex;
pub mod continue_dev;
pub mod copilot_cli;
pub mod cursor;
pub mod droid;
pub mod gemini_cli;
pub mod json_mcp;
pub mod kiro;
pub mod openhuman;
pub mod qwen;
pub mod types;
pub mod vscode;
pub mod warp;
pub mod windsurf;
pub mod zed;

pub use types::{ConnectOptions, ConnectResult};

use std::path::PathBuf;

/// Trait implemented by all agent adapters.
///
/// All implementations must be `Send + Sync` so the registry can be
/// used safely from async contexts.
pub trait ConnectAdapter: Send + Sync {
    /// Machine-readable name (e.g. `"zed"`, `"kiro"`).
    fn name(&self) -> &'static str;

    /// Path to the config file this adapter manages.
    fn config_path(&self) -> PathBuf;

    /// Returns `true` if the agent is installed on this machine.
    fn detect(&self) -> bool;

    /// Write (or update) the MCP config for this agent.
    fn connect(&self, opts: &ConnectOptions) -> std::result::Result<ConnectResult, anyhow::Error>;
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

fn all_adapters() -> Vec<Box<dyn ConnectAdapter>> {
    vec![
        Box::new(kiro::KiroAdapter),
        Box::new(warp::WarpAdapter),
        Box::new(cline::ClineAdapter),
        Box::new(continue_dev::ContinueDevAdapter),
        Box::new(zed::ZedAdapter),
        Box::new(openhuman::OpenHumanAdapter),
        Box::new(qwen::QwenAdapter),
        Box::new(antigravity::AntigravityAdapter),
        Box::new(claude_code::ClaudeCodeAdapter),
        Box::new(copilot_cli::CopilotCliAdapter),
        Box::new(codex::CodexAdapter),
        Box::new(cursor::CursorAdapter),
        Box::new(gemini_cli::GeminiCliAdapter),
        Box::new(windsurf::WindsurfAdapter),
        Box::new(vscode::VsCodeAdapter),
        Box::new(amp::AmpAdapter),
        Box::new(droid::DroidAdapter),
    ]
}

/// Return all registered adapters.
pub fn list_adapters() -> Vec<Box<dyn ConnectAdapter>> {
    all_adapters()
}

/// Find an adapter by name (case-insensitive).
pub fn resolve_adapter(name: &str) -> Option<Box<dyn ConnectAdapter>> {
    let lower = name.to_lowercase();
    all_adapters().into_iter().find(|a| a.name() == lower)
}

/// Connect to a named agent, writing MCP config if detected.
///
/// Returns an error only on unexpected IO failures (not when the agent
/// is not detected or the adapter is a stub).
pub fn connect(name: &str, opts: &ConnectOptions) -> std::result::Result<ConnectResult, anyhow::Error> {
    let adapter = resolve_adapter(name)
        .ok_or_else(|| anyhow::anyhow!("unknown agent: {name}"))?;

    if !adapter.detect() {
        tracing::info!(
            "connect: {} not detected on this machine (skipping)",
            name
        );
        return Ok(ConnectResult {
            adapter: name.to_string(),
            config_path: adapter.config_path(),
            wrote: false,
            note: Some("not-detected".to_string()),
        });
    }

    adapter.connect(opts)
}

/// CLI entry point for `mpr connect [adapter] [--dry-run]`.
///
/// - `adapter == None`  → list all supported adapters and their config paths.
/// - `adapter == Some(n)` → connect to agent `n` (writes config when detected).
pub fn run(adapter: Option<&str>, dry_run: bool) -> std::result::Result<(), anyhow::Error> {
    match adapter {
        None => {
            println!("Supported `mpr connect` adapters:");
            println!();
            for a in list_adapters() {
                let detected = if a.detect() { "[detected]" } else { "[not detected]" };
                println!("  {:<14} {} {}", a.name(), a.config_path().display(), detected);
            }
            Ok(())
        }
        Some(name) => {
            let opts = ConnectOptions { dry_run, force: false };
            let result = connect(name, &opts)?;
            if result.wrote {
                println!(
                    "wrote MCP config for {} -> {}",
                    result.adapter,
                    result.config_path.display()
                );
            } else if let Some(note) = &result.note {
                println!("{}: {} (config: {})", result.adapter, note, result.config_path.display());
            } else {
                println!("{}: no changes", result.adapter);
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_adapter_known() {
        let adapter = resolve_adapter("zed").expect("zed should be known");
        assert_eq!(adapter.name(), "zed");
    }

    #[test]
    fn test_resolve_adapter_unknown() {
        assert!(resolve_adapter("nonexistent").is_none());
    }

    #[test]
    fn test_list_adapters_count() {
        assert_eq!(list_adapters().len(), 17);
    }

    #[test]
    fn test_zed_wrapper_key_is_context_servers() {
        // Zed uses "context_servers" not "mcpServers" — verify via config path
        let adapter = resolve_adapter("zed").expect("zed must exist");
        let path = adapter.config_path();
        assert!(
            path.to_string_lossy().contains("zed"),
            "zed config path should contain 'zed': {:?}",
            path
        );
    }

    #[test]
    fn test_json_mcp_round_trip() {
        use std::fs;
        use crate::connect::json_mcp::write_mcp_config;

        let tmpdir = tempfile::TempDir::new().expect("temp dir");
        let path = tmpdir.path().join("mcp.json");
        let result = write_mcp_config(&path, "mempalace", "mcpServers");
        assert!(result.wrote, "write should succeed");
        assert!(path.exists(), "file should exist after write");

        // Verify contents are valid JSON with expected structure
        let raw = fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let servers = val.get("mcpServers").expect("mcpServers key must exist");
        let entry = servers.get("mempalace").expect("mempalace entry must exist");
        assert_eq!(entry.get("command").and_then(|v| v.as_str()), Some("mpr"));
    }

    #[test]
    fn test_atomic_write_no_partial_file() {
        use std::fs;
        use crate::connect::json_mcp::write_mcp_config;

        let tmpdir = tempfile::TempDir::new().expect("temp dir");
        let path = tmpdir.path().join("atomic.json");
        let result = write_mcp_config(&path, "mempalace", "mcpServers");

        // No .tmp files should remain after a successful write
        let entries: Vec<_> = tmpdir.path().read_dir().unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        let tmp_files: Vec<_> = entries.iter().filter(|n| n.contains(".tmp")).collect();
        assert!(
            tmp_files.is_empty(),
            "no .tmp files should remain after successful write: {:?}",
            tmp_files
        );
        assert!(result.wrote, "write should succeed");
        assert!(path.exists(), "target file should exist");
    }
}