// =====================================================================
// Telemetry integration tests — exercises the real public API
// =====================================================================
// The telemetry module (crates/core/src/telemetry.rs) is a thin façade
// over the `metrics` 0.24 + `metrics-exporter-prometheus` 0.16 stack.
//
// IMPORTANT: the `metrics` crate's global recorder can only be installed
// ONCE per process. After the first `init()` call, subsequent calls
// return `TelemetryError::AlreadyInitialized` (or, when invoked
// from a fresh `metrics::install_recorder`, fail with
// "metrics system was already initialized"). Tests below therefore
// share a single install and run as one #[test] function so they
// execute on the same thread within one process.
//
// Public surface exercised:
//   - init() / render()                          → global Prometheus handle
//   - register_counter() / register_histogram()  → pre-described + handle
//   - gauge_active_workers() / gauge_db_size()   → gauge setters
// =====================================================================

#[cfg(feature = "telemetry")]
mod telemetry_integration {
    use mempalace_core::telemetry::{
        gauge_active_workers, gauge_db_size, init, register_counter, register_histogram, render,
    };

    /// Parse the integer value at column N from a Prometheus line.
    /// `"mempalace_foo_total 42"` → Some(42.0).
    fn parse_value(snap: &str, name: &str) -> Option<f64> {
        snap.lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
    }

    /// Single test that exercises the entire public surface. The
    /// `metrics` crate's global recorder installs only once per
    /// process, so this is the natural shape for the test (a series
    /// of `#[test]` functions would race on `init()`).
    #[test]
    fn telemetry_public_api_end_to_end() {
        // init() is idempotent via INIT_CALLED guard; first call wins.
        let _ = init();

        // -- counters: register, increment, render ---------------------
        let c_search = register_counter(
            "mempalace_tt_search_total",
            "telemetry_test: scratch search counter",
        );
        c_search.increment(3);
        let c_insert = register_counter(
            "mempalace_tt_insert_total",
            "telemetry_test: scratch insert counter",
        );
        c_insert.increment(1);

        // -- histograms: register, record, render -----------------------
        let h = register_histogram(
            "mempalace_tt_latency_ms",
            "telemetry_test: scratch latency histogram",
        );
        h.record(12.5);
        h.record(7.5);

        // -- gauges: setter, render -------------------------------------
        gauge_active_workers(42.0);
        gauge_db_size(1024.0 * 1024.0);

        // -- render: assert all five instruments are present -----------
        let snap = render().expect("render() should return Some after init");

        // counters
        assert_eq!(
            parse_value(&snap, "mempalace_tt_search_total"),
            Some(3.0),
            "expected mempalace_tt_search_total = 3, snapshot:\n{snap}"
        );
        assert_eq!(
            parse_value(&snap, "mempalace_tt_insert_total"),
            Some(1.0),
            "expected mempalace_tt_insert_total = 1, snapshot:\n{snap}"
        );

        // histogram: Prometheus emits `<name>_count N` for the count
        assert_eq!(
            parse_value(&snap, "mempalace_tt_latency_ms_count"),
            Some(2.0),
            "expected mempalace_tt_latency_ms_count = 2, snapshot:\n{snap}"
        );

        // gauges
        assert_eq!(
            parse_value(&snap, "mempalace_active_workers"),
            Some(42.0),
            "expected mempalace_active_workers = 42, snapshot:\n{snap}"
        );
        assert_eq!(
            parse_value(&snap, "mempalace_db_size_bytes"),
            Some(1048576.0),
            "expected mempalace_db_size_bytes = 1048576, snapshot:\n{snap}"
        );

        // pre-described canonical names (from describe_counters() in
        // init()) are not asserted here: Prometheus only emits a
        // counter after at least one increment, so without a recorded
        // value it would be missing from the snapshot regardless of
        // describe_counter!.
    }
}

// =====================================================================
// Compile-time gate: the `telemetry` module must compile when the
// feature is OFF (no-op), and the `telemetry` feature must compile
// when ON. This is enforced by `cargo build -p mempalace-core` and
// `cargo build -p mempalace-core --features telemetry` respectively.
// =====================================================================
#[cfg(not(feature = "telemetry"))]
#[test]
fn test_telemetry_feature_gated() {
    // When telemetry is OFF, the telemetry module is not compiled.
    // This test is a no-op placeholder that always passes.
    // Real verification is `cargo build --no-default-features` (must succeed).
    let _ = true;
}
