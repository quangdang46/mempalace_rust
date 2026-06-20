//! Notes module — AGENT.md and USER.md storage.
//!
//! Pattern from hermes-agent MEMORY.md / USER.md but standalone.
//! Human-readable markdown files, version-controllable, no DB dependency.
//!
//! Storage location: `<home>/notes/AGENT.md` and `<home>/notes/USER.md`
//! where `<home>` is typically `~/.mempalace/notes/`.
//!
//! API:
//! - `Notes::new(dir)` — creates dir + empty files with headers if missing
//! - `Notes::remember(text)` — append entry with ISO timestamp to AGENT.md
//! - `Notes::set_user(key, value)` — set key=value in USER.md (format: "key: value")
//! - `Notes::recall()` — return both files content
//! - `Notes::count()` — return entry counts

use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

const AGENT_HEADER: &str = "# MemPalace Agent Notes\n\nPersonal notes the agent makes about its environment, conventions, and learnings.\nEach entry is appended with an ISO-8601 timestamp.\n\n";

const USER_HEADER: &str = "# MemPalace User Profile\n\nUser-specific settings stored as `key: value` lines.\nUpdate via `mpr user set <key> <value>`.\n\n";

pub struct Notes {
    agent_path: PathBuf,
    user_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct NotesContent {
    /// Full content of AGENT.md
    pub agent: String,
    /// Full content of USER.md
    pub user: String,
    /// Number of entries in AGENT.md (count of bullet lines)
    pub agent_entries: usize,
    /// Number of key:value lines in USER.md
    pub user_entries: usize,
}

impl Notes {
    /// Create or open notes in the given directory.
    /// Creates AGENT.md and USER.md with headers if they don't exist.
    pub fn new(notes_dir: &Path) -> Result<Self> {
        fs::create_dir_all(notes_dir)
            .with_context(|| format!("create notes dir {}", notes_dir.display()))?;
        let agent_path = notes_dir.join("AGENT.md");
        let user_path = notes_dir.join("USER.md");
        if !agent_path.exists() {
            fs::write(&agent_path, AGENT_HEADER)
                .with_context(|| format!("write {}", agent_path.display()))?;
        }
        if !user_path.exists() {
            fs::write(&user_path, USER_HEADER)
                .with_context(|| format!("write {}", user_path.display()))?;
        }
        Ok(Self {
            agent_path,
            user_path,
        })
    }

    /// Path to the notes directory (for `Notes::new`).
    pub fn dir(&self) -> &Path {
        self.agent_path.parent().unwrap_or(Path::new("."))
    }

    /// Append a new entry to AGENT.md with current ISO-8601 timestamp.
    pub fn remember(&self, entry: &str) -> Result<()> {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            anyhow::bail!("cannot remember empty entry");
        }
        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let line = format!("\n- `{}` {}\n", timestamp, trimmed);
        let mut content = fs::read_to_string(&self.agent_path)
            .with_context(|| format!("read {}", self.agent_path.display()))?;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&line);
        fs::write(&self.agent_path, content)
            .with_context(|| format!("write {}", self.agent_path.display()))?;
        Ok(())
    }

    /// Set a key=value entry in USER.md. Replaces existing entry with same key.
    pub fn set_user(&self, key: &str, value: &str) -> Result<()> {
        if key.is_empty() {
            anyhow::bail!("user key cannot be empty");
        }
        let content = fs::read_to_string(&self.user_path)
            .with_context(|| format!("read {}", self.user_path.display()))?;
        let prefix = format!("{}:", key);
        let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        // Remove existing key
        lines.retain(|l| !l.trim_start().starts_with(&prefix));
        // Append new key
        lines.push(format!("{}: {}", key, value));
        let new_content = lines.join("\n") + "\n";
        fs::write(&self.user_path, new_content)
            .with_context(|| format!("write {}", self.user_path.display()))?;
        Ok(())
    }

    /// Read both files. Counts entries as bullet lines (AGENT.md) and
    /// key:value lines (USER.md) past the header.
    pub fn recall(&self) -> Result<NotesContent> {
        let agent = fs::read_to_string(&self.agent_path).unwrap_or_default();
        let user = fs::read_to_string(&self.user_path).unwrap_or_default();
        let agent_entries = agent
            .lines()
            .filter(|l| l.trim_start().starts_with("- `"))
            .count();
        let user_entries = user
            .lines()
            .filter(|l| {
                let trimmed = l.trim();
                !trimmed.is_empty()
                    && !trimmed.starts_with('#')
                    && !trimmed.starts_with("Update via")
                    && trimmed.contains(':')
            })
            .count();
        Ok(NotesContent {
            agent,
            user,
            agent_entries,
            user_entries,
        })
    }

    /// Get a user value by key. Returns None if not found.
    pub fn get_user(&self, key: &str) -> Result<Option<String>> {
        let content = fs::read_to_string(&self.user_path)
            .with_context(|| format!("read {}", self.user_path.display()))?;
        let prefix = format!("{}:", key);
        for line in content.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix(&prefix) {
                return Ok(Some(rest.trim().to_string()));
            }
        }
        Ok(None)
    }

    /// Count entries.
    pub fn count(&self) -> Result<NotesContent> {
        self.recall().map(|c| NotesContent {
            agent: String::new(),
            user: String::new(),
            ..c
        })
    }
}
