//! The transport's own error type.
//!
//! Distinct from [`qip_core::Error`] for one reason: the caller of a publish
//! has to be able to tell "the queue is full, slow down" from "the peer
//! answered 400, this message will never be accepted" from "five attempts
//! failed and it is now in the dead-letter log". Those want three different
//! responses, and a single `Error::Io(String)` makes them one thing that has to
//! be told apart by reading prose.
//!
//! Every variant converts into a [`qip_core::Error`] for the crate boundary,
//! and the conversion is where the platform-wide vocabulary is applied: a full
//! queue is a [`qip_core::Error::Guard`] because a limit refused it, not an
//! [`qip_core::Error::Io`] because nothing at the socket went wrong.

use crate::deadletter::DeadLetterReason;
use crate::http::HttpError;

/// What went wrong on the way to a peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The outbound queue is at capacity. Backpressure, by name: the peer is
    /// slower than this cell is producing and the caller must slow down, shed
    /// load, or accept the loss deliberately.
    QueueFull { queue: String, capacity: usize },
    /// The message was refused before any send was attempted, because it must
    /// not travel this way at all.
    Inadmissible { detail: String },
    /// Every attempt failed and the message is now recorded in the dead-letter
    /// log under this key.
    DeadLettered {
        key: String,
        attempts: u32,
        reason: DeadLetterReason,
        last_error: String,
    },
    /// The peer answered, with a status that is not a success.
    PeerRejected {
        status: u16,
        peer: String,
        excerpt: String,
    },
    /// The peer answered with something that is not the protocol.
    Protocol { detail: String },
    /// The request never completed.
    Http(HttpError),
}

impl TransportError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::QueueFull { .. } => "queue_full",
            Self::Inadmissible { .. } => "inadmissible",
            Self::DeadLettered { .. } => "dead_lettered",
            Self::PeerRejected { .. } => "peer_rejected",
            Self::Protocol { .. } => "protocol",
            Self::Http(_) => "http",
        }
    }

    /// Whether the *same* message could be offered again later with a
    /// different outcome.
    ///
    /// A full queue is retryable by the caller once the peer catches up; a
    /// dead-lettered message is not, because the transport has already spent
    /// its whole ladder on it and recorded the outcome.
    pub const fn is_retryable_by_caller(&self) -> bool {
        matches!(self, Self::QueueFull { .. })
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull { queue, capacity } => write!(
                f,
                "the outbound queue {queue} is full at {capacity} messages: the peer is slower \
                 than this cell is producing, and this transport refuses rather than growing \
                 without limit"
            ),
            Self::Inadmissible { detail } => write!(f, "the message may not be sent: {detail}"),
            Self::DeadLettered {
                key,
                attempts,
                reason,
                last_error,
            } => write!(
                f,
                "{key} was dead-lettered after {attempts} attempt(s) ({reason}): {last_error}"
            ),
            Self::PeerRejected {
                status,
                peer,
                excerpt,
            } => write!(f, "{peer} answered {status}: {excerpt}"),
            Self::Protocol { detail } => {
                write!(f, "the peer is not speaking the mesh protocol: {detail}")
            }
            Self::Http(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<HttpError> for TransportError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<serde_json::Error> for TransportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Protocol {
            detail: error.to_string(),
        }
    }
}

impl From<TransportError> for qip_core::Error {
    fn from(error: TransportError) -> Self {
        let message = error.to_string();
        match error {
            // A limit refused it. Nothing failed; something was declined.
            TransportError::QueueFull { .. } => Self::guard(message),
            TransportError::Inadmissible { .. } => Self::invalid(message),
            TransportError::Protocol { .. } => Self::schema(message),
            TransportError::DeadLettered { .. } | TransportError::PeerRejected { .. } => {
                Self::io(message)
            }
            TransportError::Http(inner) => inner.into(),
        }
    }
}
