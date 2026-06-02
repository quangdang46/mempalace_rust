//! Windsurf adapter.
//!
//! Windsurf (by Codeium) stores MCP servers in
//! `~/.codeium/windsurf/mcp_config.json`.
//! Schema follows the standard MCP envelope `{ mcpServers: { ... } }`.

use std::path::PathBuf;

use crate::connect::json_mcp::write_mcp_config;
use crate::connect::types::{ConnectOptions, ConnectResult};
use crate::connect::ConnectAdapter;

pub struct WindsurfAdapter;

impl ConnectAdapter for WindsurfAdapter {
    fn name(&self) -> &'static str {
        "windsurf"
    }

    fn config_path(&self) -> PathBuf {
        dirs::home_dir()
            .map(|p| p.join(".codeium/windsurf/mcp_config.json"))
            .unwrap_or_else(|| PathBuf::from("~/.codeium/windsurf/mcp_config.json"))
    }

    fn detect(&self) -> bool {
        dirs::home_dir()
            .map(|p| p.join(".codeium/windsurf").exists())
            .unwrap_or(false)
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
