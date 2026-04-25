// Metrics module: Logging and performance metrics collection
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Metrics collector for Prometheus-compatible metrics
#[allow(dead_code)]
pub struct MetricsCollector {
    counters: Arc<Mutex<HashMap<String, u64>>>,
    gauges: Arc<Mutex<HashMap<String, f64>>>,
    histograms: Arc<Mutex<HashMap<String, Vec<f64>>>>,
    service_name: String,
}

impl MetricsCollector {
    /// Create new metrics collector
    pub fn new(service_name: &str) -> Self {
        MetricsCollector {
            counters: Arc::new(Mutex::new(HashMap::new())),
            gauges: Arc::new(Mutex::new(HashMap::new())),
            histograms: Arc::new(Mutex::new(HashMap::new())),
            service_name: service_name.to_string(),
        }
    }

    /// Increment counter
    pub fn increment_counter(&self, name: &str, value: u64) {
        if let Ok(mut counters) = self.counters.lock() {
            *counters.entry(name.to_string()).or_insert(0) += value;
        }
    }

    /// Get counter value
    pub fn get_counter(&self, name: &str) -> Option<u64> {
        if let Ok(counters) = self.counters.lock() {
            counters.get(name).copied()
        } else {
            None
        }
    }

    /// Set gauge value
    pub fn set_gauge(&self, name: &str, value: f64) {
        if let Ok(mut gauges) = self.gauges.lock() {
            gauges.insert(name.to_string(), value);
        }
    }

    /// Get gauge value
    pub fn get_gauge(&self, name: &str) -> Option<f64> {
        if let Ok(gauges) = self.gauges.lock() {
            gauges.get(name).copied()
        } else {
            None
        }
    }

    /// Record histogram value
    pub fn record_histogram(&self, name: &str, value: f64) {
        if let Ok(mut histograms) = self.histograms.lock() {
            histograms
                .entry(name.to_string())
                .or_insert_with(Vec::new)
                .push(value);
        }
    }

    /// Get histogram percentile
    pub fn get_histogram_percentile(&self, name: &str, percentile: f64) -> Option<f64> {
        if let Ok(histograms) = self.histograms.lock() {
            if let Some(values) = histograms.get(name) {
                if values.is_empty() {
                    return None;
                }

                let mut sorted = values.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

                let index = ((percentile / 100.0) * (sorted.len() - 1) as f64).round() as usize;
                let adjusted_index = index.min(sorted.len() - 1);
                return Some(sorted[adjusted_index]);
            }
        }
        None
    }

    /// Get all counters as formatted string (Prometheus format)
    pub fn export_counters(&self) -> String {
        let mut output = String::new();
        if let Ok(counters) = self.counters.lock() {
            for (name, value) in counters.iter() {
                output.push_str(&format!("# HELP {} Counter metric\n", name));
                output.push_str(&format!("# TYPE {} counter\n", name));
                output.push_str(&format!(
                    "{}{{service=\"{}\"}} {}\n",
                    name, self.service_name, value
                ));
            }
        }
        output
    }

    /// Get all gauges as formatted string (Prometheus format)
    pub fn export_gauges(&self) -> String {
        let mut output = String::new();
        if let Ok(gauges) = self.gauges.lock() {
            for (name, value) in gauges.iter() {
                output.push_str(&format!("# HELP {} Gauge metric\n", name));
                output.push_str(&format!("# TYPE {} gauge\n", name));
                output.push_str(&format!(
                    "{}{{service=\"{}\"}} {}\n",
                    name, self.service_name, value
                ));
            }
        }
        output
    }

    /// Get all metrics (counters + gauges)
    pub fn export_all(&self) -> String {
        let mut output = self.export_counters();
        output.push_str(&self.export_gauges());
        output
    }

    /// Clear all metrics
    pub fn clear(&self) {
        if let Ok(mut counters) = self.counters.lock() {
            counters.clear();
        }
        if let Ok(mut gauges) = self.gauges.lock() {
            gauges.clear();
        }
        if let Ok(mut histograms) = self.histograms.lock() {
            histograms.clear();
        }
    }

    /// Get total counters count
    pub fn counters_count(&self) -> usize {
        if let Ok(counters) = self.counters.lock() {
            counters.len()
        } else {
            0
        }
    }

    /// Get total gauges count
    pub fn gauges_count(&self) -> usize {
        if let Ok(gauges) = self.gauges.lock() {
            gauges.len()
        } else {
            0
        }
    }

    /// Get total histograms count
    pub fn histograms_count(&self) -> usize {
        if let Ok(histograms) = self.histograms.lock() {
            histograms.len()
        } else {
            0
        }
    }

    /// Get histogram average
    pub fn get_histogram_average(&self, name: &str) -> Option<f64> {
        if let Ok(histograms) = self.histograms.lock() {
            if let Some(values) = histograms.get(name) {
                if values.is_empty() {
                    return None;
                }
                let sum: f64 = values.iter().sum();
                return Some(sum / values.len() as f64);
            }
        }
        None
    }

    /// Get histogram min
    pub fn get_histogram_min(&self, name: &str) -> Option<f64> {
        if let Ok(histograms) = self.histograms.lock() {
            if let Some(values) = histograms.get(name) {
                if values.is_empty() {
                    return None;
                }
                return values
                    .iter()
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .copied();
            }
        }
        None
    }

    /// Get histogram max
    pub fn get_histogram_max(&self, name: &str) -> Option<f64> {
        if let Ok(histograms) = self.histograms.lock() {
            if let Some(values) = histograms.get(name) {
                if values.is_empty() {
                    return None;
                }
                return values
                    .iter()
                    .max_by(|a, b| a.partial_cmp(b).unwrap())
                    .copied();
            }
        }
        None
    }

    /// Get histogram count
    pub fn get_histogram_count(&self, name: &str) -> Option<usize> {
        if let Ok(histograms) = self.histograms.lock() {
            histograms.get(name).map(|values| values.len())
        } else {
            None
        }
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new("zoka")
    }
}

impl Clone for MetricsCollector {
    fn clone(&self) -> Self {
        MetricsCollector {
            counters: Arc::clone(&self.counters),
            gauges: Arc::clone(&self.gauges),
            histograms: Arc::clone(&self.histograms),
            service_name: self.service_name.clone(),
        }
    }
}

/// Structured logging message
#[derive(Clone, Debug)]
pub struct LogMessage {
    pub level: LogLevel,
    pub timestamp: u64,
    pub service: String,
    pub message: String,
    pub context: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "TRACE"),
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

/// Logger for structured logging
#[allow(dead_code)]
pub struct Logger {
    service_name: String,
    logs: Arc<Mutex<Vec<LogMessage>>>,
    min_level: LogLevel,
}

impl Logger {
    /// Create new logger
    pub fn new(service_name: &str) -> Self {
        Logger {
            service_name: service_name.to_string(),
            logs: Arc::new(Mutex::new(Vec::new())),
            min_level: LogLevel::Info,
        }
    }

    /// Set minimum log level
    pub fn set_min_level(&mut self, level: LogLevel) {
        self.min_level = level;
    }

    /// Log a message
    pub fn log(&self, level: LogLevel, message: &str) {
        if Self::_level_to_priority(&level) >= Self::_level_to_priority(&self.min_level) {
            let log_msg = LogMessage {
                level: level.clone(),
                timestamp: Self::_current_timestamp(),
                service: self.service_name.clone(),
                message: message.to_string(),
                context: HashMap::new(),
            };

            if let Ok(mut logs) = self.logs.lock() {
                logs.push(log_msg);
            }
        }
    }

    /// Log with context
    pub fn log_with_context(
        &self,
        level: LogLevel,
        message: &str,
        context: HashMap<String, String>,
    ) {
        if Self::_level_to_priority(&level) >= Self::_level_to_priority(&self.min_level) {
            let log_msg = LogMessage {
                level: level.clone(),
                timestamp: Self::_current_timestamp(),
                service: self.service_name.clone(),
                message: message.to_string(),
                context,
            };

            if let Ok(mut logs) = self.logs.lock() {
                logs.push(log_msg);
            }
        }
    }

    /// Log error
    pub fn error(&self, message: &str) {
        self.log(LogLevel::Error, message);
    }

    /// Log warning
    pub fn warn(&self, message: &str) {
        self.log(LogLevel::Warn, message);
    }

    /// Log info
    pub fn info(&self, message: &str) {
        self.log(LogLevel::Info, message);
    }

    /// Log debug
    pub fn debug(&self, message: &str) {
        self.log(LogLevel::Debug, message);
    }

    /// Log trace
    pub fn trace(&self, message: &str) {
        self.log(LogLevel::Trace, message);
    }

    /// Get all logs
    pub fn get_logs(&self) -> Vec<LogMessage> {
        if let Ok(logs) = self.logs.lock() {
            logs.clone()
        } else {
            Vec::new()
        }
    }

    /// Get logs by level
    pub fn get_logs_by_level(&self, level: LogLevel) -> Vec<LogMessage> {
        if let Ok(logs) = self.logs.lock() {
            logs.iter()
                .filter(|log| log.level == level)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Export logs as JSON-like format
    pub fn export_logs(&self) -> String {
        let mut output = String::from("[\n");
        if let Ok(logs) = self.logs.lock() {
            for (idx, log) in logs.iter().enumerate() {
                output.push_str(&format!(
                    "  {{{0}\"level\": \"{1}\",\"timestamp\": {2},\"service\": \"{3}\",\"message\": \"{4}\"{5}}}\n",
                    "{", log.level, log.timestamp, log.service, log.message,
                    if idx < logs.len() - 1 { "," } else { "" }
                ));
            }
        }
        output.push(']');
        output
    }

    /// Clear all logs
    pub fn clear(&self) {
        if let Ok(mut logs) = self.logs.lock() {
            logs.clear();
        }
    }

    /// Get log count
    pub fn log_count(&self) -> usize {
        if let Ok(logs) = self.logs.lock() {
            logs.len()
        } else {
            0
        }
    }

    // Helper: Convert log level to priority (higher number = higher priority)
    fn _level_to_priority(level: &LogLevel) -> u8 {
        match level {
            LogLevel::Trace => 0,
            LogLevel::Debug => 1,
            LogLevel::Info => 2,
            LogLevel::Warn => 3,
            LogLevel::Error => 4,
        }
    }

    // Helper: Current Unix timestamp in seconds
    fn _current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::new("zoka")
    }
}

impl Clone for Logger {
    fn clone(&self) -> Self {
        Logger {
            service_name: self.service_name.clone(),
            logs: Arc::clone(&self.logs),
            min_level: self.min_level.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_collector_creation() {
        let collector = MetricsCollector::new("test_service");
        assert_eq!(collector.counters_count(), 0);
        assert_eq!(collector.gauges_count(), 0);
    }

    #[test]
    fn test_increment_counter() {
        let collector = MetricsCollector::new("test_service");
        collector.increment_counter("test_counter", 1);
        assert_eq!(collector.get_counter("test_counter"), Some(1));

        collector.increment_counter("test_counter", 5);
        assert_eq!(collector.get_counter("test_counter"), Some(6));
    }

    #[test]
    fn test_set_and_get_gauge() {
        let collector = MetricsCollector::new("test_service");
        collector.set_gauge("test_gauge", 42.5);
        assert_eq!(collector.get_gauge("test_gauge"), Some(42.5));

        collector.set_gauge("test_gauge", 10.0);
        assert_eq!(collector.get_gauge("test_gauge"), Some(10.0));
    }

    #[test]
    fn test_histogram_recording() {
        let collector = MetricsCollector::new("test_service");
        collector.record_histogram("latency", 100.0);
        collector.record_histogram("latency", 200.0);
        collector.record_histogram("latency", 150.0);

        assert_eq!(collector.get_histogram_count("latency"), Some(3));
        assert_eq!(collector.get_histogram_min("latency"), Some(100.0));
        assert_eq!(collector.get_histogram_max("latency"), Some(200.0));
    }

    #[test]
    fn test_histogram_average() {
        let collector = MetricsCollector::new("test_service");
        collector.record_histogram("latency", 100.0);
        collector.record_histogram("latency", 200.0);
        collector.record_histogram("latency", 300.0);

        let avg = collector.get_histogram_average("latency").unwrap();
        assert!((avg - 200.0).abs() < 0.01);
    }

    #[test]
    fn test_histogram_percentile() {
        let collector = MetricsCollector::new("test_service");
        for i in 1..=100 {
            collector.record_histogram("values", i as f64);
        }

        let p50 = collector.get_histogram_percentile("values", 50.0);
        assert!(p50.is_some());
        assert!(p50.unwrap() > 40.0 && p50.unwrap() < 60.0);

        let p99 = collector.get_histogram_percentile("values", 99.0);
        assert!(p99.is_some());
        assert!(p99.unwrap() > 90.0);
    }

    #[test]
    fn test_export_counters_prometheus_format() {
        let collector = MetricsCollector::new("test_service");
        collector.increment_counter("requests", 100);

        let exported = collector.export_counters();
        assert!(exported.contains("requests"));
        assert!(exported.contains("test_service"));
        assert!(exported.contains("100"));
    }

    #[test]
    fn test_export_gauges_prometheus_format() {
        let collector = MetricsCollector::new("test_service");
        collector.set_gauge("memory", 512.5);

        let exported = collector.export_gauges();
        assert!(exported.contains("memory"));
        assert!(exported.contains("512.5"));
    }

    #[test]
    fn test_clear_metrics() {
        let collector = MetricsCollector::new("test_service");
        collector.increment_counter("counter", 5);
        collector.set_gauge("gauge", 10.0);

        assert_eq!(collector.counters_count(), 1);
        assert_eq!(collector.gauges_count(), 1);

        collector.clear();
        assert_eq!(collector.counters_count(), 0);
        assert_eq!(collector.gauges_count(), 0);
    }

    #[test]
    fn test_logger_creation() {
        let logger = Logger::new("test_service");
        assert_eq!(logger.log_count(), 0);
    }

    #[test]
    fn test_logger_info() {
        let logger = Logger::new("test_service");
        logger.info("test message");
        assert_eq!(logger.log_count(), 1);

        let logs = logger.get_logs();
        assert_eq!(logs[0].level, LogLevel::Info);
        assert_eq!(logs[0].message, "test message");
    }

    #[test]
    fn test_logger_error() {
        let logger = Logger::new("test_service");
        logger.error("error message");
        assert_eq!(logger.log_count(), 1);

        let logs = logger.get_logs();
        assert_eq!(logs[0].level, LogLevel::Error);
    }

    #[test]
    fn test_logger_debug() {
        let mut logger = Logger::new("test_service");
        logger.set_min_level(LogLevel::Debug);
        logger.debug("debug message");
        assert_eq!(logger.log_count(), 1);

        let logs = logger.get_logs();
        assert_eq!(logs[0].level, LogLevel::Debug);
    }

    #[test]
    fn test_logger_warn() {
        let logger = Logger::new("test_service");
        logger.warn("warning message");
        assert_eq!(logger.log_count(), 1);

        let logs = logger.get_logs();
        assert_eq!(logs[0].level, LogLevel::Warn);
    }

    #[test]
    fn test_logger_trace() {
        let mut logger = Logger::new("test_service");
        logger.set_min_level(LogLevel::Trace);
        logger.trace("trace message");
        assert_eq!(logger.log_count(), 1);

        let logs = logger.get_logs();
        assert_eq!(logs[0].level, LogLevel::Trace);
    }

    #[test]
    fn test_logger_min_level_filter() {
        let mut logger = Logger::new("test_service");
        logger.set_min_level(LogLevel::Warn);

        logger.debug("debug");
        logger.info("info");
        logger.warn("warning");
        logger.error("error");

        assert_eq!(logger.log_count(), 2); // Only warn and error
        let logs = logger.get_logs();
        assert!(logs
            .iter()
            .all(|log| log.level == LogLevel::Warn || log.level == LogLevel::Error));
    }

    #[test]
    fn test_logger_get_logs_by_level() {
        let logger = Logger::new("test_service");
        logger.info("info1");
        logger.error("error1");
        logger.info("info2");
        logger.error("error2");

        let info_logs = logger.get_logs_by_level(LogLevel::Info);
        assert_eq!(info_logs.len(), 2);
        assert!(info_logs.iter().all(|log| log.level == LogLevel::Info));
    }

    #[test]
    fn test_logger_log_with_context() {
        let logger = Logger::new("test_service");
        let mut context = HashMap::new();
        context.insert("user_id".to_string(), "123".to_string());
        context.insert("action".to_string(), "login".to_string());

        logger.log_with_context(LogLevel::Info, "User login", context);

        let logs = logger.get_logs();
        assert_eq!(logs[0].context.get("user_id"), Some(&"123".to_string()));
        assert_eq!(logs[0].context.get("action"), Some(&"login".to_string()));
    }

    #[test]
    fn test_logger_export_logs() {
        let logger = Logger::new("test_service");
        logger.info("message1");
        logger.error("message2");

        let exported = logger.export_logs();
        assert!(exported.contains("message1"));
        assert!(exported.contains("message2"));
        assert!(exported.contains("INFO"));
        assert!(exported.contains("ERROR"));
    }

    #[test]
    fn test_logger_clear() {
        let logger = Logger::new("test_service");
        logger.info("message1");
        logger.info("message2");

        assert_eq!(logger.log_count(), 2);

        logger.clear();
        assert_eq!(logger.log_count(), 0);
    }

    #[test]
    fn test_metrics_collector_clone() {
        let collector = MetricsCollector::new("test_service");
        collector.increment_counter("counter", 5);

        let cloned = collector.clone();
        assert_eq!(cloned.get_counter("counter"), Some(5));

        cloned.increment_counter("counter", 3);
        assert_eq!(collector.get_counter("counter"), Some(8)); // Shared state
    }

    #[test]
    fn test_logger_clone() {
        let logger = Logger::new("test_service");
        logger.info("message1");

        let cloned = logger.clone();
        cloned.info("message2");

        assert_eq!(logger.log_count(), 2); // Shared logs
    }

    #[test]
    fn test_logger_default() {
        let logger = Logger::default();
        logger.info("test");
        assert_eq!(logger.log_count(), 1);
    }

    #[test]
    fn test_metrics_collector_default() {
        let collector = MetricsCollector::default();
        collector.increment_counter("test", 1);
        assert_eq!(collector.get_counter("test"), Some(1));
    }

    #[test]
    fn test_histogram_empty_stats() {
        let collector = MetricsCollector::new("test_service");
        assert_eq!(collector.get_histogram_average("nonexistent"), None);
        assert_eq!(collector.get_histogram_min("nonexistent"), None);
        assert_eq!(collector.get_histogram_max("nonexistent"), None);
        assert_eq!(collector.get_histogram_count("nonexistent"), None);
    }

    #[test]
    fn test_multiple_metrics_types() {
        let collector = MetricsCollector::new("test_service");

        collector.increment_counter("requests", 100);
        collector.increment_counter("errors", 5);
        collector.set_gauge("cpu_usage", 45.5);
        collector.set_gauge("memory_usage", 2048.0);
        collector.record_histogram("latency", 50.0);
        collector.record_histogram("latency", 75.0);

        assert_eq!(collector.counters_count(), 2);
        assert_eq!(collector.gauges_count(), 2);
        assert_eq!(collector.histograms_count(), 1);
    }
}
