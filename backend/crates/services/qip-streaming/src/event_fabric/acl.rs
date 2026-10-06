//! Event-fabric ACL: key-scoped grants — a cell may write only its own
//! partition key. See ADR 0100 §7 and FABRIC-018.
//!
//! The grant schema is defined once, in
//! [`qip_events::event_fabric::catalogue`], and parsed from the committed
//! stream catalogue. This module is the other half: the decision a broker
//! makes with those grants on every request. It is pure — grants in, a
//! [`Scope`] out — so the rule is tested without a listener, and the
//! protocol handler in [`super::service`] is the only thing that calls it.
//!
//! # What a grant confers
//!
//! A grant names an identity, a stream, one permission and a key scope. The
//! permission is matched exactly: an `admin` grant does not let its holder
//! produce or consume, because an operator who can park a partition should
//! not thereby be able to forge the records on it. The key scope is either
//! `any` or `own_key`, and `own_key` means exactly one partition: the one
//! the identity's own key routes to under [`super::partition::partition_for`],
//! the same function the broker routes a produced key with. The two cannot
//! disagree about which partition a cell owns, because there is one function.
//!
//! # Why refusal is the default
//!
//! [`scope`] answers [`Scope::None`] for an identity the grants do not name
//! on that stream under that permission. There is no wildcard identity, no
//! "all streams" grant, and an empty grant list refuses everyone: a broker
//! started on a catalogue with its grants missing serves nobody, which is
//! the failure that gets noticed.

use qip_core::error::Result;
use qip_events::event_fabric::catalogue::{Grant, KeyScope, Permission};
use qip_transport::event_fabric::protocol::Refusal;

use super::partition::partition_for;

/// The prefix a reflex cell's identity carries. The remainder is the cell
/// id, which is the cell's partition key (ADR 0100 §4: "each cell's journal
/// is its own partition key").
const CELL_IDENTITY_PREFIX: &str = "reflex:";

/// The partition key `identity`'s own-key grants are scoped to: the cell id
/// for a `reflex:<cell>` identity, the identity itself for anything else.
pub fn own_key(identity: &str) -> &str {
    identity
        .strip_prefix(CELL_IDENTITY_PREFIX)
        .unwrap_or(identity)
}

/// What an identity may reach on one stream under one permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// No grant: the identity may not exercise this permission here at all.
    None,
    /// An own-key grant: this one partition and no other.
    Own(u32),
    /// A grant scoped to any key: every partition of the stream.
    Any,
}

impl Scope {
    /// Whether this scope admits `partition`, as the refusal the protocol
    /// answers with when it does not. `AclDenied` and `KeyOutOfScope` are
    /// kept apart because they send an operator to different places: the
    /// first to the grant list, the second to the cell id a node was started
    /// with.
    pub fn admits(&self, partition: u32) -> std::result::Result<(), Refusal> {
        match self {
            Scope::None => Err(Refusal::AclDenied),
            Scope::Own(own) if *own != partition => Err(Refusal::KeyOutOfScope),
            Scope::Own(_) | Scope::Any => Ok(()),
        }
    }

    /// The partitions of a `partition_count`-partition stream this scope
    /// admits, ascending.
    pub fn partitions(&self, partition_count: u32) -> Vec<u32> {
        match self {
            Scope::None => Vec::new(),
            Scope::Own(own) => vec![*own],
            Scope::Any => (0..partition_count).collect(),
        }
    }
}

/// The scope `grants` give `identity` on `stream` under any one of
/// `permissions`.
///
/// More than one permission is accepted because some requests are
/// legitimately made by more than one kind of caller — a producer and a
/// consumer both read a partition's metadata — and the answer is the widest
/// scope any matching grant confers.
pub fn scope(
    grants: &[Grant],
    identity: &str,
    stream: &str,
    permissions: &[Permission],
    partition_count: u32,
) -> Result<Scope> {
    let mut found = Scope::None;
    for grant in grants {
        if grant.identity != identity
            || grant.stream != stream
            || !permissions.contains(&grant.permission)
        {
            continue;
        }
        match grant.key_scope {
            KeyScope::Any => return Ok(Scope::Any),
            KeyScope::OwnKey => {
                found = Scope::Own(partition_for(own_key(identity), partition_count)?);
            }
        }
    }
    Ok(found)
}
