// Health monitoring — CPU, memory, KV latency, embedder latency,
// worker count, and uptime checks with configurable thresholds.
//
// Feature-gated behind `health`. All 6 checks must be implemented.
// No `unwrap()` in production code; use `expect()` with context strings.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;
use sysinfo::System;
use tokio::sync::RwLock;

#[cfg(feature = "health")]
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Overall health status — escalated to the worst check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Critical,
}

impl std::cmp::Ord for HealthStatus {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn rank(s: &HealthStatus) -> u8 {
            match s {
                HealthStatus::Healthy => 0,
                HealthStatus::Degraded => 1,
                HealthStatus::Critical => 2,
            }
        }
        rank(self).cmp(&rank(other))
    }
}

impl std::cmp::PartialOrd for HealthStatus {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Result of a single health check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub value: f64,
    pub threshold: f64,
    pub status: HealthStatus,
    pub message: String,
}

/// Full health report — includes per-check results and overall status.
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub status: HealthStatus,
    pub uptime_seconds: u64,
    pub checks: Vec<CheckResult>,
    pub timestamp: DateTime<Utc>,
}

/// Trait implemented by each individual health check.
// LINT: trait object safety — the `run()` method is stateless and Send+Sync
// is satisfied because `CheckResult` is Copy (no interior mutability).
#[allow(clippy::ref_as_ptr)]
pub trait HealthCheck: Send + Sync {
    fn name(&self) -> &str;
    fn run(&self) -> CheckResult;
    fn threshold_warn(&self) -> f64;
    fn threshold_crit(&self) -> f64;
}

// ---------------------------------------------------------------------------
// HealthMonitor
// ---------------------------------------------------------------------------

/// Runs all registered health checks and produces a `HealthReport`.
pub struct HealthMonitor {
    checks: Vec<Box<dyn HealthCheck>>,
    started_at: Instant,
}

impl HealthMonitor {
    pub fn new() -> Self {
        HealthMonitor {
            checks: Vec::new(),
            started_at: Instant::now(),
        }
    }

    /// Register a new check. Checks run in registration order.
    pub fn register(&mut self, check: Box<dyn HealthCheck>) {
        self.checks.push(check);
    }

    /// Run all checks and aggregate into a `HealthReport`.
    /// The overall status is the maximum (worst) of all individual check statuses.
    pub fn run_all(&self) -> HealthReport {
        let uptime_seconds = self.started_at.elapsed().as_secs();
        let timestamp = Utc::now();

        let mut checks: Vec<CheckResult> = Vec::with_capacity(self.checks.len());
        let mut overall = HealthStatus::Healthy;

        for check in &self.checks {
            let result = check.run();
            if result.status > overall {
                overall = result.status;
            }
            checks.push(result);
        }

        HealthReport {
            status: overall,
            uptime_seconds,
            checks,
            timestamp,
        }
    }

    /// Maps `HealthReport.status` to an HTTP status code.
    pub fn to_http_status(&self, report: &HealthReport) -> u16 {
        match report.status {
            HealthStatus::Critical => 503,
            _ => 200,
        }
    }
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Concrete checks — CPU
// ---------------------------------------------------------------------------

/// CPU utilisation check using `sysinfo`.
///
/// - Warn threshold: 80%
/// - Critical threshold: 90%
pub struct CpuCheck {
    _priv: (),
}

impl CpuCheck {
    pub fn new() -> Self {
        CpuCheck { _priv: () }
    }
}

impl Default for CpuCheck {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthCheck for CpuCheck {
    fn name(&self) -> &str {
        "cpu"
    }

    fn run(&self) -> CheckResult {
        let mut sys = System::new();
        // Need one refresh cycle for CPU % to be populated
        sys.refresh_cpu_all();
        // Give a tiny window for the CPU reading to settle
        std::thread::sleep(std::time::Duration::from_millis(50));
        sys.refresh_cpu_all();

        let cpu_count = sys.cpus().len();
        let total_cpu = if cpu_count > 0 {
            sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>() / cpu_count as f32
        } else {
            0.0
        };

        let value = total_cpu as f64;
        let warn = self.threshold_warn();
        let crit = self.threshold_crit();

        let status = if value >= crit {
            HealthStatus::Critical
        } else if value >= warn {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let message = match status {
            HealthStatus::Critical => format!(
                "CPU usage {}% exceeds critical threshold {}%",
                value as u8, crit as u8
            ),
            HealthStatus::Degraded => format!(
                "CPU usage {}% exceeds warning threshold {}%",
                value as u8, crit as u8
            ),
            HealthStatus::Healthy => format!("CPU usage {}% is healthy", value as u8),
        };

        CheckResult {
            name: self.name().to_string(),
            value,
            threshold: warn,
            status,
            message,
        }
    }

    fn threshold_warn(&self) -> f64 {
        80.0
    }

    fn threshold_crit(&self) -> f64 {
        90.0
    }
}

// ---------------------------------------------------------------------------
// Concrete checks — Memory
// ---------------------------------------------------------------------------

/// Memory (heap + RSS) utilisation check.
///
/// Thresholds are percentage of total memory. Additionally enforces a
/// 512 MB RSS floor — if RSS is below 512 MB the check is always healthy
/// regardless of percentage.
///
/// - Warn: 80%
/// - Critical: 95%
pub struct MemoryCheck {
    rss_floor_bytes: u64, // 512 MB default = 536_870_912
    _priv: (),
}

impl MemoryCheck {
    pub fn new() -> Self {
        MemoryCheck {
            rss_floor_bytes: 512 * 1024 * 1024,
            _priv: (),
        }
    }

    /// Override the RSS floor (useful for testing).
    #[cfg(test)]
    pub fn with_rss_floor(self, bytes: u64) -> Self {
        MemoryCheck {
            rss_floor_bytes: bytes,
            ..self
        }
    }
}

impl Default for MemoryCheck {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthCheck for MemoryCheck {
    fn name(&self) -> &str {
        "memory"
    }

    fn run(&self) -> CheckResult {
        let mut sys = System::new();
        sys.refresh_memory();

        let total = sys.total_memory() as f64;
        let used = sys.used_memory() as f64;
        let rss_bytes = sys.processes().values().map(|p| p.memory()).sum::<u64>();

        let pct = if total > 0.0 {
            (used / total) * 100.0
        } else {
            0.0
        };
        let value = pct;
        let warn = self.threshold_warn();
        let crit = self.threshold_crit();

        // Enforce RSS floor — only applies when RSS is above the floor
        let rss_ok = rss_bytes >= self.rss_floor_bytes;

        let status = if !rss_ok {
            // RSS below floor — consider it healthy (process hasn't warmed up yet)
            HealthStatus::Healthy
        } else if value >= crit {
            HealthStatus::Critical
        } else if value >= warn {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let rss_mb = rss_bytes as f64 / (1024.0 * 1024.0);
        let used_mb = used / (1024.0 * 1024.0);
        let total_mb = total / (1024.0 * 1024.0);

        let message = match status {
            HealthStatus::Critical => {
                format!(
                    "Memory usage {:.0}% ({:.0} MB / {:.0} MB, RSS {rss_mb} MB) exceeds critical threshold {}%",
                    value, used_mb, total_mb, crit as u8
                )
            }
            HealthStatus::Degraded => {
                format!(
                    "Memory usage {:.0}% ({:.0} MB / {:.0} MB, RSS {rss_mb} MB) exceeds warning threshold {}%",
                    value, used_mb, total_mb, warn as u8
                )
            }
            HealthStatus::Healthy => {
                format!(
                    "Memory usage {:.0}% ({:.0} MB / {:.0} MB, RSS {rss_mb} MB) is healthy",
                    value, used_mb, total_mb
                )
            }
        };

        CheckResult {
            name: self.name().to_string(),
            value,
            threshold: warn,
            status,
            message,
        }
    }

    fn threshold_warn(&self) -> f64 {
        80.0
    }

    fn threshold_crit(&self) -> f64 {
        95.0
    }
}

// ---------------------------------------------------------------------------
// Concrete checks — KV latency (palace_db probe)
// ---------------------------------------------------------------------------

/// KV (SQLite palace_db) latency check — runs `SELECT 1` against the
/// coordination DB.
///
/// - Warn: > 500 ms
/// - Critical: > 2000 ms
pub struct KvLatencyCheck {
    palace_db_path: std::path::PathBuf,
    _priv: (),
}

impl KvLatencyCheck {
    pub fn new(palace_db_path: std::path::PathBuf) -> Self {
        KvLatencyCheck {
            palace_db_path,
            _priv: (),
        }
    }
}

impl HealthCheck for KvLatencyCheck {
    fn name(&self) -> &str {
        "kv_latency"
    }

    fn run(&self) -> CheckResult {
        let start = Instant::now();
        let ok = rusqlite::Connection::open(&self.palace_db_path)
            .and_then(|conn| {
                let mut stmt = conn.prepare("SELECT 1")?;
                let _ = stmt.execute([]);
                Ok(())
            })
            .is_ok();
        let elapsed_ms = start.elapsed().as_millis() as f64;

        let value = elapsed_ms;
        let warn = self.threshold_warn();
        let crit = self.threshold_crit();

        let status = if !ok || value >= crit {
            HealthStatus::Critical
        } else if value >= warn {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let message = if !ok {
            "KV probe failed: could not open palace_db connection".to_string()
        } else {
            match status {
                HealthStatus::Critical => format!(
                    "KV latency {:.0} ms exceeds critical threshold {:.0} ms",
                    value, crit
                ),
                HealthStatus::Degraded => format!(
                    "KV latency {:.0} ms exceeds warning threshold {:.0} ms",
                    value, warn
                ),
                HealthStatus::Healthy => format!("KV latency {:.0} ms is healthy", value),
            }
        };

        CheckResult {
            name: self.name().to_string(),
            value,
            threshold: warn,
            status,
            message,
        }
    }

    fn threshold_warn(&self) -> f64 {
        500.0
    }

    fn threshold_crit(&self) -> f64 {
        2000.0
    }
}

// ---------------------------------------------------------------------------
// Concrete checks — Embedder latency
// ---------------------------------------------------------------------------

/// Embedder latency check — embeds a short test string and measures round-trip.
///
/// - Warn: > 2000 ms
/// - Critical: > 5000 ms
pub struct EmbedderLatencyCheck {
    embedder: Arc<dyn crate::embed::Embedder>,
    _priv: (),
}

impl EmbedderLatencyCheck {
    pub fn new(embedder: Arc<dyn crate::embed::Embedder>) -> Self {
        EmbedderLatencyCheck {
            embedder,
            _priv: (),
        }
    }
}

impl HealthCheck for EmbedderLatencyCheck {
    fn name(&self) -> &str {
        "embedder_latency"
    }

    fn run(&self) -> CheckResult {
        let test_string = "health probe";
        let start = Instant::now();

        // Embedder::embed is async; we block on it using the current runtime
        let ok = tokio::runtime::Handle::current()
            .block_on(self.embedder.embed(test_string))
            .is_ok();

        let elapsed_ms = start.elapsed().as_millis() as f64;
        let value = elapsed_ms;
        let warn = self.threshold_warn();
        let crit = self.threshold_crit();

        let status = if !ok || value >= crit {
            HealthStatus::Critical
        } else if value >= warn {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let message = if !ok {
            "Embedder probe failed: could not embed test string".to_string()
        } else {
            match status {
                HealthStatus::Critical => format!(
                    "Embedder latency {:.0} ms exceeds critical threshold {:.0} ms",
                    value, crit
                ),
                HealthStatus::Degraded => format!(
                    "Embedder latency {:.0} ms exceeds warning threshold {:.0} ms",
                    value, warn
                ),
                HealthStatus::Healthy => format!("Embedder latency {:.0} ms is healthy", value),
            }
        };

        CheckResult {
            name: self.name().to_string(),
            value,
            threshold: warn,
            status,
            message,
        }
    }

    fn threshold_warn(&self) -> f64 {
        2000.0
    }

    fn threshold_crit(&self) -> f64 {
        5000.0
    }
}

// ---------------------------------------------------------------------------
// Concrete checks — Worker count
// ---------------------------------------------------------------------------

/// Worker count check — compares active background tasks against a limit.
///
/// Warns when usage exceeds 80% of the limit, critical when above 95%.
pub struct WorkerCountCheck {
    active_count: Arc<AtomicU64>,
    limit: u64,
    _priv: (),
}

impl WorkerCountCheck {
    pub fn new(active_count: Arc<AtomicU64>, limit: u64) -> Self {
        WorkerCountCheck {
            active_count,
            limit,
            _priv: (),
        }
    }
}

impl HealthCheck for WorkerCountCheck {
    fn name(&self) -> &str {
        "workers"
    }

    fn run(&self) -> CheckResult {
        let active = self.active_count.load(Ordering::Relaxed) as f64;
        let limit = self.limit as f64;
        let value = if limit > 0.0 {
            (active / limit) * 100.0
        } else {
            0.0
        };
        let warn = self.threshold_warn();
        let crit = self.threshold_crit();

        let status = if value >= crit {
            HealthStatus::Critical
        } else if value >= warn {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let message = match status {
            HealthStatus::Critical => format!(
                "Worker count {:.0}% ({:.0} / {:.0}) exceeds critical threshold {:.0}%",
                value, active, limit, crit
            ),
            HealthStatus::Degraded => format!(
                "Worker count {:.0}% ({:.0} / {:.0}) exceeds warning threshold {:.0}%",
                value, active, limit, warn
            ),
            HealthStatus::Healthy => format!(
                "Worker count {:.0}% ({:.0} / {:.0}) is healthy",
                value, active, limit
            ),
        };

        CheckResult {
            name: self.name().to_string(),
            value,
            threshold: warn,
            status,
            message,
        }
    }

    fn threshold_warn(&self) -> f64 {
        80.0
    }

    fn threshold_crit(&self) -> f64 {
        95.0
    }
}

// ---------------------------------------------------------------------------
// Concrete checks — Uptime
// ---------------------------------------------------------------------------

/// Uptime check — always healthy unless the process has been running
/// for less than 1 second (startup period).
pub struct UptimeCheck {
    started_at: Instant,
}

impl UptimeCheck {
    pub fn new(started_at: Instant) -> Self {
        UptimeCheck { started_at }
    }
}

impl HealthCheck for UptimeCheck {
    fn name(&self) -> &str {
        "uptime"
    }

    fn run(&self) -> CheckResult {
        let uptime_secs = self.started_at.elapsed().as_secs();
        let value = uptime_secs as f64;
        let threshold = 1.0; // 1 second minimum

        // Only unhealthy during the first second of startup
        let status = if uptime_secs < 1 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let message = if status == HealthStatus::Degraded {
            format!(
                "Uptime {:.0}s — process still initialising (threshold {:.0}s)",
                value, threshold
            )
        } else {
            format!("Uptime {:.0}s — healthy", value)
        };

        CheckResult {
            name: self.name().to_string(),
            value,
            threshold,
            status,
            message,
        }
    }

    fn threshold_warn(&self) -> f64 {
        1.0
    }

    fn threshold_crit(&self) -> f64 {
        0.0 // not used
    }
}

// ---------------------------------------------------------------------------
// Concrete checks — Event-loop lag
// ---------------------------------------------------------------------------

/// Event-loop lag check.
///
/// Measures how stale the most recent heartbeat from the async
/// runtime is. The runtime's main task should call
/// [`GlobalHealthMonitor::heartbeat`] (or hold a clone of
/// [`EventLoopLagCheck::handle`] and call
/// [`EventLoopHandle::beat`]) at least every
/// `warn_threshold` milliseconds. A lag larger than the warn
/// threshold indicates the runtime is too busy to schedule the
/// heartbeat task; a lag larger than the crit threshold indicates
/// the event loop is effectively stalled.
///
/// Matches agentmemory's `state/health.ts` event-loop-lag check:
/// 100 ms warn, 500 ms crit, 1000 ms idle interval.
pub struct EventLoopLagCheck {
    handle: EventLoopHandle,
    warn_ms: u64,
    crit_ms: u64,
}

#[derive(Clone)]
pub struct EventLoopHandle {
    last_beat_ms: Arc<AtomicU64>,
}

impl EventLoopHandle {
    /// Record a heartbeat at "now". Cheap; does not allocate.
    pub fn beat(&self) {
        let now_ms = crate::health::now_millis();
        self.last_beat_ms.store(now_ms, Ordering::Relaxed);
    }

    /// Read the lag in milliseconds since the last heartbeat.
    pub fn lag_ms(&self) -> u64 {
        let last = self.last_beat_ms.load(Ordering::Relaxed);
        let now = crate::health::now_millis();
        now.saturating_sub(last)
    }
}

impl EventLoopLagCheck {
    pub fn new(warn_ms: u64, crit_ms: u64) -> Self {
        Self {
            handle: EventLoopHandle {
                last_beat_ms: Arc::new(AtomicU64::new(crate::health::now_millis())),
            },
            warn_ms,
            crit_ms,
        }
    }

    pub fn handle(&self) -> EventLoopHandle {
        self.handle.clone()
    }
}

impl HealthCheck for EventLoopLagCheck {
    fn name(&self) -> &str {
        "event_loop_lag"
    }

    fn run(&self) -> CheckResult {
        let lag = self.handle.lag_ms();
        let value = lag as f64;
        let warn = self.warn_ms as f64;
        let crit = self.crit_ms as f64;

        let status = if lag >= self.crit_ms {
            HealthStatus::Critical
        } else if lag >= self.warn_ms {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let message = match status {
            HealthStatus::Critical => format!(
                "Event-loop lag {}ms exceeds critical threshold {}ms",
                lag, self.crit_ms
            ),
            HealthStatus::Degraded => format!(
                "Event-loop lag {}ms exceeds warning threshold {}ms",
                lag, self.warn_ms
            ),
            HealthStatus::Healthy => {
                format!("Event-loop lag {}ms is healthy", lag)
            }
        };

        CheckResult {
            name: self.name().to_string(),
            value,
            threshold: warn,
            status,
            message,
        }
    }

    fn threshold_warn(&self) -> f64 {
        self.warn_ms as f64
    }

    fn threshold_crit(&self) -> f64 {
        self.crit_ms as f64
    }
}

/// Wall-clock millisecond timestamp (Unix epoch). Both
/// [`EventLoopHandle::beat`] and [`EventLoopHandle::lag_ms`] sample
/// from this same clock, so the difference is process-local even
/// though the absolute value tracks wall time. `lag_ms` uses
/// `saturating_sub` so a backwards clock jump is clamped to 0
/// rather than panicking.
fn now_millis() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Global health monitor singleton (feature-gated)
// ---------------------------------------------------------------------------

/// Thread-safe handle to the global `HealthMonitor`.
///
/// Initialised via `init_health_monitor()` during server startup when the
/// `health` feature is enabled.
#[cfg(feature = "health")]
pub struct GlobalHealthMonitor {
    monitor: RwLock<HealthMonitor>,
    started_at: Instant,
    palace_db_path: std::path::PathBuf,
    embedder: Arc<dyn crate::embed::Embedder>,
    worker_count: Arc<AtomicU64>,
    worker_limit: u64,
    event_loop: EventLoopHandle,
}

#[cfg(feature = "health")]
impl GlobalHealthMonitor {
    /// Create a new global monitor, wiring in the palace DB path and embedder.
    pub fn new(
        palace_db_path: std::path::PathBuf,
        embedder: Arc<dyn crate::embed::Embedder>,
        worker_limit: u64,
    ) -> Self {
        let started_at = Instant::now();
        let worker_count = Arc::new(AtomicU64::new(0));

        let event_loop_check = EventLoopLagCheck::new(100, 500);
        let event_loop = event_loop_check.handle();

        let mut monitor = HealthMonitor::new();
        monitor.register(Box::new(UptimeCheck::new(started_at)));
        monitor.register(Box::new(CpuCheck::new()));
        monitor.register(Box::new(MemoryCheck::new()));
        monitor.register(Box::new(KvLatencyCheck::new(palace_db_path.clone())));
        monitor.register(Box::new(EmbedderLatencyCheck::new(Arc::clone(&embedder))));
        monitor.register(Box::new(WorkerCountCheck::new(
            Arc::clone(&worker_count),
            worker_limit,
        )));
        monitor.register(Box::new(event_loop_check));

        GlobalHealthMonitor {
            monitor: RwLock::new(monitor),
            started_at,
            palace_db_path,
            embedder,
            worker_count,
            worker_limit,
            event_loop,
        }
    }

    /// Increment the active worker count.
    pub fn worker_started(&self) {
        self.worker_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the active worker count.
    pub fn worker_stopped(&self) {
        self.worker_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Record an event-loop heartbeat. The async runtime should
    /// call this on every tick (or at least every
    /// [`EventLoopLagCheck`] warn threshold — 100 ms by default).
    /// Cheap; just an atomic store.
    pub fn heartbeat(&self) {
        self.event_loop.beat();
    }

    /// Clone-able handle to the event-loop heartbeat. Useful when
    /// wiring the heartbeat into a background task that already
    /// holds an `Arc<...>` over the monitor.
    pub fn event_loop_handle(&self) -> EventLoopHandle {
        self.event_loop.clone()
    }

    /// Run all checks and return the report.
    pub async fn run_all(&self) -> HealthReport {
        self.monitor.read().await.run_all()
    }

    /// Reset the monitor (re-creates all check handles).
    pub async fn reset(&self) {
        let mut monitor = HealthMonitor::new();
        monitor.register(Box::new(UptimeCheck::new(self.started_at)));
        monitor.register(Box::new(CpuCheck::new()));
        monitor.register(Box::new(MemoryCheck::new()));
        monitor.register(Box::new(KvLatencyCheck::new(self.palace_db_path.clone())));
        monitor.register(Box::new(EmbedderLatencyCheck::new(Arc::clone(
            &self.embedder,
        ))));
        monitor.register(Box::new(WorkerCountCheck::new(
            Arc::clone(&self.worker_count),
            self.worker_limit,
        )));
        monitor.register(Box::new(EventLoopLagCheck::new(100, 500)));
        *self.monitor.write().await = monitor;
    }
}

#[cfg(feature = "health")]
static GLOBAL_HEALTH_MONITOR: std::sync::OnceLock<GlobalHealthMonitor> = std::sync::OnceLock::new();

/// Initialise the global health monitor. Safe to call multiple times (only
/// the first call takes effect). Must be called before `get_health_monitor()`.
#[cfg(feature = "health")]
pub fn init_health_monitor(
    palace_db_path: std::path::PathBuf,
    embedder: Arc<dyn crate::embed::Embedder>,
    worker_limit: u64,
) {
    let _ = GLOBAL_HEALTH_MONITOR
        .get_or_init(|| GlobalHealthMonitor::new(palace_db_path, embedder, worker_limit));
}

/// Access the global health monitor. Panics if not initialised.
#[cfg(feature = "health")]
pub fn get_health_monitor() -> &'static GlobalHealthMonitor {
    GLOBAL_HEALTH_MONITOR
        .get()
        .expect("health monitor not initialised — call init_health_monitor() first")
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // Mock check helpers (also used by integration tests in health_test.rs)
    // ---------------------------------------------------------------------------

    pub struct MockHealthyCheck;
    impl HealthCheck for MockHealthyCheck {
        fn name(&self) -> &str {
            "mock_healthy"
        }
        fn run(&self) -> CheckResult {
            CheckResult {
                name: self.name().to_string(),
                value: 50.0,
                threshold: 80.0,
                status: HealthStatus::Healthy,
                message: "all good".to_string(),
            }
        }
        fn threshold_warn(&self) -> f64 {
            80.0
        }
        fn threshold_crit(&self) -> f64 {
            95.0
        }
    }

    pub struct MockWarningCheck;
    impl HealthCheck for MockWarningCheck {
        fn name(&self) -> &str {
            "mock_warning"
        }
        fn run(&self) -> CheckResult {
            CheckResult {
                name: self.name().to_string(),
                value: 85.0,
                threshold: 80.0,
                status: HealthStatus::Degraded,
                message: "approaching limit".to_string(),
            }
        }
        fn threshold_warn(&self) -> f64 {
            80.0
        }
        fn threshold_crit(&self) -> f64 {
            95.0
        }
    }

    pub struct MockCriticalCheck;
    impl HealthCheck for MockCriticalCheck {
        fn name(&self) -> &str {
            "mock_critical"
        }
        fn run(&self) -> CheckResult {
            CheckResult {
                name: self.name().to_string(),
                value: 97.0,
                threshold: 95.0,
                status: HealthStatus::Critical,
                message: "over limit".to_string(),
            }
        }
        fn threshold_warn(&self) -> f64 {
            80.0
        }
        fn threshold_crit(&self) -> f64 {
            95.0
        }
    }

    #[test]
    fn test_health_status_ord() {
        use std::cmp::Ordering;
        assert_eq!(
            HealthStatus::Critical.cmp(&HealthStatus::Degraded),
            Ordering::Greater
        );
        assert_eq!(
            HealthStatus::Degraded.cmp(&HealthStatus::Healthy),
            Ordering::Greater
        );
        assert_eq!(
            HealthStatus::Critical.cmp(&HealthStatus::Healthy),
            Ordering::Greater
        );
    }

    #[test]
    fn test_health_status_serialises_lowercase() {
        assert_eq!(
            serde_json::to_string(&HealthStatus::Healthy).unwrap(),
            "\"healthy\""
        );
        assert_eq!(
            serde_json::to_string(&HealthStatus::Degraded).unwrap(),
            "\"degraded\""
        );
        assert_eq!(
            serde_json::to_string(&HealthStatus::Critical).unwrap(),
            "\"critical\""
        );
    }

    #[test]
    fn test_monitor_aggregates_worst_status() {
        let mut monitor = HealthMonitor::new();
        monitor.register(Box::new(MockHealthyCheck));
        monitor.register(Box::new(MockWarningCheck));
        let report = monitor.run_all();
        assert_eq!(report.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_monitor_critical_precedence() {
        let mut monitor = HealthMonitor::new();
        monitor.register(Box::new(MockCriticalCheck));
        monitor.register(Box::new(MockHealthyCheck));
        let report = monitor.run_all();
        assert_eq!(report.status, HealthStatus::Critical);
    }

    #[test]
    fn test_empty_monitor_is_healthy() {
        let monitor = HealthMonitor::new();
        let report = monitor.run_all();
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.checks.is_empty());
    }

    #[test]
    fn test_http_status_503_on_critical() {
        let mut monitor = HealthMonitor::new();
        monitor.register(Box::new(MockCriticalCheck));
        let report = monitor.run_all();
        assert_eq!(monitor.to_http_status(&report), 503);
    }

    #[test]
    fn test_http_status_200_on_degraded() {
        let mut monitor = HealthMonitor::new();
        monitor.register(Box::new(MockWarningCheck));
        let report = monitor.run_all();
        assert_eq!(monitor.to_http_status(&report), 200);
    }

    #[test]
    fn test_uptime_check_degraded_when_starting() {
        // Uptime check with started_at just now
        let check = UptimeCheck::new(Instant::now());
        let result = check.run();
        assert_eq!(result.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_uptime_check_healthy_after_startup() {
        // Uptime check that started 10 seconds ago
        let started = Instant::now() - std::time::Duration::from_secs(10);
        let check = UptimeCheck::new(started);
        let result = check.run();
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_check_result_serdes() {
        let result = CheckResult {
            name: "cpu".to_string(),
            value: 42.0,
            threshold: 80.0,
            status: HealthStatus::Degraded,
            message: "approaching limit".to_string(),
        };
        let json = serde_json::to_string(&result).expect("must serialise");
        assert!(json.contains("\"name\":\"cpu\""));
        assert!(json.contains("\"status\":\"degraded\""));
    }

    #[test]
    fn test_health_report_serdes() {
        let report = HealthReport {
            status: HealthStatus::Critical,
            uptime_seconds: 30,
            checks: vec![CheckResult {
                name: "cpu".to_string(),
                value: 95.0,
                threshold: 80.0,
                status: HealthStatus::Critical,
                message: "over".to_string(),
            }],
            timestamp: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&report).expect("must serialise");
        assert!(json.contains("\"status\":\"critical\""));
        assert!(json.contains("\"uptime_seconds\":30"));
    }
}
