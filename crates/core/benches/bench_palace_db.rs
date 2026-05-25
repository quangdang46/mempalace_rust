//! Criterion bench for `palace_db::PalaceDb::add` and `upsert_documents`
//! (mp-002, Phase 0).
//!
//! Measures the cost of inserting 1 000 deterministic drawers into a fresh
//! in-memory `PalaceDb`. Reports throughput in elements (drawers) per second.
//! Each iteration starts from a freshly opened palace so insertion cost is
//! measured against an empty `HashMap` — i.e. the cold-mining hot path.
//!
//! Run with `cargo bench --bench bench_palace_db`. CI compiles via
//! `cargo bench --no-run`.

use std::collections::HashMap;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use mempalace_core::palace_db::PalaceDb;
use tempfile::TempDir;

#[path = "common/mod.rs"]
mod common;

use common::{build_drawers, FIXTURE_DRAWER_COUNT};

/// Smaller per-iteration batch — the headline 5 k fixture is reused for the
/// search bench; here we focus on insertion throughput on a 1 k slice so
/// each Criterion sample completes in well under a second on commodity CI.
const ADD_BATCH: usize = 1_000;

fn bench_palace_db(c: &mut Criterion) {
    let drawers = build_drawers(FIXTURE_DRAWER_COUNT);

    let mut group = c.benchmark_group("palace_db");
    group.throughput(Throughput::Elements(ADD_BATCH as u64));
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    // ---- PalaceDb::add (legacy str-tuple API) ------------------------------
    group.bench_function("add_1k_str_tuples", |b| {
        // Pre-flatten metadata into String storage so the timed section only
        // measures the hashmap insert + clone, not allocation of fixture text.
        let prepared: Vec<(String, String, String, String, String)> = drawers
            .iter()
            .take(ADD_BATCH)
            .map(|d| {
                (
                    d.id.clone(),
                    d.content.clone(),
                    d.wing.clone(),
                    d.room.clone(),
                    d.source_file.clone(),
                )
            })
            .collect();

        b.iter_batched(
            || {
                // Fresh tempdir per iteration so the `add` path keeps hitting
                // an empty inner `HashMap`.
                let tmp = TempDir::new().expect("tempdir");
                let db = PalaceDb::open(tmp.path()).expect("open palace");
                (tmp, db)
            },
            |(tmp, mut db)| {
                let docs: Vec<(&str, &str)> = prepared
                    .iter()
                    .map(|(id, content, _, _, _)| (id.as_str(), content.as_str()))
                    .collect();
                let metadata_storage: Vec<Vec<(&str, &str)>> = prepared
                    .iter()
                    .map(|(_, _, w, r, s)| {
                        vec![
                            ("wing", w.as_str()),
                            ("room", r.as_str()),
                            ("source_file", s.as_str()),
                        ]
                    })
                    .collect();
                let metadata: Vec<&[(&str, &str)]> =
                    metadata_storage.iter().map(|v| v.as_slice()).collect();

                db.add(black_box(&docs), black_box(&metadata))
                    .expect("PalaceDb::add should succeed");

                // Keep `tmp` alive until after the timed section; dropping it
                // earlier would race with any background fs flush.
                black_box(&tmp);
            },
            BatchSize::SmallInput,
        );
    });

    // ---- PalaceDb::upsert_documents (typed, JSON-metadata API) -------------
    group.bench_function("upsert_1k_typed", |b| {
        let prepared: Vec<(String, String, HashMap<String, serde_json::Value>)> = drawers
            .iter()
            .take(ADD_BATCH)
            .map(|d| {
                let mut meta = HashMap::new();
                meta.insert("wing".to_string(), serde_json::json!(d.wing));
                meta.insert("room".to_string(), serde_json::json!(d.room));
                meta.insert("source_file".to_string(), serde_json::json!(d.source_file));
                meta.insert(
                    "normalize_version".to_string(),
                    serde_json::json!(mempalace_core::constants::NORMALIZE_VERSION),
                );
                (d.id.clone(), d.content.clone(), meta)
            })
            .collect();

        b.iter_batched(
            || {
                let tmp = TempDir::new().expect("tempdir");
                let db = PalaceDb::open(tmp.path()).expect("open palace");
                (tmp, db)
            },
            |(tmp, mut db)| {
                db.upsert_documents(black_box(&prepared))
                    .expect("upsert_documents should succeed");
                black_box(&tmp);
            },
            BatchSize::SmallInput,
        );
    });

    // ---- Flush 5 k drawers -> JSON on disk ---------------------------------
    // This is the cost the convo-miner pays once per file, so worth tracking
    // separately from the in-memory insert.
    group.bench_function("flush_5k_to_disk", |b| {
        let prepared: Vec<(String, String, HashMap<String, serde_json::Value>)> = drawers
            .iter()
            .map(|d| {
                let mut meta = HashMap::new();
                meta.insert("wing".to_string(), serde_json::json!(d.wing));
                meta.insert("room".to_string(), serde_json::json!(d.room));
                meta.insert("source_file".to_string(), serde_json::json!(d.source_file));
                (d.id.clone(), d.content.clone(), meta)
            })
            .collect();

        // Throughput here is a *whole* fixture per iteration.
        b.iter_batched(
            || {
                let tmp = TempDir::new().expect("tempdir");
                let mut db = PalaceDb::open(tmp.path()).expect("open palace");
                db.upsert_documents(&prepared)
                    .expect("upsert should succeed");
                (tmp, db)
            },
            |(tmp, mut db)| {
                db.flush().expect("flush should succeed");
                black_box(&tmp);
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_palace_db);
criterion_main!(benches);
