/// Embedded MemPalace plugin skills ported from agentmemory.
///
/// Each constant is the raw `include_str!` of the SKILL.md file
/// in the `skills/` subdirectory. The YAML frontmatter (name,
/// description, argument-hint, user-invocable) is the metadata
/// consumed by `PluginManifest` in `plugins/mod.rs`; the body is
/// the markdown instructions dispatched to the LLM when the skill
/// is invoked.
pub const SKILL_RECAP: &str = include_str!("skills/recap/SKILL.md");
pub const SKILL_HANDOFF: &str = include_str!("skills/handoff/SKILL.md");
pub const SKILL_RECALL: &str = include_str!("skills/recall/SKILL.md");
pub const SKILL_REMEMBER: &str = include_str!("skills/remember/SKILL.md");
pub const SKILL_FORGET: &str = include_str!("skills/forget/SKILL.md");
pub const SKILL_COMMIT_CONTEXT: &str = include_str!("skills/commit-context/SKILL.md");
pub const SKILL_COMMIT_HISTORY: &str = include_str!("skills/commit-history/SKILL.md");
pub const SKILL_SESSION_HISTORY: &str = include_str!("skills/session-history/SKILL.md");

/// Ordered list of all embedded skills from agentmemory's `plugin/skills/`.
/// Consumed by `PluginRegistry::discover_embedded()` in `plugins/mod.rs`.
pub const EMBEDDED_SKILLS: &[(&str, &str)] = &[
    ("recap", SKILL_RECAP),
    ("handoff", SKILL_HANDOFF),
    ("recall", SKILL_RECALL),
    ("remember", SKILL_REMEMBER),
    ("forget", SKILL_FORGET),
    ("commit-context", SKILL_COMMIT_CONTEXT),
    ("commit-history", SKILL_COMMIT_HISTORY),
    ("session-history", SKILL_SESSION_HISTORY),
];

#[cfg(test)]
mod tests {
    #[test]
    fn all_embedded_skills_have_frontmatter() {
        for &(name, content) in super::EMBEDDED_SKILLS {
            assert!(
                content.starts_with("---"),
                "skill {name} missing YAML frontmatter"
            );
            assert!(
                content.contains(&format!("name: {name}")),
                "skill {name} missing name field in frontmatter"
            );
        }
    }

    #[test]
    fn embedded_skills_count() {
        assert_eq!(super::EMBEDDED_SKILLS.len(), 8);
    }
}
