/// Prompt templates for LLM interactions.
///
/// This module contains system prompts and prompt builders for various
/// memory operations, ported 1:1 from agentmemory's TypeScript implementation.
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
pub use compression::{COMPRESSION_SYSTEM, STRICTER_SUFFIX, build_compression_prompt};
pub use consolidation::{
    PROCEDURAL_EXTRACTION_SYSTEM, SEMANTIC_MERGE_SYSTEM, build_procedural_extraction_prompt,
    build_semantic_merge_prompt,
};
pub use graph_extraction::{GRAPH_EXTRACTION_SYSTEM, build_graph_extraction_prompt};
pub use vision::VISION_DESCRIPTION_SYSTEM;
pub use vision::VISION_DESCRIPTION_PROMPT;
pub use xml::{get_xml_children, get_xml_tag};
