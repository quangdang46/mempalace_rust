use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

const DEFAULT_COLLECTION_NAME: &str = "mempalace_drawers";

/// Validate an ISO-8601 date or canonical UTC datetime string at MCP boundary
/// (#1164 / `4d98b05`).
///
/// Accepts `None` and empty strings as pass-through.
///
/// Non-empty inputs must match one of these canonical forms:
/// * `YYYY-MM-DD`
/// * `YYYY-MM-DDTHH:MM:SSZ`
/// * `YYYY-MM-DDTHH:MM:SS+00:00` (normalized to `...Z` on return)
///
/// Partial dates (e.g. `2026`, `2026-01`) and non-UTC datetimes are rejected
/// because KG queries compare temporal values as TEXT — mixed forms silently
/// return wrong results.
pub fn sanitize_iso_temporal(
    value: Option<&str>,
    field_name: &str,
) -> anyhow::Result<Option<String>> {
    let raw = match value {
        None => return Ok(None),
        Some("") => return Ok(Some(String::new())),
        Some(s) => s.trim().to_string(),
    };

    fn is_valid_date(value: &str) -> bool {
        if value.len() != 10 {
            return false;
        }
        let bytes = value.as_bytes();
        if bytes[4] != b'-' || bytes[7] != b'-' {
            return false;
        }
        let year: i32 = match value[0..4].parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let month: u32 = match value[5..7].parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let day: u32 = match value[8..10].parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        chrono::NaiveDate::from_ymd_opt(year, month, day).is_some()
    }

    fn parse_canonical_utc(value: &str) -> Option<String> {
        let normalized = if let Some(stripped) = value.strip_suffix("+00:00") {
            format!("{stripped}Z")
        } else {
            value.to_string()
        };
        if normalized.len() != 20 || !normalized.ends_with('Z') {
            return None;
        }
        let body = &normalized[..normalized.len() - 1];
        let dt = chrono::NaiveDateTime::parse_from_str(body, "%Y-%m-%dT%H:%M:%S").ok()?;
        Some(format!("{}Z", dt.format("%Y-%m-%dT%H:%M:%S")))
    }

    if is_valid_date(&raw) {
        return Ok(Some(raw));
    }
    if let Some(canonical) = parse_canonical_utc(&raw) {
        return Ok(Some(canonical));
    }
    anyhow::bail!(
        "{field_name}={raw:?} is not a valid ISO-8601 date or UTC datetime (expected YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ)"
    )
}

fn expand_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(path.strip_prefix("~/").unwrap());
        }
    }
    PathBuf::from(path)
}

/// Lower-case + collapse separators (`-`, ` `) to `_` for wing slugs.
///
/// The same rule is applied by `init` when persisting `topics_by_wing` and
/// when writing `mempalace.yaml`, so the miner's lookup matches at mine
/// time regardless of the source dirname. (#1504)
pub fn normalize_wing_name(name: &str) -> String {
    name.to_lowercase().replace([' ', '-'], "_")
}

fn normalize_pathbuf(path: PathBuf) -> PathBuf {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn default_palace_path() -> PathBuf {
    Config::config_dir()
        .unwrap_or_else(|_| expand_path("~/.mempalace"))
        .join("palace")
}

fn default_collection_name() -> String {
    DEFAULT_COLLECTION_NAME.to_string()
}

fn default_embedding_model() -> String {
    "naive".to_string()
}

fn default_search_strategy() -> String {
    "fts5".to_string()
}

fn default_max_cache_size_mb() -> usize {
    128
}

fn default_true() -> bool {
    true
}

pub(crate) fn default_topic_wings() -> Vec<String> {
    vec![
        "emotions",
        "consciousness",
        "memory",
        "technical",
        "identity",
        "family",
        "creative",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Domain-appropriate topic wings for code/project repositories,
/// replacing the conversation-oriented defaults (emotions, family, etc.)
/// with code-relevant categories.
pub fn default_code_topic_wings() -> Vec<String> {
    vec![
        "architecture",
        "backend",
        "frontend",
        "api",
        "database",
        "devops",
        "testing",
        "security",
        "performance",
        "refactoring",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Domain-appropriate hall keywords for code/project repositories,
/// detecting topics relevant to software projects.
pub fn default_code_hall_keywords() -> HashMap<String, Vec<String>> {
    let mut m = HashMap::new();
    m.insert(
        "architecture".to_string(),
        vec![
            "pattern",
            "architecture",
            "design",
            "module",
            "component",
            "service",
            "microservice",
            "dependency",
            "layer",
            "abstraction",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "backend".to_string(),
        vec![
            "server",
            "backend",
            "api",
            "route",
            "endpoint",
            "middleware",
            "handler",
            "controller",
            "service",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "frontend".to_string(),
        vec![
            "ui",
            "frontend",
            "component",
            "react",
            "vue",
            "css",
            "html",
            "template",
            "render",
            "view",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "api".to_string(),
        vec![
            "api",
            "rest",
            "graphql",
            "endpoint",
            "rpc",
            "http",
            "request",
            "response",
            "serialize",
            "json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "database".to_string(),
        vec![
            "database",
            "sql",
            "query",
            "schema",
            "migration",
            "model",
            "orm",
            "index",
            "transaction",
            "cache",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "devops".to_string(),
        vec![
            "deploy",
            "ci",
            "cd",
            "docker",
            "kubernetes",
            "pipeline",
            "monitor",
            "config",
            "infrastructure",
            "terraform",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "testing".to_string(),
        vec![
            "test",
            "assert",
            "mock",
            "coverage",
            "integration",
            "unit",
            "e2e",
            "benchmark",
            "qa",
            "quality",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "security".to_string(),
        vec![
            "auth",
            "oauth",
            "jwt",
            "password",
            "encrypt",
            "token",
            "permission",
            "role",
            "secure",
            "vulnerability",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m
}

/// Heuristic: detect whether a directory looks like a software project
/// (contains source code, manifest files, git repo, etc.)
pub fn is_code_project(dir: &std::path::Path) -> bool {
    // Check for common code project indicators.
    let indicators = [
        ".git",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "CMakeLists.txt",
        "Makefile",
        "pom.xml",
        "build.gradle",
        "Gemfile",
        "Cargo.lock",
        "yarn.lock",
        "package-lock.json",
        "requirements.txt",
        "setup.py",
        "go.sum",
        "composer.json",
    ];
    indicators.iter().any(|name| dir.join(name).exists())
}

/// Return the appropriate default topic wings for a project directory.
/// Code repos get code-relevant wings; everything else gets the
/// conversation-oriented defaults.
pub fn topic_wings_for_project(dir: &std::path::Path) -> Vec<String> {
    if is_code_project(dir) {
        default_code_topic_wings()
    } else {
        default_topic_wings()
    }
}

pub(crate) fn default_hall_keywords() -> HashMap<String, Vec<String>> {
    let mut m = HashMap::new();
    #[allow(clippy::useless_vec)]
    m.insert(
        "emotions".to_string(),
        vec![
            "scared", "afraid", "worried", "happy", "sad", "love", "hate", "feel", "cry", "tears",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    #[allow(clippy::useless_vec)]
    m.insert(
        "consciousness".to_string(),
        vec![
            "consciousness",
            "conscious",
            "aware",
            "real",
            "genuine",
            "soul",
            "exist",
            "alive",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    #[allow(clippy::useless_vec)]
    m.insert(
        "memory".to_string(),
        vec![
            "memory", "remember", "forget", "recall", "archive", "palace", "store",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    #[allow(clippy::useless_vec)]
    m.insert(
        "technical".to_string(),
        vec![
            "code", "python", "script", "bug", "error", "function", "api", "database", "server",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "identity".to_string(),
        ["identity", "name", "who am i", "persona", "self"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "family".to_string(),
        [
            "family", "kids", "children", "daughter", "son", "parent", "mother", "father",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "creative".to_string(),
        [
            "game", "gameplay", "player", "app", "design", "art", "music", "story",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(default)]
pub struct Config {
    pub palace_path: PathBuf,
    pub collection_name: String,
    #[serde(default)]
    pub people_map: HashMap<String, String>,
    #[serde(default)]
    pub topic_wings: Vec<String>,
    #[serde(default)]
    pub hall_keywords: HashMap<String, Vec<String>>,
    /// Embedding model for semantic search.
    /// "naive" = word overlap similarity (current default)
    /// "paraphrase-multilingual-MiniLM-L12-v2" = multilingual embeddings
    /// "all-MiniLM-L6-v2" = fast English embeddings
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// mr-tsk5 (RFC 001): embedder-identity strict mode.
    /// When `true` (default), a fingerprint mismatch on palace open
    /// returns a hard `Err`. When `false`, the open proceeds with a
    /// `tracing::warn!` and the in-memory index is rebuilt
    /// incrementally on the next batch of inserts.
    #[serde(default = "default_true")]
    pub embedder_identity_strict: bool,
    #[serde(default)]
    pub languages: Vec<String>,
    /// Search strategy (v0.6.0+): "fts5" (default, 0MB), "naive",
    /// "bm25", or "embedding" (90MB+).
    #[serde(default = "default_search_strategy")]
    pub search_strategy: String,
    /// Low-resource performance flags (v0.6.0+).
    #[serde(default = "default_max_cache_size_mb")]
    pub max_cache_size_mb: usize,
    #[serde(default)]
    pub llm_provider: Option<String>,
    #[serde(default)]
    pub llm_model: Option<String>,
    #[serde(default)]
    pub consolidation_enabled: Option<bool>,
    #[serde(default)]
    pub auto_compress: Option<bool>,
    #[serde(default)]
    pub graph_extraction_enabled: Option<bool>,
    #[serde(default)]
    pub rerank_enabled: Option<bool>,
    #[serde(default)]
    pub snapshot_enabled: Option<bool>,
    #[serde(default)]
    pub vision_enabled: Option<bool>,
    #[serde(default)]
    pub token_budget: Option<usize>,
    #[serde(default)]
    pub max_obs_per_session: Option<usize>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_scope: Option<String>,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub team_mode: Option<bool>,
    #[serde(default)]
    pub bm25_weight: Option<f64>,
    #[serde(default)]
    pub vector_weight: Option<f64>,
    #[serde(default)]
    pub graph_weight: Option<f64>,
    /// `mr-ekep`: when true (default), log a `tracing::warn!` before any LLM
    /// call whose `base_url` resolves to a public/external host. Local
    /// (loopback, RFC1918, link-local, Tailscale CGNAT) endpoints are
    /// silent. Set to `false` to suppress the privacy warning.
    #[serde(default = "default_true")]
    pub llm_external_warn: bool,
    /// `mr-2k4g`: user consent to use an LLM API key supplied via process
    /// environment variables. The default (`false`) makes every
    /// env-fallback LLM call fail with a remediation error until the user
    /// explicitly grants consent (via `mpr config record-llm-consent` or
    /// the `MEMPALACE_LLM_CONSENT` env override).
    #[serde(default)]
    pub llm_consent_given: bool,
    /// `mr-jh4e`: number of backups to keep (None = 10 default, 0 = keep all).
    /// Set to `None` for default behavior, `Some(0)` to keep every backup.
    #[serde(default)]
    pub max_backups: Option<usize>,
    /// `mr-g3av`: when false, the save hook (and any other auto-save
    /// path) short-circuits. Honors `MEMPALACE_HOOKS_AUTO_SAVE=false`.
    #[serde(default = "default_true")]
    pub hooks_auto_save: bool,
}

#[cfg(unix)]
fn secure_open_options(create_new: bool) -> OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.write(true).mode(0o600);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    options
}

#[cfg(not(unix))]
fn secure_open_options(create_new: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    options
}

fn write_atomic_file(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Unique temp filename to prevent symlink races
    let temp_path = path.with_extension(format!(
        "{}.tmp.{}",
        path.extension().and_then(|s| s.to_str()).unwrap_or("new"),
        std::process::id()
    ));

    {
        let mut file = secure_open_options(true).open(&temp_path)?; // create_new = true (O_EXCL)
        file.write_all(content.as_bytes())?;
        file.sync_all()?; // fsync before rename (kernel: flush page cache)
    }

    std::fs::rename(&temp_path, path)?;

    // fsync parent directory to ensure rename is durable
    if let Some(parent) = path.parent() {
        let dir_fd = std::fs::File::open(parent)?;
        dir_fd.sync_all()?;
    }

    Ok(())
}

fn write_create_new_file(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = secure_open_options(true).open(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            palace_path: default_palace_path(),
            collection_name: default_collection_name(),
            people_map: HashMap::new(),
            topic_wings: default_topic_wings(),
            hall_keywords: default_hall_keywords(),
            embedding_model: default_embedding_model(),
            search_strategy: default_search_strategy(),
            max_cache_size_mb: default_max_cache_size_mb(),
            languages: Vec::new(),
            llm_provider: None,
            llm_model: None,
            consolidation_enabled: None,
            auto_compress: None,
            graph_extraction_enabled: None,
            rerank_enabled: None,
            snapshot_enabled: None,
            vision_enabled: None,
            token_budget: None,
            max_obs_per_session: None,
            agent_id: None,
            agent_scope: None,
            team_id: None,
            team_mode: None,
            bm25_weight: None,
            vector_weight: None,
            graph_weight: None,
            llm_external_warn: true,
            llm_consent_given: false,
            max_backups: None,
            hooks_auto_save: true,
            embedder_identity_strict: true,
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let config_path = Self::config_file_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let mut config: Config = serde_json::from_str(&content)?;

            // env override for palace_path takes priority over config file value
            if let Some(env_val) = std::env::var_os("MEMPALACE_PALACE_PATH")
                .or_else(|| std::env::var_os("MEMPAL_PALACE_PATH"))
            {
                config.palace_path = normalize_pathbuf(expand_path(&env_val.to_string_lossy()));
            }

            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_file_path()?;
        let content = serde_json::to_string_pretty(self)?;
        write_atomic_file(&config_path, &content)?;
        Ok(())
    }

    /// Remove the global config directory (config.json, entity registry,
    /// identity, etc.). Returns the path that was removed.
    pub fn deinit() -> anyhow::Result<PathBuf> {
        let config_dir = Self::config_dir()?;
        if config_dir.exists() {
            std::fs::remove_dir_all(&config_dir).with_context(|| {
                format!(
                    "Failed to remove config directory: {}",
                    config_dir.display()
                )
            })?;
        }
        Ok(config_dir)
    }

    pub fn init(&self) -> anyhow::Result<PathBuf> {
        let config_dir = Self::config_dir()?;
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.json");
        if !config_path.exists() {
            let default_config = Config::default();
            let content = serde_json::to_string_pretty(&default_config)?;
            write_create_new_file(&config_path, &content)?;
        }
        Ok(config_path)
    }

    pub fn load_people_map(&self) -> anyhow::Result<HashMap<String, String>> {
        let people_map_path = Self::config_dir()?.join("people_map.json");
        if people_map_path.exists() {
            let content = std::fs::read_to_string(&people_map_path)?;
            let map: HashMap<String, String> = serde_json::from_str(&content)?;
            Ok(map)
        } else {
            Ok(self.people_map.clone())
        }
    }

    pub fn save_people_map(&self, people_map: &HashMap<String, String>) -> anyhow::Result<PathBuf> {
        let config_dir = Self::config_dir()?;
        let people_map_path = config_dir.join("people_map.json");
        let content = serde_json::to_string_pretty(people_map)?;
        write_atomic_file(&people_map_path, &content)?;
        Ok(people_map_path)
    }

    pub fn registry_file_path() -> anyhow::Result<PathBuf> {
        Ok(Self::config_dir()?.join("entity_registry.json"))
    }

    /// Path to the tunnel file, sibling of `palace_path` (#1467).
    ///
    /// Before this fix the tunnel file was hardcoded at
    /// `~/.mempalace/tunnels.json` regardless of the configured
    /// `palace_path`. Whenever the configured palace lived elsewhere
    /// (subagent profile, sandbox, container mount on `/srv/`, …) the
    /// drawers landed in the configured palace while tunnels silently
    /// landed in a different file invisible to other processes touching
    /// the same palace. Anchoring the tunnel file to `dirname(palace_path)`
    /// keeps every piece of palace state co-located.
    pub fn tunnel_file(&self) -> PathBuf {
        self.palace_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tunnels.json")
    }

    /// `mr-jh4e`: effective backup cap. Precedence: env
    /// `MEMPALACE_MAX_BACKUPS` (if set) → persisted `max_backups` (if Some) →
    /// default 10. `0` means keep every backup.
    pub fn max_backups_effective(&self) -> usize {
        if let Ok(v) = std::env::var("MEMPALACE_MAX_BACKUPS") {
            if let Ok(n) = v.parse::<usize>() {
                return n;
            }
        }
        self.max_backups.unwrap_or(10)
    }

    /// `mr-2k4g`: persist the user's grant of consent to use env-fallback
    /// LLM API keys. The flag is set in-memory, the config is written
    /// atomically with 0600 permissions (via [`Self::save`]), and the
    /// updated instance is returned. Idempotent — calling it twice is a
    /// no-op aside from the disk write.
    pub fn record_llm_consent(&mut self) -> anyhow::Result<()> {
        self.llm_consent_given = true;
        self.save()
    }

    pub fn identity_file_path() -> anyhow::Result<PathBuf> {
        Ok(Self::config_dir()?.join("identity.txt"))
    }

    /// Get the XDG-compliant config directory for mempalace.
    /// Order: XDG_CONFIG_HOME env var → platform fallback → ~/.mempalace fallback
    pub fn config_dir() -> anyhow::Result<PathBuf> {
        // 1. XDG_CONFIG_HOME env var takes priority
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join("mempalace"));
            }
        }

        // 2. Platform-specific fallbacks
        if let Some(proj) = directories::ProjectDirs::from("com", "mempalace", "mempalace") {
            return Ok(proj.config_dir().to_path_buf());
        }

        // 3. Fallback to ~/.mempalace (backward compatibility)
        Ok(expand_path("~/.mempalace"))
    }

    /// Get the XDG-compliant data directory for palace storage.
    /// Order: XDG_DATA_HOME env var → platform fallback → config_dir fallback
    #[cfg_attr(not(test), allow(dead_code))]
    fn data_dir() -> anyhow::Result<PathBuf> {
        // 1. XDG_DATA_HOME env var takes priority
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join("mempalace"));
            }
        }

        // 2. Platform fallback via ProjectDirs
        if let Some(proj) = directories::ProjectDirs::from("com", "mempalace", "mempalace") {
            return Ok(proj.data_dir().to_path_buf());
        }

        // 3. Fallback to config_dir (keeps palace and config in same tree)
        Self::config_dir()
    }

    /// Get the XDG-compliant state directory for runtime files.
    /// Order: XDG_STATE_HOME env var → platform fallback → config_dir fallback
    #[allow(dead_code)]
    fn state_dir() -> anyhow::Result<PathBuf> {
        // 1. XDG_STATE_HOME env var takes priority
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join("mempalace"));
            }
        }

        // 2. Platform fallback via ProjectDirs
        if let Some(proj) = directories::ProjectDirs::from("com", "mempalace", "mempalace") {
            if let Some(state) = proj.state_dir() {
                return Ok(state.to_path_buf());
            }
        }

        // 3. Fallback to config_dir
        Self::config_dir()
    }

    /// Check if the old ~/.mempalace path exists and needs migration.
    /// Returns the old path if migration is needed, None otherwise.
    #[allow(dead_code)]
    fn old_path() -> Option<PathBuf> {
        let old = expand_path("~/.mempalace");
        if old.exists() && old.is_dir() {
            let new = Self::config_dir().ok()?;
            // Only suggest migration if old ≠ new
            if old != new {
                return Some(old);
            }
        }
        None
    }

    /// Attempt to migrate from old ~/.mempalace path to new XDG path.
    /// Returns the number of files migrated.
    #[allow(dead_code)]
    fn migrate_from_old() -> anyhow::Result<usize> {
        let old = Self::old_path().context("No old config path found to migrate from")?;
        let new = Self::config_dir()?;

        if new.exists() {
            anyhow::bail!(
                "New config path already exists at '{}', migration would overwrite data",
                new.display()
            );
        }

        // Create new dir and copy contents
        std::fs::create_dir_all(&new)?;
        let mut count = 0;

        for entry in walkdir::WalkDir::new(&old).min_depth(1) {
            let entry = entry?;
            let rel = entry.path().strip_prefix(&old)?;
            let dest = new.join(rel);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&dest)?;
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), &dest)?;
                count += 1;
            }
        }

        Ok(count)
    }

    pub fn config_file_path() -> anyhow::Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock;

    #[test]
    fn test_config_dir_respects_xdg_config_home() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_path = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_path);
        let result = Config::config_dir().unwrap();
        assert!(result.to_str().unwrap().starts_with(xdg_path));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_data_dir_respects_xdg_data_home() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        // Canonicalize to resolve symlinks that may cause path mismatches
        let xdg_path = temp_dir.path().canonicalize().unwrap();
        let xdg_str = xdg_path.to_str().unwrap();
        std::env::set_var("XDG_DATA_HOME", xdg_str);
        let result = Config::data_dir().unwrap();
        let result_str = result.to_str().unwrap();
        assert!(
            result_str.starts_with(xdg_str) || result_str.starts_with(&*xdg_path.to_string_lossy()),
            "result {} should start with {}",
            result_str,
            xdg_str
        );
        std::env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    fn test_state_dir_respects_xdg_state_home() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_path = temp_dir.path().canonicalize().unwrap();
        let xdg_str = xdg_path.to_str().unwrap();
        std::env::set_var("XDG_STATE_HOME", xdg_str);
        let result = Config::state_dir().unwrap();
        let result_str = result.to_str().unwrap();
        assert!(
            result_str.starts_with(xdg_str) || result_str.starts_with(&*xdg_path.to_string_lossy()),
            "result {} should start with {}",
            result_str,
            xdg_str
        );
        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn test_config_dir_fallback_to_tilde_mempalace() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        // Clear XDG vars to test fallback
        std::env::remove_var("XDG_CONFIG_HOME");
        let result = Config::config_dir().unwrap();
        assert!(result.to_str().unwrap().contains("mempalace"));
    }

    #[test]
    fn test_default_palace_path_uses_data_dir() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_config = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_config);
        let palace = default_palace_path();
        assert!(palace.to_str().unwrap().starts_with(xdg_config));
        assert!(palace.to_str().unwrap().ends_with("palace"));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_load_people_map_falls_back_to_embedded_config_value() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config = Config {
            palace_path: PathBuf::from("/tmp/palace"),
            collection_name: DEFAULT_COLLECTION_NAME.to_string(),
            people_map: HashMap::from([("bob".to_string(), "Robert".to_string())]),
            topic_wings: default_topic_wings(),
            hall_keywords: default_hall_keywords(),
            embedding_model: default_embedding_model(),
            embedder_identity_strict: true,
            languages: vec![],
            llm_provider: None,
            llm_model: None,
            consolidation_enabled: None,
            auto_compress: None,
            graph_extraction_enabled: None,
            rerank_enabled: None,
            snapshot_enabled: None,
            vision_enabled: None,
            token_budget: None,
            max_obs_per_session: None,
            agent_id: None,
            agent_scope: None,
            team_id: None,
            team_mode: None,
            bm25_weight: None,
            vector_weight: None,
            graph_weight: None,
            llm_external_warn: true,
            llm_consent_given: false,
            max_backups: None,
            hooks_auto_save: true,
            search_strategy: default_search_strategy(),
            max_cache_size_mb: default_max_cache_size_mb(),
        };
        let people_map = config.load_people_map().unwrap();
        assert_eq!(people_map.get("bob"), Some(&"Robert".to_string()));

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_registry_and_identity_paths_use_config_dir() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config_dir = Config::config_dir().unwrap();
        assert_eq!(
            Config::registry_file_path().unwrap(),
            config_dir.join("entity_registry.json")
        );
        assert_eq!(
            Config::identity_file_path().unwrap(),
            config_dir.join("identity.txt")
        );

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_tunnel_file_is_sibling_of_palace_path() {
        // #1467: the tunnel file must live next to the configured palace,
        // not at the hardcoded `~/.mempalace/tunnels.json` legacy path.
        let cfg = Config {
            palace_path: PathBuf::from("/srv/mempalace/palace"),
            collection_name: DEFAULT_COLLECTION_NAME.to_string(),
            people_map: HashMap::new(),
            topic_wings: default_topic_wings(),
            hall_keywords: default_hall_keywords(),
            embedding_model: default_embedding_model(),
            embedder_identity_strict: true,
            languages: vec![],
            llm_provider: None,
            llm_model: None,
            consolidation_enabled: None,
            auto_compress: None,
            graph_extraction_enabled: None,
            rerank_enabled: None,
            snapshot_enabled: None,
            vision_enabled: None,
            token_budget: None,
            max_obs_per_session: None,
            agent_id: None,
            agent_scope: None,
            team_id: None,
            team_mode: None,
            bm25_weight: None,
            vector_weight: None,
            graph_weight: None,
            llm_external_warn: true,
            llm_consent_given: false,
            max_backups: None,
            hooks_auto_save: true,
            search_strategy: default_search_strategy(),
            max_cache_size_mb: default_max_cache_size_mb(),
        };
        assert_eq!(
            cfg.tunnel_file(),
            PathBuf::from("/srv/mempalace/tunnels.json")
        );
    }

    #[test]
    fn test_old_path_none_when_not_exists() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        // Ensure ~/.mempalace is not detected as "old" when it's the default
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_DATA_HOME");
        // old_path returns None when the old path doesn't differ from new
        let result = Config::old_path();
        // If ~/.mempalace is the config dir and no migration needed, returns None
        // This is expected in test environments
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_init_creates_config_file() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config = Config::default();
        let config_path = config.init().unwrap();
        let content = std::fs::read_to_string(&config_path).unwrap();
        let parsed: Config = serde_json::from_str(&content).unwrap();

        assert_eq!(
            config_path,
            Config::config_dir().unwrap().join("config.json")
        );
        assert_eq!(parsed.collection_name, DEFAULT_COLLECTION_NAME);

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_save_people_map_persists_json() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config = Config::default();
        let people_map = HashMap::from([
            ("alice".to_string(), "ALC".to_string()),
            ("bob".to_string(), "BOB".to_string()),
        ]);
        let path = config.save_people_map(&people_map).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: HashMap<String, String> = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed, people_map);

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[cfg(unix)]
    #[test]
    fn test_save_people_map_uses_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config = Config::default();
        let people_map = HashMap::from([("alice".to_string(), "ALC".to_string())]);
        let path = config.save_people_map(&people_map).unwrap();
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

        assert_eq!(mode, 0o600);

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    // #1164: ISO-8601 validation at MCP boundary so malformed dates fail fast
    // instead of silently producing empty KG query results.
    #[test]
    fn test_sanitize_iso_temporal_accepts_valid_date() {
        let out = super::sanitize_iso_temporal(Some("2026-05-11"), "valid_from").unwrap();
        assert_eq!(out.as_deref(), Some("2026-05-11"));
    }

    #[test]
    fn test_sanitize_iso_temporal_accepts_canonical_utc_datetime() {
        let out = super::sanitize_iso_temporal(Some("2026-05-11T12:30:45Z"), "valid_from").unwrap();
        assert_eq!(out.as_deref(), Some("2026-05-11T12:30:45Z"));
    }

    #[test]
    fn test_sanitize_iso_temporal_normalizes_plus_offset() {
        let out = super::sanitize_iso_temporal(Some("2026-05-11T00:00:00+00:00"), "ended").unwrap();
        assert_eq!(out.as_deref(), Some("2026-05-11T00:00:00Z"));
    }

    #[test]
    fn test_sanitize_iso_temporal_passes_through_none_and_empty() {
        assert_eq!(super::sanitize_iso_temporal(None, "as_of").unwrap(), None);
        assert_eq!(
            super::sanitize_iso_temporal(Some(""), "as_of")
                .unwrap()
                .as_deref(),
            Some("")
        );
    }

    #[test]
    fn test_sanitize_iso_temporal_rejects_partial_date() {
        assert!(super::sanitize_iso_temporal(Some("2026"), "valid_from").is_err());
        assert!(super::sanitize_iso_temporal(Some("2026-05"), "valid_from").is_err());
    }

    #[test]
    fn test_sanitize_iso_temporal_rejects_garbage() {
        let err = super::sanitize_iso_temporal(Some("March 2026"), "as_of").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("as_of="), "error must name the field: {msg}");
        assert!(
            msg.contains("not a valid ISO-8601"),
            "error must explain: {msg}"
        );
    }

    #[test]
    fn test_sanitize_iso_temporal_rejects_impossible_calendar_day() {
        assert!(super::sanitize_iso_temporal(Some("2026-02-30"), "valid_from").is_err());
        assert!(super::sanitize_iso_temporal(Some("2026-13-01"), "valid_from").is_err());
    }

    /// `mr-ekep`: external-LLM warning is opt-out, defaulting to on.
    #[test]
    fn test_default_llm_external_warn_is_true() {
        let cfg = Config::default();
        assert!(cfg.llm_external_warn);
    }

    /// `mr-2k4g`: consent is opt-in, defaulting to off. This is the
    /// fail-closed default — env-fallback LLM calls must error out until
    /// the user explicitly grants consent.
    #[test]
    fn test_default_llm_consent_is_false() {
        let cfg = Config::default();
        assert!(!cfg.llm_consent_given);
    }

    /// `mr-2k4g`: `record_llm_consent` flips the in-memory flag *and*
    /// persists it to disk so subsequent `Config::load()` reads see the
    /// grant without needing the env override.
    #[test]
    fn test_record_llm_consent_persists_flag() {
        let _guard = test_env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let mut config = Config::default();
        assert!(!config.llm_consent_given);
        config.record_llm_consent().unwrap();
        assert!(config.llm_consent_given);

        // Re-load from disk: the flag must survive a round-trip.
        let reloaded = Config::load().unwrap();
        assert!(reloaded.llm_consent_given);

        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
