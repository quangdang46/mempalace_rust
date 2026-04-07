//! Hermes memory provider integration for MemPalace.
//!
//! Provides a MemoryProvider trait and MemPalace-backed implementation
//! for integrating with the Hermes open-source AI agent framework.
//!
//! ## Hermes Integration
//!
//! Hermes is an open-source AI agent framework that supports memory providers.
//! This module provides a MemPalace-backed implementation of the Hermes MemoryProvider ABC.
//!
//! ## Hermes SDK Status
//!
//! When Hermes SDK is available on crates.io, this module will be expanded
//! to provide a native Rust implementation of the MemoryProvider trait.
//! Currently provides a MemPalace-backed provider that stores directly to palace.

use std::path::PathBuf;

/// Hermes memory provider configuration.
#[derive(Debug, Clone)]
pub struct HermesConfig {
    /// Hermes server endpoint (e.g., "http://localhost:8000")
    pub endpoint: String,
    /// MemPalace palace path for storage
    pub palace_path: PathBuf,
    /// API key if Hermes requires authentication
    pub api_key: Option<String>,
}

impl HermesConfig {
    /// Create a new Hermes config.
    pub fn new(endpoint: &str, palace_path: PathBuf) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            palace_path,
            api_key: None,
        }
    }

    /// Set an API key for authenticated Hermes servers.
    #[must_use]
    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = Some(key.to_string());
        self
    }
}

/// Hermes MemoryProvider trait.
/// Implement this to create a custom memory provider for Hermes.
/// Hermes will call these methods to store and retrieve memories.
pub trait HermesMemoryProvider: Send + Sync {
    /// File a conversation turn into memory.
    fn file_turn(&self, role: &str, content: &str) -> anyhow::Result<()>;

    /// Retrieve relevant memories for a query.
    fn retrieve(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Get recent conversation turns.
    fn recent_turns(&self, limit: usize) -> anyhow::Result<Vec<MemoryEntry>>;
}

/// A single memory entry from Hermes.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    /// Role that generated this entry (e.g., "user", "assistant", "system")
    pub role: String,
    /// The content of the turn
    pub content: String,
    /// Timestamp (if available)
    pub timestamp: Option<String>,
}

/// MemPalace-backed Hermes provider using PalaceDb directly.
/// This provides Hermes MemoryProvider functionality without requiring
/// a running Hermes server - it stores directly to the local palace.
pub struct MemPalaceHermesProvider {
    palace_path: PathBuf,
}

impl MemPalaceHermesProvider {
    /// Create a new MemPalace-backed Hermes provider.
    #[must_use]
    pub fn new(palace_path: PathBuf) -> Self {
        Self { palace_path }
    }
}

impl HermesMemoryProvider for MemPalaceHermesProvider {
    fn file_turn(&self, role: &str, content: &str) -> anyhow::Result<()> {
        use crate::palace_db::PalaceDb;

        let mut db = PalaceDb::open(&self.palace_path)?;
        let timestamp = chrono::Utc::now().to_rfc3339();
        let drawer_id = format!(
            "hermes_{}_{}",
            role.replace(' ', "_"),
            timestamp.replace(':', "_")
        );

        db.add(
            &[(&drawer_id, content)],
            &[&[
                ("role", role),
                ("wing", "hermes"),
                ("timestamp", &timestamp),
            ]],
        )?;
        db.flush()?;
        Ok(())
    }

    fn retrieve(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        use crate::palace_db::PalaceDb;

        // Use get_all for now since query is async - simple wing filter + text match
        let results = PalaceDb::open(&self.palace_path)?.get_all(Some("hermes"), None, limit);

        let query_lower = query.to_lowercase();
        let filtered: Vec<MemoryEntry> = results
            .into_iter()
            .filter(|r| {
                r.documents
                    .first()
                    .map(|d| d.to_lowercase().contains(&query_lower))
                    .unwrap_or(false)
            })
            .take(limit)
            .map(|r| {
                let meta = r.metadatas.first();
                let role = meta
                    .and_then(|m| m.get("role"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let timestamp = meta
                    .and_then(|m| m.get("timestamp"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                MemoryEntry {
                    role: role.to_string(),
                    content: r.documents.first().cloned().unwrap_or_default(),
                    timestamp,
                }
            })
            .collect();

        Ok(filtered)
    }

    fn recent_turns(&self, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        let results = crate::palace_db::PalaceDb::open(&self.palace_path)?.get_all(
            Some("hermes"),
            None,
            limit,
        );

        Ok(results
            .into_iter()
            .map(|r| {
                let meta = r.metadatas.first();
                let role = meta
                    .and_then(|m| m.get("role"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let timestamp = meta
                    .and_then(|m| m.get("timestamp"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                MemoryEntry {
                    role: role.to_string(),
                    content: r.documents.first().cloned().unwrap_or_default(),
                    timestamp,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_mem_palace_hermes_provider() {
        let temp = TempDir::new().unwrap();
        let provider = MemPalaceHermesProvider::new(temp.path().to_path_buf());

        // File a turn
        provider.file_turn("user", "Hello, world!").unwrap();

        // Retrieve it
        let results = provider.retrieve("Hello", 5).unwrap();
        assert!(!results.is_empty(), "Expected results but got empty");
        assert_eq!(results[0].role, "user");

        // Recent turns
        let recent = provider.recent_turns(10).unwrap();
        assert!(!recent.is_empty());
    }
}
