//! Visual Studio Code adapter.
//!
//! VS Code stores workspace MCP servers in `.vscode/mcp.json` and
//! user-level servers in the platform-specific User config dir:
//!   - macOS: `~/Library/Application Support/Code/User/mcp.json`
//!   - Linux: `~/.config/Code/User/mcp.json`
//!   - Windows: `%APPDATA%/Code/User/mcp.json`
//!
//! Schema follows the standard MCP envelope `{ mcpServers: { ... } }`.

use std::path::PathBuf;

use crate::connect::json_mcp::write_mcp_config;
use crate::connect::types::{ConnectOptions, ConnectResult};
use crate::connect::ConnectAdapter;

pub struct VsCodeAdapter;

impl VsCodeAdapter {
    /// Resolve the platform-specific user config dir for VS Code.
    fn user_config_dir() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            dirs::home_dir()
                .map(|p| p.join("Library/Application Support/Code/User"))
                .unwrap_or_else(|| PathBuf::from("~/Library/Application Support/Code/User"))
        }
        #[cfg(target_os = "windows")]
        {
            std::env::var("APPDATA")
                .map(|p| PathBuf::from(p).join("Code/User"))
                .unwrap_or_else(|_| PathBuf::from("%APPDATA%/Code/User"))
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            dirs::home_dir()
                .map(|p| p.join(".config/Code/User"))
                .unwrap_or_else(|| PathBuf::from("~/.config/Code/User"))
        }
    }
}

impl ConnectAdapter for VsCodeAdapter {
    fn name(&self) -> &'static str {
        "vscode"
    }

    fn config_path(&self) -> PathBuf {
        Self::user_config_dir().join("mcp.json")
    }

    fn detect(&self) -> bool {
        Self::user_config_dir().exists()
    }

    fn connect(&self, opts: &ConnectOptions) -> std::result::Result<ConnectResult, anyhow::Error> {
        let path = self.config_path();
        let result = write_mcp_config(&path, "mempalace", "mcpServers");
        if opts.dry_run {
            tracing::info!(
                "connect [dry-run] {} -> {:?} (wrote={})",
                self.name(),
                path,
                result.wrote
            );
        }
        Ok(result)
    }
}
