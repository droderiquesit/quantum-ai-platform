//! What this cell knows about the cross-region mirrors it takes part in
//! (blueprint §31.1), and how those facts reach §33.1's path-3 extension.
//!
//! # The split between what arrives on the wire and what does not
//!
//! §31.1's SETUP distributes "reference price R, threshold, targets". Two of
//! those three arrive on the policy payload's tenth slot,
//! `qip_contracts::policy::InventoryTargets`, which carries `targets` and
//! `reference_prices` keyed by instrument — and, as its own module notes,
//! **no band field at all**.
//!
//! That missing field is the design, not a gap to be filled. The payload
//! travels a wire that authenticates nobody beyond a signature, and a field
//! on it that could *widen* a safety band would move a capital boundary onto
//! that wire. `FeasibilityConstraints::withdrawn_venues` makes the same
//! argument in the other direction and states it plainly: a wire may
//! subtract, never add. So the centre says **where** a region should be and
//! **what the world price is**, and this cell's own operator says **how far
//! off it may drift** before the direction is forced, and the second never
//! arrives from outside the process.
//!
//! The threshold sits on the same side of that line as the bands. It decides
//! how small a dislocation is worth trading, and a payload that could shrink
//! it to nothing would turn every tick into a trade.
//!
//! # What this cannot do
//!
//! It names no venue and cannot make one reachable. A [`MirrorArrangement`]
//! is keyed by instrument, and which venues a cell may trade is decided
//! entirely by `CellConfig::venues`, checked at `Cell::install_arbitrage`
//! against every edge of the desk's graph and again at the node's
//! `graph_from_whitelist`. An arrangement naming an instrument the cell never
//! sees simply never matches an edge.
//!
//! It also produces no order and no size. Everything here feeds
//! `qip_routing::extension::check`, which can only refuse.

use qip_core::ObjectId;
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use qip_core::{Decimal, Timestamp};
use qip_routing::mirror::{DistributedReference, InventoryBand};
use std::collections::BTreeMap;

/// One mirrored instrument's local discipline.
///
/// `soft` and `hard` are half-widths measured from whatever target the
/// centre distributes; `market` names the book this cell reads a local price
/// from, because an inventory is held in an instrument and a price is quoted
/// on a market and the two are not the same identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirroredInstrument {
    soft: Decimal,
    hard: Decimal,
    market: ObjectId,
    threshold: Decimal,
}

impl MirroredInstrument {
    /// Refuse a discipline that could not gate anything.
    ///
    /// The band widths are checked here as well as in
    /// [`InventoryBand::new`], because the target they will be combined with
    /// arrives per pass from the policy and a band that is malformed at
    /// configuration time would otherwise first be discovered on a pass, as
    /// a refusal naming the centre.
    pub fn new(soft: Decimal, hard: Decimal, market: ObjectId, threshold: Decimal) -> Result<Self> {
        // `Decimal::ZERO` is a target every band admits, so this proves the
        // widths and the threshold without asserting anything about where
        // the centre will put the target.
        InventoryBand::new(Decimal::ZERO, soft, hard)?;
        if !threshold.is_positive() {
            return Err(Error::invalid(format!(
                "a dislocation threshold of {threshold} for {} would make every local price a \
                 dislocation; supply the distance from the distributed reference that is worth \
                 trading",
                market.as_str()
            )));
        }
        Ok(Self {
            soft,
            hard,
            market,
            threshold,
        })
    }

    pub fn market(&self) -> &ObjectId {
        &self.market
    }

    pub const fn threshold(&self) -> Decimal {
        self.threshold
    }

    /// The band this instrument is gated by, once the centre's target for it
    /// is known.
    pub fn band_around(&self, target: Decimal) -> Result<InventoryBand> {
        InventoryBand::new(target, self.soft, self.hard)
    }

    /// The reference the §33.1 extension will check the window of.
    ///
    /// `observed_at` is the policy slot's own `produced_at` and `ttl` is
    /// `PolicyItem::InventoryTargets::time_to_live`, so the freshness
    /// question is asked once, by the gate, rather than twice. A cell that
    /// filtered the slot on freshness first would leave the extension's own
    /// window check unable to fire — a control that reads as protection and
    /// cannot.
    pub fn reference_at(
        &self,
        price: Decimal,
        observed_at: Timestamp,
        ttl: Duration,
    ) -> Result<DistributedReference> {
        DistributedReference::new(price, self.threshold, observed_at, ttl)
    }
}

/// Every mirror this cell takes part in, and the measured round trip to each
/// region on the other side of one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MirrorArrangement {
    instruments: BTreeMap<ObjectId, MirroredInstrument>,
    round_trips: BTreeMap<String, Duration>,
}

impl MirrorArrangement {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare one instrument mirrored, with the discipline this region
    /// holds it under.
    pub fn with_instrument(mut self, object: ObjectId, discipline: MirroredInstrument) -> Self {
        self.instruments.insert(object, discipline);
        self
    }

    /// Record the measured round trip to one remote region.
    ///
    /// Refused when it is not a measurement. `MirrorFacts::new` refuses a
    /// round trip of zero because it makes every firm quote look like it
    /// outlasts the wire; catching it here names the configuration instead
    /// of the cycle.
    pub fn with_round_trip(
        mut self,
        region: impl Into<String>,
        round_trip: Duration,
    ) -> Result<Self> {
        let region = region.into();
        if region.trim() != region || region.is_empty() {
            return Err(Error::invalid(format!(
                "region id {region:?} is empty or carries surrounding whitespace; two ids \
                 differing by a space are two regions to a mirror edge, so supply it exactly"
            )));
        }
        if round_trip.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "a round trip of {} ns to region {region} is not a measurement; §31 puts New \
                 York to London at roughly 28 ms each way, and a zero here makes every remote \
                 quote look like it outlasts the wire",
                round_trip.as_nanos()
            )));
        }
        self.round_trips.insert(region, round_trip);
        Ok(self)
    }

    pub fn instrument(&self, object: &ObjectId) -> Option<&MirroredInstrument> {
        self.instruments.get(object)
    }

    /// The measured round trip to `region`, or a refusal naming the regions
    /// that were measured.
    ///
    /// Not an `Option` and not a default: a default round trip would be a
    /// number nobody measured sitting where §30.2's row 6 is decided.
    pub fn round_trip(&self, region: &str) -> Result<Duration> {
        self.round_trips.get(region).copied().ok_or_else(|| {
            let known: Vec<&str> = self.round_trips.keys().map(String::as_str).collect();
            Error::not_found(format!(
                "no round trip to region {region} has been measured, and a mirror edge reaching \
                 it cannot be routed without one; measure it beside the {} already recorded \
                 [{}], or do not mirror into that region",
                self.round_trips.len(),
                known.join(", ")
            ))
        })
    }

    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty()
    }

    pub fn len(&self) -> usize {
        self.instruments.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(literal: &str) -> Decimal {
        Decimal::parse(literal).expect("a test decimal parses")
    }

    fn object(id: &str) -> ObjectId {
        ObjectId::from_string(id)
    }

    fn discipline() -> MirroredInstrument {
        MirroredInstrument::new(d("5"), d("20"), object("BTCUSD"), d("100"))
            .expect("a valid discipline")
    }

    #[test]
    fn a_band_whose_widths_are_malformed_is_refused_at_configuration_rather_than_on_a_pass() {
        // The failure this prevents: a cell that installs a nonsensical band
        // and only discovers it when a cross-region cycle arrives, where the
        // refusal reads as though the centre's target were the problem.
        let refusal = MirroredInstrument::new(d("20"), d("5"), object("BTCUSD"), d("100"))
            .expect_err("a hard band inside the soft band can never be reached");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal
                .message()
                .contains("is not wider than the soft band"),
            "the refusal should name the band: {}",
            refusal.message()
        );
        // The half that proves it admits a good value.
        assert!(MirroredInstrument::new(d("5"), d("20"), object("BTCUSD"), d("100")).is_ok());
    }

    #[test]
    fn a_threshold_of_zero_is_refused_because_every_tick_would_be_a_dislocation() {
        let refusal = MirroredInstrument::new(d("5"), d("20"), object("BTCUSD"), Decimal::ZERO)
            .expect_err("a zero threshold gates nothing");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal
                .message()
                .contains("would make every local price a dislocation"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_round_trip_nobody_measured_is_refused_rather_than_defaulted() {
        let arrangement = MirrorArrangement::new()
            .with_instrument(object("BTC"), discipline())
            .with_round_trip("eu-west", Duration::from_millis(28))
            .expect("a measured round trip");
        // Premise: one region really is measured, so this is not an
        // empty-map artefact.
        assert_eq!(
            arrangement
                .round_trip("eu-west")
                .expect("the measured region"),
            Duration::from_millis(28)
        );
        let refusal = arrangement
            .round_trip("ap-south")
            .expect_err("an unmeasured region has no round trip");
        assert_eq!(refusal.code(), "not_found");
        assert!(
            refusal
                .message()
                .contains("no round trip to region ap-south"),
            "the refusal should name the region: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_round_trip_of_zero_is_refused_at_the_configuration_that_supplied_it() {
        let refusal = MirrorArrangement::new()
            .with_round_trip("eu-west", Duration::ZERO)
            .expect_err("an unmeasured round trip is not a measurement");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("is not a measurement"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_region_id_with_surrounding_whitespace_is_refused_rather_than_trimmed() {
        // Two ids differing by a space are two regions to a mirror edge, and
        // a cell that trimmed one here would look up a round trip under a
        // name the router never produces.
        let refusal = MirrorArrangement::new()
            .with_round_trip(" eu-west", Duration::from_millis(28))
            .expect_err("whitespace makes it a different region");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("surrounding whitespace"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn the_band_is_built_around_whatever_target_the_centre_distributed() {
        // The split this whole module exists for: the width is local and the
        // centre moves only the centre of the band.
        let band = discipline().band_around(d("100")).expect("a band");
        assert_eq!(band.target(), d("100"));
        assert_eq!(band.soft(), d("5"));
        assert_eq!(band.hard(), d("20"));
    }
}
