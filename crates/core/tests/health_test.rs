// Health monitoring tests — TDD Red/Green cycle
//
// These tests define the expected behaviour of the health monitoring system.
// They are written FIRST (RED), then the implementation is added (GREEN).

use mempalace_core::health::{CheckResult, HealthCheck, HealthMonitor, HealthReport, HealthStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Mock check types for testing
// ---------------------------------------------------------------------------

struct MockHealthyCheck;

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

struct MockWarningCheck;

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

struct MockCriticalCheck;

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

// A check whose value can be configured at runtime
struct DynamicCheck {
    value: Arc<AtomicU64>,
    name: String,
    warn_threshold: f64,
    crit_threshold: f64,
    status: HealthStatus,
    message: String,
}

impl DynamicCheck {
    fn new(name: &str, warn: f64, crit: f64, status: HealthStatus, message: &str) -> Self {
        DynamicCheck {
            value: Arc::new(AtomicU64::new(0)),
            name: name.to_string(),
            warn_threshold: warn,
            crit_threshold: crit,
            status,
            message: message.to_string(),
        }
    }
    fn set_value(&self, v: u64) {
        self.value.store(v, Ordering::SeqCst);
    }
}

impl HealthCheck for DynamicCheck {
    fn name(&self) -> &str {
        &self.name
    }
    fn run(&self) -> CheckResult {
        CheckResult {
            name: self.name.clone(),
            value: self.value.load(Ordering::SeqCst) as f64,
            threshold: self.warn_threshold,
            status: self.status.clone(),
            message: self.message.clone(),
        }
    }
    fn threshold_warn(&self) -> f64 {
        self.warn_threshold
    }
    fn threshold_crit(&self) -> f64 {
        self.crit_threshold
    }
}

// ---------------------------------------------------------------------------
// Tests: HealthMonitor status aggregation
// ---------------------------------------------------------------------------

#[test]
fn test_healthy_when_all_checks_pass() {
    let mut monitor = HealthMonitor::new();
    monitor.register(Box::new(MockHealthyCheck));
    let report = monitor.run_all();
    assert_eq!(report.status, HealthStatus::Healthy);
    assert_eq!(monitor.to_http_status(&report), 200);
}

#[test]
fn test_degraded_when_one_check_warns() {
    let mut monitor = HealthMonitor::new();
    monitor.register(Box::new(MockHealthyCheck));
    monitor.register(Box::new(MockWarningCheck));
    let report = monitor.run_all();
    assert_eq!(report.status, HealthStatus::Degraded);
    assert_eq!(monitor.to_http_status(&report), 200);
}

#[test]
fn test_critical_returns_503() {
    let mut monitor = HealthMonitor::new();
    monitor.register(Box::new(MockCriticalCheck));
    let report = monitor.run_all();
    assert_eq!(report.status, HealthStatus::Critical);
    assert_eq!(monitor.to_http_status(&report), 503);
}

#[test]
fn test_critical_takes_precedence_over_degraded() {
    let mut monitor = HealthMonitor::new();
    monitor.register(Box::new(MockWarningCheck));
    monitor.register(Box::new(MockCriticalCheck));
    let report = monitor.run_all();
    assert_eq!(report.status, HealthStatus::Critical);
    assert_eq!(monitor.to_http_status(&report), 503);
}

#[test]
fn test_degraded_takes_precedence_over_healthy() {
    let mut monitor = HealthMonitor::new();
    monitor.register(Box::new(MockHealthyCheck));
    monitor.register(Box::new(MockWarningCheck));
    let report = monitor.run_all();
    assert_eq!(report.status, HealthStatus::Degraded);
}

#[test]
fn test_report_contains_all_check_results() {
    let mut monitor = HealthMonitor::new();
    monitor.register(Box::new(MockHealthyCheck));
    monitor.register(Box::new(MockWarningCheck));
    monitor.register(Box::new(MockCriticalCheck));
    let report = monitor.run_all();
    assert_eq!(report.checks.len(), 3);
    let names: Vec<_> = report.checks.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"mock_healthy"));
    assert!(names.contains(&"mock_warning"));
    assert!(names.contains(&"mock_critical"));
}

#[test]
fn test_report_contains_uptime_seconds() {
    let mut monitor = HealthMonitor::new();
    monitor.register(Box::new(MockHealthyCheck));
    std::thread::sleep(Duration::from_millis(100));
    let report = monitor.run_all();
    assert!(report.uptime_seconds < u64::MAX);
}

#[test]
fn test_report_contains_timestamp() {
    let mut monitor = HealthMonitor::new();
    monitor.register(Box::new(MockHealthyCheck));
    let report = monitor.run_all();
    let ts = &report.timestamp;
    assert!(
        ts.timestamp() > 0,
        "timestamp should be a valid unix timestamp"
    );
}

#[test]
fn test_empty_monitor_returns_healthy() {
    let monitor = HealthMonitor::new();
    let report = monitor.run_all();
    assert_eq!(report.status, HealthStatus::Healthy);
    assert_eq!(report.checks.len(), 0);
}

// ---------------------------------------------------------------------------
// Tests: CheckResult fields are populated correctly
// ---------------------------------------------------------------------------

#[test]
fn test_check_result_has_all_fields() {
    let result = CheckResult {
        name: "cpu".to_string(),
        value: 75.0,
        threshold: 80.0,
        status: HealthStatus::Healthy,
        message: "CPU usage is normal".to_string(),
    };
    assert_eq!(result.name, "cpu");
    assert_eq!(result.value, 75.0);
    assert_eq!(result.threshold, 80.0);
    assert_eq!(result.status, HealthStatus::Healthy);
    assert_eq!(result.message, "CPU usage is normal");
}

// ---------------------------------------------------------------------------
// Tests: HealthStatus serialises correctly (lowercase)
// ---------------------------------------------------------------------------

#[test]
fn test_health_status_serialisation() {
    let healthy = serde_json::to_string(&HealthStatus::Healthy).unwrap();
    let degraded = serde_json::to_string(&HealthStatus::Degraded).unwrap();
    let critical = serde_json::to_string(&HealthStatus::Critical).unwrap();
    assert_eq!(healthy, "\"healthy\"");
    assert_eq!(degraded, "\"degraded\"");
    assert_eq!(critical, "\"critical\"");
}

#[test]
fn test_health_report_serialisation() {
    let report = HealthReport {
        status: HealthStatus::Healthy,
        uptime_seconds: 42,
        checks: vec![CheckResult {
            name: "cpu".to_string(),
            value: 50.0,
            threshold: 80.0,
            status: HealthStatus::Healthy,
            message: "ok".to_string(),
        }],
        timestamp: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&report).expect("HealthReport must serialise to JSON");
    assert!(json.contains("\"status\":\"healthy\""));
    assert!(json.contains("\"uptime_seconds\":42"));
    assert!(json.contains("\"name\":\"cpu\""));
}

// ---------------------------------------------------------------------------
// Tests: HealthStatus ordering (Critical > Degraded > Healthy)
// ---------------------------------------------------------------------------

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
