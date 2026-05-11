---
name: sync-upstream-mempalace
description: How to keep the Rust port (`quangdang46/mempalace_rust`) in sync with the upstream Python repo (`milla-jovovich/mempalace`). Use whenever the user asks to port, sync, mirror, check gaps, or replay upstream fixes. Covers cloning both repos, building the diff plan against the reference commit recorded in `port.txt`, prioritising correctness fixes over features, and the smallest-blast-radius port checklist.
---

# Sync the Rust port with the upstream Python MemPalace

The Rust port `quangdang46/mempalace_rust` shadows the Python implementation
`milla-jovovich/mempalace`. The ledger of the last shared commit lives in
`port.txt` at the repo root. Everything in this skill is a recipe for taking
the upstream code from "last synced commit" to "current HEAD", spotting the
gaps, and porting them safely.

## Layout

| Path | Role |
| ---- | ---- |
| `/home/ubuntu/repos/mempalace_rust` | Rust port — make all PRs here |
| `/home/ubuntu/repos/mempalace_upstream` | Upstream Python — read-only reference |
| `port.txt` (in the Rust repo) | Sync ledger: last reference commit + open gaps |
| `.devin/skills/sync-upstream-mempalace/SKILL.md` | This skill |

The Python repo lives at `https://github.com/milla-jovovich/mempalace`. The
Rust port lives at `https://github.com/quangdang46/mempalace_rust`.

## Step 1 — Refresh both checkouts

```bash
# Rust port
cd /home/ubuntu/repos/mempalace_rust
git fetch origin
git checkout main && git pull --ff-only

# Upstream Python (cloned read-only the first time)
if [ ! -d /home/ubuntu/repos/mempalace_upstream ]; then
  git clone https://github.com/milla-jovovich/mempalace.git /home/ubuntu/repos/mempalace_upstream
fi
cd /home/ubuntu/repos/mempalace_upstream
git fetch origin && git checkout main && git pull --ff-only
```

Always work off the **upstream `main`**, never a feature branch.

## Step 2 — Read the existing ledger

```bash
head -120 /home/ubuntu/repos/mempalace_rust/port.txt
```

The bottom of `port.txt` lists the last synced commit (e.g. `94f1689`) and the
"Remaining gaps" section. Treat that section as authoritative — the last
session already decided what was deferred.

## Step 3 — Build the diff plan against the ledger reference commit

```bash
cd /home/ubuntu/repos/mempalace_upstream
LAST_SYNC=$(grep -oE '\b[0-9a-f]{7,40}\b' /home/ubuntu/repos/mempalace_rust/port.txt | tail -1)

# What changed since last sync? (newest first)
git log --oneline --no-merges "$LAST_SYNC..HEAD" -- mempalace/ | head -100

# Read the canonical changelog for headline fixes/features
sed -n '1,120p' CHANGELOG.md
```

This produces the candidate list. Filter it with the priorities below.

## Step 4 — Triage what to port (and what NOT to)

Port in this order:

1. **Correctness fixes that affect data integrity or query results.** These
   land first because they protect users from silent data corruption.
   Examples we have already ported in this skill's parent PR:
   - `#1214` — KG rejects inverted intervals (`valid_to < valid_from`)
   - `#1215` — `EntityRegistry.save()` atomic write + parent-dir fsync
   - `#1243` — diary `agent_name` lowercased so reads are case-insensitive
   - `#1164` — ISO-8601 validation at MCP boundary for `as_of`, `valid_from`,
     `valid_to`, `ended`
   - `#1314` — `tool_kg_add` forwards `valid_to`/`source_file`,
     `tool_kg_invalidate` resolves `ended` to today

2. **MCP boundary changes** (tool input schemas, new fields, validation).
   These are user-visible and small; high signal, low blast radius.

3. **CLI ergonomics changes** that don't require new dependencies.

4. **Features behind feature flags** (LLM refine, new providers, …) —
   defer if not user-requested, but record them in `port.txt` so the next
   sync session can pick them up.

5. **Docs / i18n / website / CHANGELOG-only** changes — almost never port
   verbatim into the Rust repo; the Python README is the canonical product
   doc.

For each candidate commit, before porting, read the **commit body** — the
upstream maintainers (Arnold Wender, Igor Lins e Silva, …) write extensive
rationale that you can usually compress into the Rust comment block above the
fix.

```bash
cd /home/ubuntu/repos/mempalace_upstream
git show <sha> -- mempalace/<file>.py | head -120
```

## Step 5 — Port the change to Rust

Map Python module → Rust module:

| Python (`mempalace/`) | Rust (`crates/core/src/`) |
| --- | --- |
| `mcp_server.py` | `mcp_server.rs` |
| `knowledge_graph.py` | `knowledge_graph.rs` |
| `entity_registry.py` | `entity_registry.rs` |
| `entity_detector.py` | `entity_detector.rs` |
| `config.py` | `config.rs` |
| `searcher.py` | `searcher.rs` |
| `layers.py` | `layers.rs` |
| `palace.py` | `palace.rs` |
| `palace_graph.py` | `palace_graph.rs` |
| `miner.py` | `miner.rs` |
| `convo_miner.py` | `convo_miner.rs` |
| `dialect.py` | `dialect.rs` |
| `normalize.py` | `normalize.rs` |
| `repair.py` | `repair.rs` |

Rules of thumb:

- **Comment the upstream issue number** (`#1214`, `#1243`, …) in the Rust
  code where the fix lands. This makes the next sync session trivial.
- **Write a Rust regression test that mirrors the upstream test name.**
  Upstream uses pytest, Rust uses `#[test]`; the function names should be
  recognisably the same (e.g. `test_diary_read_case_insensitive_agent`).
- **Use `anyhow::bail!` for new validation errors** to match the existing
  Rust error style. Map Python `ValueError(...)` to
  `anyhow::bail!("{field}={raw:?} is not …")`.
- **For atomic file IO**, use `std::fs::OpenOptions` + `sync_all()` +
  `std::fs::rename`. On `cfg(unix)`, also open the parent directory and
  `sync_all()` it. See `crates/core/src/entity_registry.rs::save` for the
  reference implementation.
- **For MCP tool schema changes**, both the `Deserialize` `struct Input`
  AND the `serde_json::json!(...)` `properties` block in the tool
  registration need updating.

## Step 6 — Build, test, lint (gating)

```bash
cd /home/ubuntu/repos/mempalace_rust

# 1. Build everything
cargo build --workspace

# 2. Run all tests
cargo test --workspace

# 3. Clippy with warnings-as-errors (the repo standard)
cargo clippy --workspace --all-targets -- -D warnings
```

If `cargo build` fails because rustc complains about `feature edition2024`,
run `rustup update stable` once and retry — recent dependencies require
stable >= 1.85.

If `cargo clippy` flags `redundant_guards` on a `Some(s) if s.is_empty()`
match arm, rewrite as `Some("")`.

## Step 7 — Update `port.txt`

After porting, append a new section to `port.txt`:

- The new reference commit hash (current upstream HEAD).
- A bullet list of ported issue numbers / commits.
- The updated "Remaining gaps" — copy forward anything still deferred,
  remove what got done.

Keep the existing history in the file; this is an append-only ledger so
the next sync can audit decisions.

## Step 8 — Open the PR

```bash
cd /home/ubuntu/repos/mempalace_rust
git checkout -b devin/$(date +%s)-port-upstream-fixes
git add -A
git commit -m "fix: port upstream correctness fixes (#1164, #1214, #1215, #1243, #1314)"
git push -u origin HEAD
```

Use `git_pr(action="fetch_template")` then `git_pr(action="create")`. PR
description must include:

- One section per ported upstream issue/commit, with the upstream link.
- The "Remaining gaps" delta vs. the previous `port.txt`.
- Test summary: tests added, `cargo test --workspace` pass count,
  `cargo clippy --workspace --all-targets -- -D warnings` clean.

## Step 9 — Verify CI

```text
git(action="pr_checks", wait_mode="all")
```

If CI fails on something unrelated to the diff (e.g. a flaky test), confirm
by running the same test locally before claiming it is preexisting.

## Common pitfalls

- **Do NOT carry over Python `Optional[None]` semantics naively.** Rust
  `Option<&str>` + a sanitizer that returns `Ok(None)` on `None` is the
  canonical pattern; don't introduce magic `""` sentinels in storage.
- **Do NOT alter the SQLite schema lightly.** If an upstream fix adds a
  column (e.g. `source_drawer_id`), either add a migration or scope the
  port to only the parts the existing schema already supports, and record
  the deferred schema change in `port.txt`.
- **Do NOT swallow upstream issue numbers.** Each ported bugfix gets its
  number in a Rust comment so future syncs can grep for it.
- **Do NOT port docs/i18n** unless the user explicitly asks. The Python
  README is the canonical product doc; mirroring it into Rust drifts fast.

## Quick reference — find the last synced commit

```bash
grep -oE '\b[0-9a-f]{7,40}\b' /home/ubuntu/repos/mempalace_rust/port.txt | tail -1
```

That's the only piece of state the next session needs to start the loop
again from Step 1.
