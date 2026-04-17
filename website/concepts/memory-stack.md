# Memory Stack

MemPalace uses a 4-layer memory stack. Each layer loads progressively more data only when needed.

## The Layers

| Layer | What | Size | When |
|-------|------|------|------|
| **L0** | Identity — who is this AI? | ~50-100 tokens | Always loaded |
| **L1** | Essential Story — top moments | ~500-800 tokens | Always loaded |
| **L2** | Room Recall — filtered retrieval | ~200–500 each | When topic comes up |
| **L3** | Deep Search — full semantic query | Variable | When explicitly asked |

In the current implementation, a typical wake-up is roughly **~600-900 tokens** for L0 + L1. Searches only fire when needed.

## Layer 0: Identity

A plain text file at `~/.mempalace/identity.txt`. Always loaded as the AI's self-concept.

```text
I am Atlas, a personal AI assistant for Alice.
Traits: warm, direct, remembers everything.
People: Alice (creator), Bob (Alice's partner).
Project: A journaling app that helps people process emotions.
```

~50 tokens. Tells the AI who it is and who it works with.

## Layer 1: Essential Story

Auto-generated from the highest-importance drawers in the palace. Groups by room, picks the top moments, and keeps the output bounded.

The generation process:
1. Reads all drawers from embedvec
2. Scores each by importance/emotional weight
3. Takes the top 15 moments
4. Groups by room for readability
5. Truncates to fit within 3,200 characters

```
## L1 — ESSENTIAL STORY

[auth-migration]
  - Team decided to migrate from Auth0 to Clerk — pricing + DX  (session_2026-01-15.md)
  - Kai debugged the OAuth token refresh issue  (session_2026-01-20.md)

[deploy-process]
  - Switched to blue-green deploys after the January outage  (session_2026-02-01.md)
```

## Layer 2: On-Demand Recall

Loaded when a specific topic or wing comes up in conversation. Retrieves drawers filtered by wing and/or room — typically ~200–500 tokens.

```rust
use mempalace::layers::MemoryStack;

let stack = MemoryStack::new("~/.mempalace/palace".to_string())?;
let recall = stack.recall(wing="driftwood", room="auth")?;
// → returns recent drawers about auth in the driftwood project
```

## Layer 3: Deep Search

Full semantic search against the entire palace. This is what fires when you or the AI explicitly asks a question.

```rust
let results = stack.search("why did we switch to GraphQL", wing="myapp", room="architecture", n_results=5)?;
// → returns top-5 matching drawers with similarity scores
```

## Wake-Up Budget

The point of the stack is bounded startup context, not a fixed universal token count. The exact size depends on your identity file and what Layer 1 selects, but the implementation keeps wake-up meaningfully smaller than loading the full corpus into the prompt.

## Using the Stack

### CLI

```bash
# Wake-up context (L0 + L1)
mpr wake-up

# Project-specific wake-up
mpr wake-up --wing driftwood
```

### Rust API

```rust
use mempalace::layers::MemoryStack;

let stack = MemoryStack::new("~/.mempalace/palace".to_string())?;

// L0 + L1: wake-up (~600-900 tokens in typical use)
println!("{}", stack.wake_up("myapp")?);

// L2: on-demand recall
println!("{}", stack.recall("myapp", "architecture", 10)?);

// L3: deep search
println!("{}", stack.search("pricing change", "myapp", "", 5)?);

# Status
print(stack.status())
```
