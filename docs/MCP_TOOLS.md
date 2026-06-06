# MCP Tool Expansion — mempalace Feature Parity

Target: **43+ MCP tools** (currently 19) to match mempalace (51 tools).

## Why

mempalace provides 51 MCP tools covering team collaboration, audit trails,
governance, snapshots, signals, action sketches, and self-healing. mempalace
currently has 19 tools. This gap means agents using mempalace miss critical
governance, collaboration, and introspection capabilities.

## New Tool Categories

### 1. Team Sharing (2 tools)
- `memory_team_share` — Share a memory or drawer with team members
- `memory_team_feed` — Retrieve team-shared memories feed

**Reference:** mempalace `share_memory`, `get_shared_memories`

### 2. Audit Trail (1 tool)
- `memory_audit` — Query audit log: who accessed/modified what, when, why

**Reference:** mempalace `get_audit_log`

### 3. Governance Delete (1 tool)
- `memory_governance_delete` — Delete with mandatory reason + audit trail entry

**Reference:** mempalace `governance_delete_memory`

### 4. Snapshots (3 tools)
- `memory_snapshot_create` — Create git-versioned snapshot of palace state
- `memory_snapshot_list` — List available snapshots
- `memory_snapshot_restore` — Restore palace to a specific snapshot

**Reference:** mempalace `create_snapshot`, `list_snapshots`, `restore_snapshot`

### 5. Inter-Agent Signals (3 tools)
- `memory_signal_send` — Send a signal to another agent
- `memory_signal_read` — Read pending signals for this agent
- `memory_signal_clear` — Clear a consumed signal

**Reference:** mempalace `send_signal`, `read_signals`, `clear_signal`

### 6. Action Sketches (3 tools)
- `memory_sketch_create` — Create a draft/rough action sketch
- `memory_sketch_promote` — Promote a sketch to a confirmed memory
- `memory_crystallize` — Convert sketch to formal memory

**Reference:** mempalace `create_action_sketch`, `promote_sketch`, `crystallize_sketch`

### 7. Self-Healing (2 tools)
- `memory_diagnose` — Diagnose palace health issues (duplicates, broken refs, entropy)
- `memory_heal` — Auto-fix diagnosed issues

**Reference:** mempalace `diagnose_memories`, `heal_memories`

### 8. Facets/Layers (3 tools)
- `memory_facet_tag` — Tag a memory with custom facets
- `memory_facet_query` — Query memories by facet
- `memory_facet_list` — List all available facets

**Reference:** mempalace `tag_facet`, `query_by_facet`, `list_facets`

### 9. Session Tracking (2 tools)
- `memory_sessions` — List all sessions
- `memory_timeline` — Get chronological event timeline

**Reference:** mempalace `get_sessions`, `get_timeline`

### 10. Misc Tools (2 tools)
- `memory_profile` — Get memory statistics + health scores
- `memory_export` — Export memories in various formats
- `memory_relations` — Query knowledge graph for entity relations

**Reference:** mempalace `get_memory_profile`, `export_memories`, `get_memory_relations`

## Implementation Priority

| Priority | Tool(s) | Rationale |
|----------|---------|-----------|
| P1 | memory_profile, memory_export | Quick wins; base for other tools |
| P1 | memory_sessions, memory_timeline | Directly useful for context building |
| P1 | memory_audit | Governance requirement |
| P1 | memory_governance_delete | Required for compliance |
| P2 | memory_team_share, memory_team_feed | Team collaboration |
| P2 | memory_snapshot_* | Version control for memories |
| P2 | memory_signal_* | Inter-agent communication |
| P3 | memory_sketch_* | Session workflow enhancement |
| P3 | memory_diagnose, memory_heal | Reliability/autonomous repair |
| P3 | memory_facet_* | Organization/filtering |

## Existing vs New Tools

### Current Tools (19)
```
mempalace_status          mempalace_list_wings        mempalace_list_rooms
mempalace_get_taxonomy    mempalace_get_aaak_spec     mempalace_kg_query
mempalace_kg_add          mempalace_kg_invalidate    mempalace_kg_timeline
mempalace_kg_stats        mempalace_traverse         mempalace_find_tunnels
mempalace_graph_stats     mempalace_search           mempalace_check_duplicate
mempalace_add_drawer      mempalace_delete_drawer    mempalace_diary_write
mempalace_diary_read
```

### New Tools (22+)
```
memory_team_share         memory_team_feed           memory_audit
memory_governance_delete  memory_snapshot_create     memory_snapshot_list
memory_snapshot_restore   memory_signal_send         memory_signal_read
memory_signal_clear      memory_sketch_create       memory_sketch_promote
memory_crystallize        memory_diagnose            memory_heal
memory_facet_tag          memory_facet_query         memory_facet_list
memory_sessions           memory_timeline            memory_profile
memory_export            memory_relations
```

Total after expansion: **41+ tools**

## Implementation Notes

### File Location
`crates/core/src/mcp_server.rs` — add new tool handlers alongside existing ones.

### Pattern to Follow
For each new tool:
1. Add dispatch case in `make_dispatch()` (line ~315)
2. Add tool schema in `make_tools()` (line ~352)
3. Add handler function (e.g., `fn tool_memory_profile(...)`)
4. Add to `MUTATION_TOOLS` if it writes state
5. Add tests in the `#[cfg(test)]` module
6. Update this document with status

### WAL Integration
New write tools automatically get WAL logging via `invoke_with_wal()`.

### Read-Only Mode
Mutation tools are hidden in `tools/list` when `--read-only` flag is set.
Access still blocked at call time via `read_only_guard()`.

## Tracking Beads

- `mr-mcp-43-tools-mempalace-06or` — Epic: expand to 43+ tools
- `mr-qo08` — memory_team_share + memory_team_feed
- `mr-ecvv` — memory_audit
- `mr-qh3a` — memory_governance_delete
- `mr-nnba` — memory_snapshot_*
- `mr-e4rh` — memory_signal_*
- `mr-fewp` — memory_sketch_* + memory_crystallize
- `mr-mo0i` — memory_diagnose + memory_heal
- `mr-iq5k` — memory_facet_*
- `mr-p60j` — memory_sessions + memory_timeline
- `mr-gb55` — memory_profile + memory_export + memory_relations
- `mr-qvcc` — docs: update MCP tool documentation
