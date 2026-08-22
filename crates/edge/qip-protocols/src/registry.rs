//! Which decoder serves which feed.
//!
//! A venue is not enough to pick a decoder: one venue routinely publishes an
//! ITCH feed, a FIX drop copy and a binary derivatives feed, and the same venue
//! runs an A line and a B line of the same feed that must be decoded
//! independently so their sequence gaps stay separate. The key is therefore
//! venue *and* feed.
//!
//! The registry owns a [`StreamAssembler`] per feed alongside the decoder, so a
//! caller pushing socket reads never has to think about frame boundaries. It
//! refuses to replace a registered decoder: a feed silently rebound to a second
//! decoder would keep two sequence positions and two order tables for the same
//! stream, and would look like packet loss.

use crate::decoder::Decoder;
use crate::framing::StreamAssembler;
use qip_contracts::{MarketMessage, VenueId};
use qip_core::error::{Error, Result};
use qip_core::Timestamp;
use std::collections::BTreeMap;

/// A venue's feed, as the registry keys it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeedKey {
    pub venue: VenueId,
    pub feed: String,
}

impl FeedKey {
    pub fn new(venue: VenueId, feed: impl Into<String>) -> Self {
        Self {
            venue,
            feed: feed.into(),
        }
    }

    /// The same shape [`qip_contracts::Origin::stream_key`] uses, minus the
    /// partition — a feed carries every partition it is subscribed to.
    pub fn label(&self) -> String {
        format!("{}/{}", self.venue.as_str(), self.feed)
    }
}

#[derive(Debug)]
struct Registration {
    decoder: Box<dyn Decoder>,
    assembler: StreamAssembler,
}

/// Maps venue and feed to the decoder that understands its wire format.
#[derive(Debug, Default)]
pub struct ProtocolRegistry {
    feeds: BTreeMap<FeedKey, Registration>,
}

impl ProtocolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a decoder to a feed.
    pub fn register(
        &mut self,
        venue: VenueId,
        feed: impl Into<String>,
        decoder: Box<dyn Decoder>,
    ) -> Result<()> {
        let key = FeedKey::new(venue, feed);
        if self.feeds.contains_key(&key) {
            return Err(Error::invalid(format!(
                "{} already has a decoder registered; rebinding it would split the stream's sequence position in two",
                key.label()
            )));
        }
        self.feeds.insert(
            key,
            Registration {
                decoder,
                assembler: StreamAssembler::new(),
            },
        );
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.feeds.is_empty()
    }

    pub fn len(&self) -> usize {
        self.feeds.len()
    }

    /// Every registered feed and the protocol serving it.
    pub fn feeds(&self) -> Vec<(FeedKey, String)> {
        self.feeds
            .iter()
            .map(|(key, entry)| (key.clone(), entry.decoder.protocol().to_string()))
            .collect()
    }

    pub fn decoder_mut(&mut self, venue: &VenueId, feed: &str) -> Result<&mut (dyn Decoder + 'static)> {
        let key = FeedKey::new(venue.clone(), feed);
        self.feeds
            .get_mut(&key)
            .map(|entry| entry.decoder.as_mut())
            .ok_or_else(|| Error::not_found(format!("no decoder registered for {}", key.label())))
    }

    /// Decode a socket read for one feed, carrying any partial trailing frame.
    ///
    /// This is the only method a capture loop needs: the reassembly, the decoder
    /// and the diagnostics all hang off the feed key.
    pub fn push(
        &mut self,
        venue: &VenueId,
        feed: &str,
        bytes: &[u8],
        captured_at: Timestamp,
    ) -> Result<Vec<MarketMessage>> {
        let key = FeedKey::new(venue.clone(), feed);
        let entry = self
            .feeds
            .get_mut(&key)
            .ok_or_else(|| Error::not_found(format!("no decoder registered for {}", key.label())))?;
        entry
            .assembler
            .push(entry.decoder.as_mut(), bytes, captured_at)
    }

    /// Bytes held back from a feed awaiting the rest of a frame.
    pub fn pending(&self, venue: &VenueId, feed: &str) -> Option<usize> {
        self.feeds
            .get(&FeedKey::new(venue.clone(), feed))
            .map(|entry| entry.assembler.pending())
    }

    /// Drop the carry-over buffer for a feed after a resynchronisation.
    pub fn resynchronise(&mut self, venue: &VenueId, feed: &str) -> Result<()> {
        let key = FeedKey::new(venue.clone(), feed);
        let entry = self
            .feeds
            .get_mut(&key)
            .ok_or_else(|| Error::not_found(format!("no decoder registered for {}", key.label())))?;
        entry.assembler.reset();
        Ok(())
    }
}
