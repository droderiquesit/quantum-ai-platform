//! `qip-protocols` — native venue protocol decoders.
//!
//! Every venue speaks its own dialect and every dialect stops here. What leaves
//! this crate is [`qip_contracts::MarketMessage`], attributed to a venue, a feed
//! and a sequence number, and nothing downstream can tell whether it arrived as
//! FIX tags, as big-endian ITCH, as an SBE block or as a JSON frame off a
//! WebSocket. That is what lets one order book implementation serve every venue
//! class the platform trades.
//!
//! Four properties are held to across every decoder here, and the tests assert
//! them rather than asserting particular bytes:
//!
//! * **A truncated message produces nothing and consumes nothing.** The bytes
//!   stay in the caller's buffer until the rest of the message arrives.
//! * **A message that fails its own integrity check is refused by name.** FIX
//!   checksum and body-length disagreement are the two that matter, because a
//!   corrupt message accepted silently is indistinguishable from a real one for
//!   as long as it takes to lose money on it.
//! * **An unrecognised message type is skipped with a record.** The rest of the
//!   batch decodes; the operator learns what was dropped and why.
//! * **Decoding is a pure function of the bytes and the timestamps handed in.**
//!   No clock, no randomness, and identifiers derived from message identity, so
//!   a replayed capture reproduces the earlier run exactly — down to the
//!   identifiers a downstream deduplicator compares.
//!
//! Instrument identity is carried by [`qip_contracts::Origin::partition`], and
//! symbols are resolved to partitions by [`InstrumentPartitions`] at decode
//! time. Nothing downstream ever sees a symbol string, which keeps the
//! per-instrument state of every consumer keyed by an integer.

pub mod bytes;
pub mod decoder;
pub mod fix;
pub mod framing;
pub mod itch;
pub mod registry;
pub mod sbe;

pub use bytes::{ByteOrder, Reader};
pub use decoder::{
    Decoder, Diagnostics, FeedIdentity, InstrumentPartitions, SkipReason, SkipRecord, message_id,
};
pub use fix::{FixDecoder, FixFields};
pub use framing::{Framing, JsonFrameDecoder, StreamAssembler};
pub use itch::ItchDecoder;
pub use registry::{FeedKey, ProtocolRegistry};
pub use sbe::{
    FieldRole, SbeDecoder, SbeEncoding, SbeField, SbeGroup, SbeMessage, SbeMessageKind, SbeSchema,
    SbeValue,
};
