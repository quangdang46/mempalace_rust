//! instructions.rs — CLI tool for reading instruction markdown files.

#[allow(dead_code)]
const INSTRUCTIONS_DIR: &str = "instructions";

#[allow(dead_code)]
const AVAILABLE_INSTRUCTIONS: &[&str] = &["init", "search", "mine", "status", "help"];

/// Embedded instruction content for each available instruction.
fn instruction_content(name: &str) -> Option<&'static str> {
    match name {
        "init" => Some(INIT_INSTRUCTION),
        "search" => Some(SEARCH_INSTRUCTION),
        "mine" => Some(MINE_INSTRUCTION),
        "status" => Some(STATUS_INSTRUCTION),
        "help" => Some(HELP_INSTRUCTION),
        _ => None,
    }
}

/// Run the instructions command — prints the instruction .md file for the given name.
pub fn run_instructions(name: &str) -> anyhow::Result<()> {
    if let Some(content) = instruction_content(name) {
        println!("{content}");
        Ok(())
    } else {
        anyhow::bail!(
            "Unknown instructions: {name}\nAvailable: {}",
            AVAILABLE_INSTRUCTIONS.join(", ")
        );
    }
}

/// List available instruction names.
pub fn available_instructions() -> Vec<&'static str> {
    AVAILABLE_INSTRUCTIONS.to_vec()
}

const INIT_INSTRUCTION: &str = r#"# MemPalace Init

Guide the user through a complete MemPalace setup. Follow each step in order,
stopping to report errors and attempt remediation before proceeding.

## Step 1: Check Rust installation

Run `mpr --version` and confirm the version is shown. If mpr is not found,
install with: `curl -fsSL "https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh" | bash`

## Step 2: Ask for project directory

Ask the user which project directory they want to initialize with MemPalace.
Offer the current working directory as the default. Wait for their response
before continuing.

## Step 3: Initialize the palace

Run `mpr init --yes <dir>` where `<dir>` is the directory from Step 2.

If this fails, report the error and stop.

## Step 4: Configure MCP server

Run the following command to register the MemPalace MCP server with Claude:

    claude mcp add mpr -- mpr mcp

If this fails, report the error but continue to the next step (MCP
configuration can be done manually later).

## Step 5: Verify installation

Run `mpr status` and confirm the output shows a healthy palace.

If the command fails or reports errors, walk the user through troubleshooting
based on the output.

## Step 6: Show next steps

Tell the user setup is complete and suggest these next actions:

- Use `mpr mine` to start adding data to their palace
- Use `mpr search` to query their palace and retrieve stored knowledge

"#;

const SEARCH_INSTRUCTION: &str = r#"# MemPalace Search

When the user wants to search their MemPalace memories, follow these steps:

## 1. Parse the Search Query

Extract the core search intent from the user's message. Identify any explicit
or implicit filters:
- Wing -- a top-level category (e.g., "work", "personal", "research")
- Room -- a sub-category within a wing
- Keywords / semantic query -- the actual search terms

## 2. Determine Wing/Room Filters

If the user mentions a specific domain, topic area, or context, map it to the
appropriate wing and/or room. If unsure, omit filters to search globally. You
can discover the taxonomy first if needed.

## 3. Use MCP Tools (Preferred)

If MCP tools are available, use them in this priority order:

- mempalace_search(query, wing, room) -- Primary search tool. Pass the semantic
  query and any wing/room filters.
- mempalace_list_wings -- Discover all available wings. Use when the user asks
  what categories exist or you need to resolve a wing name.
- mempalace_list_rooms(wing) -- List rooms within a specific wing. Use to help
  the user navigate or to resolve a room name.
- mempalace_get_taxonomy -- Retrieve the full wing/room/drawer tree. Use when
  the user wants an overview of their entire memory structure.
- mempalace_traverse(room) -- Walk the knowledge graph starting from a room.
  Use when the user wants to explore connections and related memories.
- mempalace_find_tunnels(wing1, wing2) -- Find cross-wing connections (tunnels)
  between two wings. Use when the user asks about relationships between
  different knowledge domains.

## 4. CLI Fallback

If MCP tools are not available, fall back to the CLI:

    mpr search "query" [--wing X] [--room Y]

## 5. Present Results

When presenting search results:
- Always include source attribution: wing, room, and drawer for each result
- Show relevance or similarity scores if available
- Group results by wing/room when returning multiple hits
- Quote or summarize the memory content clearly

## 6. Offer Next Steps

After presenting results, offer the user options to go deeper:
- Drill deeper -- search within a specific room or narrow the query
- Traverse -- explore the knowledge graph from a related room
- Check tunnels -- look for cross-wing connections if the topic spans domains
- Browse taxonomy -- show the full structure for manual exploration

"#;

const MINE_INSTRUCTION: &str = r#"# MemPalace Mine

When the user invokes this skill, follow these steps:

## 1. Ask what to mine

Ask the user what they want to mine and where the source data is located.
Clarify:
- Is it a project directory (code, docs, notes)?
- Is it conversation exports (Claude, ChatGPT, Slack)?
- Do they want auto-classification (decisions, milestones, problems)?

## 2. Choose the mining mode

There are three mining modes:

### Project mining

    mpr mine <dir>

Mines code files, documentation, and notes from a project directory.

### Conversation mining

    mpr mine <dir> --mode convos

Mines conversation exports from Claude, ChatGPT, or Slack into the palace.

### General extraction (auto-classify)

    mpr mine <dir> --mode convos --extract general

Auto-classifies mined content into decisions, milestones, and problems.

## 3. Optionally split mega-files first

If the source directory contains very large files, suggest splitting them
before mining:

    mpr split <dir> [--dry-run]

Use --dry-run first to preview what will be split without making changes.

## 4. Optionally tag with a wing

If the user wants to organize mined content under a specific wing, add the
--wing flag:

    mpr mine <dir> --wing <name>

## 5. Show progress and results

Run the selected mining command and display progress as it executes. After
completion, summarize the results including:
- Number of items mined
- Categories or classifications applied
- Any warnings or skipped files

## 6. Suggest next steps

After mining completes, suggest the user try:
- `mpr search` -- search the newly mined content
- `mpr status` -- check the current state of their palace
- Mine more data from additional sources

"#;

const STATUS_INSTRUCTION: &str = r#"# MemPalace Status

Display the current state of the user's memory palace.

## Step 1: Gather Palace Status

Check if MCP tools are available (look for mpr_status in available tools).

- If MCP is available: Call the mpr_status tool to retrieve palace state.
- If MCP is not available: Run the CLI command: mpr status

## Step 2: Display Wing/Room/Drawer Counts

Present the palace structure counts clearly:
- Number of wings
- Number of rooms
- Number of drawers
- Total memories stored

Keep the output concise -- use a brief summary format, not verbose tables.

## Step 3: Knowledge Graph Stats (MCP only)

If MCP tools are available, also call:
- mpr_kg_stats -- for a knowledge graph overview (triple count, entity
  count, relationship types)
- mpr_graph_stats -- for connectivity information (connected components,
  average connections per entity)

Present these alongside the palace counts in a unified summary.

## Step 4: Suggest Next Actions

Based on the current state, suggest one relevant action:

- Empty palace (zero memories): Suggest `mpr mine` to add data from
  files, URLs, or text.
- Has data but no knowledge graph (memories exist but KG stats show zero
  triples): Suggest considering adding knowledge graph triples for richer
  queries.
- Healthy palace (has memories and KG data): Suggest `mpr search` to
  query your memories.

## Output Style

- Be concise and informative -- aim for a quick glance, not a report.
- Use short labels and numbers, not prose paragraphs.
- If any step fails or a tool is unavailable, note it briefly and continue
  with what is available.

"#;

const HELP_INSTRUCTION: &str = r#"# MemPalace

AI memory system. Store everything, find anything. Local, free, no API key.

---
## Slash Commands

| Command          | Description                    |
|------------------|--------------------------------|
| /mempalace:init  | Install and set up MemPalace   |
| /mempalace:search| Search your memories           |
| /mempalace:mine  | Mine projects and conversations|
| /mempalace:status| Palace overview and stats      |
| /mempalace:help  | This help message              |

---
## MCP Tools (14)

### Palace (read)
- mpr_status -- Palace status and stats
- mpr_list_wings -- List all wings
- mpr_list_rooms -- List rooms in a wing
- mpr_get_taxonomy -- Get the full taxonomy tree
- mpr_search -- Search memories by query
- mpr_check_duplicate -- Check if a memory already exists

### Palace (write)
- mpr_add_drawer -- Add a new memory (drawer)
- mpr_delete_drawer -- Delete a memory (drawer)

### Knowledge Graph
- mpr_kg_query -- Query the knowledge graph
- mpr_kg_add -- Add a knowledge graph entry
- mpr_kg_invalidate -- Invalidate a knowledge graph entry
- mpr_kg_timeline -- View knowledge graph timeline
- mpr_kg_stats -- Knowledge graph statistics

### Navigation
- mpr_traverse -- Traverse the palace structure
- mpr_find_tunnels -- Find cross-wing connections
- mpr_graph_stats -- Graph connectivity statistics

### Agent Diary
- mpr_diary_write -- Write a diary entry
- mpr_diary_read -- Read diary entries

---
## CLI Commands

    mpr init <dir>                  Initialize a new palace
    mpr mine <dir>                  Mine a project (default mode)
    mpr mine <dir> --mode convos    Mine conversation exports
    mpr search "query"              Search your memories
    mpr split <dir>                 Split large transcript files
    mpr wake-up                     Load palace into context
    mpr compress                    Compress palace storage
    mpr status                      Show palace status
    mpr doctor                      Palace health check
    mpr mcp                         Show MCP setup command
    mpr instructions <name>         Output skill instructions

---
## Architecture

    Wings (projects/people)
      +-- Rooms (topics)
            +-- Closets (summaries)
                  +-- Drawers (verbatim memories)
    Halls connect rooms within a wing.
    Tunnels connect rooms across wings.

The palace is stored locally using embedvec for vector search and SQLite for
metadata. No cloud services or API keys required.

---
## Getting Started

1. `mpr init` -- Set up your palace
2. `mpr mine` -- Mine a project or conversation
3. `mpr search` -- Find what you stored

"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_available_instructions() {
        let avail = available_instructions();
        assert!(avail.contains(&"init"));
        assert!(avail.contains(&"search"));
        assert!(avail.contains(&"mine"));
        assert!(avail.contains(&"status"));
        assert!(avail.contains(&"help"));
        assert_eq!(avail.len(), 5);
    }

    #[test]
    fn test_instruction_content() {
        assert!(instruction_content("init").is_some());
        assert!(instruction_content("search").is_some());
        assert!(instruction_content("mine").is_some());
        assert!(instruction_content("status").is_some());
        assert!(instruction_content("help").is_some());
        assert!(instruction_content("nonexistent").is_none());
    }

    #[test]
    fn test_run_instructions_success() {
        let result = run_instructions("help");
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_instructions_unknown() {
        let result = run_instructions("nonexistent");
        assert!(result.is_err());
    }
}
