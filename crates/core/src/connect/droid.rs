//! Factory Droid adapter.
//!
//! Droid (by Factory) stores MCP servers in `~/.factory/mcp.json`.
//! Schema follows the standard MCP envelope `{ mcpServers: { ... } }`.

use std::path::PathBuf;

use crate::connect::json_mcp::write_mcp_config;
use crate::connect::types::{ConnectOptions, ConnectResult};
use crate::connect::ConnectAdapter;

pub struct DroidAdapter;

impl ConnectAdapter for DroidAdapter {
    fn name(&self) -> &'static str {
        "droid"
    }

    fn config_path(&self) -> PathBuf {
        dirs::home_dir()
            .map(|p| p.join(".factory/mcp.json"))
            .unwrap_or_else(|| PathBuf::from("~/.factory/mcp.json"))
    }

    fn detect(&self) -> bool {
        dirs::home_dir()
            .map(|p| p.join(".factory").exists())
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
