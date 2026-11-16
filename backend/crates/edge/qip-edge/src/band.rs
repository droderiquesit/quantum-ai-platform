//! Pre-positioned directional bands: what a cell may still do on one side when
//! the peer mesh is degraded (ADR 0154 decision 8, ADR 0662, R12-RES-7095).
//!
//! A multi-cell cycle needs the counterparty's reservation before it fires,
//! so with no reachable peer none can. What the requirement allows instead of
//! stopping coordination outright is a *band*: an amount the centre
//! positioned in advance for one cell on one side, usable only while the mesh
//! is degraded and only inside the cell's own share.
//!
//! # A band is reserved against the share, never added to it
//!
//! The share is the cell's disjoint slice of its region's grant (ADR 0039).
//! A band that widened it would be a second source of capital authority the
//! centre never signed into the grant. So [`BandBook`] is opened over a
//! share, and the sum of its live bands may not exceed it: a band that would
//! is refused, not trimmed. `reserved() + free() == share` is the invariant a
//! test can assert, which is why the type exists apart from the share.
//!
//! # Epoch
//!
//! A band carries the epoch the centre positioned it under. One at or below
//! the epoch already held for the same `(cell, direction)` is refused: a
//! replayed older band is how a withdrawn position would come back (ADR
//! 0008's argument for payload sequences, applied here).
//!
//! # Nothing here places an order
//!
//! This is arithmetic. A band authorises no order by existing; the pass still
//! sizes, gates and routes anything it sends, and [`BandBook::usable`] returns
//! nothing while the mesh is healthy so a band cannot shadow a coordinated
//! cycle.

use qip_contracts::message::BookSide;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// One pre-positioned amount for one cell on one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectionalBand {
    pub cell: String,
    pub direction: BookSide,
    pub amount: Decimal,
    pub epoch: u64,
}

impl DirectionalBand {
    /// Refuses an empty cell name and an amount that is not positive: a band
    /// of nothing positions nothing, and a negative one would read as room.
    pub fn new(
        cell: impl Into<String>,
        direction: BookSide,
        amount: Decimal,
        epoch: u64,
    ) -> Result<Self> {
        let cell = cell.into();
        if cell.trim().is_empty() {
            return Err(Error::invalid(
                "a directional band names the cell it is positioned for",
            ));
        }
        if amount <= Decimal::ZERO {
            return Err(Error::invalid(
                "a directional band's amount must be positive; a band of nothing positions \
                 nothing and a negative one would read as room",
            ));
        }
        Ok(Self {
            cell,
            direction,
            amount,
            epoch,
        })
    }
}

/// The live bands of one cell, held inside its share.
#[derive(Clone, Debug)]
pub struct BandBook {
    cell: String,
    share: Decimal,
    bands: BTreeMap<&'static str, DirectionalBand>,
}

impl BandBook {
    /// A book over `share`, the cell's applied region share. Refused when the
    /// share is negative.
    pub fn new(cell: impl Into<String>, share: Decimal) -> Result<Self> {
        if share < Decimal::ZERO {
            return Err(Error::invalid(
                "a band book is opened over a share that is not negative",
            ));
        }
        Ok(Self {
            cell: cell.into(),
            share,
            bands: BTreeMap::new(),
        })
    }

    /// Reserve `band` against the share. Refused for another cell's band, for
    /// an epoch not above the one held for that side, and for an amount that
    /// would take the reserved total above the share — in each case with the
    /// book unchanged. A new band on a side replaces the older one it
    /// supersedes, so the check is against the total after the replacement.
    pub fn pre_position(&mut self, band: DirectionalBand) -> Result<()> {
        if band.cell != self.cell {
            return Err(Error::denied(format!(
                "a band for {} cannot be reserved in {}'s book",
                band.cell, self.cell
            )));
        }
        let side = band.direction.as_str();
        if let Some(held) = self.bands.get(side)
            && band.epoch <= held.epoch
        {
            return Err(Error::denied(format!(
                "a {side} band at epoch {} is not newer than the epoch {} already held; a \
                 replayed band would bring back a position the centre replaced",
                band.epoch, held.epoch
            )));
        }
        let others = self
            .bands
            .iter()
            .filter(|(held_side, _)| **held_side != side)
            .try_fold(Decimal::ZERO, |sum, (_, held)| sum.checked_add(held.amount))
            .ok_or_else(|| Error::numeric("the reserved bands overflow the arithmetic"))?;
        let total = others
            .checked_add(band.amount)
            .ok_or_else(|| Error::numeric("the reserved bands overflow the arithmetic"))?;
        if total > self.share {
            return Err(Error::denied(format!(
                "reserving {} on {side} would take the bands to {total}, above the share \
                 {}; a band is reserved against the share and never added to it",
                band.amount, self.share
            )));
        }
        self.bands.insert(side, band);
        Ok(())
    }

    /// The share the bands are reserved against.
    pub fn share(&self) -> Decimal {
        self.share
    }

    /// The sum of the live bands.
    pub fn reserved(&self) -> Decimal {
        // `pre_position` keeps the sum at or below the share, so this cannot
        // overflow; if it somehow did, the restrictive reading is a book with
        // nothing free.
        self.bands
            .values()
            .try_fold(Decimal::ZERO, |sum, band| sum.checked_add(band.amount))
            .unwrap_or(self.share)
    }

    /// The share less the bands. `reserved() + free() == share()`.
    pub fn free(&self) -> Decimal {
        self.share
            .checked_sub(self.reserved())
            .unwrap_or(Decimal::ZERO)
    }

    /// The bands a pass may size against: none while the mesh is healthy, so a
    /// band cannot stand in for a coordinated cycle, and all of them while it
    /// is degraded.
    pub fn usable(&self, mesh_degraded: bool) -> Vec<&DirectionalBand> {
        if mesh_degraded {
            self.bands.values().collect()
        } else {
            Vec::new()
        }
    }
}
