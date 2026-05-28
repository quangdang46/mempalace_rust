//! Shared deterministic fixture for criterion benches (mp-002, Phase 0).
//!
//! Goals:
//! * No external `rand` dep — uses a fixed-seed `xorshift64` so every bench
//!   run on every CI host produces the same 5 000 drawers byte-for-byte.
//! * No real palace on disk required — every helper writes into a
//!   caller-supplied [`tempfile::TempDir`].
//! * Cheap to call from a Criterion `setup` closure: regenerating the
//!   fixture is O(N) and bounded by JSON I/O when flushing the palace.
//!
//! The fixture intentionally mirrors the production drawer schema (`wing`,
//! `room`, `source_file`, `created_at`, `normalize_version`) so the search
//! bench exercises the same metadata-filter code path the CLI does.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use mempalace_core::palace_db::PalaceDb;

/// Number of drawers used for the headline 5 k-drawer fixture (gap report
/// 04 P2 #18).
pub const FIXTURE_DRAWER_COUNT: usize = 5_000;

/// Deterministic seed — bumping this invalidates historical bench numbers,
/// so keep stable across releases.
pub const FIXTURE_SEED: u64 = 0x6D70_3030_3220_5345; // "mp002 SE"

const WINGS: &[&str] = &[
    "wing_kai",
    "wing_priya",
    "wing_driftwood",
    "wing_orion",
    "wing_general",
];

const ROOMS: &[&str] = &[
    "auth-migration",
    "graphql-switch",
    "ci-pipeline",
    "billing",
    "deploy",
    "rate-limiting",
    "observability",
    "indexing",
];

/// Pool of word stems used to synthesise document text. Keeping this small
/// and English keeps `naive_similarity` (the production query path) firing
/// realistic overlap scores instead of degenerating to all-zero.
const TOKEN_POOL: &[&str] = &[
    "auth",
    "token",
    "refresh",
    "rotate",
    "deploy",
    "rollback",
    "feature",
    "flag",
    "graphql",
    "schema",
    "migration",
    "postgres",
    "sqlite",
    "index",
    "vector",
    "search",
    "embedding",
    "model",
    "throughput",
    "latency",
    "billing",
    "invoice",
    "stripe",
    "clerk",
    "oauth",
    "password",
    "session",
    "cache",
    "warm",
    "cold",
    "queue",
    "broker",
    "kafka",
    "redis",
    "memory",
    "leak",
    "regression",
    "metric",
    "trace",
    "span",
    "log",
    "structured",
    "rate",
    "limit",
    "burst",
    "circuit",
    "breaker",
    "retry",
    "exponential",
    "jitter",
    "consensus",
    "raft",
    "quorum",
    "leader",
    "follower",
    "shard",
];

/// Fixed-seed xorshift64* PRNG. Tiny, stable, allocation-free, and enough
/// for synthetic-text generation. Do not use for anything cryptographic.
pub struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        // xorshift64 hates a zero state; xor a constant in just in case.
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn gen_range(&mut self, max: usize) -> usize {
        debug_assert!(
            max > 0,
            "DeterministicRng::gen_range needs a positive bound"
        );
        (self.next_u64() % max as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        &slice[self.gen_range(slice.len())]
    }

    pub fn pick_str<'a>(&mut self, slice: &'a [&'a str]) -> &'a str {
        slice[self.gen_range(slice.len())]
    }
}

/// A single drawer record matching the production schema.
#[derive(Debug, Clone)]
pub struct DrawerFixture {
    pub id: String,
    pub content: String,
    pub wing: String,
    pub room: String,
    pub source_file: String,
}

/// Synthesise `count` drawers from the deterministic seed.
pub fn build_drawers(count: usize) -> Vec<DrawerFixture> {
    let mut rng = DeterministicRng::new(FIXTURE_SEED);
    (0..count)
        .map(|i| {
            let wing = (*rng.pick(WINGS)).to_string();
            let room = (*rng.pick(ROOMS)).to_string();
            // 40-80 token sentence — long enough to chunk meaningfully but
            // short enough that 5 000 of them stay under ~5 MB JSON.
            let token_count = 40 + rng.gen_range(40);
            let mut tokens = Vec::with_capacity(token_count);
            for _ in 0..token_count {
                tokens.push(rng.pick_str(TOKEN_POOL));
            }
            let content = format!("drawer-{i:05} {}", tokens.join(" "));
            let source_file = format!("fixture/{wing}/{room}/{i:05}.md");
            DrawerFixture {
                id: format!("fixture-{i:05}"),
                content,
                wing,
                room,
                source_file,
            }
        })
        .collect()
}

/// Write `drawers` into a fresh palace at `palace_path`, calling `flush`
/// once at the end so `searcher::search_memories` sees a "Ready" palace.
pub fn write_palace(palace_path: &Path, drawers: &[DrawerFixture]) -> anyhow::Result<()> {
    std::fs::create_dir_all(palace_path)?;
    let mut db = PalaceDb::open(palace_path)?;

    // Use the typed upsert path so we can attach the `normalize_version`
    // metadata the production miners stamp on every drawer.
    let upserts: Vec<(
        String,
        String,
        std::collections::HashMap<String, serde_json::Value>,
    )> = drawers
        .iter()
        .map(|d| {
            let mut meta = std::collections::HashMap::new();
            meta.insert("wing".to_string(), serde_json::json!(d.wing));
            meta.insert("room".to_string(), serde_json::json!(d.room));
            meta.insert("source_file".to_string(), serde_json::json!(d.source_file));
            meta.insert(
                "created_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            meta.insert(
                "normalize_version".to_string(),
                serde_json::json!(mempalace_core::constants::NORMALIZE_VERSION),
            );
            (d.id.clone(), d.content.clone(), meta)
        })
        .collect();

    db.upsert_documents(&upserts)?;
    db.flush()?;
    Ok(())
}

/// Build a temp project directory the miner can chew through. Returns the
/// dir handle (drop = cleanup) plus the count of files written.
///
/// The directory ships a minimal `mempalace.json` so `miner::mine` succeeds
/// without the test having to call `onboarding`.
pub fn write_project_dir(
    project_dir: &Path,
    file_count: usize,
    chars_per_file: usize,
) -> anyhow::Result<usize> {
    std::fs::create_dir_all(project_dir)?;

    // Minimal config — wing + a single catch-all room.
    let config = serde_json::json!({
        "wing": "wing_bench",
        "rooms": [
            {
                "name": "general",
                "description": "Bench fixture room",
                "keywords": [],
            }
        ],
    });
    std::fs::write(
        project_dir.join("mempalace.json"),
        serde_json::to_string_pretty(&config)?,
    )?;

    let mut rng = DeterministicRng::new(FIXTURE_SEED ^ 0xBE);
    for i in 0..file_count {
        let mut buf = String::with_capacity(chars_per_file + 32);
        buf.push_str(&format!("# bench-file-{i:04}\n\n"));
        while buf.len() < chars_per_file {
            buf.push_str(rng.pick_str(TOKEN_POOL));
            buf.push(' ');
            // Insert paragraph breaks every ~120 chars so the chunker has
            // legitimate split points.
            if rng.gen_range(20) == 0 {
                buf.push_str("\n\n");
            }
        }
        let path: PathBuf = project_dir.join(format!("bench-file-{i:04}.md"));
        std::fs::write(path, buf)?;
    }

    Ok(file_count)
}
