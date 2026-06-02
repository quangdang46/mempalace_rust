// =====================================================================
// Lifecycle scripts — embedded JavaScript hooks for the plugin system
// =====================================================================
//
// The 14 scripts in this directory are 1:1 ports from
// `agentmemory/plugin/scripts/`. They are thin JavaScript shims that
// read a JSON payload from stdin and forward it to the mempalace REST
// API. The Rust core (see `hooks_cli.rs`) has equivalent handlers
// for every script — the .mjs files exist so that the plugin system
// can invoke them from the agent's JS plugin context (Claude Code,
// Copilot, Codex, etc.) without needing a Rust binary on PATH.
//
// Each constant embeds the script source via `include_str!` so the
// file is part of the compiled library and can be surfaced via
// `discover_lifecycle_scripts()` below. Adapted references:
//   - `AGENTMEMORY_URL` → `MEMPALACE_URL` (env var)
//   - `AGENTMEMORY_SECRET` → `MEMPALACE_SECRET` (env var)
//   - `/agentmemory/` → `/mempalace/` (REST paths)

/// File name → source code.
pub static DIAGNOSTICS: &str = include_str!("lifecycle/diagnostics.mjs");
pub static NOTIFICATION: &str = include_str!("lifecycle/notification.mjs");
pub static POST_COMMIT: &str = include_str!("lifecycle/post-commit.mjs");
pub static POST_TOOL_FAILURE: &str = include_str!("lifecycle/post-tool-failure.mjs");
pub static POST_TOOL_USE: &str = include_str!("lifecycle/post-tool-use.mjs");
pub static PRE_COMPACT: &str = include_str!("lifecycle/pre-compact.mjs");
pub static PRE_TOOL_USE: &str = include_str!("lifecycle/pre-tool-use.mjs");
pub static PROMPT_SUBMIT: &str = include_str!("lifecycle/prompt-submit.mjs");
pub static SESSION_END: &str = include_str!("lifecycle/session-end.mjs");
pub static SESSION_START: &str = include_str!("lifecycle/session-start.mjs");
pub static STOP: &str = include_str!("lifecycle/stop.mjs");
pub static SUBAGENT_START: &str = include_str!("lifecycle/subagent-start.mjs");
pub static SUBAGENT_STOP: &str = include_str!("lifecycle/subagent-stop.mjs");
pub static TASK_COMPLETED: &str = include_str!("lifecycle/task-completed.mjs");

/// All 14 embedded lifecycle scripts with their file names.
pub const LIFECYCLE_SCRIPTS: &[(&str, &str)] = &[
    ("diagnostics.mjs", DIAGNOSTICS),
    ("notification.mjs", NOTIFICATION),
    ("post-commit.mjs", POST_COMMIT),
    ("post-tool-failure.mjs", POST_TOOL_FAILURE),
    ("post-tool-use.mjs", POST_TOOL_USE),
    ("pre-compact.mjs", PRE_COMPACT),
    ("pre-tool-use.mjs", PRE_TOOL_USE),
    ("prompt-submit.mjs", PROMPT_SUBMIT),
    ("session-end.mjs", SESSION_END),
    ("session-start.mjs", SESSION_START),
    ("stop.mjs", STOP),
    ("subagent-start.mjs", SUBAGENT_START),
    ("subagent-stop.mjs", SUBAGENT_STOP),
    ("task-completed.mjs", TASK_COMPLETED),
];

/// Return the list of embedded lifecycle script file names.
pub fn discover_embedded_slugs() -> Vec<String> {
    LIFECYCLE_SCRIPTS
        .iter()
        .map(|(name, _)| name.trim_end_matches(".mjs").to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_count_is_14() {
        assert_eq!(LIFECYCLE_SCRIPTS.len(), 14);
    }

    #[test]
    fn discover_embedded_slugs_returns_14_unique() {
        let slugs = discover_embedded_slugs();
        assert_eq!(slugs.len(), 14);
        let unique: std::collections::HashSet<_> = slugs.iter().collect();
        assert_eq!(unique.len(), 14, "slugs must be unique: {slugs:?}");
    }

    #[test]
    fn each_script_is_non_empty() {
        for (name, source) in LIFECYCLE_SCRIPTS {
            assert!(!source.is_empty(), "{name} is empty");
            // Most scripts reference AGENTMEMORY_URL (REST API base);
            // diagnostics.mjs is KV-only and does not, so skip it.
            if *name != "diagnostics.mjs" {
                assert!(
                    source.contains("AGENTMEMORY_URL"),
                    "{name} missing AGENTMEMORY_URL"
                );
            }
        }
    }
}
