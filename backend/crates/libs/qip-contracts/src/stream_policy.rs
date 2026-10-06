//! Stream policies for managing message delivery and backpressure (CONTRACT-037).
//!
//! Defines how streams should handle acknowledgments, overload, and delivery
//! guarantees.

use serde::{Deserialize, Serialize};

/// Acknowledgment policy for stream messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AckPolicy {
    /// Acknowledge immediately upon receipt.
    Immediate,
    /// Acknowledge after processing.
    AfterProcessing,
    /// No acknowledgment required.
    None,
}

/// Behavior when a stream is overloaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverloadBehavior {
    /// Drop oldest messages.
    DropOldest,
    /// Drop newest messages.
    DropNewest,
    /// Block until buffer is drained.
    Block,
    /// Refuse new messages.
    Refuse,
}

/// Policy for stream message delivery and handling.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamPolicy {
    /// Acknowledgment policy.
    pub ack: AckPolicy,
    /// Behavior when overloaded.
    pub overload: OverloadBehavior,
    /// Maximum messages to buffer before overload.
    pub max_buffered: usize,
}

impl Default for StreamPolicy {
    fn default() -> Self {
        Self {
            ack: AckPolicy::Immediate,
            overload: OverloadBehavior::Refuse,
            max_buffered: 1024,
        }
    }
}
