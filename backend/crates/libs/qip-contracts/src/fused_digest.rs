//! The distilled fused state as it crosses to one reflex cell (R12-TICK-2086;
//! ADR 0133 decision on what crosses between the centre and the reflex lane).
//!
//! The centre fuses world events with tick reactions and distils an
//! opportunity map. A reflex cell must not receive the map as built: it is
//! derived from licensed tick data, so which region may hold it is the
//! licence's decision, and a map the cell reads after it has aged out is a
//! reaction that is no longer live. A [`FusedDigest`] is that map addressed to
//! one cell in one region, with its own validity, and [`FusedDigest::read`] is
//! the only way to take entries out of it: it refuses a digest for another
//! region and one past its validity, and it never returns an entry past its
//! own expiry.
//!
//! This is a wire type only. It is **not yet a slot of `PolicyPayload`**:
//! adding the slot changes the signing string and every payload literal, and
//! that is a versioned change of its own.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// One live reaction: the instrument, a posterior signed by its expected
/// direction, and when it stops being live.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusedDigestEntry {
    pub instrument: String,
    /// Posterior that a reaction is live, signed by expected direction. A
    /// statistic, so `f64`; it is never a price or a size.
    pub score: f64,
    pub expires_at: Timestamp,
}

/// The fused state distilled for one cell in one region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusedDigest {
    pub cell: String,
    pub region: String,
    pub as_of: Timestamp,
    /// After this instant the whole digest is stale whatever its entries say.
    pub valid_until: Timestamp,
    pub entries: Vec<FusedDigestEntry>,
    /// Digest of the fused state it was distilled from, so a replay can find
    /// the source.
    pub source_digest: String,
}

impl FusedDigest {
    /// Refuse a digest that states nothing it needs to: an empty cell or
    /// region, a validity that does not follow `as_of`, a non-finite score.
    pub fn validate(&self) -> Result<()> {
        if self.cell.trim().is_empty() || self.region.trim().is_empty() {
            return Err(Error::invalid(
                "a fused digest names the cell and region it is for; an unaddressed digest \
                 would be read by whichever cell saw it first",
            ));
        }
        if self.valid_until <= self.as_of {
            return Err(Error::invalid(
                "a fused digest is valid for a positive time after its instant",
            ));
        }
        if self.entries.iter().any(|e| !e.score.is_finite()) {
            return Err(Error::invalid(
                "a fused digest entry has a non-finite score",
            ));
        }
        Ok(())
    }

    /// The entries a cell in `region` may act on at `now`. Refuses a digest
    /// for another region (the licence places the data, not the reader),
    /// a stale digest, and a digest that fails [`Self::validate`]; entries past
    /// their own expiry are left out, not returned flagged.
    pub fn read(&self, now: Timestamp, region: &str) -> Result<Vec<&FusedDigestEntry>> {
        self.validate()?;
        if self.region != region {
            return Err(Error::denied(format!(
                "this fused digest is for region {}, not {region}; its data may not be placed \
                 there",
                self.region
            )));
        }
        if now > self.valid_until {
            return Err(Error::denied(format!(
                "this fused digest was valid until {} and it is {now}; a stale digest is not \
                 read",
                self.valid_until
            )));
        }
        Ok(self.entries.iter().filter(|e| e.expires_at > now).collect())
    }

    /// How long the digest has left at `now`, zero once stale.
    pub fn remaining(&self, now: Timestamp) -> Duration {
        if now >= self.valid_until {
            Duration::ZERO
        } else {
            self.valid_until.since(now)
        }
    }
}
