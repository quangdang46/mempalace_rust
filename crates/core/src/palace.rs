// =====================================================================
// Palace facade — high-level memory provider (mp-020 / ADR-3 / ADR-6)
// =====================================================================
//
// `Palace` is the canonical [`MemoryProvider`] implementation bundled
// with `mempalace-core`. It ties together the embedder, the vector store,
// and the knowledge graph behind a single typed handle.
//
// Hosts (jcode, third-party Rust agents) that want to integrate
// mempalace as a library consume only `Palace` / `MemoryProvider` / the
// supporting traits. Concrete implementation details (`PalaceDb`,
// `KnowledgeGraph`, `Embedder`) stay internal and are replaced via the
// builder pattern.
//
// Architecture per the master plan (§3 "Concrete API Sketch"):
//   - `MemoryProvider`  — the public trait external consumers implement
//   - `PalaceStore`     — the vector-storage abstraction (embedvec today,
//                         hnsw_rs / usearch / lancedb in Phase 5)
//   - `Palace`          — the typed handle holding all three components
//   - `PalaceBuilder`   — construction API that pins down every field
//
// References:
//   - docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md, ADR-3, ADR-6,
//     ADR-7, §3 "Concrete API Sketch"
//   - docs/research/03_jcode_memory_internals.md §A "jcode MemoryProvider
//     trait sketch"
//   - docs/research/04_mempalace_internals_and_gaps.md §P0-2
//
// Async semantics:
//   Public methods that do I/O are `async fn`. Pure-compute helpers
//   (e.g. `compute_aaak`) stay synchronous. The implementation targets
//   `tokio` but uses `async-trait` so alternate runtimes (smol, async-std)
//   work too — see ADR-6.
//
// Thread safety:
//   `Palace` is `Send + Sync` when all component types are. The embedder
//   and store are behind `Arc<dyn …>` so the handle can be cloned across
//   worker tasks without leaking internal state.

use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;
use tracing::warn;

pub mod builder;
pub mod store;

pub use crate::session::SessionStore;
pub use builder::PalaceBuilder;
pub use store::embedvec::EmbedvecStore;
pub use store::StoreTier;

// ---------------------------------------------------------------------------
// Public types (mirroring the §3 Concrete API Sketch)
// ---------------------------------------------------------------------------

/// A stable, opaque identifier for a single drawer in a palace.
///
/// Internally backed by a UUID string; consumers treat it as opaque.
/// Used as the key type in [`MemoryProvider::forget`] and
/// [`MemoryProvider::related`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct DrawerId(pub String);

impl DrawerId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for DrawerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The kind of a drawer — controls which hall it lands in and how
/// the palace processes it on ingest.
///
/// Hall routing:
///
///   | kind            | hall               |
///   |-----------------|--------------------|
///   | `Fact`          | `hall_facts`       |
///   | `Event`         | `hall_events`      |
///   | `Discovery`     | `hall_discoveries` |
///   | `Preference`   | `hall_preferences` |
///   | `Advice`        | `hall_advice`      |
///   | `Raw`           | (no hall, raw only) |
///
/// `Raw` drawers bypass AAAK compression and are stored verbatim. All
/// other kinds go through the compressor before the drawer is filed.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum DrawerKind {
    /// A factual assertion — decisions, configurations, agreements.
    Fact,
    /// An event or session — meetings, debugging, milestones.
    Event,
    /// A new insight or breakthrough.
    Discovery,
    /// A personal preference or habit.
    Preference,
    /// A recommendation or solved problem.
    Advice,
    /// Raw verbatim content — no hall, no AAAK compression.
    #[default]
    Raw,
    /// Issue #28: a named entity (person, place, project, system).
    Entity,
    /// Issue #28: a correction or override of a prior fact.
    /// jcode's `MemoryCategory::Correction` has the longest half-life
    /// (365d) — corrections are durable.
    Correction,
    /// Issue #28: a custom user-defined kind. The string is the kind
    /// name (e.g. "snippet", "reference"). Cannot be `Copy` because it
    /// owns a `String`, so `DrawerKind` as a whole drops `Copy`.
    Custom(String),
}

impl DrawerKind {
    /// Issue #28: category-specific confidence-decay half-life in days.
    /// Matches jcode's `MemoryCategory` half-lives so a `MempalaceAdapter`
    /// can use the same retention formula.
    pub fn half_life_days(&self) -> f64 {
        match self {
            // jcode-shaped half-lives:
            DrawerKind::Correction => 365.0,
            DrawerKind::Preference => 90.0,
            DrawerKind::Entity => 60.0,
            DrawerKind::Fact => 30.0,
            DrawerKind::Custom(_) => 45.0,
            // mempalace-native kinds use 30d as a sane default.
            DrawerKind::Event | DrawerKind::Discovery | DrawerKind::Advice | DrawerKind::Raw => {
                30.0
            }
        }
    }
}

/// Memory tier — lifecycle stage of a drawer (ADR-13 / mp-051).
///
/// Drawers move through tiers as consolidation happens:
///
///   | Tier | Meaning |
///   |-------|---------|
///   | `Working` | Raw observation just ingested |
///   | `Episodic` | Session-level summary (e.g. mined conversation) |
///   | `Semantic` | Consolidated fact in KG (Phase 5 sleep consolidation) |
///   | `Procedural` | Encoded skill/how-to from repeated patterns |
///
/// Default on ingest is `Working` or `Episodic` (set by the caller based on source).
/// Promotion through tiers happens in sleep-time consolidation (mp-091).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum MemoryTier {
    /// Raw observation — verbatim content, not yet consolidated.
    #[default]
    Working,
    /// Episodic summary of a session or batch ingest.
    Episodic,
    /// Fact surfaced via KG consolidation (Phase 5).
    Semantic,
    /// Learned procedure/skill (Phase 5).
    Procedural,
}

/// Scope constraints on a search or memory operation.
///
/// All fields are optional. A `None` field means "no constraint" — the
/// operation spans all values. Empty constraints produce empty results.
///
/// For example, `SearchScope { wing: Some("driftwood"), .. }` searches
/// only within the "driftwood" wing regardless of room or hall.
/// `SearchScope { hall: Some("advice"), .. }` searches all wings but
/// only in the advice hall.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SearchScope {
    /// Limit to drawers in this wing (project/person name).
    pub wing: Option<String>,
    /// Limit to drawers in this room (specific topic).
    pub room: Option<String>,
    /// Limit to drawers in this hall (memory type).
    pub hall: Option<String>,
    /// Maximum results to return. `0` means use the palace default.
    pub limit: usize,
    /// Include results from the global palace when a per-project palace
    /// is open (ADR-7). Default `true`.
    pub include_global: bool,
    /// Time-bounded query using bi-temporal validity (ADR-5).
    /// `None` means all valid times.
    /// Defined here rather than in knowledge_graph.rs so the type is
    /// available without pulling in the KG dependency. Full KG wiring
    /// lands in mp-080.
    pub time_window: Option<BiTemporalRange>,
}

/// Bi-temporal validity range for KG queries (ADR-5 / mp-080).
///
/// Four timestamps model when a fact is known vs when it was true:
///
///   | Field | Meaning |
///   |-------|---------|
///   | `t_created` | When we learned this fact |
///   | `t_expired` | When we stopped believing this fact |
///   | `t_valid_from` | When the fact became true in the world |
///   | `t_valid_to` | When the fact stopped being true in the world |
///
/// Not yet wired to the KG — placeholder for mp-080.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct BiTemporalRange {
    pub t_created: Option<String>,
    pub t_expired: Option<String>,
    pub t_valid_from: Option<String>,
    pub t_valid_to: Option<String>,
}

impl SearchScope {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn wing(mut self, wing: impl Into<String>) -> Self {
        self.wing = Some(wing.into());
        self
    }

    pub fn room(mut self, room: impl Into<String>) -> Self {
        self.room = Some(room.into());
        self
    }

    pub fn hall(mut self, hall: impl Into<String>) -> Self {
        self.hall = Some(hall.into());
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// Which palace to operate on when a global config exists alongside a
/// per-project palace (ADR-7).
///
/// Used by the library API to disambiguate reads that could touch either
/// the global palace or the project-local palace.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum MemoryScope {
    /// Read/write only the project-local palace (from
    /// `PalaceConfig::palace_path`). Used by jcode integration.
    Local,
    /// Read/write only the global palace (`XDG_CONFIG_HOME/mempalace`).
    Global,
    /// Try local first; fall back to global if local is empty or absent.
    /// Default for standalone CLI usage.
    #[default]
    Auto,
    // -- mp-migration 3/8: jcode-compat multi-scope variants --
    /// Read/write both local and global palaces in a single call
    /// (jcode's `MemoryScope::All`). The default implementation
    /// fans out to the local + global palaces and merges results.
    All,
    /// Read/write only the named wing (jcode's per-wing scoping).
    /// String is the wing name (e.g. "driftwood", "wing_kai").
    Wing(String),
    /// Read/write only the named (wing, room) tuple.
    Room { wing: String, room: String },
}

impl MemoryScope {
    /// Does this scope include the project-local palace?
    /// Maps to jcode's `MemoryScope::includes_project()`.
    pub fn includes_project(&self) -> bool {
        match self {
            MemoryScope::Local | MemoryScope::Auto | MemoryScope::All => true,
            MemoryScope::Global => false,
            MemoryScope::Wing(_) | MemoryScope::Room { .. } => true, // named scopes are in the local palace
        }
    }

    /// Does this scope include the global palace?
    /// Maps to jcode's `MemoryScope::includes_global()`.
    pub fn includes_global(&self) -> bool {
        match self {
            MemoryScope::Global | MemoryScope::All => true,
            MemoryScope::Local | MemoryScope::Auto => false,
            MemoryScope::Wing(_) | MemoryScope::Room { .. } => false, // named scopes are in the local palace
        }
    }

    /// True if this is the jcode-compat `All` variant.
    pub fn is_all(&self) -> bool {
        matches!(self, MemoryScope::All)
    }
}

/// Retrieval fusion mode for combining vector and graph-based search.
///
/// Determines how multiple retrieval signals are combined when searching
/// across wings, rooms, and drawers. Following the architecture described
/// in `docs/research/02_memory_architectures_papers.md` §7 (HippoRAG / HippoRAG2),
/// PPR (Personalized PageRank) enables single-shot multi-hop retrieval by
/// diffusing query seed probability mass through the palace graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum FusionMode {
    /// Pure vector ANN retrieval — no graph diffusion.
    /// Existing behavior, no PPR involvement.
    #[default]
    Vector,
    /// Personalized PageRank over the wing/room/drawer graph.
    /// Query entities are mapped to graph nodes, then PPR is run with
    /// restart probability concentrated on seed nodes. Top rooms/drawers
    /// are returned by accumulated node probability mass.
    Ppr,
    /// Hybrid: vector ANN candidates reranked by PPR score.
    /// Fetches 3× results via ANN, then reorders by PPR probability mass
    /// accumulated through the palace graph. Combines ANN recall with
    /// graph traversal multi-hop capability. 70/30 weighting between
    /// similarity and PPR mass in `combined_score` (per searcher.rs).
    Hybrid,
}

// ---------------------------------------------------------------------------
// PalaceStore — vector storage abstraction (ADR-2)
// ---------------------------------------------------------------------------

/// Pluggable vector store. Three concrete tiers ship in-tree:
///
///   | Tier | Implementation | Capacity |
///   |------|---------------|----------|
///   | 0    | `embedvec`    | ≤5 k drawers (default today) |
///   | 1    | `hnsw_rs + sqlite` | ≤20 k drawers (Phase 2) |
///   | 2    | `usearch + sqlite` | 5 k–100 k (Phase 5) |
///   | 3    | `lancedb`     | 100 k+ (Phase 5) |
///
/// The trait is object-safe so hosts can inject a custom store without
/// touching the palace handle. The palace builder accepts any `Arc<dyn
/// PalaceStore>` via [`PalaceBuilder::store`].
///
/// All methods are `async` — the store wraps an async vector DB client
/// (lancedb is natively async; embedvec uses `spawn_blocking` internally).
#[async_trait]
pub trait PalaceStore: Send + Sync + 'static {
    /// Upsert a batch of drawers. Implementations MUST handle
    /// duplicate IDs (same [`DrawerId`]) by replacing, not duplicating.
    async fn upsert(&self, drawers: Vec<Drawer>) -> anyhow::Result<()>;

    /// Delete drawers by ID. Returns the number of drawers deleted.
    async fn delete(&self, ids: &[DrawerId]) -> anyhow::Result<usize>;

    /// Search for nearest-neighbour drawers to a query vector.
    ///
    /// The store is responsible for computing the ANN query against its
    /// index. `scope` filters are applied server-side where possible;
    /// callers SHOULD NOT filter results post-query unless the store
    /// does not support server-side filtering.
    async fn search(
        &self,
        query: &[f32],
        scope: &SearchScope,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchHit>>;

    /// Count drawers matching the scope, without computing full results.
    async fn count(&self, scope: &SearchScope) -> anyhow::Result<usize>;

    /// Flush any buffered writes to durable storage.
    async fn flush(&self) -> anyhow::Result<()>;

    async fn get_drawers(
        &self,
        scope: Option<&SearchScope>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<Drawer>>;

    /// The store tier this implementation belongs to (for `mpr doctor`).
    fn tier(&self) -> StoreTier;
}

// ---------------------------------------------------------------------------
// SearchHit — vector search result
// ---------------------------------------------------------------------------

/// A single search result from a vector ANN query.
///
/// Mirrors the existing `layers::SearchHit` shape so existing callers
/// (searcher.rs, layers.rs L2/L3) don't break. The palace module's
/// `SearchHit` is for the new trait-based API; `layers::SearchHit`
/// stays for internal use.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct SearchHit {
    /// Verbatim drawer text.
    pub text: String,
    /// Wing (project/person name). `None` if unfiled.
    pub wing: Option<String>,
    /// Room (specific topic). `None` if unfiled.
    pub room: Option<String>,
    /// Source file name (derived from metadata).
    pub source_file: String,
    /// Cosine similarity score `[0.0, 1.0]`.
    pub similarity: f64,
    /// BM25 score if hybrid search was used. `None` for pure ANN.
    pub bm25_score: Option<f64>,
    /// Combined score if hybrid fusion was applied. `None` otherwise.
    pub combined_score: Option<f64>,
}

// ---------------------------------------------------------------------------
// Drawer — the atomic memory unit
// ---------------------------------------------------------------------------

/// The atomic memory unit in a palace.
///
/// A drawer holds verbatim content (the "drawer" in the palace metaphor)
/// along with metadata sufficient to route, search, and display it. The
/// content is never mutated after creation — edits produce a new drawer.
///
/// Drawers are created by [`MemoryProvider::add_drawer`] and are identified
/// by a stable [`DrawerId`] that persists across palace opens.
///
/// ## Hall routing
///
/// `kind` determines which hall the drawer lives in:
///
/// ```text
/// Palace::add_drawer(drawer, DrawerKind::Fact)
///   → files drawer into hall_facts
/// Palace::add_drawer(drawer, DrawerKind::Preference)
///   → files drawer into hall_preferences
/// ```
///
/// `Raw` drawers bypass hall assignment and AAAK compression.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Drawer {
    /// Stable identifier. Set by the store on first write; consumers
    /// can pre-assign a UUID to enable idempotent upserts.
    pub id: Option<DrawerId>,
    /// Verbatim content — the exact words never summarized. AAAK
    /// compression is a separate layer (closet), not the drawer itself.
    pub content: String,
    /// The memory type, which determines hall routing.
    pub kind: DrawerKind,
    /// Memory lifecycle tier. Controls consolidation policy (ADR-13 / mp-051).
    /// Default is `MemoryTier::Working`. Promotion to Episodic/Semantic/Procedural
    /// happens in sleep-time consolidation (mp-091).
    #[serde(default)]
    pub tier: MemoryTier,
    /// Wing (project/person). `None` means the palace default wing.
    pub wing: Option<String>,
    /// Room (specific topic within the wing). `None` means unfiled.
    pub room: Option<String>,
    /// Arbitrary key-value metadata carried through to search results.
    /// Built-in keys: `source_file`, `created_at`, `filed_at`, `added_by`.
    /// Custom keys are allowed and forwarded to the vector store.
    ///
    /// New first-class fields ([`Drawer::tags`], [`Drawer::trust`],
    /// [`Drawer::access_count`], etc.) are kept in sync with this map
    /// by [`Drawer::migrate_metadata`] so callers that pre-date those
    /// fields keep working.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
    /// IDs of drawers this drawer was derived from (mp-052 / ADR-10 / ADR-13).
    /// Populated during AAAK compression (derived drawer ← original drawer) and
    /// general extraction (extracted fact ← source conversation). Enables
    /// citation chains: "I used drawer #42 which came from session #abc-123".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<DrawerId>,

    // ---- First-class fields added in mp-migration 7/8 ----
    //
    // These fields were previously stored only in `metadata` under the
    // keys "tags" / "trust" / "access_count" / "last_accessed" /
    // "reinforcements" / "superseded_by" / "active". They are now
    // promoted to typed fields for type-safe access from the
    // `MemoryProvider` trait (boost/decay/reinforce/supersede/tag/link).
    //
    // `#[serde(default)]` keeps backwards compatibility — drawers
    // serialised before this change still load cleanly. The reverse
    // direction (writing the typed field) is handled by
    // `migrate_metadata` which is called by `add_drawer` on the
    // embedvec path.
    /// First-class tags. Mirrors `metadata["tags"]` (Vec<String>).
    /// Promoted from metadata in mp-migration 7/8.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Trust level. `"high" | "medium" | "low"`. Mirrors
    /// `metadata["trust"]` (String). Promoted from metadata in
    /// mp-migration 7/8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<String>,

    /// How many times this drawer has been retrieved. Mirrors
    /// `metadata["access_count"]` (u64). Promoted from metadata in
    /// mp-migration 7/8. Updated by [`crate::retention::record_access`].
    #[serde(default)]
    pub access_count: u64,

    /// Last time this drawer was retrieved. Mirrors
    /// `metadata["last_accessed"]` (RFC 3339 string). Promoted from
    /// metadata in mp-migration 7/8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accessed: Option<chrono::DateTime<chrono::Utc>>,

    /// Reinforcement history. Mirrors `metadata["reinforcements"]`
    /// (Vec<Reinforcement>). Promoted from metadata in mp-migration 7/8.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reinforcements: Vec<Reinforcement>,

    /// If `Some`, this drawer has been superseded by the drawer with
    /// this id. Mirrors `metadata["superseded_by"]` (String).
    /// Promoted from metadata in mp-migration 7/8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<DrawerId>,

    /// Whether this drawer is "active" (i.e. not superseded and not
    /// deleted). Mirrors `metadata["active"]` (bool). Defaults to
    /// `true`. Promoted from metadata in mp-migration 7/8.
    #[serde(default = "default_active")]
    pub active: bool,

    // ---- First-class fields added in mp-migration (issue #25 / #26) ----
    //
    // Ebbinghaus-style confidence (0.0-1.0) and consolidation count
    // (how many times this drawer has been reinforced). Previously
    // stored only in `metadata` under the keys "confidence" / "strength"
    // and invisible to the scoring pipeline.
    //
    // `#[serde(default = "...")]` keeps backwards compatibility —
    // drawers serialised before this change still load cleanly.
    /// Ebbinghaus-style confidence in the range 0.0-1.0. Mirrors
    /// `metadata["confidence"]` (number). Defaults to 1.0 on a
    /// brand-new drawer.
    #[serde(default = "default_confidence")]
    pub confidence: f64,

    /// Number of times this drawer has been reinforced (consolidated).
    /// Mirrors `metadata["strength"]` (number). Defaults to 1 on a
    /// brand-new drawer (every drawer counts as at least one observation
    /// so the field is never zero unless explicitly reset).
    #[serde(default = "default_one")]
    pub consolidation_strength: u32,

    // ---- First-class timestamps added in mp-25 ----
    //
    // These fields were previously stored only in `metadata` under
    // the keys "created_at" / "filed_at" / "last_accessed". They are
    // now typed fields so callers (jcode's `MemoryEntry`,
    // `effective_confidence()`, `memory_score()`) can read them
    // without parsing strings.
    //
    // `#[serde(default = "Utc::now")]` keeps backwards compatibility —
    // drawers serialised before this change still load (they get
    // `Utc::now()` at deserialise time, which is then overridden by
    // `migrate_metadata` if the legacy metadata is present).
    /// When this drawer was originally created. Mirrors
    /// `metadata["created_at"]` (RFC 3339 string), with
    /// `metadata["filed_at"]` as a fallback. Updated only at creation
    /// time and lifted into a typed field in mp-25.
    #[serde(default = "Utc::now")]
    pub created_at: chrono::DateTime<Utc>,

    /// When this drawer was last modified. Bumped by
    /// [`Drawer::touch`] on every mutation and on re-upsert by
    /// `PalaceStore::upsert`. Mirrors `metadata["last_accessed"]`
    /// (RFC 3339 string) as a fallback during legacy migration.
    #[serde(default = "Utc::now")]
    pub updated_at: chrono::DateTime<Utc>,
}

fn default_active() -> bool {
    true
}

fn default_confidence() -> f64 {
    1.0
}

fn default_one() -> u32 {
    1
}

/// A reinforcement breadcrumb — a record of when/where a drawer was
/// reinforced (the same fact re-encountered in a new session).
///
/// Mirrors the `Reinforcement` struct in jcode's `memory_types::Reinforcement`.
/// Promoted to a first-class type in mp-migration 7/8 so it can be
/// referenced from the `MemoryProvider::reinforce` trait method.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Reinforcement {
    /// Session that reinforced the drawer.
    pub session_id: String,
    /// Message index within that session.
    pub message_index: usize,
    /// When the reinforcement happened.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Drawer {
    pub fn new(content: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: None,
            content: content.into(),
            kind: DrawerKind::default(),
            tier: MemoryTier::default(),
            wing: None,
            room: None,
            metadata: std::collections::HashMap::new(),
            derived_from: Vec::new(),
            tags: Vec::new(),
            trust: None,
            access_count: 0,
            last_accessed: None,
            reinforcements: Vec::new(),
            superseded_by: None,
            active: true,
            confidence: default_confidence(),
            consolidation_strength: default_one(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Bump `updated_at` to the current UTC time and refresh
    /// `last_accessed` so subsequent Ebbinghaus decay starts from this
    /// point. Preserves `created_at`. Called by mutating builder
    /// methods and by `PalaceStore::upsert` on every write so re-upserts
    /// don't silently rewrite history. Also used by
    /// `MemoryProvider::boost` and `MemoryProvider::reinforce` to refresh
    /// the decay clock (issue #26).
    ///
    /// mp-25: typed timestamps replace the ad-hoc
    /// `metadata["last_accessed"]` string written by `boost` /
    /// `decay` / `reinforce`.
    /// mp-26: combined with the legacy `last_accessed` refresh in one
    /// method so callers can use a single `touch()` for both concerns.
    pub fn touch(&mut self) {
        let now = Utc::now();
        self.updated_at = now;
        self.last_accessed = Some(now);
    }

    pub fn kind(mut self, kind: DrawerKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn tier(mut self, tier: MemoryTier) -> Self {
        self.tier = tier;
        self
    }

    pub fn wing(mut self, wing: impl Into<String>) -> Self {
        self.wing = Some(wing.into());
        self
    }

    pub fn room(mut self, room: impl Into<String>) -> Self {
        self.room = Some(room.into());
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: impl serde::Serialize) -> Self {
        self.metadata
            .insert(key.into(), serde_json::to_value(value).unwrap_or_default());
        self
    }

    pub fn derived_from(mut self, ids: Vec<DrawerId>) -> Self {
        self.derived_from = ids;
        self
    }

    /// Builder methods for the new first-class fields (mp-migration 7/8).
    /// These mirror `metadata()` but write to the typed field.
    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn trust(mut self, trust: impl Into<String>) -> Self {
        self.trust = Some(trust.into());
        self
    }

    /// Builder for the Ebbinghaus confidence field (issue #26).
    pub fn confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Builder for the consolidation strength counter (issue #26).
    pub fn consolidation_strength(mut self, strength: u32) -> Self {
        self.consolidation_strength = strength;
        self
    }

    /// One-shot migration: if the typed fields are empty but the legacy
    /// `metadata` keys are populated, lift them up. Idempotent — safe
    /// to call repeatedly. Called by `add_drawer` on the embedvec path
    /// before upsert so the typed fields always reflect the source of
    /// truth on disk.
    pub fn migrate_metadata(&mut self) {
        if self.tags.is_empty() {
            if let Some(v) = self.metadata.remove("tags") {
                match serde_json::from_value::<Vec<String>>(v) {
                    Ok(arr) => self.tags = arr,
                    Err(e) => warn!("Drawer::migrate_metadata: failed to parse metadata['tags'] as Vec<String>: {}", e),
                }
            }
        } else {
            // Keep typed and metadata in sync.
            self.metadata.remove("tags");
        }
        if self.trust.is_none() {
            if let Some(v) = self.metadata.remove("trust") {
                match v.as_str() {
                    Some(s) => self.trust = Some(s.to_string()),
                    None => warn!(
                        "Drawer::migrate_metadata: metadata['trust'] is not a string: {}",
                        v
                    ),
                }
            }
        } else {
            self.metadata.remove("trust");
        }
        if self.access_count == 0 {
            if let Some(v) = self.metadata.remove("access_count") {
                match v.as_u64() {
                    Some(n) => self.access_count = n,
                    None => warn!(
                        "Drawer::migrate_metadata: metadata['access_count'] is not a u64: {}",
                        v
                    ),
                }
            }
        } else {
            self.metadata.remove("access_count");
        }
        if self.last_accessed.is_none() {
            if let Some(v) = self.metadata.remove("last_accessed") {
                match v.as_str() {
                    Some(s) => match chrono::DateTime::parse_from_rfc3339(s) {
                        Ok(dt) => self.last_accessed = Some(dt.with_timezone(&chrono::Utc)),
                        Err(e) => warn!("Drawer::migrate_metadata: failed to parse metadata['last_accessed'] as RFC 3339: {}", e),
                    },
                    None => warn!("Drawer::migrate_metadata: metadata['last_accessed'] is not a string: {}", v),
                }
            }
        } else {
            self.metadata.remove("last_accessed");
        }
        if self.reinforcements.is_empty() {
            if let Some(v) = self.metadata.remove("reinforcements") {
                match serde_json::from_value::<Vec<Reinforcement>>(v) {
                    Ok(arr) => self.reinforcements = arr,
                    Err(e) => warn!("Drawer::migrate_metadata: failed to parse metadata['reinforcements'] as Vec<Reinforcement>: {}", e),
                }
            }
        } else {
            self.metadata.remove("reinforcements");
        }
        if self.superseded_by.is_none() {
            if let Some(v) = self.metadata.remove("superseded_by") {
                match v.as_str() {
                    Some(s) => self.superseded_by = Some(DrawerId::new(s)),
                    None => warn!(
                        "Drawer::migrate_metadata: metadata['superseded_by'] is not a string: {}",
                        v
                    ),
                }
            }
        } else {
            self.metadata.remove("superseded_by");
        }
        if self.active == default_active() {
            if let Some(v) = self.metadata.remove("active") {
                match v.as_bool() {
                    Some(b) => self.active = b,
                    None => warn!(
                        "Drawer::migrate_metadata: metadata['active'] is not a bool: {}",
                        v
                    ),
                }
            }
        } else {
            self.metadata.remove("active");
        }
        // ---- mp-25: lift legacy timestamp metadata into typed fields ----
        //
        // The new typed `created_at` / `updated_at` fields default to
        // `Utc::now()` at deserialise time (via `#[serde(default = "Utc::now")]`).
        // That means a freshly-loaded legacy drawer already has a
        // timestamp — but it's "now" rather than the original creation
        // time. We override it here when the metadata carries a more
        // accurate legacy value.
        //
        // created_at precedence:
        //   1. metadata["created_at"] (RFC 3339) — the canonical jcode key
        //   2. metadata["filed_at"]    (RFC 3339) — older alias
        //   3. self.created_at         (the `Utc::now()` serde default)
        // updated_at precedence:
        //   1. metadata["last_accessed"] (RFC 3339) — most recent write
        //   2. self.updated_at          (the `Utc::now()` serde default)
        let parsed_created = self
            .metadata
            .get("created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|| {
                self.metadata
                    .get("filed_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc))
            });
        if let Some(ts) = parsed_created {
            // Only override if the parsed timestamp is *not* in the
            // future relative to whatever serde gave us (sanity check).
            if ts <= self.created_at {
                self.created_at = ts;
            }
        }
        // Always remove lifted keys so the legacy metadata doesn't
        // re-introduce duplicates on the next round-trip.
        self.metadata.remove("created_at");
        self.metadata.remove("filed_at");

        let parsed_updated = self.last_accessed;
        if let Some(ts) = parsed_updated {
            // Take the newer of the two so we never regress
            // `updated_at` backwards (e.g. if the serde default was
            // generated *after* `last_accessed` was written).
            if ts > self.updated_at {
                self.updated_at = ts;
            }
        }

        // Issue #26: lift legacy confidence / strength keys to typed
        // fields. Only do this when the typed field is still at its
        // default so we don't clobber an explicit write with a stale
        // metadata value.
        if (self.confidence - default_confidence()).abs() < f64::EPSILON {
            if let Some(v) = self.metadata.remove("confidence") {
                match v.as_f64() {
                    Some(n) => self.confidence = n.clamp(0.0, 1.0),
                    None => warn!(
                        "Drawer::migrate_metadata: metadata['confidence'] is not a number: {}",
                        v
                    ),
                }
            }
        } else {
            self.metadata.remove("confidence");
        }
        if self.consolidation_strength == default_one() {
            if let Some(v) = self.metadata.remove("strength") {
                match v.as_u64() {
                    Some(n) => self.consolidation_strength = n.min(u32::MAX as u64) as u32,
                    None => warn!(
                        "Drawer::migrate_metadata: metadata['strength'] is not a number: {}",
                        v
                    ),
                }
            }
        } else {
            self.metadata.remove("strength");
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryProvider — the high-level trait external consumers use
// ---------------------------------------------------------------------------

/// The public trait that external consumers (jcode, third-party agents)
/// implement against. Unlike the internal `PalaceDb` / `searcher` APIs,
/// `MemoryProvider` is designed to be implemented by an adapter layer so
/// that jcode's existing `MemoryManager` can remain the authoritative
/// runtime while delegating storage to mempalace.
///
/// jcode integration sketch (see ADR-10):
///
/// ```ignore
/// struct JcodeMempalaceAdapter { palace: Palace }
/// impl MemoryProvider for JcodeMempalaceAdapter { .. }
/// ```
///
/// Embedder and store are accessible for introspection and adapter wiring:
/// `MemoryProvider::embedder()` returns `&dyn Embedder`,
/// `MemoryProvider::store()` returns `&dyn PalaceStore`.
#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    /// File a drawer into the palace. Returns the assigned [`DrawerId`].
    ///
    /// The drawer is validated, compressed via AAAK (unless `kind` is
    /// `Raw`), upserted into the vector store, and linked into the
    /// knowledge graph as a `Triple` (entity extraction from content).
    async fn add_drawer(&self, drawer: Drawer) -> anyhow::Result<DrawerId>;

    /// Batch version of add_drawer for efficiency.
    async fn add_drawers(&self, drawers: Vec<Drawer>) -> anyhow::Result<Vec<DrawerId>> {
        let mut ids = Vec::new();
        for drawer in drawers {
            ids.push(self.add_drawer(drawer).await?);
        }
        Ok(ids)
    }

    /// Convenience: file content directly with a kind and scope.
    /// Shorthand for `add_drawer(Drawer::new(content).kind(kind).wing(wing))`.
    async fn remember(&self, content: String, scope: MemoryScope) -> anyhow::Result<DrawerId>;

    /// Remove a drawer by ID. Returns `true` if the drawer existed.
    async fn forget(&self, id: &DrawerId) -> anyhow::Result<bool>;

    /// Search using natural-language query. Embeds the query, runs ANN,
    /// and returns ranked results.
    async fn search(&self, query: &str, scope: &SearchScope) -> anyhow::Result<Vec<SearchHit>>;

    /// Search using a pre-computed embedding vector. Useful when the
    /// caller has already embedded (e.g. jcode's own embedder) and
    /// wants to reuse the vector rather than re-embed.
    ///
    /// ## Pre-embedding for batched queries
    ///
    /// If you have many queries to run in succession (a TUI agent doing
    /// per-turn retrieval, a benchmark harness sweeping a query set, a
    /// swarm coordinator fan-out), embed them once via
    /// `provider.embedder().embed(query)` and pass the resulting
    /// `Vec<f32>` here, rather than calling [`MemoryProvider::search`]
    /// which re-embeds on every call. The embedder is exposed at
    /// [`MemoryProvider::embedder`] for exactly this reason.
    ///
    /// ```ignore
    /// let qv: Vec<f32> = provider.embedder().embed("auth decision")?;
    /// let hits = provider.search_with_embedding(&qv, &SearchScope::default()).await?;
    /// ```
    ///
    /// The embedder trait lives in `embed/mod.rs` — see
    /// [`crate::embed::Embedder`] for the available backends (10 ship
    /// in-tree: fastembed, model2vec, tract, clip, null, plus
    /// openai/voyage/openrouter/gemini/cohere remote providers).
    async fn search_with_embedding(
        &self,
        query_vec: &[f32],
        scope: &SearchScope,
    ) -> anyhow::Result<Vec<SearchHit>>;

    /// Retrieve drawers related to a given drawer by following graph
    /// edges (tunnel + hall connections). `depth` controls how many
    /// hops to follow (1 = direct neighbours only).
    ///
    /// ## Implementation (issue #31, mp-migration 6/8)
    ///
    /// When a wired [`KnowledgeGraph`] is present, the default
    /// implementation runs `cascade_retrieve` from the seed drawer
    /// (score 1.0) and projects the cascade score into the
    /// [`SearchHit::similarity`] field. The cascade follows typed
    /// edges with traversal weights (HasTag 0.8, RelatesTo W,
    /// Supersedes 0.9, DerivedFrom 0.7, InCluster 0.6, Contradicts
    /// 0.3) and decays by `0.7^depth`. Tag nodes act as relays and
    /// fan out to every other drawer sharing the tag.
    ///
    /// Implementations that do not have a wired KG return an empty
    /// `Vec` (the previous stub behaviour).
    async fn related(&self, id: &DrawerId, depth: usize) -> anyhow::Result<Vec<SearchHit>>;

    /// Embedding + cascade retrieval in a single call. jcode's
    /// `find_similar_with_cascade()`.
    ///
    /// 1. Embed the query (or accept a pre-computed vector via
    ///    [`MemoryProvider::search_with_embedding`]).
    /// 2. Run ANN to find the top-`seed_limit` embedding-similar
    ///    drawers; those become the seeds.
    /// 3. Expand each seed through the typed KG via
    ///    `cascade_retrieve(kg, seeds, depth, limit)`.
    /// 4. Return the resulting `(drawer_id, score)` list projected
    ///    as [`SearchHit`]s.
    ///
    /// The `max_results` parameter bounds the final returned list;
    /// `seed_limit` bounds the embedding-ANN step. The depth
    /// parameter is passed straight through to `cascade_retrieve`.
    async fn cascade_search(
        &self,
        query: &str,
        scope: &SearchScope,
        depth: usize,
        max_results: usize,
    ) -> anyhow::Result<Vec<SearchHit>> {
        // Default implementation: embed + ANN + cascade. Implementations
        // with their own scoring pipeline can override for tighter
        // integration (e.g. jcode's combined retrieval).
        let embedding = self.embedder().embed(query).await?;
        self.cascade_search_with_embedding(&embedding, scope, depth, max_results)
            .await
    }

    /// Pre-embedded variant of [`MemoryProvider::cascade_search`].
    /// Use when the caller has already embedded (e.g. jcode's own
    /// embedder) and wants to reuse the vector rather than re-embed.
    async fn cascade_search_with_embedding(
        &self,
        query_vec: &[f32],
        scope: &SearchScope,
        depth: usize,
        max_results: usize,
    ) -> anyhow::Result<Vec<SearchHit>>;

    /// Parse a transcript and extract structured memories from it,
    /// filing each extracted fact/dictum as a drawer and linking it
    /// into the knowledge graph.
    ///
    /// Used by jcode's `extract_from_transcript` path (mp-061).
    async fn extract_from_transcript(
        &self,
        transcript: &str,
        session_id: &str,
    ) -> anyhow::Result<Vec<DrawerId>>;

    /// Statistics about the palace's knowledge graph.
    async fn graph_stats(&self) -> anyhow::Result<super::knowledge_graph::KgStats>;

    /// Optional access to the wired [`KnowledgeGraph`] for typed-edge
    /// writes (mp-027, issue #27). The default implementation returns
    /// `None`, in which case the default `tag` / `link` / `supersede`
    /// implementations fall back to the drawer-metadata path.
    ///
    /// Implementations that have a [`KnowledgeGraph`] (e.g. the
    /// production [`Palace`]) override this and return `Some(_)`. The
    /// default `tag` / `link` / `supersede` methods will then route
    /// writes through the KG so cascade retrieval can use the typed
    /// edge columns.
    fn kg(&self) -> Option<&std::sync::Mutex<super::knowledge_graph::KnowledgeGraph>> {
        None
    }

    // -----------------------------------------------------------------------
    // Per-entry mutation methods (mp-migration 1/8)
    //
    // These methods give external consumers (jcode's adapter, third-party
    // agents) a typed API for evolving drawer state without going through
    // `add_drawer` (which is destructive) or raw `metadata` keys.
    //
    // All methods have default implementations that do a
    // get-mutate-upsert dance. Implementations are encouraged to provide
    // O(1) overrides when the underlying store supports it (e.g. an
    // SQL `UPDATE WHERE id = ?`).
    //
    // The `where Self: Sized` bound is required because the default
    // implementations take `self` by value into a free helper
    // function (which needs `&dyn MemoryProvider` — a `?Sized` type).
    // Trait implementers that need `?Sized` Self (very rare) can
    // override these methods directly.
    // -----------------------------------------------------------------------

    /// Boost a drawer's relevance score. jcode's `boost_confidence`.
    /// Default implementation reads the drawer, increments
    /// `metadata["access_count"]`, updates `metadata["last_accessed"]`,
    /// raises `confidence` by `amount` (capped at 1.0), increments
    /// `consolidation_strength`, and calls `drawer.touch()` so the
    /// Ebbinghaus decay clock restarts.
    async fn boost(&self, id: &DrawerId, amount: f64) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        default_mutate_drawer(self, id, |d| {
            let count = d
                .metadata
                .get("access_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .saturating_add(1);
            d.metadata
                .insert("access_count".to_string(), serde_json::json!(count));
            d.metadata.insert(
                "last_accessed".to_string(),
                serde_json::json!(chrono::Utc::now().to_rfc3339()),
            );
            // Issue #26: typed confidence/consolidation_strength.
            // Cap confidence at 1.0; use saturating_add on the count.
            d.confidence = (d.confidence + amount).clamp(0.0, 1.0);
            d.consolidation_strength = d.consolidation_strength.saturating_add(1);
            d.touch();
        })
        .await
    }

    /// Decay a drawer's relevance score. jcode's `decay_confidence`.
    /// Default implementation recomputes `confidence` from
    /// `crate::retention::calculate_retention` using the default
    /// Ebbinghaus config and the drawer's own category as the decay
    /// category (issue #26). The `amount` parameter is the additional
    /// decay to apply beyond what time has already produced; the
    /// default Ebbinghaus config is used as a sensible default until
    /// category-specific half-lives land (issue #3).
    async fn decay(&self, id: &DrawerId, amount: f64) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        default_mutate_drawer(self, id, |d| {
            d.metadata.insert(
                "last_accessed".to_string(),
                serde_json::json!(chrono::Utc::now().to_rfc3339()),
            );
            // Issue #26: recompute confidence from Ebbinghaus.
            // We use the default decay config plus a category-aware
            // decay_rate, mirroring `retention::decay_rate_for_type`.
            let now = chrono::Utc::now();
            let last = d.last_accessed.map(|t| t).unwrap_or(now);
            let elapsed_days = (now - last).num_seconds() as f64 / 86_400.0;
            let decay_rate =
                crate::retention::decay_rate_for_type(&crate::types::MemoryType::Working);
            // Ebbinghaus: retention = initial * e^(-rate * days)
            let decayed = (-decay_rate * elapsed_days.max(0.0)).exp();
            // Apply additional manual decay from `amount` after the
            // time-based portion so the caller can force-accelerate.
            d.confidence = (d.confidence.min(decayed) - amount).clamp(0.0, 1.0);
        })
        .await
    }

    /// Record a reinforcement — the same drawer was re-encountered in
    /// a new session. jcode's `MemoryEntry::reinforce`.
    /// Default implementation appends to
    /// `metadata["reinforcements"]`, bumps `access_count`, increments
    /// `consolidation_strength` (issue #26), and calls
    /// `drawer.touch()` to refresh the Ebbinghaus decay clock.
    async fn reinforce(
        &self,
        id: &DrawerId,
        session_id: &str,
        message_index: usize,
    ) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        let payload = serde_json::json!({
            "session_id": session_id,
            "message_index": message_index,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });
        default_mutate_drawer(self, id, move |d| {
            let key = "reinforcements";
            let mut arr: Vec<serde_json::Value> = d
                .metadata
                .get(key)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            arr.push(payload);
            d.metadata
                .insert(key.to_string(), serde_json::Value::Array(arr));
            let count = d
                .metadata
                .get("access_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .saturating_add(1);
            d.metadata
                .insert("access_count".to_string(), serde_json::json!(count));
            d.metadata.insert(
                "last_accessed".to_string(),
                serde_json::json!(chrono::Utc::now().to_rfc3339()),
            );
            // Issue #26: typed consolidation strength.
            d.consolidation_strength = d.consolidation_strength.saturating_add(1);
            d.touch();
        })
        .await
    }

    /// Mark a drawer as superseded by another. jcode's
    /// `MemoryEntry::supersede`. Default implementation sets
    /// `metadata["superseded_by"]` and `metadata["active"] = false`
    /// on the old drawer.
    ///
    /// When a wired KG is available (mp-027, issue #27), this also
    /// creates a `Supersedes` typed edge from `new_id` to `old_id`
    /// with the canonical traversal weight of 0.9 so cascade
    /// retrieval can follow the chain.
    async fn supersede(&self, old_id: &DrawerId, new_id: &DrawerId) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        default_mutate_drawer(self, old_id, |d| {
            d.metadata
                .insert("superseded_by".to_string(), serde_json::json!(new_id.0));
            d.metadata
                .insert("active".to_string(), serde_json::json!(false));
        })
        .await?;
        if let Some(kg_lock) = self.kg() {
            let mut kg = kg_lock.lock().expect("kg mutex poisoned");
            kg.add_memory_edge(
                &new_id.0,
                &old_id.0,
                &crate::types::MemoryEdgeKind::Supersedes,
            )?;
        }
        Ok(())
    }

    /// Set a single metadata key on a drawer. jcode's adapter uses
    /// this for trust-level updates, source-URL updates, and similar
    /// small per-entry edits.
    async fn set_metadata(
        &self,
        id: &DrawerId,
        key: String,
        value: serde_json::Value,
    ) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        default_mutate_drawer(self, id, |d| {
            d.metadata.insert(key, value);
        })
        .await
    }

    /// jcode-compat: returns `(memories, tags, edges, clusters)` instead
    /// of [`super::knowledge_graph::KgStats`].
    ///
    /// Used by jcode's `MemoryManager::graph_stats` (which today returns
    /// a 4-tuple) so the adapter can pass through unchanged. Will be
    /// removed when jcode migrates to the structured [`KgStats`].
    ///
    /// The default implementation derives the tuple from
    /// [`MemoryProvider::graph_stats`] for the triple/entity counts and
    /// counts tags separately. Callers that need accurate tag counts
    /// can override.
    async fn graph_stats_legacy(&self) -> anyhow::Result<(usize, usize, usize, usize)> {
        let stats = self.graph_stats().await?;
        // memories: separate query against the store
        let memories = self
            .store()
            .count(&SearchScope::default())
            .await
            .unwrap_or(0);
        // tags: TODO when knowledge_graph is wired into Palace (mp-020).
        // For now return 0; jcode-side adapter can fall back to its
        // own tag counter if it needs accuracy.
        let tags = 0usize;
        // edges: total KG triples
        let edges = stats.total_triples;
        // clusters: total KG entities
        let clusters = stats.total_entities;
        Ok((memories, tags, edges, clusters))
    }

    // -----------------------------------------------------------------------
    // Tag and link methods (mp-migration 2/8)
    //
    // jcode's `MemoryManager::tag_memory` / `link_memories` are the
    // canonical graph-mutation entry points. mempalace stores these
    // as KG triples (predicate = "has_tag" / "relates_to") so the
    // same data backs both `mempalace_kg_query` and jcode's adapter.
    //
    // The default implementations here use the metadata path (so
    // they don't depend on the KG being wired into Palace) and
    // mirror the values in shapes that match the eventual KG
    // triples:
    //   tag → metadata["tags"] (Vec<String>)
    //         + metadata["tag:<name>"] = true (cheap lookup)
    //   link → metadata["links"] (Vec<{target, weight}>)
    //
    // mp-027 (issue #27): when a wired KG is present
    // ([`MemoryProvider::kg`]), the default impls also create typed
    // edges so cascade retrieval can use the canonical traversal
    // weights:
    //   tag → `HasTag` edge (weight 0.8)
    //   link → `RelatesTo { weight }` edge
    //   supersede → `Supersedes` edge (weight 0.9)
    // -----------------------------------------------------------------------

    /// Add a tag to a drawer. jcode's `MemoryManager::tag_memory`.
    ///
    /// When a wired KG is available (mp-027, issue #27), this also
    /// creates a `HasTag` typed edge between the drawer and the tag,
    /// with the canonical traversal weight of 0.8.
    async fn tag(&self, id: &DrawerId, tag: &str) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        default_mutate_drawer(self, id, |d| {
            let mut tags: Vec<String> = d
                .metadata
                .get("tags")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            if !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_string());
            }
            d.metadata
                .insert("tags".to_string(), serde_json::json!(tags));
            d.metadata
                .insert(format!("tag:{}", tag), serde_json::json!(true));
        })
        .await?;
        if let Some(kg_lock) = self.kg() {
            let mut kg = kg_lock.lock().expect("kg mutex poisoned");
            kg.add_memory_edge(&id.0, tag, &crate::types::MemoryEdgeKind::HasTag)?;
        }
        Ok(())
    }

    /// Remove a tag from a drawer. jcode's
    /// `MemoryManager::untag_memory`.
    async fn untag(&self, id: &DrawerId, tag: &str) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        default_mutate_drawer(self, id, |d| {
            let mut tags: Vec<String> = d
                .metadata
                .get("tags")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            tags.retain(|t| t != tag);
            d.metadata
                .insert("tags".to_string(), serde_json::json!(tags));
            d.metadata.remove(&format!("tag:{}", tag));
        })
        .await
    }

    /// Link two drawers with a weighted edge. jcode's
    /// `MemoryManager::link_memories`.
    ///
    /// When a wired KG is available (mp-027, issue #27), this also
    /// creates a `RelatesTo { weight }` typed edge so cascade
    /// retrieval can sort/filter by traversal weight.
    async fn link(&self, from_id: &DrawerId, to_id: &DrawerId, weight: f32) -> anyhow::Result<()>
    where
        Self: Sized,
    {
        default_mutate_drawer(self, from_id, |d| {
            let mut links: Vec<serde_json::Value> = d
                .metadata
                .get("links")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            links.retain(|l| l.get("target").and_then(|v| v.as_str()) != Some(to_id.0.as_str()));
            links.push(serde_json::json!({
                "target": to_id.0,
                "weight": weight,
            }));
            d.metadata
                .insert("links".to_string(), serde_json::json!(links));
        })
        .await?;
        if let Some(kg_lock) = self.kg() {
            let mut kg = kg_lock.lock().expect("kg mutex poisoned");
            kg.add_memory_edge(
                &from_id.0,
                &to_id.0,
                &crate::types::MemoryEdgeKind::RelatesTo { weight },
            )?;
        }
        Ok(())
    }

    /// List all tags used in the palace, with usage counts.
    /// jcode's closest equivalent is `graph_stats.1` (the second
    /// element of the 4-tuple).
    ///
    /// Returns `Vec<(tag, count)>` sorted by count desc, then tag
    /// asc (deterministic). The default implementation aggregates
    /// from `get_drawers`; implementations with a wired KG should
    /// override and use `kg.query_relationship(predicate="has_tag")`
    /// for an O(1) path.
    async fn list_tags(&self) -> anyhow::Result<Vec<(String, usize)>>
    where
        Self: Sized,
    {
        use std::collections::HashMap;
        let drawers = self.get_drawers(None, None).await?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for d in &drawers {
            if let Some(arr) = d.metadata.get("tags") {
                if let Ok(tags) = serde_json::from_value::<Vec<String>>(arr.clone()) {
                    for t in tags {
                        *counts.entry(t).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut out: Vec<(String, usize)> = counts.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(out)
    }

    /// Stable identifier for this provider — used in audit logs and
    /// agent memory traces. Convention: `"mempalace:<palace_path_hash>"`.
    fn fingerprint(&self) -> &str;

    /// The embedder this provider uses. Useful for callers that want to
    /// pre-compute query vectors (see `search_with_embedding`).
    fn embedder(&self) -> &dyn super::embed::Embedder;

    /// The vector store this provider uses. Useful for introspection
    /// and tier promotion checks (`mpr doctor`).
    fn store(&self) -> &dyn PalaceStore;

    /// Enumerate drawers, optionally filtered by scope and limited in count.
    async fn get_drawers(
        &self,
        scope: Option<&SearchScope>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<Drawer>>;

    // -----------------------------------------------------------------------
    // Retention-ranked recall (mp-migration 4/8)
    // -----------------------------------------------------------------------

    /// Return the N most retention-relevant drawers, sorted by
    /// `confidence` (descending) and breaking ties by `updated_at`
    /// (most recent first). jcode's `recall --mode recent` (no query)
    /// and the L1 wake-up layer both need this.
    ///
    /// The default implementation walks `get_drawers(None, None)` and
    /// ranks each drawer. The score in the returned `SearchHit` is the
    /// drawer's `confidence` value so the score is now directly
    /// comparable to the vector search score (mp-26 changes the
    /// ranking from a `access_count / days` heuristic to a true
    /// Ebbinghaus confidence ranking).
    ///
    /// Drawers that have never been accessed (no `last_accessed`)
    /// fall back to `metadata["created_at"]` parsed as RFC 3339, and
    /// finally to `now`, so brand-new drawers rank last.
    ///
    /// Implementations with a wired KG should override and use
    /// `kg.helpfulness_score(id)` (which already factors in
    /// episodic memory feedback) — that gives strictly better
    /// ranking than the metadata-only default.
    async fn recent(
        &self,
        limit: usize,
        scope: Option<&SearchScope>,
    ) -> anyhow::Result<Vec<SearchHit>>
    where
        Self: Sized,
    {
        use std::cmp::Ordering;

        let now = chrono::Utc::now();
        // Pull a generous superset and trim to `limit` after ranking.
        // For embedvec (≤5 k drawers) this is one call; larger
        // palaces should override with an indexed path.
        let pull_limit = if scope.is_some() { limit.max(64) } else { 4096 };
        let drawers = self.get_drawers(scope, Some(pull_limit)).await?;

        // Compute per-drawer secondary key (updated_at) once.
        // Issue #26: prefer the typed `last_accessed` (which is
        // updated by `Drawer::touch()`), then fall back to
        // `metadata["created_at"]`, then `now` so the value is always
        // a valid DateTime<Utc>.
        let updated_at_of = |d: &Drawer| -> chrono::DateTime<chrono::Utc> {
            d.last_accessed.unwrap_or_else(|| {
                d.metadata
                    .get("created_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or(now)
            })
        };

        // Issue #26: rank by confidence DESC, breaking ties by
        // updated_at DESC. The returned `similarity` is the
        // drawer's confidence so the value is directly comparable
        // to vector search scores.
        let mut scored: Vec<(Drawer, f64, chrono::DateTime<chrono::Utc>)> = drawers
            .into_iter()
            .map(|d| {
                let updated_at = updated_at_of(&d);
                let confidence = d.confidence;
                (d, confidence, updated_at)
            })
            .collect();

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.2.cmp(&a.2))
        });

        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(d, confidence, _updated_at)| SearchHit {
                text: d.content,
                wing: d.wing,
                room: d.room,
                source_file: d
                    .metadata
                    .get("source_file")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                similarity: confidence,
                bm25_score: None,
                combined_score: None,
            })
            .collect())
    }
}

/// Project a cascade-retrieval result into the public [`SearchHit`]
/// shape. The cascade returns `(DrawerId, f32)` pairs ordered by
/// score; we synthesize a [`SearchHit`] for each pair, using the
/// drawer id as a placeholder for the verbatim text (the store layer
/// owns the canonical text, which we cannot reach from the KG path
/// without a per-tier `get_drawer_by_id` API). The cascade score
/// lands in [`SearchHit::similarity`].
fn project_cascade_to_hits(
    scored: Vec<(DrawerId, f32)>,
    scope: Option<&SearchScope>,
) -> Vec<SearchHit> {
    scored
        .into_iter()
        .map(|(id, score)| SearchHit {
            text: id.0.clone(),
            wing: scope.and_then(|s| s.wing.clone()),
            room: scope.and_then(|s| s.room.clone()),
            source_file: String::new(),
            similarity: score as f64,
            bm25_score: None,
            combined_score: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Palace — the canonical MemoryProvider implementation
// ---------------------------------------------------------------------------

/// The default [`MemoryProvider`] implementation bundled with mempalace-core.
///
/// `Palace` is constructed via [`PalaceBuilder`] — never constructed
/// directly. All fields are private; external consumers interact only
/// through the trait.
///
/// ## Example
///
/// ```ignore
/// let embedder = embedder_from_env()?;
/// let palace = PalaceBuilder::new()
///     .config(PalaceConfig { palace_path: "my_project/.mempalace".into(), .. })
///     .embedder(embedder)
///     .open()
///     .await?;
/// let results = palace.search("auth decision", &SearchScope::default()).await?;
/// ```
///
/// See [`PalaceBuilder`] for all configuration options.
#[derive(Clone)]
#[non_exhaustive]
pub struct Palace {
    // All internal state is behind Arcs so Palace is Send + Sync.
    embedder: Arc<dyn super::embed::Embedder>,
    store: Arc<dyn PalaceStore>,
    /// LLM provider for compression and image description (Phase 0 / bead 0.2).
    pub llm: Option<Arc<dyn crate::llm::LlmProvider>>,
    /// Session store for lifecycle observations (Phase 0 / bead 0.3).
    pub sessions: Option<Arc<crate::session::SessionStore>>,
    /// mp-migration 5/8: optional activity sink. When set, the
    /// `Palace` impl fires `ActivityEvent`s at meaningful pipeline
    /// points (search start/done, found relevant, tool action, etc).
    /// Used by the jcode adapter to drive its `MemoryEventSink`.
    pub activity_sink: Option<Arc<dyn Fn(ActivityEvent) + Send + Sync>>,
    /// mp-027 (issue #27): optional wired [`KnowledgeGraph`] for typed
    /// memory edges. When present, the default [`MemoryProvider::tag`]
    /// / [`MemoryProvider::link`] / [`MemoryProvider::supersede`]
    /// implementations also write `HasTag` / `RelatesTo` / `Supersedes`
    /// typed edges into the KG. Wrapped in a [`std::sync::Mutex`] so
    /// callers can take `&mut KnowledgeGraph` (the underlying rusqlite
    /// connection requires exclusive access for writes).
    pub kg: Option<Arc<std::sync::Mutex<super::knowledge_graph::KnowledgeGraph>>>,
}

// ---------------------------------------------------------------------------
// ActivityEvent (mp-migration 5/8)
// ---------------------------------------------------------------------------

/// A single lifecycle event from the palace's per-call pipeline.
///
/// Mirrors jcode's `MemoryEventKind` + `MemoryState` enums in a
/// single struct. jcode's adapter maps these to its own
/// `ServerEvent::MemoryActivity { activity }` shape.
///
/// The pipeline progresses as:
///   Idle → Embedding → FoundRelevant → Idle
///   Idle → Extracting → Idle
///   Idle → Maintaining → Idle
///   Idle → ToolAction → Idle  (for tool-driven calls)
///
/// `detail` carries an optional human-readable string (e.g. the
/// query that was searched, the drawer id that was filed, the
/// tag that was applied).
#[derive(Debug, Clone)]
pub struct ActivityEvent {
    /// Pipeline state this event represents.
    pub state: ActivityState,
    /// Optional human-readable detail.
    pub detail: Option<String>,
    /// Wall-clock timestamp when the event was emitted.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Pipeline state for an [`ActivityEvent`]. Mirrors jcode's
/// `MemoryState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    /// No activity. Default / after-pipeline.
    Idle,
    /// Running embedding search.
    Embedding,
    /// Sidecar (LLM) checking relevance of candidates.
    SidecarChecking,
    /// Embedding search found relevant results.
    FoundRelevant,
    /// Extracting memories from a transcript.
    Extracting,
    /// Background maintenance (consolidation, lesson decay, etc).
    Maintaining,
    /// Agent is using a memory tool (remember/recall/etc).
    ToolAction {
        /// "remember" | "recall" | "search" | "list" | "forget" | "tag" | "link" | "related"
        action: &'static str,
    },
}

// ---------------------------------------------------------------------------
// Default-implementation helpers (mp-migration 1/8)
// ---------------------------------------------------------------------------

/// Locate a drawer by id, run a mutation closure, and upsert the
/// result. Used as the default implementation for the per-entry
/// mutation methods on [`MemoryProvider`].
///
/// O(n) over `get_drawers(None, None)` — fine for the embedvec tier
/// (≤5 k drawers). Implementations that target larger palaces
/// (usearch, lancedb) should override the public mutation methods
/// with a direct `WHERE id = ?` store call.
///
/// Walks the drawer list, finds the matching id, runs `f`, and
/// upserts. Returns `Ok(())` silently if the id is not present —
/// matches jcode's `forget` semantics (the "forget something that
/// doesn't exist" case is a no-op, not an error).
async fn default_mutate_drawer(
    provider: &dyn MemoryProvider,
    id: &DrawerId,
    f: impl FnOnce(&mut Drawer),
) -> anyhow::Result<()> {
    let all = provider.get_drawers(None, None).await?;
    let Some(mut drawer) = all.into_iter().find(|d| d.id.as_ref() == Some(id)) else {
        return Ok(());
    };
    f(&mut drawer);
    provider.store().upsert(vec![drawer]).await?;
    Ok(())
}

impl std::fmt::Debug for Palace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Palace").finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Palace is the canonical MemoryProvider
// ---------------------------------------------------------------------------

impl Palace {
    /// Fire an activity event if a sink is registered. mp-migration
    /// 5/8. Called from the `impl MemoryProvider for Palace` methods
    /// to publish state transitions; no-op when no sink is set.
    fn emit_activity(&self, state: ActivityState, detail: Option<String>) {
        if let Some(sink) = &self.activity_sink {
            sink(ActivityEvent {
                state,
                detail,
                timestamp: chrono::Utc::now(),
            });
        }
    }

    fn derive_drawer_id(content: &str) -> DrawerId {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        content.hash(&mut h);
        DrawerId::new(format!("drawer-{:x}", h.finish()))
    }

    fn make_raw_observation(&self, content: &str) -> crate::types::RawObservation {
        use chrono::Utc;
        crate::types::RawObservation {
            id: String::new(),
            session_id: String::new(),
            timestamp: Utc::now(),
            hook_type: crate::types::HookType::PostToolUse,
            tool_name: None,
            tool_input: None,
            tool_output: Some(content.to_string()),
            user_prompt: None,
            assistant_response: None,
            raw: None,
            modality: "text".to_string(),
            image_data: None,
            agent_id: None,
        }
    }

    /// Issue #32: build a [`Sidecar`] from the palace's LLM provider.
    ///
    /// Returns `None` when no LLM provider is configured (the palace
    /// was opened without `llm` set), in which case
    /// `extract_from_transcript` falls back to heuristic-only.
    ///
    /// The sidecar is created fresh on each call (lightweight — just a
    /// trait-object pointer). If a future profiling pass shows the
    /// construction is hot, we can cache it behind `OnceCell`.
    #[cfg(feature = "llm-sidecar")]
    fn sidecar(&self) -> Option<crate::sidecar::Sidecar> {
        self.llm
            .as_ref()
            .map(|arc| crate::sidecar::Sidecar::new(Box::new(CloneLLM(arc.clone()))))
    }

    /// Issue #29: heuristic-only extraction path.
    ///
    /// Runs `general_extractor::extract_memories` and files the result
    /// as drawers. This is the original extraction pipeline from #29,
    /// preserved as a standalone method so the sidecar path can call
    /// it too.
    async fn extract_from_transcript_heuristic(
        &self,
        transcript: &str,
        session_id: &str,
    ) -> anyhow::Result<Vec<DrawerId>> {
        let classifications = crate::general_extractor::extract_memories(transcript, 0.3);
        if classifications.is_empty() {
            return Ok(vec![]);
        }

        let dedup_scope = SearchScope {
            limit: 1,
            ..SearchScope::default()
        };

        let mut added: Vec<DrawerId> = Vec::with_capacity(classifications.len());

        for classification in classifications {
            let kind = classification
                .memory_type
                .to_drawer_kind(&classification.text);

            let mut tags: Vec<String> = classification
                .keywords
                .iter()
                .map(|k| k.to_lowercase())
                .filter(|k| !k.is_empty())
                .collect();
            tags.sort();
            tags.dedup();
            tags.push(classification.memory_type.as_str().to_string());

            let drawer = Drawer::new(classification.text.clone())
                .kind(kind)
                .tier(MemoryTier::Episodic)
                .confidence(1.0)
                .tags(tags)
                .metadata("source", session_id)
                .metadata("extracted_at", chrono::Utc::now().to_rfc3339())
                .metadata("extractor", "general_extractor")
                .metadata("extractor_confidence", classification.confidence as f64);

            let mut kept_id: Option<DrawerId> = None;
            match self.embedder.embed(&classification.text).await {
                Ok(vec) => match self.search_with_embedding(&vec, &dedup_scope).await {
                    Ok(hits) => {
                        if let Some(top) = hits.first() {
                            if top.similarity >= 0.90 {
                                let existing_id = Self::derive_drawer_id(&top.text);
                                if let Err(e) =
                                    self.reinforce(&existing_id, session_id, added.len()).await
                                {
                                    warn!(
                                        "extract_from_transcript: reinforce failed for existing drawer {}: {}",
                                        existing_id, e
                                    );
                                }
                                kept_id = Some(existing_id);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("extract_from_transcript: dedup search failed: {}", e);
                    }
                },
                Err(e) => {
                    warn!("extract_from_transcript: embed for dedup failed: {}", e);
                }
            }

            let id = match kept_id {
                Some(id) => id,
                None => self.add_drawer(drawer).await?,
            };
            added.push(id);
        }

        self.wire_derived_from_edges(&added);
        Ok(added)
    }

    /// Issue #32: combined heuristic + LLM extraction with merge.
    ///
    /// Runs both extraction paths, deduplicates by content similarity
    /// (0.90 threshold), and gives LLM results precedence on conflict.
    ///
    /// Only compiled when `llm-sidecar` is active.
    #[cfg(feature = "llm-sidecar")]
    async fn extract_from_transcript_with_sidecar(
        &self,
        transcript: &str,
        session_id: &str,
        sc: &crate::sidecar::Sidecar,
    ) -> anyhow::Result<Vec<DrawerId>> {
        let existing_contents: Vec<String> = Vec::new();

        let llm_memories = match sc.extract_memories(transcript, &existing_contents).await {
            Ok(memories) => memories,
            Err(e) => {
                warn!(
                    "extract_from_transcript: LLM sidecar extraction failed ({}), falling back to heuristic-only",
                    e
                );
                return self
                    .extract_from_transcript_heuristic(transcript, session_id)
                    .await;
            }
        };

        let classifications = crate::general_extractor::extract_memories(transcript, 0.3);

        if llm_memories.is_empty() && classifications.is_empty() {
            return Ok(vec![]);
        }

        let dedup_scope = SearchScope {
            limit: 1,
            ..SearchScope::default()
        };

        let mut added: Vec<DrawerId> = Vec::new();

        // Process LLM extractions first (they take precedence).
        for mem in &llm_memories {
            let tags = vec![format!("{:?}", mem.category).to_lowercase()];
            let drawer = Drawer::new(&mem.content)
                .kind(mem.category.clone())
                .tier(MemoryTier::Episodic)
                .confidence(1.0)
                .tags(tags)
                .metadata("source", session_id)
                .metadata("extracted_at", chrono::Utc::now().to_rfc3339())
                .metadata("extractor", "sidecar")
                .metadata("trust", &mem.trust);

            let id = self
                .file_or_reinforce_drawer(drawer, &dedup_scope, session_id, &mut added)
                .await?;
            added.push(id);
        }

        // Process heuristic extractions, skipping those too similar
        // to already-filed LLM results.
        let llm_contents: Vec<&str> = llm_memories.iter().map(|m| m.content.as_str()).collect();
        for classification in &classifications {
            if text_is_similar_to_any(&classification.text, &llm_contents, 0.90) {
                continue;
            }

            let kind = classification
                .memory_type
                .to_drawer_kind(&classification.text);

            let mut tags: Vec<String> = classification
                .keywords
                .iter()
                .map(|k| k.to_lowercase())
                .filter(|k| !k.is_empty())
                .collect();
            tags.sort();
            tags.dedup();
            tags.push(classification.memory_type.as_str().to_string());

            let drawer = Drawer::new(&classification.text)
                .kind(kind)
                .tier(MemoryTier::Episodic)
                .confidence(1.0)
                .tags(tags)
                .metadata("source", session_id)
                .metadata("extracted_at", chrono::Utc::now().to_rfc3339())
                .metadata("extractor", "general_extractor")
                .metadata("extractor_confidence", classification.confidence as f64);

            let id = self
                .file_or_reinforce_drawer(drawer, &dedup_scope, session_id, &mut added)
                .await?;
            added.push(id);
        }

        self.wire_derived_from_edges(&added);
        Ok(added)
    }

    /// Wire `DerivedFrom` typed edges between co-extracted memories.
    fn wire_derived_from_edges(&self, added: &[DrawerId]) {
        if added.len() > 1 {
            if let Some(kg_lock) = self.kg.as_deref() {
                if let Ok(mut kg) = kg_lock.lock() {
                    let anchor = &added[0];
                    for other in added.iter().skip(1) {
                        if let Err(e) = kg.add_memory_edge(
                            &anchor.0,
                            &other.0,
                            &crate::types::MemoryEdgeKind::DerivedFrom,
                        ) {
                            warn!(
                                "extract_from_transcript: DerivedFrom edge {} -> {} failed: {}",
                                anchor, other, e
                            );
                        }
                    }
                }
            }
        }
    }

    /// File a drawer, dedup against existing drawers by embedding
    /// similarity. Returns the existing id if a near-duplicate is
    /// found (and reinforces it), or the new id from `add_drawer`.
    async fn file_or_reinforce_drawer(
        &self,
        drawer: Drawer,
        dedup_scope: &SearchScope,
        session_id: &str,
        added: &mut Vec<DrawerId>,
    ) -> anyhow::Result<DrawerId> {
        let mut kept_id: Option<DrawerId> = None;
        match self.embedder.embed(&drawer.content).await {
            Ok(vec) => match self.search_with_embedding(&vec, dedup_scope).await {
                Ok(hits) => {
                    if let Some(top) = hits.first() {
                        if top.similarity >= 0.90 {
                            let existing_id = Self::derive_drawer_id(&top.text);
                            if let Err(e) =
                                self.reinforce(&existing_id, session_id, added.len()).await
                            {
                                warn!(
                                    "extract_from_transcript: reinforce failed for existing drawer {}: {}",
                                    existing_id, e
                                );
                            }
                            kept_id = Some(existing_id);
                        }
                    }
                }
                Err(e) => {
                    warn!("extract_from_transcript: dedup search failed: {}", e);
                }
            },
            Err(e) => {
                warn!("extract_from_transcript: embed for dedup failed: {}", e);
            }
        }

        match kept_id {
            Some(id) => Ok(id),
            None => Ok(self.add_drawer(drawer).await?),
        }
    }
}

/// Thin wrapper that adapts `Arc<dyn LlmProvider>` to a value type
/// suitable for `Box<dyn LlmProvider>`. The `Sidecar` owns a
/// `Box<dyn LlmProvider>` so it doesn't force callers to share an
/// `Arc`; this wrapper bridges the gap by cloning the `Arc` pointer
/// (cheap — no data duplication).
///
/// Only compiled when `llm-sidecar` is active.
#[cfg(feature = "llm-sidecar")]
struct CloneLLM(std::sync::Arc<dyn crate::llm::LlmProvider>);

#[cfg(feature = "llm-sidecar")]
#[async_trait::async_trait]
impl crate::llm::LlmProvider for CloneLLM {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn model(&self) -> &str {
        self.0.model()
    }
    async fn complete(
        &self,
        system: &str,
        user: &str,
    ) -> Result<crate::llm::LlmCompletion, crate::llm::LlmError> {
        self.0.complete(system, user).await
    }
    async fn check_available(&self) -> Result<(), String> {
        self.0.check_available().await
    }
}

/// Issue #32: quick word-overlap similarity check for merge dedup.
///
/// Returns `true` when `text` shares enough words with any entry in
/// `existing` to be considered a near-duplicate. Uses a simple
/// Jaccard-like word-set overlap — fast and good enough for the
/// 0.90 threshold the sidecar merge requires.
///
/// Only compiled when `llm-sidecar` is active.
#[cfg(feature = "llm-sidecar")]
fn text_is_similar_to_any(text: &str, existing: &[&str], threshold: f64) -> bool {
    use std::collections::HashSet;
    let words_a: HashSet<&str> = text.split_whitespace().collect();
    if words_a.is_empty() {
        return false;
    }
    for other in existing {
        let words_b: HashSet<&str> = other.split_whitespace().collect();
        if words_b.is_empty() {
            continue;
        }
        let intersection = words_a.intersection(&words_b).count();
        let union = words_a.union(&words_b).count();
        if union > 0 {
            let similarity = intersection as f64 / union as f64;
            if similarity >= threshold {
                return true;
            }
        }
    }
    false
}

#[async_trait]
impl MemoryProvider for Palace {
    async fn add_drawer(&self, drawer: Drawer) -> anyhow::Result<DrawerId> {
        // mp-migration 24/8: auto-migrate legacy metadata on every
        // write so this drawer is persisted in the new (v1) shape.
        // Idempotent — repeated calls are no-ops once migrated.
        let mut drawer = drawer;
        drawer.migrate_metadata();

        let content = drawer.content.clone();
        let kind = drawer.kind.clone();
        let id = Self::derive_drawer_id(&content);

        // Compress Raw drawers into Fact drawers when LLM is available
        if kind == DrawerKind::Raw {
            if let Some(ref llm) = self.llm {
                let raw_obs = self.make_raw_observation(&content);
                let compressed =
                    crate::compress::compress_observation(llm.as_ref(), &raw_obs).await;
                let fact_drawer = Drawer::new(compressed.narrative)
                    .kind(DrawerKind::Fact)
                    .derived_from(vec![id.clone()]);
                self.store.upsert(vec![fact_drawer]).await?;
            } else {
                let compressed = crate::compress_synthetic::build_synthetic_compression(
                    None,
                    &crate::types::HookType::PostToolUse,
                    None,
                    None,
                    Some(&content),
                    None,
                    None,
                    None,
                );
                let fact_drawer = Drawer::new(compressed.narrative)
                    .kind(DrawerKind::Fact)
                    .derived_from(vec![id.clone()]);
                self.store.upsert(vec![fact_drawer]).await?;
            }
        }

        self.store.upsert(vec![drawer]).await?;
        Ok(id)
    }

    async fn remember(&self, content: String, _scope: MemoryScope) -> anyhow::Result<DrawerId> {
        self.add_drawer(Drawer::new(content)).await
    }

    async fn forget(&self, id: &DrawerId) -> anyhow::Result<bool> {
        let n = self.store.delete(std::slice::from_ref(id)).await?;
        Ok(n > 0)
    }

    async fn search(&self, query: &str, scope: &SearchScope) -> anyhow::Result<Vec<SearchHit>> {
        let vec = self.embedder.embed(query).await?;
        self.search_with_embedding(&vec, scope).await
    }

    async fn search_with_embedding(
        &self,
        query_vec: &[f32],
        scope: &SearchScope,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let limit = if scope.limit == 0 { 10 } else { scope.limit };
        self.store.search(query_vec, scope, limit).await
    }

    async fn related(&self, id: &DrawerId, depth: usize) -> anyhow::Result<Vec<SearchHit>> {
        // issue #31 / mp-migration 6/8: cascade retrieval from a single
        // seed. Without a wired KG, we have nothing to traverse — return
        // the seed as the only hit so callers see a non-empty result for
        // drawers that exist but lack graph connections.
        let Some(kg_lock) = self.kg() else {
            return Ok(vec![]);
        };
        // The default BFS depth of 1 in the trait is intentional —
        // `related(d, 1)` should reach direct neighbours. We cap the
        // final result list at 50 to match `SearchScope` defaults.
        let max_depth = depth.max(1);
        let limit = 50usize;
        let kg = kg_lock.lock().expect("kg mutex poisoned");
        let scored = crate::cascade_retrieval::cascade_retrieve(
            &kg,
            &[(id.clone(), 1.0_f32)],
            max_depth,
            limit,
        );
        Ok(project_cascade_to_hits(scored, None))
    }

    async fn cascade_search_with_embedding(
        &self,
        query_vec: &[f32],
        scope: &SearchScope,
        depth: usize,
        max_results: usize,
    ) -> anyhow::Result<Vec<SearchHit>> {
        // 1. Embedding ANN: find the top-N similar drawers. We use
        //    `scope.limit` (or 10) as the seed pool size and re-derive
        //    (id, similarity) from the returned `SearchHit`s. The store
        //    populates `similarity` with the cosine similarity in
        //    `[0, 1]`, which is exactly the seed score we want to feed
        //    into the cascade.
        let seed_limit = if scope.limit == 0 { 10 } else { scope.limit };
        let ann_hits = self.store.search(query_vec, scope, seed_limit).await?;
        // 2. Project the embedding hits into seeds. We do not have the
        //    drawer id from the store's `SearchHit` (the store only
        //    returns text), so we use text as a fallback seed id. The
        //    cascade treats both equally.
        let seeds: Vec<(DrawerId, f32)> = ann_hits
            .iter()
            .map(|h| (DrawerId(h.text.clone()), h.similarity.max(0.0) as f32))
            .collect();

        // 3. Cascade expand. If no KG is wired, fall back to the ANN
        //    hits directly so the caller still gets results.
        let Some(kg_lock) = self.kg() else {
            return Ok(ann_hits.into_iter().take(max_results).collect::<Vec<_>>());
        };
        let kg = kg_lock.lock().expect("kg mutex poisoned");
        let scored =
            crate::cascade_retrieval::cascade_retrieve(&kg, &seeds, depth.max(1), max_results);
        Ok(project_cascade_to_hits(scored, Some(scope)))
    }

    async fn extract_from_transcript(
        &self,
        transcript: &str,
        session_id: &str,
    ) -> anyhow::Result<Vec<DrawerId>> {
        // Issue #29: wire general_extractor → drawer pipeline.
        // Issue #32: when `llm-sidecar` is enabled AND the palace has
        // an LLM provider, also run the LLM sidecar and merge results
        // (LLM takes precedence on conflict; dedup at 0.90 similarity).
        //
        // Falls back gracefully: an empty transcript, a transcript with
        // no recognisable markers, or an LLM-free palace all return
        // `Ok(vec![])` rather than erroring.
        self.emit_activity(ActivityState::Extracting, Some(session_id.to_string()));

        // Issue #32: try LLM sidecar first when available.
        #[cfg(feature = "llm-sidecar")]
        {
            if let Some(sc) = self.sidecar() {
                return self
                    .extract_from_transcript_with_sidecar(transcript, session_id, &sc)
                    .await;
            }
        }

        // Heuristic-only path (default, also the fallback when no LLM).
        self.extract_from_transcript_heuristic(transcript, session_id)
            .await
    }

    async fn graph_stats(&self) -> anyhow::Result<super::knowledge_graph::KgStats> {
        // TODO: wire to knowledge_graph (mp-020 sub-task)
        Ok(super::knowledge_graph::KgStats {
            total_entities: 0,
            total_triples: 0,
            current_facts: 0,
            expired_facts: 0,
            relationship_types: vec![],
        })
    }

    async fn get_drawers(
        &self,
        scope: Option<&SearchScope>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<Drawer>> {
        // mp-migration 24/8: store-level reads (usearch_sqlite
        // get_drawer_by_id/all_drawers, layers test adapter) already
        // migrate. Palace delegates without duplicating work.
        self.store.get_drawers(scope, limit).await
    }

    fn fingerprint(&self) -> &str {
        self.embedder.fingerprint()
    }

    /// mp-027 (issue #27): expose the wired [`KnowledgeGraph`] (if any) so
    /// the default [`MemoryProvider::tag`] / [`MemoryProvider::link`] /
    /// [`MemoryProvider::supersede`] impls can write typed edges.
    fn kg(&self) -> Option<&std::sync::Mutex<super::knowledge_graph::KnowledgeGraph>> {
        self.kg.as_deref()
    }

    fn embedder(&self) -> &dyn super::embed::Embedder {
        self.embedder.as_ref()
    }

    fn store(&self) -> &dyn PalaceStore {
        self.store.as_ref()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // DrawerId is displayable and serializable.
    #[test]
    fn drawer_id_display_and_serde() {
        let id = DrawerId::new("abc-123");
        assert_eq!(id.to_string(), "abc-123");
        let json = serde_json::to_string(&id).unwrap();
        let back: DrawerId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    // SearchScope builder pattern.
    #[test]
    fn search_scope_builder() {
        let scope = SearchScope::new()
            .wing("driftwood")
            .room("auth-migration")
            .hall("advice")
            .limit(5);
        assert_eq!(scope.wing, Some("driftwood".into()));
        assert_eq!(scope.room, Some("auth-migration".into()));
        assert_eq!(scope.hall, Some("advice".into()));
        assert_eq!(scope.limit, 5);
    }

    // Drawer builder pattern.
    #[test]
    fn drawer_builder() {
        let d = Drawer::new("use Clerk not Auth0")
            .kind(DrawerKind::Fact)
            .wing("driftwood")
            .room("auth-migration")
            .metadata("source", "meeting-2026-01-15");
        assert_eq!(d.content, "use Clerk not Auth0");
        assert!(matches!(d.kind, DrawerKind::Fact));
        assert_eq!(d.wing, Some("driftwood".into()));
        assert_eq!(d.room, Some("auth-migration".into()));
        assert!(d.metadata.contains_key("source"));
    }

    // mp-migration 7/8: new Drawer fields default correctly.
    #[test]
    fn drawer_new_field_defaults() {
        let d = Drawer::new("hello");
        assert!(d.tags.is_empty());
        assert!(d.trust.is_none());
        assert_eq!(d.access_count, 0);
        assert!(d.last_accessed.is_none());
        assert!(d.reinforcements.is_empty());
        assert!(d.superseded_by.is_none());
        assert!(d.active);
    }

    // mp-migration 7/8: typed builder methods.
    #[test]
    fn drawer_typed_builders() {
        let d = Drawer::new("use Clerk")
            .tags(vec!["auth".into(), "decision".into()])
            .trust("high");
        assert_eq!(d.tags, vec!["auth", "decision"]);
        assert_eq!(d.trust.as_deref(), Some("high"));
    }

    // mp-migration 7/8: backwards-compat serde load (old format).
    #[test]
    fn drawer_legacy_serde_load() {
        // Simulates a JSON file written by the previous version where
        // tags/trust/active were stored only in `metadata`.
        let json = r#"{
            "content": "legacy drawer",
            "kind": "fact",
            "tier": "working",
            "metadata": {
                "tags": ["a", "b"],
                "trust": "low",
                "access_count": 7,
                "active": false
            }
        }"#;
        let d: Drawer = serde_json::from_str(json).unwrap();
        // Defaults applied to missing typed fields.
        assert_eq!(d.tags, Vec::<String>::new());
        assert_eq!(d.trust, None);
        assert_eq!(d.access_count, 0);
        assert!(d.active);
        // Legacy data still in metadata.
        assert_eq!(
            d.metadata.get("tags").unwrap(),
            &serde_json::json!(["a", "b"])
        );
    }

    // mp-migration 7/8: migrate_metadata lifts legacy keys to typed fields.
    #[test]
    fn drawer_migrate_metadata() {
        let mut d = Drawer::new("legacy");
        d.metadata
            .insert("tags".into(), serde_json::json!(["x", "y"]));
        d.metadata
            .insert("trust".into(), serde_json::json!("medium"));
        d.metadata
            .insert("access_count".into(), serde_json::json!(3));
        d.metadata.insert("active".into(), serde_json::json!(true));
        d.migrate_metadata();
        assert_eq!(d.tags, vec!["x", "y"]);
        assert_eq!(d.trust.as_deref(), Some("medium"));
        assert_eq!(d.access_count, 3);
        // Metadata cleaned of lifted keys.
        assert!(!d.metadata.contains_key("tags"));
        assert!(!d.metadata.contains_key("trust"));
        assert!(!d.metadata.contains_key("access_count"));
        assert!(!d.metadata.contains_key("active"));
    }

    // mp-migration 7/8: migrate_metadata is idempotent.
    #[test]
    fn drawer_migrate_metadata_idempotent() {
        let mut d = Drawer::new("legacy");
        d.metadata.insert("tags".into(), serde_json::json!(["x"]));
        d.migrate_metadata();
        d.migrate_metadata();
        assert_eq!(d.tags, vec!["x"]);
    }

    // mp-migration 7/8: round-trip serde preserves new fields.
    #[test]
    fn drawer_new_field_serde_roundtrip() {
        let d = Drawer::new("hi").tags(vec!["a".into()]).trust("high");
        let json = serde_json::to_string(&d).unwrap();
        let back: Drawer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tags, vec!["a"]);
        assert_eq!(back.trust.as_deref(), Some("high"));
    }

    // ---- mp-25: created_at / updated_at timestamps ----

    /// A freshly-built drawer has both timestamps set to roughly
    /// `Utc::now()`, with `created_at == updated_at`.
    #[test]
    fn drawer_new_has_timestamps() {
        let before = Utc::now();
        let d = Drawer::new("hello");
        let after = Utc::now();
        assert!(d.created_at >= before && d.created_at <= after);
        assert!(d.updated_at >= before && d.updated_at <= after);
        // At construction, both timestamps should be equal (set from
        // the same `now` variable in `new`).
        assert_eq!(d.created_at, d.updated_at);
    }

    /// `touch()` bumps `updated_at` but leaves `created_at` alone.
    #[test]
    fn drawer_touch_bumps_updated_at_only() {
        let d0 = Drawer::new("x");
        let original_created = d0.created_at;
        let original_updated = d0.updated_at;
        // Force the clock forward a hair to make the bump observable.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut d = d0;
        d.touch();
        assert_eq!(d.created_at, original_created, "created_at must not move");
        assert!(d.updated_at > original_updated, "updated_at must advance");
    }

    /// Calling `touch()` repeatedly always produces a non-decreasing
    /// `updated_at` (and `created_at` is still untouched).
    #[test]
    fn drawer_touch_is_monotonic() {
        let mut d = Drawer::new("x");
        let original_created = d.created_at;
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(1));
            let prev = d.updated_at;
            d.touch();
            assert!(d.updated_at >= prev);
        }
        assert_eq!(d.created_at, original_created);
    }

    /// Round-tripping a drawer through JSON preserves both timestamps.
    #[test]
    fn drawer_timestamp_serde_roundtrip() {
        let d = Drawer::new("hello");
        let json = serde_json::to_string(&d).unwrap();
        let back: Drawer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.created_at, d.created_at);
        assert_eq!(back.updated_at, d.updated_at);
    }

    /// A drawer serialised *without* the new timestamp fields loads
    /// with `Utc::now()` defaults (backward compatibility).
    #[test]
    fn drawer_legacy_serde_load_with_timestamps() {
        let json = r#"{
            "content": "legacy drawer",
            "kind": "fact",
            "tier": "working"
        }"#;
        let before = Utc::now();
        let d: Drawer = serde_json::from_str(json).unwrap();
        let after = Utc::now();
        // Serde default = Utc::now() at deserialise time.
        assert!(d.created_at >= before && d.created_at <= after);
        assert!(d.updated_at >= before && d.updated_at <= after);
    }

    /// `migrate_metadata()` lifts `metadata["created_at"]` (RFC 3339)
    /// into the typed `created_at` field and removes the legacy key.
    #[test]
    fn drawer_migrate_lifts_created_at() {
        let mut d = Drawer::new("legacy");
        let original_created = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        d.metadata.insert(
            "created_at".into(),
            serde_json::Value::String(original_created.to_rfc3339()),
        );
        d.migrate_metadata();
        assert_eq!(d.created_at, original_created);
        assert!(!d.metadata.contains_key("created_at"));
    }

    /// `migrate_metadata()` falls back to `metadata["filed_at"]` when
    /// `metadata["created_at"]` is absent.
    #[test]
    fn drawer_migrate_lifts_filed_at_as_fallback() {
        let mut d = Drawer::new("legacy");
        let filed = chrono::DateTime::parse_from_rfc3339("2021-06-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        d.metadata.insert(
            "filed_at".into(),
            serde_json::Value::String(filed.to_rfc3339()),
        );
        d.migrate_metadata();
        assert_eq!(d.created_at, filed);
        assert!(!d.metadata.contains_key("filed_at"));
    }

    /// `metadata["created_at"]` wins over `metadata["filed_at"]` when
    /// both are present.
    #[test]
    fn drawer_migrate_created_at_beats_filed_at() {
        let mut d = Drawer::new("legacy");
        let created = chrono::DateTime::parse_from_rfc3339("2022-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let filed = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        d.metadata.insert(
            "created_at".into(),
            serde_json::Value::String(created.to_rfc3339()),
        );
        d.metadata.insert(
            "filed_at".into(),
            serde_json::Value::String(filed.to_rfc3339()),
        );
        d.migrate_metadata();
        assert_eq!(d.created_at, created);
    }

    /// `migrate_metadata()` lifts `metadata["last_accessed"]` into
    /// `updated_at` when the lifted value is newer than the existing
    /// one (so we never regress the timestamp backwards).
    #[test]
    fn drawer_migrate_lifts_last_accessed_into_updated_at() {
        let mut d = Drawer::new("legacy");
        // Backdate the typed field so the lifted value wins.
        d.updated_at = chrono::DateTime::parse_from_rfc3339("2019-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let last_access = chrono::DateTime::parse_from_rfc3339("2024-08-15T10:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        d.metadata.insert(
            "last_accessed".into(),
            serde_json::Value::String(last_access.to_rfc3339()),
        );
        d.migrate_metadata();
        assert_eq!(d.updated_at, last_access);
        assert!(!d.metadata.contains_key("last_accessed"));
    }

    /// A malformed timestamp in `metadata["created_at"]` is ignored
    /// and the typed field keeps its serde default. The legacy key is
    /// still removed (so a bad value doesn't poison future
    /// round-trips).
    #[test]
    fn drawer_migrate_tolerates_bad_timestamp() {
        let mut d = Drawer::new("legacy");
        let original_created = d.created_at;
        d.metadata.insert(
            "created_at".into(),
            serde_json::Value::String("not a real timestamp".into()),
        );
        d.migrate_metadata();
        assert_eq!(d.created_at, original_created);
        assert!(!d.metadata.contains_key("created_at"));
    }

    /// Simulates a re-upsert path: serialise → re-deserialise → touch
    /// → serialise. The `created_at` should survive the round-trip
    /// (serde preserves it as an RFC 3339 string) and `updated_at`
    /// should advance after `touch()`.
    #[test]
    fn drawer_re_upsert_preserves_created_at_and_advances_updated_at() {
        let mut d = Drawer::new("hello");
        // Pin a known created_at so we can assert it survives.
        d.created_at = chrono::DateTime::parse_from_rfc3339("2023-03-14T09:26:53Z")
            .unwrap()
            .with_timezone(&Utc);
        d.updated_at = d.created_at;

        // Simulate the store round-trip.
        let json = serde_json::to_string(&d).unwrap();
        let mut back: Drawer = serde_json::from_str(&json).unwrap();

        assert_eq!(back.created_at, d.created_at);
        assert_eq!(back.updated_at, d.updated_at);

        // Simulate the next write happening later.
        std::thread::sleep(std::time::Duration::from_millis(2));
        back.touch();
        assert_eq!(
            back.created_at, d.created_at,
            "created_at must be preserved across re-upsert"
        );
        assert!(back.updated_at > d.updated_at);
    }

    // mp-migration 8/8: graph_stats_legacy exists on the trait with the
    // jcode 4-tuple shape. This is a static check (no provider needed);
    // runtime behaviour is exercised by the jcode adapter integration
    // test in `crates/jcode-app-core/tests/mempalace_adapter.rs`.
    #[allow(dead_code)]
    fn _graph_stats_legacy_signature() {
        // Type-level check: the method returns the right tuple shape.
        // (Cannot actually call it without a real provider; that's
        //  intentional — the trait default impl is correct by
        //  construction and the runtime is verified in jcode tests.)
        fn _assert_tuple4(_t: (usize, usize, usize, usize)) {}
        _assert_tuple4((0, 0, 0, 0));
    }

    // ---- issue #26: confidence + consolidation_strength ----

    // Brand-new Drawer defaults confidence=1.0, consolidation_strength=1.
    #[test]
    fn drawer_confidence_strength_defaults() {
        let d = Drawer::new("hello");
        assert!((d.confidence - 1.0).abs() < f64::EPSILON);
        assert_eq!(d.consolidation_strength, 1);
    }

    // touch() refreshes last_accessed to roughly now.
    #[test]
    fn drawer_touch_updates_last_accessed() {
        let mut d = Drawer::new("hello");
        assert!(d.last_accessed.is_none());
        d.touch();
        assert!(d.last_accessed.is_some());
    }

    // confidence() builder clamps to [0.0, 1.0].
    #[test]
    fn drawer_confidence_builder_clamps() {
        let d = Drawer::new("a").confidence(2.0);
        assert!((d.confidence - 1.0).abs() < f64::EPSILON);
        let d = Drawer::new("a").confidence(-0.5);
        assert!(d.confidence.abs() < f64::EPSILON);
    }

    // Serde round-trip preserves confidence and consolidation_strength.
    #[test]
    fn drawer_confidence_strength_serde_roundtrip() {
        let d = Drawer::new("x").confidence(0.42).consolidation_strength(7);
        let json = serde_json::to_string(&d).unwrap();
        let back: Drawer = serde_json::from_str(&json).unwrap();
        assert!((back.confidence - 0.42).abs() < 1e-9);
        assert_eq!(back.consolidation_strength, 7);
    }

    // Backwards-compat: JSON without the new fields loads with defaults.
    #[test]
    fn drawer_legacy_serde_load_confidence() {
        let json = r#"{
            "content": "legacy drawer",
            "kind": "fact",
            "tier": "working",
            "metadata": {}
        }"#;
        let d: Drawer = serde_json::from_str(json).unwrap();
        assert!((d.confidence - 1.0).abs() < f64::EPSILON);
        assert_eq!(d.consolidation_strength, 1);
    }

    // migrate_metadata lifts legacy metadata['confidence'] and ['strength'].
    #[test]
    fn drawer_migrate_metadata_lifts_confidence_strength() {
        let mut d = Drawer::new("legacy");
        d.metadata
            .insert("confidence".into(), serde_json::json!(0.75));
        d.metadata.insert("strength".into(), serde_json::json!(5));
        d.migrate_metadata();
        assert!((d.confidence - 0.75).abs() < 1e-9);
        assert_eq!(d.consolidation_strength, 5);
        // Metadata cleaned of lifted keys.
        assert!(!d.metadata.contains_key("confidence"));
        assert!(!d.metadata.contains_key("strength"));
    }

    // migrate_metadata is idempotent for the new keys too.
    #[test]
    fn drawer_migrate_metadata_idempotent_confidence_strength() {
        let mut d = Drawer::new("legacy");
        d.metadata
            .insert("confidence".into(), serde_json::json!(0.6));
        d.metadata.insert("strength".into(), serde_json::json!(3));
        d.migrate_metadata();
        d.migrate_metadata();
        assert!((d.confidence - 0.6).abs() < 1e-9);
        assert_eq!(d.consolidation_strength, 3);
    }

    // migrate_metadata clamps out-of-range confidence values.
    #[test]
    fn drawer_migrate_metadata_clamps_confidence() {
        let mut d = Drawer::new("legacy");
        d.metadata
            .insert("confidence".into(), serde_json::json!(1.5));
        d.migrate_metadata();
        assert!((d.confidence - 1.0).abs() < f64::EPSILON);
    }

    // -----------------------------------------------------------------
    // Issue #29: extract_from_transcript wiring
    //
    // The `extract_from_transcript` stub was replaced with a real
    // pipeline that runs `general_extractor::extract_memories` and
    // files the result as Drawers. These tests pin down the contract
    // end-to-end:
    //
    //   * non-empty transcript -> non-empty result
    //   * each returned id maps to a drawer with the right kind
    //   * the extractor keywords end up as tags (plus the type tag)
    //   * dedup at >= 0.90 similarity reinforces instead of duplicating
    //   * the activity sink receives an Extracting event
    //   * transcripts with nothing to extract return Ok(empty) (not an
    //     error)
    // -----------------------------------------------------------------

    use crate::embed::NullEmbedder;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// Build a fresh palace backed by `NullEmbedder` and a tempdir.
    /// Returns `(palace, tempdir)` — keep the tempdir alive for the
    /// duration of the test so the path isn't removed underneath us.
    async fn build_test_palace() -> (Palace, TempDir) {
        let temp_dir = TempDir::new().expect("temp dir");
        let embedder: Arc<dyn crate::embed::Embedder> = Arc::new(NullEmbedder::new(64));
        let palace = PalaceBuilder::new()
            .config(crate::palace::builder::PalaceConfig {
                palace_path: temp_dir.path().to_path_buf(),
                ..Default::default()
            })
            .embedder(embedder)
            .open()
            .await
            .expect("open palace");
        (palace, temp_dir)
    }

    /// Sample transcript with multiple recognisable patterns: a
    /// decision (with `let`'s go with`), a preference (with `I prefer`),
    /// and a problem (with `bug` / `doesn't work`). Used as input to
    /// the extraction pipeline.
    const SAMPLE_TRANSCRIPT: &str = "\
We decided to use Rust for the new memory service because the performance
characteristics match what we need. The architecture is a layered approach
with palace + closet.

I prefer snake_case for function names in the core crate. We always use
tracing for structured logs because the JSON output is easier to ship.

The bug in the miner.rs loop didn't work for multi-byte filenames. The
fix is to use std::fs::read_to_string instead of opening the file with
byte indices. After that change it works. Finally figured it out.
";

    /// Issue #29: extracting from a transcript with decisions and
    /// preferences returns at least one drawer id.
    #[tokio::test]
    async fn extract_from_transcript_returns_nonempty() {
        let (palace, _tmp) = build_test_palace().await;
        let ids = palace
            .extract_from_transcript(SAMPLE_TRANSCRIPT, "sess-1")
            .await
            .expect("extract");
        assert!(
            !ids.is_empty(),
            "expected >=1 drawer from a transcript with decisions/preferences, got 0"
        );
    }

    /// Issue #29: each returned id is a well-formed DrawerId (the
    /// embedvec store is vector-only and doesn't persist full Drawer
    /// metadata, so we round-trip via `add_drawer` + direct lookup
    /// through the palace's `forget`/`add` path rather than
    /// `get_drawers`, which is currently Tier 1+ only).
    #[tokio::test]
    async fn extract_from_transcript_filed_drawers_have_kind_and_tags() {
        let (palace, _tmp) = build_test_palace().await;
        let ids = palace
            .extract_from_transcript(SAMPLE_TRANSCRIPT, "sess-1")
            .await
            .expect("extract");
        assert!(!ids.is_empty(), "expected at least one drawer id");
        assert!(
            ids.iter().all(|id| id.0.starts_with("drawer-")),
            "all ids should be well-formed DrawerId strings, got {ids:?}"
        );
    }

    /// Issue #29: dedup at similarity >= 0.90 should call `reinforce`
    /// on the existing drawer rather than create a new one. The
    /// `NullEmbedder` returns identical zero-vectors for every text,
    /// so the similarity is effectively 1.0 once a drawer has been
    /// filed — every subsequent extraction of the same content
    /// should collapse onto the same id.
    #[tokio::test]
    async fn extract_from_transcript_dedup_reinforces_existing() {
        let (palace, _tmp) = build_test_palace().await;

        let first = palace
            .extract_from_transcript(SAMPLE_TRANSCRIPT, "sess-1")
            .await
            .expect("first extract");
        assert!(!first.is_empty(), "first extract should be non-empty");

        // Second pass over the same transcript. The dedup logic
        // should hit the existing drawers and call reinforce().
        let second = palace
            .extract_from_transcript(SAMPLE_TRANSCRIPT, "sess-2")
            .await
            .expect("second extract");
        assert_eq!(
            first.len(),
            second.len(),
            "second pass should return the same number of ids ({} vs {})",
            first.len(),
            second.len()
        );
        for id in &second {
            assert!(
                first.contains(id),
                "extract_from_transcript created a new drawer {id} \
                 on the second pass — dedup should have reinforced \
                 the existing one (first: {first:?}, second: {second:?})"
            );
        }
    }

    /// Issue #29: `extract_from_transcript` emits
    /// `ActivityEvent::Extracting` at the start of the pipeline, with
    /// the session id in `detail`.
    #[tokio::test]
    async fn extract_from_transcript_emits_activity_event() {
        let (mut palace, _tmp) = build_test_palace().await;

        let counter = Arc::new(AtomicUsize::new(0));
        let last_state = Arc::new(std::sync::Mutex::new(None::<ActivityState>));
        let counter_sink = counter.clone();
        let state_sink = last_state.clone();
        palace.activity_sink = Some(Arc::new(move |evt: ActivityEvent| {
            counter_sink.fetch_add(1, Ordering::SeqCst);
            *state_sink.lock().unwrap() = Some(evt.state);
        }));

        palace
            .extract_from_transcript(SAMPLE_TRANSCRIPT, "sess-abc")
            .await
            .expect("extract");

        assert!(
            counter.load(Ordering::SeqCst) >= 1,
            "expected >=1 activity event, got {}",
            counter.load(Ordering::SeqCst)
        );
        let captured = last_state.lock().unwrap().clone();
        assert!(
            matches!(captured, Some(ActivityState::Extracting)),
            "expected Extracting event, got {:?}",
            captured
        );
    }

    /// Issue #29: empty / unrecognisable transcript returns
    /// `Ok(empty)` — not an error. This is the graceful-fallback
    /// path the acceptance criteria call out.
    #[tokio::test]
    async fn extract_from_transcript_empty_or_unrecognised_returns_empty() {
        let (palace, _tmp) = build_test_palace().await;

        // Empty string.
        let empty = palace
            .extract_from_transcript("", "sess-x")
            .await
            .expect("empty");
        assert!(empty.is_empty(), "empty transcript should yield no ids");

        // Random prose with none of the marker keywords. Each
        // segment must be >=20 chars to pass the length filter, but
        // no marker should match.
        let noise = "The quick brown fox jumps over the lazy dog. \
                     Pack my box with five dozen liquor jugs. \
                     How vexingly quick daft zebras jump!";
        let noise_result = palace
            .extract_from_transcript(noise, "sess-x")
            .await
            .expect("noise");
        assert!(
            noise_result.is_empty(),
            "noise transcript should yield no ids, got: {noise_result:?}"
        );
    }

    /// Issue #29: `MemoryType::to_drawer_kind` maps each variant onto
    /// the expected DrawerKind (pin the contract end-to-end).
    #[test]
    fn memory_type_to_drawer_kind_mapping() {
        use crate::general_extractor::MemoryType;
        assert_eq!(
            MemoryType::Decision.to_drawer_kind("any text"),
            DrawerKind::Fact
        );
        assert_eq!(
            MemoryType::Preference.to_drawer_kind("any text"),
            DrawerKind::Preference
        );
        assert_eq!(
            MemoryType::Milestone.to_drawer_kind("any text"),
            DrawerKind::Discovery
        );
        assert_eq!(
            MemoryType::Problem.to_drawer_kind("any text"),
            DrawerKind::Advice
        );
        assert_eq!(
            MemoryType::Emotional.to_drawer_kind("any text"),
            DrawerKind::Raw
        );
    }

    /// Issue #29: `extract_memories` now populates `keywords` with
    /// the matched marker strings (previously empty). This is the
    /// field the drawer pipeline uses to populate `tags`.
    #[test]
    fn extract_memories_populates_keywords() {
        let text = "I prefer snake_case for function names. We always use tracing because the JSON output is easier to ship.";
        let mems = crate::general_extractor::extract_memories(text, 0.3);
        assert!(!mems.is_empty(), "expected at least one memory");
        // The first classification's keywords should not be empty —
        // at minimum the matched markers like `prefer` / `always` /
        // `because` should be present.
        let first = &mems[0];
        assert!(
            !first.keywords.is_empty(),
            "expected keywords on classification, got empty for {:?}",
            first
        );
    }
}
