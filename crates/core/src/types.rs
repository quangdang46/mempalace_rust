//! AgentMemory-equivalent types for mempalace_rust.
//!
//! Maps ALL TypeScript types from agentmemory's `src/types.ts` to Rust
//! structs/enums with Serde derive macros. This is the foundational type
//! system that every other module depends on.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Enums (15 total)
// ============================================================================

/// Type of observation captured from agent lifecycle hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationType {
    FileRead,
    FileWrite,
    FileCreate,
    FileEdit,
    FileDelete,
    CommandRun,
    CommandExec,
    GitOperation,
    Search,
    WebFetch,
    ToolUse,
    ToolFailure,
    Conversation,
    UserPrompt,
    AssistantResponse,
    SessionStart,
    SessionEnd,
    SubagentStart,
    SubagentStop,
    Subagent,
    Notification,
    Error,
    Decision,
    Discovery,
    Task,
    Other,
}

impl std::fmt::Display for ObservationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileRead => write!(f, "file_read"),
            Self::FileWrite => write!(f, "file_write"),
            Self::FileCreate => write!(f, "file_create"),
            Self::FileEdit => write!(f, "file_edit"),
            Self::FileDelete => write!(f, "file_delete"),
            Self::CommandRun => write!(f, "command_run"),
            Self::CommandExec => write!(f, "command_exec"),
            Self::GitOperation => write!(f, "git_operation"),
            Self::Search => write!(f, "search"),
            Self::WebFetch => write!(f, "web_fetch"),
            Self::ToolUse => write!(f, "tool_use"),
            Self::ToolFailure => write!(f, "tool_failure"),
            Self::Conversation => write!(f, "conversation"),
            Self::UserPrompt => write!(f, "user_prompt"),
            Self::AssistantResponse => write!(f, "assistant_response"),
            Self::SessionStart => write!(f, "session_start"),
            Self::SessionEnd => write!(f, "session_end"),
            Self::SubagentStart => write!(f, "subagent_start"),
            Self::SubagentStop => write!(f, "subagent_stop"),
            Self::Subagent => write!(f, "subagent"),
            Self::Notification => write!(f, "notification"),
            Self::Error => write!(f, "error"),
            Self::Decision => write!(f, "decision"),
            Self::Discovery => write!(f, "discovery"),
            Self::Task => write!(f, "task"),
            Self::Other => write!(f, "other"),
        }
    }
}

impl std::str::FromStr for ObservationType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "file_read" => Ok(Self::FileRead),
            "file_write" => Ok(Self::FileWrite),
            "file_create" => Ok(Self::FileCreate),
            "file_edit" => Ok(Self::FileEdit),
            "file_delete" => Ok(Self::FileDelete),
            "command_run" => Ok(Self::CommandRun),
            "command_exec" => Ok(Self::CommandExec),
            "git_operation" => Ok(Self::GitOperation),
            "search" => Ok(Self::Search),
            "web_fetch" => Ok(Self::WebFetch),
            "tool_use" => Ok(Self::ToolUse),
            "tool_failure" => Ok(Self::ToolFailure),
            "conversation" => Ok(Self::Conversation),
            "user_prompt" => Ok(Self::UserPrompt),
            "assistant_response" => Ok(Self::AssistantResponse),
            "session_start" => Ok(Self::SessionStart),
            "session_end" => Ok(Self::SessionEnd),
            "subagent_start" => Ok(Self::SubagentStart),
            "subagent_stop" => Ok(Self::SubagentStop),
            "subagent" => Ok(Self::Subagent),
            "notification" => Ok(Self::Notification),
            "error" => Ok(Self::Error),
            "decision" => Ok(Self::Decision),
            "discovery" => Ok(Self::Discovery),
            "task" => Ok(Self::Task),
            "other" => Ok(Self::Other),
            _ => Err(format!("unknown ObservationType: {s}")),
        }
    }
}

/// Lifecycle hook types that trigger memory operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookType {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PreCompact,
    SubagentStart,
    SubagentStop,
    Stop,
    SessionEnd,
    Notification,
    TaskCompleted,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionStart => write!(f, "session_start"),
            Self::UserPromptSubmit => write!(f, "user_prompt_submit"),
            Self::PreToolUse => write!(f, "pre_tool_use"),
            Self::PostToolUse => write!(f, "post_tool_use"),
            Self::PostToolUseFailure => write!(f, "post_tool_use_failure"),
            Self::PreCompact => write!(f, "pre_compact"),
            Self::SubagentStart => write!(f, "subagent_start"),
            Self::SubagentStop => write!(f, "subagent_stop"),
            Self::Stop => write!(f, "stop"),
            Self::SessionEnd => write!(f, "session_end"),
            Self::Notification => write!(f, "notification"),
            Self::TaskCompleted => write!(f, "task_completed"),
        }
    }
}

impl std::str::FromStr for HookType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "session_start" => Ok(Self::SessionStart),
            "user_prompt_submit" => Ok(Self::UserPromptSubmit),
            "pre_tool_use" => Ok(Self::PreToolUse),
            "post_tool_use" => Ok(Self::PostToolUse),
            "post_tool_use_failure" => Ok(Self::PostToolUseFailure),
            "pre_compact" => Ok(Self::PreCompact),
            "subagent_start" => Ok(Self::SubagentStart),
            "subagent_stop" => Ok(Self::SubagentStop),
            "stop" => Ok(Self::Stop),
            "session_end" => Ok(Self::SessionEnd),
            "notification" => Ok(Self::Notification),
            "task_completed" => Ok(Self::TaskCompleted),
            _ => Err(format!("unknown HookType: {s}")),
        }
    }
}

/// Classification of persisted memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Working,
    Episodic,
    Semantic,
    Procedural,
    Insight,
    Lesson,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Working => write!(f, "working"),
            Self::Episodic => write!(f, "episodic"),
            Self::Semantic => write!(f, "semantic"),
            Self::Procedural => write!(f, "procedural"),
            Self::Insight => write!(f, "insight"),
            Self::Lesson => write!(f, "lesson"),
        }
    }
}

impl std::str::FromStr for MemoryType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "working" => Ok(Self::Working),
            "episodic" => Ok(Self::Episodic),
            "semantic" => Ok(Self::Semantic),
            "procedural" => Ok(Self::Procedural),
            "insight" => Ok(Self::Insight),
            "lesson" => Ok(Self::Lesson),
            _ => Err(format!("unknown MemoryType: {s}")),
        }
    }
}

/// Four-tier memory consolidation levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidationTier {
    Working,
    Episodic,
    Semantic,
    Procedural,
}

/// Node types in the knowledge graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphNodeType {
    File,
    Function,
    Concept,
    Error,
    Decision,
    Pattern,
    Library,
    Person,
    Project,
    Preference,
    Location,
    Organization,
    Event,
}

/// Edge types connecting knowledge graph nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphEdgeType {
    Uses,
    Imports,
    Modifies,
    Causes,
    Fixes,
    DependsOn,
    RelatedTo,
    Prefers,
    BlockedBy,
    CausedBy,
    OptimizesFor,
    Rejected,
    Avoids,
    LocatedIn,
    SucceededBy,
    Implements,
}

/// Memory-specific edge kinds used by `KnowledgeGraph`.
///
/// jcode's `MemoryGraph` has six typed edge kinds with traversal weights that
/// drive cascade retrieval. mempalace stores them in the `triples` table with
/// dedicated `edge_kind` and `weight` columns so callers can filter and
/// weight edges without parsing the predicate string.
///
/// `RelatesTo` carries its own per-edge weight; the other variants use the
/// canonical traversal weights returned by [`MemoryEdgeKind::traversal_weight`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MemoryEdgeKind {
    HasTag,
    InCluster,
    RelatesTo { weight: f32 },
    Supersedes,
    Contradicts,
    DerivedFrom,
}

impl MemoryEdgeKind {
    /// Default traversal weight for this edge kind. Used during cascade
    /// retrieval and as the stored `weight` column for kinds that don't
    /// carry their own per-edge weight. `RelatesTo` returns the user-supplied
    /// weight, defaulting to 1.0.
    pub fn traversal_weight(&self) -> f32 {
        match self {
            Self::HasTag => 0.8,
            Self::InCluster => 0.6,
            Self::RelatesTo { weight } => *weight,
            Self::Supersedes => 0.9,
            Self::Contradicts => 0.3,
            Self::DerivedFrom => 0.7,
        }
    }

    /// Stable name used for the `edge_kind` column and the `predicate` field
    /// in the triples table. `RelatesTo` collapses to `"relates_to"` so the
    /// per-edge weight lives in the dedicated `weight` column rather than
    /// being encoded into the predicate.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HasTag => "has_tag",
            Self::InCluster => "in_cluster",
            Self::RelatesTo { .. } => "relates_to",
            Self::Supersedes => "supersedes",
            Self::Contradicts => "contradicts",
            Self::DerivedFrom => "derived_from",
        }
    }

    /// Parse a stored `edge_kind` column back into a typed edge kind. For
    /// `RelatesTo`, the per-edge weight comes from the separate `weight`
    /// column; this method only recovers the variant shape and uses a
    /// default weight of 1.0. Use [`MemoryEdgeKind::from_kind_and_weight`]
    /// when the weight is also known.
    pub fn from_kind_str(s: &str) -> Option<Self> {
        match s {
            "has_tag" | "HasTag" => Some(Self::HasTag),
            "in_cluster" | "InCluster" => Some(Self::InCluster),
            "relates_to" | "RelatesTo" => Some(Self::RelatesTo { weight: 1.0 }),
            "supersedes" | "Supersedes" => Some(Self::Supersedes),
            "contradicts" | "Contradicts" => Some(Self::Contradicts),
            "derived_from" | "DerivedFrom" => Some(Self::DerivedFrom),
            _ => None,
        }
    }

    /// Parse a stored `(edge_kind, weight)` pair back into a typed edge kind.
    /// The weight is honoured for `RelatesTo` and ignored for the fixed-weight
    /// variants (the canonical `traversal_weight` is always returned).
    pub fn from_kind_and_weight(kind: &str, _weight: Option<f64>) -> Option<Self> {
        match kind {
            "has_tag" | "HasTag" => Some(Self::HasTag),
            "in_cluster" | "InCluster" => Some(Self::InCluster),
            "relates_to" | "RelatesTo" => Some(Self::RelatesTo {
                weight: _weight.map(|w| w as f32).unwrap_or(1.0),
            }),
            "supersedes" | "Supersedes" => Some(Self::Supersedes),
            "contradicts" | "Contradicts" => Some(Self::Contradicts),
            "derived_from" | "DerivedFrom" => Some(Self::DerivedFrom),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryEdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Edge types specific to action graphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionEdgeType {
    Blocks,
    BlockedBy,
    DependsOn,
    RelatesTo,
    Supersedes,
}

impl std::fmt::Display for ActionEdgeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocks => write!(f, "blocks"),
            Self::BlockedBy => write!(f, "blocked_by"),
            Self::DependsOn => write!(f, "depends_on"),
            Self::RelatesTo => write!(f, "relates_to"),
            Self::Supersedes => write!(f, "supersedes"),
        }
    }
}

/// Lifecycle status of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    Blocked,
}

impl std::fmt::Display for ActionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

impl std::str::FromStr for ActionStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "blocked" | "Blocked" => Ok(Self::Blocked),
            _ => Err(format!("unknown ActionStatus: {s}")),
        }
    }
}

/// Types of inter-agent signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    Info,
    Request,
    Response,
    Alert,
    Handoff,
}

impl std::fmt::Display for SignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Request => write!(f, "request"),
            Self::Response => write!(f, "response"),
            Self::Alert => write!(f, "alert"),
            Self::Handoff => write!(f, "handoff"),
        }
    }
}

impl std::str::FromStr for SignalType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "info" => Ok(Self::Info),
            "request" => Ok(Self::Request),
            "response" => Ok(Self::Response),
            "alert" => Ok(Self::Alert),
            "handoff" => Ok(Self::Handoff),
            _ => Err(format!("unknown SignalType: {s}")),
        }
    }
}

/// Types of checkpoints for blocking conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointType {
    Ci,
    Approval,
    Deploy,
    Timer,
    Manual,
}

impl std::fmt::Display for CheckpointType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ci => write!(f, "ci"),
            Self::Approval => write!(f, "approval"),
            Self::Deploy => write!(f, "deploy"),
            Self::Timer => write!(f, "timer"),
            Self::Manual => write!(f, "manual"),
        }
    }
}

impl std::str::FromStr for CheckpointType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ci" => Ok(Self::Ci),
            "approval" => Ok(Self::Approval),
            "deploy" => Ok(Self::Deploy),
            "timer" => Ok(Self::Timer),
            "manual" => Ok(Self::Manual),
            _ => Err(format!("unknown CheckpointType: {s}")),
        }
    }
}

/// Status of a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointStatus {
    Pending,
    Passed,
    Failed,
    Skipped,
}

impl std::fmt::Display for CheckpointStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Passed => write!(f, "passed"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

impl std::str::FromStr for CheckpointStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            _ => Err(format!("unknown CheckpointStatus: {s}")),
        }
    }
}

/// Types of sentinels (watch triggers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SentinelType {
    Webhook,
    Timer,
    Threshold,
    Pattern,
    StateChange,
    Custom,
}

/// Circuit breaker state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// Agent scope mode for multi-agent coordination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentScopeMode {
    Shared,
    Private,
}

/// Team memory mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamMode {
    Cooperative,
    Competitive,
}

// ============================================================================
// Core Structs (~30 total)
// ============================================================================

/// A coding session with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project: String,
    pub cwd: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: String,
    pub observation_count: usize,
    pub model: Option<String>,
    pub tags: Vec<String>,
    pub first_prompt: Option<String>,
    pub summary: Option<String>,
    pub commit_shas: Vec<String>,
    pub agent_id: Option<String>,
}

impl Session {
    pub fn new(id: impl Into<String>, project: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            project: project.into(),
            cwd: cwd.into(),
            started_at: Utc::now(),
            ended_at: None,
            status: "active".to_string(),
            observation_count: 0,
            model: None,
            tags: Vec::new(),
            first_prompt: None,
            summary: None,
            commit_shas: Vec::new(),
            agent_id: None,
        }
    }
}

/// Raw, unprocessed observation from a lifecycle hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    pub id: String,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub hook_type: HookType,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    pub tool_output: Option<String>,
    pub user_prompt: Option<String>,
    pub assistant_response: Option<String>,
    pub raw: Option<String>,
    pub modality: String,
    pub image_data: Option<ImageData>,
    pub agent_id: Option<String>,
}

/// Compressed, structured observation distilled by LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedObservation {
    pub id: String,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub observation_type: ObservationType,
    pub title: String,
    pub subtitle: Option<String>,
    pub facts: Vec<String>,
    pub narrative: String,
    pub concepts: Vec<String>,
    pub files: Vec<String>,
    pub importance: u8,
    pub confidence: f64,
    pub image_ref: Option<String>,
    pub image_description: Option<String>,
    pub modality: String,
    pub agent_id: Option<String>,
}

/// A consolidated memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub concepts: Vec<String>,
    pub files: Vec<String>,
    pub session_ids: Vec<String>,
    pub strength: f64,
    pub version: u32,
    pub parent_id: Option<String>,
    pub supersedes: Vec<String>,
    pub related_ids: Vec<String>,
    pub source_observation_ids: Vec<String>,
    pub is_latest: bool,
    pub forget_after: Option<DateTime<Utc>>,
    pub image_ref: Option<String>,
    pub agent_id: Option<String>,
    pub project: String,
}

/// Semantic memory — extracted facts and patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticMemory {
    pub id: String,
    pub facts: Vec<String>,
    pub concepts: Vec<String>,
    pub confidence: f64,
    pub source_memory_id: String,
    pub created_at: DateTime<Utc>,
}

/// Procedural memory — workflows and decision patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProceduralMemory {
    pub id: String,
    pub workflow: String,
    pub steps: Vec<String>,
    pub triggers: Vec<String>,
    pub source_memory_id: String,
    pub created_at: DateTime<Utc>,
}

/// Relationship between memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRelation {
    pub from_id: String,
    pub to_id: String,
    pub relation_type: String,
    pub weight: f64,
}

/// Retention scoring for memory decay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionScore {
    pub memory_id: String,
    pub retention_strength: f64,
    pub last_accessed: DateTime<Utc>,
    pub access_count: usize,
    pub decay_rate: f64,
}

/// Configuration for Ebbinghaus decay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayConfig {
    pub initial_retention: f64,
    pub decay_rate: f64,
    pub reinforcement_multiplier: f64,
    pub minimum_retention: f64,
}

/// A block of context injected into a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBlock {
    pub content: String,
    pub source: String,
    pub relevance_score: f64,
    pub token_count: usize,
    pub memory_id: Option<String>,
}

/// An actionable task derived from observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: ActionStatus,
    pub priority: u8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub assigned_to: Option<String>,
    pub project: String,
    pub tags: Vec<String>,
    pub source_observation_ids: Vec<String>,
    pub source_memory_ids: Vec<String>,
    pub result: Option<String>,
    pub parent_id: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub sketch_id: Option<String>,
    pub crystallized_into: Option<String>,
}

/// Dependency edge between actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEdge {
    pub from_id: String,
    pub to_id: String,
    pub edge_type: ActionEdgeType,
}

/// Ownership lease for an action with expiration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    pub id: String,
    pub action_id: String,
    pub holder: String,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub project: String,
}

/// Blocking condition for action execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub action_id: String,
    pub checkpoint_type: CheckpointType,
    pub status: CheckpointStatus,
    pub condition: String,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Inter-agent message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: String,
    pub from: String,
    pub to: String,
    pub thread_id: Option<String>,
    pub reply_to: Option<String>,
    pub signal_type: SignalType,
    pub content: String,
    pub metadata: HashMap<String, serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Composable sequence of actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<RoutineStep>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub frozen: bool,
    pub tags: Vec<String>,
    pub source_procedural_ids: Vec<String>,
}

/// A single step within a routine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineStep {
    pub action_id: String,
    pub order: usize,
    pub condition: Option<String>,
}

/// Result of running a routine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineRun {
    pub id: String,
    pub routine_id: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: String,
    pub step_results: HashMap<String, String>,
}

/// Draft action awaiting promotion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sketch {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub action_ids: Vec<String>,
    pub project: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub promoted_at: Option<DateTime<Utc>>,
    pub discarded_at: Option<DateTime<Utc>>,
}

/// Narrative summary of completed action sequences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crystal {
    pub id: String,
    pub action_ids: Vec<String>,
    pub narrative: String,
    pub key_outcomes: Vec<String>,
    pub files_affected: Vec<String>,
    pub lessons: Vec<String>,
    pub session_id: Option<String>,
    pub project: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Decayable knowledge with reinforcement tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub id: String,
    pub content: String,
    pub context: Option<String>,
    pub retention: f64,
    pub tags: Vec<String>,
    pub confidence: f64,
    pub project: Option<String>,
    pub source: Option<String>,
    pub source_ids: Vec<String>,
    pub last_reinforced: Option<DateTime<Utc>>,
    pub reinforcement_count: usize,
    pub decay_rate: f64,
    pub last_decayed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
}

/// Reinforceable high-confidence learning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Insight {
    pub id: String,
    pub title: String,
    pub content: String,
    pub confidence: f64,
    pub reinforcements: usize,
    pub source_observation_id: Option<String>,
    pub source_concept_cluster: Option<String>,
    pub source_memory_ids: Vec<String>,
    pub source_lesson_ids: Vec<String>,
    pub source_crystal_ids: Vec<String>,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub decay_rate: f64,
    pub last_reinforced_at: Option<DateTime<Utc>>,
    pub last_decayed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
}

/// Multi-dimensional tag on actions/memories/observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Facet {
    pub id: String,
    pub name: String,
    pub value: String,
    pub target_id: String,
    pub target_type: String,
    pub dimension: String,
    pub created_at: DateTime<Utc>,
}

/// Watch trigger for automated responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sentinel {
    pub id: String,
    pub name: String,
    pub sentinel_type: SentinelType,
    pub status: String,
    pub config: HashMap<String, serde_json::Value>,
    pub condition: String,
    pub action: String,
    pub active: bool,
    pub linked_action_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub triggered_at: Option<DateTime<Utc>>,
    pub last_triggered: Option<DateTime<Utc>>,
    pub result: Option<serde_json::Value>,
}

/// A slot for context injection with token budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySlot {
    pub id: String,
    pub name: String,
    pub content: String,
    pub token_count: usize,
    pub priority: u8,
    pub last_updated: DateTime<Utc>,
}

/// Project-level profile for context injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectProfile {
    pub project: String,
    pub top_concepts: Vec<FrequencyEntry>,
    pub top_files: Vec<FrequencyEntry>,
    pub top_patterns: Vec<FrequencyEntry>,
    pub conventions: Vec<String>,
    pub common_errors: Vec<String>,
    pub recent_activity: Vec<String>,
    pub session_count: usize,
    pub total_observations: usize,
    pub language: Option<String>,
    pub framework: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// A peer in the P2P mesh network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshPeer {
    pub id: String,
    pub url: String,
    pub name: String,
    pub status: String,
    pub shared_scopes: Vec<String>,
    pub sync_filter: Option<SyncFilter>,
    pub last_sync_at: Option<DateTime<Utc>>,
}

/// Filter for mesh sync scope.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncFilter {
    pub project: Option<String>,
}

/// Audit trail entry for operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub operation: String,
    pub user_id: Option<String>,
    pub function_id: String,
    pub target_ids: Vec<String>,
    pub details: HashMap<String, serde_json::Value>,
    pub quality_score: Option<f64>,
}

/// Team profile with aggregated concepts/files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamProfile {
    pub team_id: String,
    pub members: Vec<String>,
    pub top_concepts: Vec<FrequencyEntry>,
    pub top_files: Vec<FrequencyEntry>,
    pub shared_patterns: Vec<String>,
    pub total_shared_items: usize,
    pub updated_at: DateTime<Utc>,
}

/// A frequency-counted entry (concept or file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyEntry {
    pub key: String,
    pub frequency: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub id: String,
    pub observation_id: String,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub title: String,
    pub narrative: String,
    pub relative_position: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub id: String,
    pub pattern_type: String,
    pub description: String,
    pub files: Vec<String>,
    pub frequency: usize,
    pub sessions: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Export/import data container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub version: String,
    pub exported_at: DateTime<Utc>,
    pub sessions: Vec<Session>,
    pub observations: Vec<RawObservation>,
    pub compressed_observations: Vec<CompressedObservation>,
    pub memories: Vec<Memory>,
    pub actions: Vec<Action>,
    pub signals: Vec<Signal>,
    pub routines: Vec<Routine>,
    pub project: String,
}

/// Node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub node_type: GraphNodeType,
    pub properties: HashMap<String, serde_json::Value>,
    pub source_observation_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub aliases: Vec<String>,
    pub stale: bool,
}

/// Edge connecting knowledge graph nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub edge_type: GraphEdgeType,
    pub weight: f64,
    pub source_observation_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub context: Option<EdgeContext>,
    pub version: u32,
    pub superseded_by: Option<String>,
    pub is_latest: bool,
    pub stale: bool,
}

/// Additional context for a graph edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeContext {
    pub reasoning: Option<String>,
    pub sentiment: Option<String>,
    pub alternatives: Vec<String>,
    pub situational_factors: Vec<String>,
    pub confidence: Option<f64>,
}

/// Image data attached to observations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageData {
    pub base64: Option<String>,
    pub path: Option<String>,
    pub mime_type: String,
    pub description: Option<String>,
}

/// Payload for lifecycle hook events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPayload {
    pub hook_type: HookType,
    pub session_id: String,
    pub project: String,
    pub cwd: String,
    pub timestamp: DateTime<Utc>,
    pub data: HashMap<String, serde_json::Value>,
}

/// Result from hybrid search combining BM25, vector, and graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchResult {
    pub observation: CompressedObservation,
    pub bm25_score: f64,
    pub vector_score: f64,
    pub graph_score: f64,
    pub combined_score: f64,
    pub session_id: String,
    pub graph_context: Option<Vec<GraphNode>>,
}

/// Configuration for a team's shared memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamConfig {
    pub team_id: String,
    pub user_id: String,
    pub mode: TeamMode,
}

/// Type of item shared in team memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedItemType {
    Memory,
    Pattern,
    Observation,
}

/// Visibility of a shared item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemVisibility {
    Shared,
    Private,
}

/// An item shared between team members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSharedItem {
    pub id: String,
    pub shared_by: String,
    pub shared_at: DateTime<Utc>,
    pub item_type: SharedItemType,
    pub content: serde_json::Value,
    pub project: String,
    pub visibility: ItemVisibility,
}

/// Audit operation types (~60 types from agentmemory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOperation {
    // Session operations
    SessionCreate,
    SessionUpdate,
    SessionClose,
    SessionDelete,
    // Observation operations
    ObservationCreate,
    ObservationUpdate,
    ObservationDelete,
    ObservationCompress,
    // Memory operations
    MemoryCreate,
    MemoryUpdate,
    MemoryDelete,
    MemorySearch,
    MemoryConsolidate,
    // Action operations
    ActionCreate,
    ActionUpdate,
    ActionDelete,
    ActionComplete,
    ActionBlock,
    ActionUnblock,
    // Graph operations
    GraphNodeCreate,
    GraphNodeUpdate,
    GraphNodeDelete,
    GraphEdgeCreate,
    GraphEdgeUpdate,
    GraphEdgeDelete,
    // Team operations
    TeamShare,
    TeamProfileUpdate,
    // Mesh operations
    MeshSync,
    MeshPeerAdd,
    MeshPeerRemove,
    // Slot operations
    SlotCreate,
    SlotUpdate,
    SlotDelete,
    SlotPin,
    SlotUnpin,
    // Lease operations
    LeaseAcquire,
    LeaseRelease,
    LeaseExpire,
    // Signal operations
    SignalCreate,
    SignalUpdate,
    SignalDelete,
    // Routine operations
    RoutineCreate,
    RoutineUpdate,
    RoutineDelete,
    RoutineRun,
    // Profile operations
    ProfileUpdate,
    // Lesson operations
    LessonCreate,
    LessonUpdate,
    LessonDelete,
    // Export/Import
    Export,
    Import,
    // Governance
    GovernanceCheck,
    GovernanceUpdate,
    // Context
    ContextBuild,
    // Other
    Unknown,
}

impl std::fmt::Display for AuditOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionCreate => write!(f, "session_create"),
            Self::SessionUpdate => write!(f, "session_update"),
            Self::SessionClose => write!(f, "session_close"),
            Self::SessionDelete => write!(f, "session_delete"),
            Self::ObservationCreate => write!(f, "observation_create"),
            Self::ObservationUpdate => write!(f, "observation_update"),
            Self::ObservationDelete => write!(f, "observation_delete"),
            Self::ObservationCompress => write!(f, "observation_compress"),
            Self::MemoryCreate => write!(f, "memory_create"),
            Self::MemoryUpdate => write!(f, "memory_update"),
            Self::MemoryDelete => write!(f, "memory_delete"),
            Self::MemorySearch => write!(f, "memory_search"),
            Self::MemoryConsolidate => write!(f, "memory_consolidate"),
            Self::ActionCreate => write!(f, "action_create"),
            Self::ActionUpdate => write!(f, "action_update"),
            Self::ActionDelete => write!(f, "action_delete"),
            Self::ActionComplete => write!(f, "action_complete"),
            Self::ActionBlock => write!(f, "action_block"),
            Self::ActionUnblock => write!(f, "action_unblock"),
            Self::GraphNodeCreate => write!(f, "graph_node_create"),
            Self::GraphNodeUpdate => write!(f, "graph_node_update"),
            Self::GraphNodeDelete => write!(f, "graph_node_delete"),
            Self::GraphEdgeCreate => write!(f, "graph_edge_create"),
            Self::GraphEdgeUpdate => write!(f, "graph_edge_update"),
            Self::GraphEdgeDelete => write!(f, "graph_edge_delete"),
            Self::TeamShare => write!(f, "team_share"),
            Self::TeamProfileUpdate => write!(f, "team_profile_update"),
            Self::MeshSync => write!(f, "mesh_sync"),
            Self::MeshPeerAdd => write!(f, "mesh_peer_add"),
            Self::MeshPeerRemove => write!(f, "mesh_peer_remove"),
            Self::SlotCreate => write!(f, "slot_create"),
            Self::SlotUpdate => write!(f, "slot_update"),
            Self::SlotDelete => write!(f, "slot_delete"),
            Self::SlotPin => write!(f, "slot_pin"),
            Self::SlotUnpin => write!(f, "slot_unpin"),
            Self::LeaseAcquire => write!(f, "lease_acquire"),
            Self::LeaseRelease => write!(f, "lease_release"),
            Self::LeaseExpire => write!(f, "lease_expire"),
            Self::SignalCreate => write!(f, "signal_create"),
            Self::SignalUpdate => write!(f, "signal_update"),
            Self::SignalDelete => write!(f, "signal_delete"),
            Self::RoutineCreate => write!(f, "routine_create"),
            Self::RoutineUpdate => write!(f, "routine_update"),
            Self::RoutineDelete => write!(f, "routine_delete"),
            Self::RoutineRun => write!(f, "routine_run"),
            Self::ProfileUpdate => write!(f, "profile_update"),
            Self::LessonCreate => write!(f, "lesson_create"),
            Self::LessonUpdate => write!(f, "lesson_update"),
            Self::LessonDelete => write!(f, "lesson_delete"),
            Self::Export => write!(f, "export"),
            Self::Import => write!(f, "import"),
            Self::GovernanceCheck => write!(f, "governance_check"),
            Self::GovernanceUpdate => write!(f, "governance_update"),
            Self::ContextBuild => write!(f, "context_build"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// State for the circuit breaker pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerState {
    pub state: CircuitState,
    pub failure_count: usize,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub failure_window_ms: u64,
    pub recovery_timeout_ms: u64,
    pub failure_threshold: usize,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_type_roundtrip() {
        for variant in [
            ObservationType::FileRead,
            ObservationType::FileWrite,
            ObservationType::ToolUse,
            ObservationType::ToolFailure,
            ObservationType::UserPrompt,
            ObservationType::SessionStart,
            ObservationType::SessionEnd,
            ObservationType::Other,
        ] {
            let s = variant.to_string();
            let parsed: ObservationType = s.parse().unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn hook_type_roundtrip() {
        for variant in [
            HookType::SessionStart,
            HookType::PostToolUse,
            HookType::PostToolUseFailure,
            HookType::SessionEnd,
            HookType::Notification,
        ] {
            let s = variant.to_string();
            let parsed: HookType = s.parse().unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn memory_type_roundtrip() {
        for variant in [
            MemoryType::Working,
            MemoryType::Episodic,
            MemoryType::Semantic,
            MemoryType::Procedural,
            MemoryType::Insight,
            MemoryType::Lesson,
        ] {
            let s = variant.to_string();
            let parsed: MemoryType = s.parse().unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn consolidation_tier_roundtrip() {
        for variant in [
            ConsolidationTier::Working,
            ConsolidationTier::Episodic,
            ConsolidationTier::Semantic,
            ConsolidationTier::Procedural,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: ConsolidationTier = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn graph_node_type_roundtrip() {
        for variant in [
            GraphNodeType::File,
            GraphNodeType::Concept,
            GraphNodeType::Decision,
            GraphNodeType::Project,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: GraphNodeType = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn graph_edge_type_roundtrip() {
        for variant in [
            GraphEdgeType::Uses,
            GraphEdgeType::DependsOn,
            GraphEdgeType::Fixes,
            GraphEdgeType::RelatedTo,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: GraphEdgeType = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn memory_edge_kind_traversal_weights() {
        // jcode's canonical traversal weights (issue #27).
        assert_eq!(MemoryEdgeKind::HasTag.traversal_weight(), 0.8);
        assert_eq!(MemoryEdgeKind::InCluster.traversal_weight(), 0.6);
        assert_eq!(MemoryEdgeKind::Supersedes.traversal_weight(), 0.9);
        assert_eq!(MemoryEdgeKind::Contradicts.traversal_weight(), 0.3);
        assert_eq!(MemoryEdgeKind::DerivedFrom.traversal_weight(), 0.7);
        // RelatesTo honours the user-supplied weight, default 1.0.
        assert_eq!(
            MemoryEdgeKind::RelatesTo { weight: 0.42 }.traversal_weight(),
            0.42
        );
        assert_eq!(
            MemoryEdgeKind::RelatesTo { weight: 1.0 }.traversal_weight(),
            1.0
        );
    }

    #[test]
    fn memory_edge_kind_as_str() {
        assert_eq!(MemoryEdgeKind::HasTag.as_str(), "has_tag");
        assert_eq!(MemoryEdgeKind::InCluster.as_str(), "in_cluster");
        assert_eq!(
            MemoryEdgeKind::RelatesTo { weight: 0.5 }.as_str(),
            "relates_to"
        );
        assert_eq!(MemoryEdgeKind::Supersedes.as_str(), "supersedes");
        assert_eq!(MemoryEdgeKind::Contradicts.as_str(), "contradicts");
        assert_eq!(MemoryEdgeKind::DerivedFrom.as_str(), "derived_from");
    }

    #[test]
    fn memory_edge_kind_from_kind_str() {
        assert_eq!(
            MemoryEdgeKind::from_kind_str("has_tag"),
            Some(MemoryEdgeKind::HasTag)
        );
        assert_eq!(
            MemoryEdgeKind::from_kind_str("supersedes"),
            Some(MemoryEdgeKind::Supersedes)
        );
        assert_eq!(
            MemoryEdgeKind::from_kind_str("relates_to"),
            Some(MemoryEdgeKind::RelatesTo { weight: 1.0 })
        );
        // Accept the original PascalCase form too.
        assert_eq!(
            MemoryEdgeKind::from_kind_str("DerivedFrom"),
            Some(MemoryEdgeKind::DerivedFrom)
        );
        assert_eq!(MemoryEdgeKind::from_kind_str("nonsense"), None);
    }

    #[test]
    fn memory_edge_kind_from_kind_and_weight() {
        assert_eq!(
            MemoryEdgeKind::from_kind_and_weight("has_tag", Some(0.8)),
            Some(MemoryEdgeKind::HasTag)
        );
        // RelatesTo round-trips with its weight preserved.
        let rt = MemoryEdgeKind::from_kind_and_weight("relates_to", Some(0.42)).unwrap();
        match rt {
            MemoryEdgeKind::RelatesTo { weight } => {
                assert!((weight - 0.42).abs() < 1e-6);
            }
            _ => panic!("expected RelatesTo"),
        }
        // None for unknown kind.
        assert_eq!(MemoryEdgeKind::from_kind_and_weight("nope", None), None);
    }

    #[test]
    fn memory_edge_kind_serde_roundtrip() {
        for variant in [
            MemoryEdgeKind::HasTag,
            MemoryEdgeKind::InCluster,
            MemoryEdgeKind::RelatesTo { weight: 0.75 },
            MemoryEdgeKind::Supersedes,
            MemoryEdgeKind::Contradicts,
            MemoryEdgeKind::DerivedFrom,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: MemoryEdgeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn action_status_roundtrip() {
        for variant in [
            ActionStatus::Pending,
            ActionStatus::InProgress,
            ActionStatus::Completed,
            ActionStatus::Failed,
            ActionStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: ActionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn signal_type_roundtrip() {
        for variant in [
            SignalType::Info,
            SignalType::Request,
            SignalType::Response,
            SignalType::Alert,
            SignalType::Handoff,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: SignalType = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn circuit_state_roundtrip() {
        for variant in [
            CircuitState::Closed,
            CircuitState::Open,
            CircuitState::HalfOpen,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: CircuitState = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn session_roundtrip() {
        let session = Session::new("s-1", "my-project", "/tmp/project");
        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session.id, parsed.id);
        assert_eq!(session.project, parsed.project);
        assert_eq!(session.cwd, parsed.cwd);
    }

    #[test]
    fn memory_roundtrip() {
        let memory = Memory {
            id: "m-1".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            memory_type: MemoryType::Semantic,
            title: "Auth uses Clerk".into(),
            content: "The project uses Clerk for authentication".into(),
            concepts: vec!["auth".into(), "clerk".into()],
            files: vec!["src/auth.ts".into()],
            session_ids: vec!["s-1".into()],
            strength: 0.8,
            version: 1,
            parent_id: None,
            supersedes: vec![],
            related_ids: vec![],
            source_observation_ids: vec!["o-1".into()],
            is_latest: true,
            forget_after: None,
            image_ref: None,
            agent_id: None,
            project: "my-project".into(),
        };
        let json = serde_json::to_string(&memory).unwrap();
        let parsed: Memory = serde_json::from_str(&json).unwrap();
        assert_eq!(memory.id, parsed.id);
        assert_eq!(memory.title, parsed.title);
        assert_eq!(memory.memory_type, parsed.memory_type);
    }

    #[test]
    fn action_roundtrip() {
        let action = Action {
            id: "a-1".into(),
            title: "Fix auth bug".into(),
            description: "Token expires too early".into(),
            status: ActionStatus::Pending,
            priority: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            created_by: Some("agent-1".into()),
            assigned_to: None,
            project: "my-project".into(),
            tags: vec!["bug".into(), "auth".into()],
            source_observation_ids: vec![],
            source_memory_ids: vec![],
            result: None,
            parent_id: None,
            metadata: HashMap::new(),
            sketch_id: None,
            crystallized_into: None,
        };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(action.id, parsed.id);
        assert_eq!(action.status, parsed.status);
    }

    #[test]
    fn signal_roundtrip() {
        let signal = Signal {
            id: "sig-1".into(),
            from: "Alice".into(),
            to: "Bob".into(),
            thread_id: Some("t-1".into()),
            reply_to: None,
            signal_type: SignalType::Request,
            content: "Please review PR".into(),
            metadata: HashMap::new(),
            created_at: Utc::now(),
            read_at: None,
            expires_at: None,
        };
        let json = serde_json::to_string(&signal).unwrap();
        let parsed: Signal = serde_json::from_str(&json).unwrap();
        assert_eq!(signal.id, parsed.id);
        assert_eq!(signal.signal_type, parsed.signal_type);
    }

    #[test]
    fn session_with_optional_fields() {
        let session = Session {
            id: "s-2".into(),
            project: "proj".into(),
            cwd: "/tmp".into(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            status: "completed".into(),
            observation_count: 42,
            model: Some("claude-sonnet-4".into()),
            tags: vec!["test".into()],
            first_prompt: Some("hello".into()),
            summary: Some("done".into()),
            commit_shas: vec!["abc123".into()],
            agent_id: Some("agent-1".into()),
        };
        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session.ended_at, parsed.ended_at);
        assert_eq!(session.model, parsed.model);
        assert_eq!(session.commit_shas, parsed.commit_shas);
    }

    #[test]
    fn export_data_roundtrip() {
        let export = ExportData {
            version: "1.0".into(),
            exported_at: Utc::now(),
            sessions: vec![],
            observations: vec![],
            compressed_observations: vec![],
            memories: vec![],
            actions: vec![],
            signals: vec![],
            routines: vec![],
            project: "my-project".into(),
        };
        let json = serde_json::to_string(&export).unwrap();
        let parsed: ExportData = serde_json::from_str(&json).unwrap();
        assert_eq!(export.version, parsed.version);
    }

    #[test]
    fn hook_payload_roundtrip() {
        let payload = HookPayload {
            hook_type: HookType::PostToolUse,
            session_id: "s-1".into(),
            project: "proj".into(),
            cwd: "/tmp".into(),
            timestamp: Utc::now(),
            data: HashMap::new(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: HookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.hook_type, parsed.hook_type);
    }

    #[test]
    fn circuit_breaker_state_roundtrip() {
        let state = CircuitBreakerState {
            state: CircuitState::Closed,
            failure_count: 0,
            last_failure_at: None,
            last_success_at: Some(Utc::now()),
            failure_window_ms: 60_000,
            recovery_timeout_ms: 30_000,
            failure_threshold: 3,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: CircuitBreakerState = serde_json::from_str(&json).unwrap();
        assert_eq!(state.state, parsed.state);
        assert_eq!(state.failure_threshold, parsed.failure_threshold);
    }

    #[test]
    fn unknown_observation_type_errors() {
        let result = "nonexistent".parse::<ObservationType>();
        assert!(result.is_err());
    }

    #[test]
    fn unknown_hook_type_errors() {
        let result = "nonexistent".parse::<HookType>();
        assert!(result.is_err());
    }
}
