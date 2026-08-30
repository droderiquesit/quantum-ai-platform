//! Platform error type.
//!
//! One error enum crosses crate boundaries. Variants describe *what kind of
//! failure* a caller must handle, not which module raised it — the message
//! carries the detail.

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Input violated a documented precondition (bad symbol, negative size…).
    Invalid(String),
    /// A required entity, instrument, model or record does not exist.
    NotFound(String),
    /// Operation is legal but not permitted in the current state or role.
    Denied(String),
    /// Arithmetic overflow, non-convergence, or a numerically undefined result.
    Numeric(String),
    /// Serialization / schema / contract violation.
    Schema(String),
    /// Underlying storage, transport or provider failure.
    Io(String),
    /// A configured external dependency is not available in this deployment.
    Unavailable(String),
    /// Guard tripped: leakage, limit breach, kill switch, budget exhausted.
    Guard(String),
    /// Deadline exceeded.
    Timeout(String),
}

impl Error {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn denied(msg: impl Into<String>) -> Self {
        Self::Denied(msg.into())
    }
    pub fn numeric(msg: impl Into<String>) -> Self {
        Self::Numeric(msg.into())
    }
    pub fn schema(msg: impl Into<String>) -> Self {
        Self::Schema(msg.into())
    }
    pub fn io(msg: impl Into<String>) -> Self {
        Self::Io(msg.into())
    }
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::Unavailable(msg.into())
    }
    pub fn guard(msg: impl Into<String>) -> Self {
        Self::Guard(msg.into())
    }
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::Timeout(msg.into())
    }

    /// Stable machine-readable code, surfaced in API responses and metrics.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid",
            Self::NotFound(_) => "not_found",
            Self::Denied(_) => "denied",
            Self::Numeric(_) => "numeric",
            Self::Schema(_) => "schema",
            Self::Io(_) => "io",
            Self::Unavailable(_) => "unavailable",
            Self::Guard(_) => "guard",
            Self::Timeout(_) => "timeout",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m)
            | Self::NotFound(m)
            | Self::Denied(m)
            | Self::Numeric(m)
            | Self::Schema(m)
            | Self::Io(m)
            | Self::Unavailable(m)
            | Self::Guard(m)
            | Self::Timeout(m) => m,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Schema(e.to_string())
    }
}
