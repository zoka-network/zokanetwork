// Health checks module: Liveness probes, readiness checks, and sync status
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Health check status
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded => write!(f, "degraded"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

/// Individual health check result
#[derive(Clone, Debug)]
pub struct HealthCheckResult {
    pub name: String,
    pub status: HealthStatus,
    pub message: String,
    pub details: String,
    pub timestamp: u64,
    pub response_time_ms: u64,
}

impl HealthCheckResult {
    /// Create a passing health check
    pub fn pass(name: &str, message: &str) -> Self {
        HealthCheckResult {
            name: name.to_string(),
            status: HealthStatus::Healthy,
            message: message.to_string(),
            details: String::new(),
            timestamp: Self::current_timestamp(),
            response_time_ms: 0,
        }
    }

    /// Create a degraded health check
    pub fn degraded(name: &str, message: &str, details: &str) -> Self {
        HealthCheckResult {
            name: name.to_string(),
            status: HealthStatus::Degraded,
            message: message.to_string(),
            details: details.to_string(),
            timestamp: Self::current_timestamp(),
            response_time_ms: 0,
        }
    }

    /// Create a failing health check
    pub fn fail(name: &str, message: &str, details: &str) -> Self {
        HealthCheckResult {
            name: name.to_string(),
            status: HealthStatus::Unhealthy,
            message: message.to_string(),
            details: details.to_string(),
            timestamp: Self::current_timestamp(),
            response_time_ms: 0,
        }
    }

    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

/// Health monitoring system with liveness and readiness checks
#[allow(dead_code)]
pub struct HealthChecker {
    service_name: String,
    checks: Arc<Mutex<Vec<HealthCheckResult>>>,
    peer_count: Arc<Mutex<usize>>,
    block_height: Arc<Mutex<u64>>,
    is_syncing: Arc<Mutex<bool>>,
    sync_progress: Arc<Mutex<f64>>,
    last_block_time: Arc<Mutex<u64>>,
    mempool_size: Arc<Mutex<usize>>,
    memory_used_mb: Arc<Mutex<u64>>,
    cpu_usage_percent: Arc<Mutex<f64>>,
}

impl HealthChecker {
    /// Create new health checker
    pub fn new(service_name: &str) -> Self {
        HealthChecker {
            service_name: service_name.to_string(),
            checks: Arc::new(Mutex::new(Vec::new())),
            peer_count: Arc::new(Mutex::new(0)),
            block_height: Arc::new(Mutex::new(0)),
            is_syncing: Arc::new(Mutex::new(false)),
            sync_progress: Arc::new(Mutex::new(0.0)),
            last_block_time: Arc::new(Mutex::new(0)),
            mempool_size: Arc::new(Mutex::new(0)),
            memory_used_mb: Arc::new(Mutex::new(0)),
            cpu_usage_percent: Arc::new(Mutex::new(0.0)),
        }
    }

    /// Perform a liveness check (is the service responding?)
    pub fn liveness_check(&self) -> HealthCheckResult {
        HealthCheckResult::pass("liveness", "Service is responding")
    }

    /// Perform a readiness check (can it serve requests?)
    pub fn readiness_check(&self) -> HealthCheckResult {
        if let Ok(peer_count) = self.peer_count.lock() {
            if *peer_count > 0 {
                return HealthCheckResult::pass(
                    "readiness",
                    &format!("Service is ready with {} connected peers", peer_count),
                );
            }
        }

        HealthCheckResult::degraded(
            "readiness",
            "Service is ready but has no connected peers",
            "P2P network connectivity may be compromised",
        )
    }

    /// Check blockchain sync status
    pub fn sync_status_check(&self) -> HealthCheckResult {
        if let Ok(is_syncing) = self.is_syncing.lock() {
            if *is_syncing {
                if let Ok(progress) = self.sync_progress.lock() {
                    if *progress >= 95.0 {
                        return HealthCheckResult::pass(
                            "sync_status",
                            &format!("Blockchain syncing: {:.1}% complete", progress),
                        );
                    } else {
                        return HealthCheckResult::degraded(
                            "sync_status",
                            "Blockchain is syncing",
                            &format!("Progress: {:.1}%", progress),
                        );
                    }
                }
            } else {
                return HealthCheckResult::pass("sync_status", "Blockchain is synchronized");
            }
        }

        HealthCheckResult::fail(
            "sync_status",
            "Unable to determine sync status",
            "Internal state locked",
        )
    }

    /// Check peer connectivity
    pub fn peer_connectivity_check(&self) -> HealthCheckResult {
        if let Ok(peer_count) = self.peer_count.lock() {
            if *peer_count >= 3 {
                return HealthCheckResult::pass(
                    "peer_connectivity",
                    &format!("{} peers connected", peer_count),
                );
            } else if *peer_count > 0 {
                return HealthCheckResult::degraded(
                    "peer_connectivity",
                    &format!("Only {} peer connected", peer_count),
                    "Recommended minimum is 3+ peers for redundancy",
                );
            }
        }

        HealthCheckResult::fail(
            "peer_connectivity",
            "No peers connected",
            "P2P network is unreachable",
        )
    }

    /// Check mempool health
    pub fn mempool_check(&self) -> HealthCheckResult {
        if let Ok(mempool_size) = self.mempool_size.lock() {
            if *mempool_size < 10000 {
                return HealthCheckResult::pass(
                    "mempool",
                    &format!("Mempool healthy with {} transactions", mempool_size),
                );
            } else if *mempool_size < 50000 {
                return HealthCheckResult::degraded(
                    "mempool",
                    &format!("Mempool size: {} transactions", mempool_size),
                    "Memory usage is elevated",
                );
            }
        }

        HealthCheckResult::fail(
            "mempool",
            "Mempool is full or exceeds capacity",
            "Unable to accept new transactions",
        )
    }

    /// Check resource usage (memory and CPU)
    pub fn resource_check(&self) -> HealthCheckResult {
        let mut details = String::new();

        if let Ok(mem) = self.memory_used_mb.lock() {
            details.push_str(&format!("Memory: {} MB", mem));
        }

        if let Ok(cpu) = self.cpu_usage_percent.lock() {
            if !details.is_empty() {
                details.push_str(", ");
            }
            details.push_str(&format!("CPU: {:.1}%", cpu));
        }

        if let Ok(mem) = self.memory_used_mb.lock() {
            if *mem > 4000 {
                return HealthCheckResult::degraded(
                    "resources",
                    "Resource usage is high",
                    &details,
                );
            }
        }

        if let Ok(cpu) = self.cpu_usage_percent.lock() {
            if *cpu > 80.0 {
                return HealthCheckResult::degraded("resources", "CPU usage is high", &details);
            }
        }

        HealthCheckResult::pass(
            "resources",
            &format!("Resource usage is normal: {}", details),
        )
    }

    /// Check block production
    pub fn block_production_check(&self) -> HealthCheckResult {
        let now = Self::_current_timestamp();

        if let Ok(last_block_time) = self.last_block_time.lock() {
            let time_since_block = now - *last_block_time;

            if time_since_block < 60 {
                return HealthCheckResult::pass(
                    "block_production",
                    &format!("Block produced {} seconds ago", time_since_block),
                );
            } else if time_since_block < 300 {
                return HealthCheckResult::degraded(
                    "block_production",
                    "Block production is slow",
                    &format!("Last block {} seconds ago", time_since_block),
                );
            }
        }

        HealthCheckResult::fail(
            "block_production",
            "No blocks produced recently",
            "Consensus may be stalled",
        )
    }

    /// Run all health checks
    pub fn run_all_checks(&self) -> Vec<HealthCheckResult> {
        vec![
            self.liveness_check(),
            self.readiness_check(),
            self.sync_status_check(),
            self.peer_connectivity_check(),
            self.mempool_check(),
            self.resource_check(),
            self.block_production_check(),
        ]
    }

    /// Get overall health status
    pub fn overall_status(&self) -> HealthStatus {
        let checks = self.run_all_checks();

        if checks.iter().any(|c| c.status == HealthStatus::Unhealthy) {
            HealthStatus::Unhealthy
        } else if checks.iter().any(|c| c.status == HealthStatus::Degraded) {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }

    /// Update peer count
    pub fn set_peer_count(&self, count: usize) {
        if let Ok(mut peers) = self.peer_count.lock() {
            *peers = count;
        }
    }

    /// Get current peer count
    pub fn get_peer_count(&self) -> usize {
        if let Ok(peers) = self.peer_count.lock() {
            *peers
        } else {
            0
        }
    }

    /// Update block height
    pub fn set_block_height(&self, height: u64) {
        if let Ok(mut h) = self.block_height.lock() {
            *h = height;
        }
    }

    /// Get current block height
    pub fn get_block_height(&self) -> u64 {
        if let Ok(h) = self.block_height.lock() {
            *h
        } else {
            0
        }
    }

    /// Set syncing status
    pub fn set_syncing(&self, syncing: bool) {
        if let Ok(mut s) = self.is_syncing.lock() {
            *s = syncing;
        }
    }

    /// Set sync progress (0.0 - 100.0)
    pub fn set_sync_progress(&self, progress: f64) {
        if let Ok(mut p) = self.sync_progress.lock() {
            *p = progress.min(100.0).max(0.0);
        }
    }

    /// Update last block time
    pub fn set_last_block_time(&self, timestamp: u64) {
        if let Ok(mut t) = self.last_block_time.lock() {
            *t = timestamp;
        }
    }

    /// Update mempool size
    pub fn set_mempool_size(&self, size: usize) {
        if let Ok(mut m) = self.mempool_size.lock() {
            *m = size;
        }
    }

    /// Update memory usage
    pub fn set_memory_used_mb(&self, mb: u64) {
        if let Ok(mut m) = self.memory_used_mb.lock() {
            *m = mb;
        }
    }

    /// Update CPU usage
    pub fn set_cpu_usage_percent(&self, percent: f64) {
        if let Ok(mut c) = self.cpu_usage_percent.lock() {
            *c = percent.min(100.0).max(0.0);
        }
    }

    /// Export health status as JSON-like format
    pub fn export_status(&self) -> String {
        let checks = self.run_all_checks();
        let overall = self.overall_status();

        let mut output = format!("{{\n  \"service\": \"{}\",\n", self.service_name);
        output.push_str(&format!("  \"status\": \"{}\",\n", overall));
        output.push_str("  \"checks\": [\n");

        for (idx, check) in checks.iter().enumerate() {
            output.push_str(&format!(
                "    {{\n      \"name\": \"{}\",\n      \"status\": \"{}\",\n      \"message\": \"{}\",\n      \"details\": \"{}\"\n    }}{}\n",
                check.name, check.status, check.message, check.details,
                if idx < checks.len() - 1 { "," } else { "" }
            ));
        }

        output.push_str("  ]\n");
        output.push('}');

        output
    }

    /// Add custom health check result
    pub fn add_check(&self, result: HealthCheckResult) {
        if let Ok(mut checks) = self.checks.lock() {
            checks.push(result);
        }
    }

    /// Get all recorded checks
    pub fn get_checks(&self) -> Vec<HealthCheckResult> {
        if let Ok(checks) = self.checks.lock() {
            checks.clone()
        } else {
            Vec::new()
        }
    }

    /// Clear all recorded checks
    pub fn clear_checks(&self) {
        if let Ok(mut checks) = self.checks.lock() {
            checks.clear();
        }
    }

    fn _current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new("zoka")
    }
}

impl Clone for HealthChecker {
    fn clone(&self) -> Self {
        HealthChecker {
            service_name: self.service_name.clone(),
            checks: Arc::clone(&self.checks),
            peer_count: Arc::clone(&self.peer_count),
            block_height: Arc::clone(&self.block_height),
            is_syncing: Arc::clone(&self.is_syncing),
            sync_progress: Arc::clone(&self.sync_progress),
            last_block_time: Arc::clone(&self.last_block_time),
            mempool_size: Arc::clone(&self.mempool_size),
            memory_used_mb: Arc::clone(&self.memory_used_mb),
            cpu_usage_percent: Arc::clone(&self.cpu_usage_percent),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_checker_creation() {
        let checker = HealthChecker::new("test_service");
        assert_eq!(checker.get_peer_count(), 0);
        assert_eq!(checker.get_block_height(), 0);
    }

    #[test]
    fn test_liveness_check() {
        let checker = HealthChecker::new("test");
        let result = checker.liveness_check();
        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(result.name, "liveness");
    }

    #[test]
    fn test_readiness_check_no_peers() {
        let checker = HealthChecker::new("test");
        let result = checker.readiness_check();
        assert_eq!(result.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_readiness_check_with_peers() {
        let checker = HealthChecker::new("test");
        checker.set_peer_count(5);
        let result = checker.readiness_check();
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_sync_status_check_syncing() {
        let checker = HealthChecker::new("test");
        checker.set_syncing(true);
        checker.set_sync_progress(50.0);
        let result = checker.sync_status_check();
        assert_eq!(result.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_sync_status_check_nearly_synced() {
        let checker = HealthChecker::new("test");
        checker.set_syncing(true);
        checker.set_sync_progress(99.0);
        let result = checker.sync_status_check();
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_sync_status_check_synced() {
        let checker = HealthChecker::new("test");
        checker.set_syncing(false);
        let result = checker.sync_status_check();
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_peer_connectivity_check_healthy() {
        let checker = HealthChecker::new("test");
        checker.set_peer_count(5);
        let result = checker.peer_connectivity_check();
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_peer_connectivity_check_degraded() {
        let checker = HealthChecker::new("test");
        checker.set_peer_count(1);
        let result = checker.peer_connectivity_check();
        assert_eq!(result.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_peer_connectivity_check_unhealthy() {
        let checker = HealthChecker::new("test");
        let result = checker.peer_connectivity_check();
        assert_eq!(result.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_mempool_check_healthy() {
        let checker = HealthChecker::new("test");
        checker.set_mempool_size(1000);
        let result = checker.mempool_check();
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_mempool_check_degraded() {
        let checker = HealthChecker::new("test");
        checker.set_mempool_size(20000);
        let result = checker.mempool_check();
        assert_eq!(result.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_mempool_check_unhealthy() {
        let checker = HealthChecker::new("test");
        checker.set_mempool_size(60000);
        let result = checker.mempool_check();
        assert_eq!(result.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_resource_check_healthy() {
        let checker = HealthChecker::new("test");
        checker.set_memory_used_mb(500);
        checker.set_cpu_usage_percent(20.0);
        let result = checker.resource_check();
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_resource_check_high_memory() {
        let checker = HealthChecker::new("test");
        checker.set_memory_used_mb(5000);
        let result = checker.resource_check();
        assert_eq!(result.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_resource_check_high_cpu() {
        let checker = HealthChecker::new("test");
        checker.set_cpu_usage_percent(85.0);
        let result = checker.resource_check();
        assert_eq!(result.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_block_production_check_recent() {
        let checker = HealthChecker::new("test");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        checker.set_last_block_time(now - 30);
        let result = checker.block_production_check();
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_block_production_check_slow() {
        let checker = HealthChecker::new("test");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        checker.set_last_block_time(now - 150);
        let result = checker.block_production_check();
        assert_eq!(result.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_block_production_check_stalled() {
        let checker = HealthChecker::new("test");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        checker.set_last_block_time(now - 600);
        let result = checker.block_production_check();
        assert_eq!(result.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_run_all_checks() {
        let checker = HealthChecker::new("test");
        checker.set_peer_count(5);
        let results = checker.run_all_checks();
        assert_eq!(results.len(), 7);
    }

    #[test]
    fn test_overall_status_healthy() {
        let checker = HealthChecker::new("test");
        checker.set_peer_count(5);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        checker.set_last_block_time(now - 30);
        let status = checker.overall_status();
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[test]
    fn test_overall_status_degraded() {
        let checker = HealthChecker::new("test");
        checker.set_peer_count(1);

        // Set recent block time to avoid unhealthy block_production
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        checker.set_last_block_time(now - 30);

        let status = checker.overall_status();
        assert_eq!(status, HealthStatus::Degraded);
    }

    #[test]
    fn test_overall_status_unhealthy() {
        let checker = HealthChecker::new("test");
        let status = checker.overall_status();
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_set_and_get_block_height() {
        let checker = HealthChecker::new("test");
        checker.set_block_height(100);
        assert_eq!(checker.get_block_height(), 100);
    }

    #[test]
    fn test_set_and_get_peer_count() {
        let checker = HealthChecker::new("test");
        checker.set_peer_count(5);
        assert_eq!(checker.get_peer_count(), 5);
    }

    #[test]
    fn test_set_syncing() {
        let checker = HealthChecker::new("test");

        // Initially syncing is false, so sync_status is Healthy
        let checks = checker.run_all_checks();
        assert_eq!(checks[2].status, HealthStatus::Healthy);

        // When we start syncing with low progress, it becomes Degraded
        checker.set_syncing(true);
        checker.set_sync_progress(50.0);
        let checks = checker.run_all_checks();
        assert_eq!(checks[2].status, HealthStatus::Degraded);

        // When progress is high, it becomes Healthy again
        checker.set_sync_progress(99.0);
        let checks = checker.run_all_checks();
        assert_eq!(checks[2].status, HealthStatus::Healthy);
    }

    #[test]
    fn test_set_sync_progress_clamped() {
        let checker = HealthChecker::new("test");
        checker.set_sync_progress(150.0); // Should be clamped to 100
        checker.set_syncing(true);
        let check = checker.sync_status_check();
        assert!(check.message.contains("100.0"));
    }

    #[test]
    fn test_export_status() {
        let checker = HealthChecker::new("test_service");
        checker.set_peer_count(5);
        let exported = checker.export_status();
        assert!(exported.contains("test_service"));
        assert!(exported.contains("healthy"));
        assert!(exported.contains("checks"));
    }

    #[test]
    fn test_add_and_get_checks() {
        let checker = HealthChecker::new("test");
        let custom_check = HealthCheckResult::pass("custom", "Custom check passed");
        checker.add_check(custom_check);
        let checks = checker.get_checks();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].name, "custom");
    }

    #[test]
    fn test_clear_checks() {
        let checker = HealthChecker::new("test");
        checker.add_check(HealthCheckResult::pass("test1", "Message1"));
        checker.add_check(HealthCheckResult::pass("test2", "Message2"));
        assert_eq!(checker.get_checks().len(), 2);

        checker.clear_checks();
        assert_eq!(checker.get_checks().len(), 0);
    }

    #[test]
    fn test_health_check_result_pass() {
        let result = HealthCheckResult::pass("test", "All good");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(result.name, "test");
    }

    #[test]
    fn test_health_check_result_degraded() {
        let result = HealthCheckResult::degraded("test", "Warning", "Details");
        assert_eq!(result.status, HealthStatus::Degraded);
        assert_eq!(result.details, "Details");
    }

    #[test]
    fn test_health_check_result_fail() {
        let result = HealthCheckResult::fail("test", "Error", "Details");
        assert_eq!(result.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_checker_clone() {
        let checker = HealthChecker::new("test");
        checker.set_peer_count(5);
        checker.set_block_height(100);

        let cloned = checker.clone();
        assert_eq!(cloned.get_peer_count(), 5);
        assert_eq!(cloned.get_block_height(), 100);

        cloned.set_peer_count(10);
        assert_eq!(checker.get_peer_count(), 10); // Shared state
    }

    #[test]
    fn test_health_checker_default() {
        let checker = HealthChecker::default();
        assert_eq!(checker.get_peer_count(), 0);
    }
}
