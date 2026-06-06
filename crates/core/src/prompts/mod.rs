/// Prompt templates for LLM interactions.
///
/// This module contains system prompts and prompt builders for various
/// memory operations, ported 1:1 from mempalace's TypeScript implementation.
///
/// # Modules
///
/// - [`compression`] — Memory compression prompts and prompt builder
/// - [`consolidation`] — Semantic merge and procedural extraction prompts
/// - [`graph_extraction`] — Knowledge graph entity/relationship extraction prompts
/// - [`vision`] — Image description prompt
/// - [`xml`] — XML parsing utilities for LLM response extraction
pub mod compression;
pub mod consolidation;
pub mod graph_extraction;
pub mod vision;
pub mod xml;

// Re-export key constants for ergonomic access
pub use compression::{build_compression_prompt, COMPRESSION_SYSTEM, STRICTER_SUFFIX};
pub use consolidation::{
    build_procedural_extraction_prompt, build_semantic_merge_prompt, PROCEDURAL_EXTRACTION_SYSTEM,
    SEMANTIC_MERGE_SYSTEM,
};
pub use graph_extraction::{build_graph_extraction_prompt, GRAPH_EXTRACTION_SYSTEM};
pub use vision::VISION_DESCRIPTION_PROMPT;
pub use vision::VISION_DESCRIPTION_SYSTEM;
pub use xml::{get_xml_children, get_xml_tag};
