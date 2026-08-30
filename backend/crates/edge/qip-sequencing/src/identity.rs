//! Identifiers for messages this crate synthesises.
//!
//! A gap and a failover both produce a message the venue never sent — a
//! [`MessageBody::Reset`] telling consumers the book they hold is no longer
//! trustworthy. Those messages need identifiers, and the identifiers have to be
//! derived rather than generated: the platform replays event logs and diffs the
//! results byte for byte, so a reset minted from a counter or a clock would make
//! two identical runs differ.
//!
//! The derivation deliberately mirrors the one in `qip-protocols` rather than
//! sharing it. The two crates are siblings with no dependency between them, and
//! adding one so that a sixteen-line hash could be shared would let a decoder's
//! change ripple into the sequencer. The `tag` argument keeps the two spaces
//! disjoint: a synthesised reset can never collide with a decoded message.

use qip_contracts::{MarketMessage, MessageBody, Origin};
use qip_core::{Hasher256, ObjectId, Timestamp};

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A deterministic identifier for a message this crate invented.
pub fn synthetic_id(origin: &Origin, tag: &str, at: Timestamp) -> ObjectId {
    let mut hasher = Hasher256::new();
    hasher.update(b"qip-sequencing\x1f");
    hasher.update(origin.stream_key().as_bytes());
    hasher.update(b"\x1f");
    hasher.update(&origin.sequence.to_be_bytes());
    hasher.update(b"\x1f");
    hasher.update(tag.as_bytes());
    let digest = hasher.finish();

    let mut entropy: u128 = 0;
    for &byte in digest.iter().take(10) {
        entropy = (entropy << 8) | u128::from(byte);
    }
    let millis = u128::try_from(at.as_millis().max(0)).unwrap_or(0) & 0xFFFF_FFFF_FFFF;
    let mut value = (millis << 80) | entropy;

    let mut buf = [0u8; 26];
    for slot in buf.iter_mut().rev() {
        *slot = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    ObjectId::from_string(String::from_utf8_lossy(&buf).into_owned())
}

/// The message that tells a consumer to discard its book and rebuild.
///
/// Both times are the moment the platform decided the book was untrustworthy,
/// not the moment the venue sent anything — because the venue sent nothing. That
/// is the honest stamping: the fact being reported is "this cell lost data at
/// this instant", and it became true and known at the same time.
pub fn reset_message(origin: Origin, reason: impl Into<String>, at: Timestamp) -> MarketMessage {
    let object_id = synthetic_id(&origin, "reset", at);
    MarketMessage::new(
        object_id,
        origin,
        MessageBody::Reset {
            reason: reason.into(),
        },
        at,
        at,
    )
}
