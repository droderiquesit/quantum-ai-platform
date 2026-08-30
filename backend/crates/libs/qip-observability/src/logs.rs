//! Structured logging.
//!
//! Records are structured values, not formatted strings: the fields survive to
//! the log backend where they can be queried. Every record carries the trace
//! and correlation ids in scope, which is what lets an operator pivot from a
//! log line to the full decision lineage.

use qip_core::{Clock, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
    Critical = 4,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogRecord {
    pub at: Timestamp,
    pub severity: Severity,
    pub service: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

impl LogRecord {
    /// One-line rendering for a terminal.
    pub fn to_line(&self) -> String {
        let fields: Vec<String> = self
            .fields
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let suffix = if fields.is_empty() {
            String::new()
        } else {
            format!(" {}", fields.join(" "))
        };
        format!(
            "{} {:<8} {} {}{}",
            self.at.to_rfc3339(),
            self.severity.as_str(),
            self.service,
            self.message,
            suffix
        )
    }
}

/// Collects structured log records.
#[derive(Debug)]
pub struct Logger {
    service: String,
    clock: Arc<dyn Clock>,
    records: Mutex<Vec<LogRecord>>,
    minimum: AtomicU8,
    /// Also write to stderr. Off by default so tests stay quiet.
    echo: AtomicU8,
    capacity: usize,
}

impl Logger {
    pub fn new(service: impl Into<String>, clock: Arc<dyn Clock>) -> Self {
        Self {
            service: service.into(),
            clock,
            records: Mutex::new(Vec::new()),
            minimum: AtomicU8::new(Severity::Info as u8),
            echo: AtomicU8::new(0),
            capacity: 20_000,
        }
    }

    pub fn set_minimum_severity(&self, severity: Severity) {
        self.minimum.store(severity as u8, Ordering::SeqCst);
    }

    /// Mirror records to stderr, for a foreground process.
    pub fn set_echo(&self, echo: bool) {
        self.echo.store(u8::from(echo), Ordering::SeqCst);
    }

    pub fn log(
        &self,
        severity: Severity,
        message: impl Into<String>,
        fields: BTreeMap<String, String>,
    ) {
        if (severity as u8) < self.minimum.load(Ordering::SeqCst) {
            return;
        }
        let record = LogRecord {
            at: self.clock.now(),
            severity,
            service: self.service.clone(),
            message: message.into(),
            fields,
        };
        if self.echo.load(Ordering::SeqCst) == 1 {
            eprintln!("{}", record.to_line());
        }
        let mut guard = self.records.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= self.capacity {
            guard.remove(0);
        }
        guard.push(record);
    }

    pub fn debug(&self, message: impl Into<String>) {
        self.log(Severity::Debug, message, BTreeMap::new());
    }
    pub fn info(&self, message: impl Into<String>) {
        self.log(Severity::Info, message, BTreeMap::new());
    }
    pub fn warn(&self, message: impl Into<String>) {
        self.log(Severity::Warn, message, BTreeMap::new());
    }
    pub fn error(&self, message: impl Into<String>) {
        self.log(Severity::Error, message, BTreeMap::new());
    }
    pub fn critical(&self, message: impl Into<String>) {
        self.log(Severity::Critical, message, BTreeMap::new());
    }

    /// Log with fields: `logger.with(Severity::Warn, "limit breached", [("limit", "leverage")])`.
    pub fn with<const N: usize>(
        &self,
        severity: Severity,
        message: impl Into<String>,
        fields: [(&str, &str); N],
    ) {
        let map = fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self.log(severity, message, map);
    }

    pub fn records(&self) -> Vec<LogRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Records at or above a severity.
    pub fn at_least(&self, severity: Severity) -> Vec<LogRecord> {
        self.records()
            .into_iter()
            .filter(|r| r.severity >= severity)
            .collect()
    }

    pub fn clear(&self) {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}
