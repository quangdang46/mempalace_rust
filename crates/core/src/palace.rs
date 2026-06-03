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
use std::sync::Arc;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
    /// IDs of drawers this drawer was derived from (mp-052 / ADR-10 / ADR-13).
    /// Populated during AAAK compression (derived drawer ← original drawer) and
    /// general extraction (extracted fact ← source conversation). Enables
    /// citation chains: "I used drawer #42 which came from session #abc-123".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<DrawerId>,
}

impl Drawer {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            id: None,
            content: content.into(),
            kind: DrawerKind::default(),
            tier: MemoryTier::default(),
            wing: None,
            room: None,
            metadata: std::collections::HashMap::new(),
            derived_from: Vec::new(),
        }
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
    async fn search_with_embedding(
        &self,
        query_vec: &[f32],
        scope: &SearchScope,
    ) -> anyhow::Result<Vec<SearchHit>>;

    /// Retrieve drawers related to a given drawer by following graph
    /// edges (tunnel + hall connections). `depth` controls how many
    /// hops to follow (1 = direct neighbours only).
    async fn related(&self, id: &DrawerId, depth: usize) -> anyhow::Result<Vec<SearchHit>>;

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
}

#[async_trait]
impl MemoryProvider for Palace {
    async fn add_drawer(&self, drawer: Drawer) -> anyhow::Result<DrawerId> {
        let content = drawer.content.clone();
        let kind = drawer.kind;
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

    async fn related(&self, _id: &DrawerId, _depth: usize) -> anyhow::Result<Vec<SearchHit>> {
        // TODO: follow palace graph edges (mp-020 sub-task)
        Ok(vec![])
    }

    async fn extract_from_transcript(
        &self,
        _transcript: &str,
        _session_id: &str,
    ) -> anyhow::Result<Vec<DrawerId>> {
        // TODO: wire to convo_miner + general_extractor (mp-061)
        Ok(vec![])
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
        self.store.get_drawers(scope, limit).await
    }

    fn fingerprint(&self) -> &str {
        self.embedder.fingerprint()
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
}
