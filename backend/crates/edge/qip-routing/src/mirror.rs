//! Blueprint §31.1's cross-region solve: the direction-gating table that
//! makes two regions' cells unable to trade the same side of one mirrored
//! asset, without either of them talking to the other.
//!
//! # What §31.1 actually asks for, and which half is here
//!
//! The section has three parts. SETUP holds the asset in both regions and
//! distributes a reference price, a threshold and per-instrument targets.
//! EXECUTE has each region compare its own local price against the reference
//! and trade its own side. The table then gates the direction each region is
//! permitted to take from where its own inventory sits.
//!
//! The load-bearing sentence is the last one: *"Direction gating makes
//! same-side trading across regions impossible by construction rather than by
//! coordination."* That is a property of the table alone. If region A's
//! inventory is below target it may only buy, and if region B's is above
//! target it may only sell; neither cell learns anything about the other, and
//! there is no instant at which both are permitted to buy the same asset. The
//! second sentence — *"A stale reference can cost one side's band, never
//! both"* — is why the reference carries a time to live rather than being a
//! number the cell keeps using.
//!
//! This module is that table, that reference, and the gate that composes
//! them. It is inert: it holds no venue, produces no order and has no
//! reference to a gateway. It answers one question — *may this side trade in
//! this direction right now* — and every answer other than yes is a refusal
//! naming what to do instead.
//!
//! # What is deliberately not here
//!
//! * **Any sizing.** §31.1's third row reads "either direction, reduced
//!   size", and this gate has no sizing authority: it never sees a quantity
//!   and cannot narrow one. It reports [`SizeDiscipline::Reduced`] and leaves
//!   the consequence to a caller that can act on it — see
//!   [`crate::extension`], which refuses rather than trading a size the
//!   blueprint says must be smaller and nothing here can make smaller. A
//!   multiplier returned by a type nobody applies would be the control that
//!   reads as protection and cannot fire.
//! * **Finding the dislocation.** [`DistributedReference::indication`] says
//!   which direction the reference *permits*; it is a gate over a decision
//!   some scanner already made, never the search itself. This platform's
//!   arbitrage scanner finds the cycle; asking this module to find one too
//!   would be a second source of truth for a fact the scan already holds.
//! * **The remote side's inventory.** A cell cannot measure it and this
//!   module never claims to. That is the point of the construction: the
//!   remote cell runs its own copy of this table against its own book.

use qip_contracts::message::BookSide;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// Which way a side of a mirror trades.
///
/// Named rather than reusing [`BookSide`] directly, because a book side is
/// the side of somebody else's book and this is a statement about what the
/// platform does. The two are related by exactly one rule, stated once here
/// and nowhere else: **a buy takes the ask and a sell takes the bid.** The
/// arbitrage graph writes a conversion that acquires an asset with
/// `BookSide::Ask` for the same reason, and a mismatch between the two
/// conventions would invert every direction this table gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Direction {
    Buy,
    Sell,
}

impl Direction {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    /// The side of the book this direction consumes.
    pub const fn taking(&self) -> BookSide {
        match self {
            Self::Buy => BookSide::Ask,
            Self::Sell => BookSide::Bid,
        }
    }

    /// The direction that consumes this side of the book.
    pub const fn from_taking(side: BookSide) -> Self {
        match side {
            BookSide::Ask => Self::Buy,
            BookSide::Bid => Self::Sell,
        }
    }

    pub const fn opposite(&self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

/// One of §31.1's four region states, in the table's own order.
///
/// Not `#[non_exhaustive]`, for the reason [`crate::path::ExecutionPath`] is
/// not: a fifth state must break every `match` that dispatches on one rather
/// than falling into a default arm nobody considered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RegionState {
    /// Row 1. Below target and inside the hard band.
    BelowTarget,
    /// Row 2. Above target and inside the hard band.
    AboveTarget,
    /// Row 3. Within the soft band of the target.
    AtTargetInsideBand,
    /// Row 4. Past the hard band on either side.
    ///
    /// Evaluated **first**, so it overrides rows 1 and 2 rather than being
    /// shadowed by them. Rows 1 and 2 are "below" and "above" without
    /// qualification, and read in the table's printed order a holding past
    /// the hard band would match row 1 or row 2 and never reach row 4, which
    /// would make the hard band a limit that cannot fire.
    OutsideHardBand,
}

impl RegionState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::BelowTarget => "below_target",
            Self::AboveTarget => "above_target",
            Self::AtTargetInsideBand => "at_target_inside_band",
            Self::OutsideHardBand => "outside_hard_band",
        }
    }
}

/// §31.1's "permitted direction" column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PermittedDirection {
    /// "May buy only."
    BuyOnly,
    /// "May sell only."
    SellOnly,
    /// "Either direction", which the table pairs with reduced size.
    Either,
}

impl PermittedDirection {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::BuyOnly => "buy_only",
            Self::SellOnly => "sell_only",
            Self::Either => "either",
        }
    }

    /// Whether this permission admits `direction`.
    pub const fn permits(&self, direction: Direction) -> bool {
        match (self, direction) {
            (Self::Either, _)
            | (Self::BuyOnly, Direction::Buy)
            | (Self::SellOnly, Direction::Sell) => true,
            (Self::BuyOnly, Direction::Sell) | (Self::SellOnly, Direction::Buy) => false,
        }
    }
}

/// Whether the row the holding matched permits the pass's own size.
///
/// Two values rather than a multiplier, and the distinction matters: a
/// multiplier is a number somebody has to apply, and this module cannot
/// apply one. [`Self::Reduced`] is a statement that the blueprint requires a
/// smaller size than the one on the table, which a caller either honours or
/// refuses on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SizeDiscipline {
    Full,
    Reduced,
}

impl SizeDiscipline {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Reduced => "reduced",
        }
    }
}

/// The inventory target and the two bands around it, for one mirrored
/// instrument at one region.
///
/// `Decimal` throughout: these are quantities of a held asset, compared
/// against a holding that money is paid for, and a band boundary decided by
/// a float's rounding is a limit that fires or does not depending on which
/// way the last bit went.
///
/// # Why the bands are local configuration and never arrive on the wire
///
/// `qip_contracts::policy::InventoryTargets` carries targets and reference
/// prices and **no band field at all**. That is not an oversight to be
/// filled in: the policy payload travels a wire that authenticates nobody
/// beyond its signature, and a field on it that could *widen* a safety band
/// would move a capital boundary onto that wire. The same argument
/// `FeasibilityConstraints::withdrawn_venues` makes in the other direction —
/// a wire may subtract, never add — applies here. The centre says where a
/// cell should be; the operator's own configuration says how far off it may
/// drift before the direction is forced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryBand {
    target: Decimal,
    soft: Decimal,
    hard: Decimal,
}

impl InventoryBand {
    /// Refuse a band that would make one of §31.1's four rows unreachable.
    ///
    /// `soft` and `hard` are half-widths measured from the target, not
    /// absolute holdings, because the table is symmetric about the target and
    /// a pair of absolute bounds would let a caller write a band the target
    /// sits outside.
    pub fn new(target: Decimal, soft: Decimal, hard: Decimal) -> Result<Self> {
        if target.is_negative() {
            return Err(Error::invalid(format!(
                "an inventory target of {target} is a standing short, and §31.1's mirror is one \
                 asset held in both regions; supply the quantity this region is meant to hold, \
                 or zero if the mirror is meant to run flat"
            )));
        }
        if !soft.is_positive() {
            return Err(Error::invalid(format!(
                "a soft band of {soft} makes the at-target row reachable only by an exactly \
                 equal holding, so every real holding would be read as above or below target; \
                 supply the half-width around the target that counts as at target"
            )));
        }
        if hard <= soft {
            return Err(Error::invalid(format!(
                "a hard band of {hard} is not wider than the soft band of {soft}, so the \
                 outside-hard-band row could only be reached by a holding the above/below rows \
                 already claimed; widen the hard band or narrow the soft one"
            )));
        }
        Ok(Self { target, soft, hard })
    }

    pub const fn target(&self) -> Decimal {
        self.target
    }

    pub const fn soft(&self) -> Decimal {
        self.soft
    }

    pub const fn hard(&self) -> Decimal {
        self.hard
    }

    /// §31.1's table, evaluated against a holding.
    ///
    /// Fallible because the deviation is a subtraction of two quantities a
    /// caller supplies, and a subtraction that overflows would otherwise
    /// wrap into the opposite band — a holding far above the hard band
    /// reading as one far below it, and the direction gate then permitting
    /// exactly the side that made the breach worse.
    pub fn posture(&self, held: Decimal) -> Result<RegionPosture> {
        let deviation = held.checked_sub(self.target).ok_or_else(|| {
            Error::numeric(format!(
                "the deviation of a holding of {held} from a target of {} does not fit a \
                 Decimal; supply the holding this region actually has rather than a sentinel",
                self.target
            ))
        })?;
        let distance = deviation.abs();
        // Row 4 first. See `RegionState::OutsideHardBand` for why the
        // table's printed order is not the evaluation order.
        let (state, permitted, size) = if distance > self.hard {
            let reducing = if deviation.is_positive() {
                PermittedDirection::SellOnly
            } else {
                PermittedDirection::BuyOnly
            };
            // Full size in the reducing direction: the holding is past the
            // bound the operator set, and the fastest way back inside it is
            // not a smaller trade.
            (RegionState::OutsideHardBand, reducing, SizeDiscipline::Full)
        } else if distance <= self.soft {
            (
                RegionState::AtTargetInsideBand,
                PermittedDirection::Either,
                SizeDiscipline::Reduced,
            )
        } else if deviation.is_negative() {
            (
                RegionState::BelowTarget,
                PermittedDirection::BuyOnly,
                SizeDiscipline::Full,
            )
        } else {
            (
                RegionState::AboveTarget,
                PermittedDirection::SellOnly,
                SizeDiscipline::Full,
            )
        };
        Ok(RegionPosture {
            state,
            permitted,
            size,
            deviation,
        })
    }
}

/// Where one region stands against its own band, and what that permits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RegionPosture {
    state: RegionState,
    permitted: PermittedDirection,
    size: SizeDiscipline,
    deviation: Decimal,
}

impl RegionPosture {
    pub const fn state(&self) -> RegionState {
        self.state
    }

    pub const fn permitted(&self) -> PermittedDirection {
        self.permitted
    }

    pub const fn size(&self) -> SizeDiscipline {
        self.size
    }

    /// Holding minus target: positive when the region is long of where it
    /// should be.
    pub const fn deviation(&self) -> Decimal {
        self.deviation
    }
}

/// The reference price §31.1 distributes, with the threshold either side of
/// it and the window it stays usable for.
///
/// The window is the whole of *"a stale reference can cost one side's band,
/// never both"*: a region that keeps trading against a reference the centre
/// stopped republishing is a region taking one side of a spread nobody is
/// taking the other side of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistributedReference {
    price: Decimal,
    threshold: Decimal,
    observed_at: Timestamp,
    ttl: Duration,
}

impl DistributedReference {
    /// Refuse a reference that cannot gate anything.
    pub fn new(
        price: Decimal,
        threshold: Decimal,
        observed_at: Timestamp,
        ttl: Duration,
    ) -> Result<Self> {
        if !price.is_positive() {
            return Err(Error::invalid(format!(
                "a reference price of {price} is not a price; every local price would read as \
                 above it by more than any threshold, so one side would be permitted to sell \
                 for ever — supply the reference the centre distributed"
            )));
        }
        if !threshold.is_positive() {
            return Err(Error::invalid(format!(
                "a dislocation threshold of {threshold} admits any local price that is not \
                 exactly the reference, which is almost every price and so gates nothing; \
                 supply the distance from the reference that counts as a dislocation"
            )));
        }
        if ttl.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "a reference time to live of {} ns has expired before it is read; supply the \
                 window the reference stays usable for, because a reference with none is one a \
                 region keeps trading against after the centre stops publishing it",
                ttl.as_nanos()
            )));
        }
        Ok(Self {
            price,
            threshold,
            observed_at,
            ttl,
        })
    }

    pub const fn price(&self) -> Decimal {
        self.price
    }

    pub const fn threshold(&self) -> Decimal {
        self.threshold
    }

    pub const fn observed_at(&self) -> Timestamp {
        self.observed_at
    }

    pub const fn ttl(&self) -> Duration {
        self.ttl
    }

    /// How old the reference is at `now`, or a refusal.
    ///
    /// A reference dated after the instant it is read is refused rather than
    /// treated as brand new. Two clocks disagreeing is the one case where
    /// "age zero" is the most dangerous possible answer: it makes a
    /// reference that will never expire, and the freshness check below is
    /// the only thing standing between a cell and trading one side of a
    /// spread for ever.
    pub fn age(&self, now: Timestamp) -> Result<Duration> {
        if now.as_nanos() < self.observed_at.as_nanos() {
            return Err(Error::invalid(format!(
                "the reference is dated {} and is being read at {}, which is earlier; the \
                 centre's clock and this cell's disagree, and an age computed from them would \
                 make the reference outlast its own window",
                self.observed_at.as_nanos(),
                now.as_nanos()
            )));
        }
        Ok(now.since(self.observed_at))
    }

    /// Whether the reference is still inside its window — §33.1's
    /// "reference inside TTL", for path 3.
    pub fn is_fresh(&self, now: Timestamp) -> Result<bool> {
        Ok(self.age(now)?.as_nanos() < self.ttl.as_nanos())
    }

    /// §31.1's EXECUTE rule: the one direction this region is permitted to
    /// take by the reference, given its own local price.
    ///
    /// *"Region A: local price below R by > threshold -> BUYS locally.
    /// Region B: local price above R by > threshold -> SELLS locally."*
    /// Strictly beyond the threshold on either side; a local price inside it
    /// is not a dislocation and is refused as one, which is the silence
    /// §33.1 requires to be logged rather than dropped.
    pub fn indication(&self, local_price: Decimal, now: Timestamp) -> Result<Direction> {
        let age = self.age(now)?;
        if age.as_nanos() >= self.ttl.as_nanos() {
            return Err(Error::denied(format!(
                "the distributed reference is {} ms old and its window is {} ms; a region \
                 trading against a reference the centre stopped republishing takes one side of \
                 a spread nobody takes the other side of, so wait for the next reference",
                age.as_millis(),
                self.ttl.as_millis()
            )));
        }
        if !local_price.is_positive() {
            return Err(Error::invalid(format!(
                "a local price of {local_price} is not a price; supply the price this region's \
                 own book is showing rather than a sentinel, because the direction is decided \
                 by comparing it against the reference"
            )));
        }
        let gap = local_price.checked_sub(self.price).ok_or_else(|| {
            Error::numeric(format!(
                "the gap between a local price of {local_price} and a reference of {} does not \
                 fit a Decimal",
                self.price
            ))
        })?;
        if gap.abs() <= self.threshold {
            return Err(Error::denied(format!(
                "the local price {local_price} is within {} of the reference {}, and §31.1 \
                 trades only beyond the threshold; there is no dislocation to take and this \
                 region takes neither side",
                self.threshold, self.price
            )));
        }
        Ok(if gap.is_negative() {
            Direction::Buy
        } else {
            Direction::Sell
        })
    }
}

/// What the gate allowed, and on what evidence.
///
/// Private fields and no `Deserialize`: this is a record of a decision, and a
/// decoder would be a second way to make one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MirrorPermission {
    direction: Direction,
    posture: RegionPosture,
}

impl MirrorPermission {
    /// The direction this region may take, which is both the one the
    /// reference indicates and the one its band permits.
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    pub const fn posture(&self) -> RegionPosture {
        self.posture
    }

    /// The sentence an operator reads beside §31.1's table.
    pub fn rationale(&self) -> String {
        format!(
            "region state {} permits {} and the distributed reference indicates {}; size \
             discipline {}",
            self.posture.state().as_str(),
            self.posture.permitted().as_str(),
            self.direction.as_str(),
            self.posture.size().as_str()
        )
    }
}

/// §31.1's gate: may this region trade `intended` in this mirrored asset,
/// right now.
///
/// Three refusals, and each is a different thing gone wrong:
///
/// * the reference is stale, or the local price is inside the threshold —
///   both from [`DistributedReference::indication`];
/// * the reference indicates the *other* direction, which is the construction
///   that makes same-side trading across regions impossible: a region whose
///   local price is above the reference may only sell, whatever it wanted;
/// * the band forbids the direction, which is the table.
///
/// It never returns a nearest permission and never widens one. `intended` is
/// what the caller's own cycle would do; this says yes or refuses.
pub fn direction_gate(
    band: &InventoryBand,
    held: Decimal,
    reference: &DistributedReference,
    local_price: Decimal,
    intended: Direction,
    now: Timestamp,
) -> Result<MirrorPermission> {
    let indicated = reference.indication(local_price, now)?;
    if indicated != intended {
        return Err(Error::denied(format!(
            "this region would {} and its local price of {local_price} against the reference {} \
             permits only {}; §31.1 gates the direction from the reference so that two regions \
             can never take the same side of one mirrored asset, and reversing this cycle's \
             direction to fit is not the same cycle",
            intended.as_str(),
            reference.price(),
            indicated.as_str()
        )));
    }
    let posture = band.posture(held)?;
    if !posture.permitted().permits(intended) {
        return Err(Error::denied(format!(
            "this region holds {held} against a target of {} — {} — and may {}, so it may not \
             {}; rebalance the mirror or take the cycle at the region whose band permits it",
            band.target(),
            posture.state().as_str(),
            posture.permitted().as_str(),
            intended.as_str()
        )));
    }
    Ok(MirrorPermission {
        direction: intended,
        posture,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(literal: &str) -> Decimal {
        Decimal::parse(literal).expect("a test decimal parses")
    }

    fn band() -> InventoryBand {
        InventoryBand::new(d("100"), d("5"), d("20")).expect("a valid band")
    }

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn reference() -> DistributedReference {
        DistributedReference::new(d("50000"), d("100"), t(1_000), Duration::from_secs(60))
            .expect("a valid reference")
    }

    #[test]
    fn a_region_below_its_target_may_buy_and_may_not_sell() {
        let posture = band().posture(d("90")).expect("a posture");
        // Premise: the holding really is below the soft band and inside the
        // hard one, or this would be testing row 3 or row 4.
        assert_eq!(posture.deviation(), d("-10"));
        assert_eq!(posture.state(), RegionState::BelowTarget);
        assert_eq!(posture.permitted(), PermittedDirection::BuyOnly);
        assert!(posture.permitted().permits(Direction::Buy));
        assert!(!posture.permitted().permits(Direction::Sell));
        assert_eq!(posture.size(), SizeDiscipline::Full);
    }

    #[test]
    fn a_region_above_its_target_may_sell_and_may_not_buy() {
        let posture = band().posture(d("110")).expect("a posture");
        assert_eq!(posture.deviation(), d("10"));
        assert_eq!(posture.state(), RegionState::AboveTarget);
        assert_eq!(posture.permitted(), PermittedDirection::SellOnly);
        assert!(posture.permitted().permits(Direction::Sell));
        assert!(!posture.permitted().permits(Direction::Buy));
    }

    #[test]
    fn two_regions_on_opposite_sides_of_their_targets_can_never_take_the_same_side() {
        // §31.1's whole claim, asserted as a property rather than as prose:
        // whatever holdings the two regions have, there is no direction both
        // are permitted to take at full size. The failure this prevents is a
        // pair of cells that each read the dislocation the same way and both
        // buy, leaving the platform long in two regions and hedged in
        // neither — which is what "impossible by construction rather than by
        // coordination" is meant to rule out.
        let band = band();
        let holdings = ["70", "79", "81", "95", "100", "105", "119", "121", "140"];
        let mut compared = 0_usize;
        for a in holdings {
            for b in holdings {
                let (pa, pb) = (
                    band.posture(d(a)).expect("a posture"),
                    band.posture(d(b)).expect("a posture"),
                );
                // Only the rows that trade at full size make the structural
                // claim; row 3 is "either direction, reduced size" and is
                // exactly the case the caller must refuse or shrink.
                if pa.size() != SizeDiscipline::Full || pb.size() != SizeDiscipline::Full {
                    continue;
                }
                compared += 1;
                for direction in [Direction::Buy, Direction::Sell] {
                    assert!(
                        !(pa.permitted().permits(direction) && pb.permitted().permits(direction))
                            || pa.permitted() == pb.permitted(),
                        "holdings {a} and {b} both permit {} at full size from different \
                         permissions",
                        direction.as_str()
                    );
                }
            }
        }
        // Premise: the loop above really did compare full-size pairs. Without
        // this the test passes when every pair was skipped.
        assert!(compared >= 36, "only {compared} full-size pairs compared");
    }

    #[test]
    fn a_holding_past_the_hard_band_is_read_as_a_breach_and_not_as_merely_above_target() {
        // The failure this prevents is the table read in its printed order:
        // rows 1 and 2 say "below target" and "above target" with no upper
        // bound, so a holding far past the hard band matches row 2 first and
        // row 4 never fires. A hard band that cannot fire is the
        // `MaxExpectedShortfall` shape.
        let band = band();
        let inside = band.posture(d("119")).expect("a posture");
        assert_eq!(inside.state(), RegionState::AboveTarget);
        let breached = band.posture(d("121")).expect("a posture");
        assert_eq!(breached.state(), RegionState::OutsideHardBand);
        assert_eq!(breached.permitted(), PermittedDirection::SellOnly);

        let breached_low = band.posture(d("79")).expect("a posture");
        assert_eq!(breached_low.state(), RegionState::OutsideHardBand);
        assert_eq!(
            breached_low.permitted(),
            PermittedDirection::BuyOnly,
            "past the hard band on the low side, the reducing direction is to buy back toward \
             target"
        );
    }

    #[test]
    fn a_holding_exactly_on_a_band_edge_is_inside_that_band() {
        // Both boundaries stated once, because a band whose edges move
        // between readings is a limit nobody can reproduce.
        let band = band();
        assert_eq!(
            band.posture(d("105")).expect("a posture").state(),
            RegionState::AtTargetInsideBand,
            "the soft edge is inside the soft band"
        );
        assert_eq!(
            band.posture(d("120")).expect("a posture").state(),
            RegionState::AboveTarget,
            "the hard edge is not yet a breach"
        );
    }

    #[test]
    fn a_region_at_target_may_go_either_way_and_is_told_the_size_must_be_reduced() {
        let posture = band().posture(d("102")).expect("a posture");
        assert_eq!(posture.state(), RegionState::AtTargetInsideBand);
        assert_eq!(posture.permitted(), PermittedDirection::Either);
        assert!(posture.permitted().permits(Direction::Buy));
        assert!(posture.permitted().permits(Direction::Sell));
        assert_eq!(
            posture.size(),
            SizeDiscipline::Reduced,
            "§31.1's third row is 'either direction, reduced size', and dropping the second \
             half makes the at-target row the most permissive of the four"
        );
    }

    #[test]
    fn a_band_whose_hard_edge_is_no_wider_than_its_soft_edge_is_refused() {
        let refusal = InventoryBand::new(d("100"), d("10"), d("10"))
            .expect_err("a hard band inside the soft band can never be reached");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal
                .message()
                .contains("is not wider than the soft band"),
            "the refusal should say why: {}",
            refusal.message()
        );
        // The half that proves the gate admits a good value.
        assert!(InventoryBand::new(d("100"), d("10"), d("11")).is_ok());
    }

    #[test]
    fn a_negative_inventory_target_is_refused_because_a_mirror_is_an_asset_held() {
        // §31.1's SETUP is "hold asset X in BOTH regions". A target below
        // zero is a standing short, and admitting one would let a region
        // holding nothing read as above target — which is the one state that
        // permits it to sell, so the band would license exactly the trade a
        // region with no inventory cannot make.
        let refusal = InventoryBand::new(d("-1"), d("5"), d("20"))
            .expect_err("a target below zero is a standing short");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("is a standing short"),
            "the refusal should say why: {}",
            refusal.message()
        );
        // Zero is admitted, because a mirror meant to run flat is a real
        // arrangement and this gate must not refuse it.
        assert!(InventoryBand::new(Decimal::ZERO, d("5"), d("20")).is_ok());
    }

    #[test]
    fn a_soft_band_of_zero_is_refused_rather_than_making_the_at_target_row_unreachable() {
        let refusal = InventoryBand::new(d("100"), Decimal::ZERO, d("20"))
            .expect_err("a zero soft band leaves no at-target row");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("makes the at-target row"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_local_price_below_the_reference_by_more_than_the_threshold_indicates_a_buy() {
        let reference = reference();
        // Premise: the gap really is beyond the threshold.
        assert_eq!(
            reference
                .indication(d("49899"), t(1_010))
                .expect("indicated"),
            Direction::Buy
        );
        assert_eq!(
            reference
                .indication(d("50101"), t(1_010))
                .expect("indicated"),
            Direction::Sell
        );
    }

    #[test]
    fn a_local_price_inside_the_threshold_is_refused_as_no_dislocation_rather_than_guessed() {
        let reference = reference();
        // Exactly on the threshold is inside it: §31.1 says "by > threshold".
        let refusal = reference
            .indication(d("50100"), t(1_010))
            .expect_err("a price exactly at the threshold is not beyond it");
        assert_eq!(refusal.code(), "denied");
        assert!(
            refusal
                .message()
                .contains("there is no dislocation to take"),
            "the refusal should name the silence: {}",
            refusal.message()
        );
        // And one tick further out is a dislocation, so the gate is not
        // simply refusing everything.
        assert!(reference.indication(d("50101"), t(1_010)).is_ok());
    }

    #[test]
    fn a_reference_older_than_its_window_indicates_nothing() {
        let reference = reference();
        // Premise: the same price inside the window does indicate.
        assert!(reference.indication(d("49000"), t(1_059)).is_ok());
        let refusal = reference
            .indication(d("49000"), t(1_060))
            .expect_err("a reference at exactly its window has expired");
        assert_eq!(refusal.code(), "denied");
        assert!(
            refusal.message().contains("stopped republishing"),
            "the refusal should say what a stale reference costs: {}",
            refusal.message()
        );
        assert!(!reference.is_fresh(t(1_060)).expect("a readable age"));
        assert!(reference.is_fresh(t(1_059)).expect("a readable age"));
    }

    #[test]
    fn a_reference_dated_after_the_instant_it_is_read_is_refused_rather_than_read_as_brand_new() {
        let reference = reference();
        let refusal = reference
            .age(t(999))
            .expect_err("a reference from the future is a clock disagreement");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("which is earlier"),
            "the refusal should name the disagreement: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_reference_price_of_zero_is_refused_because_it_would_permit_one_side_for_ever() {
        let refusal =
            DistributedReference::new(Decimal::ZERO, d("100"), t(1_000), Duration::from_secs(60))
                .expect_err("a reference of zero is not a price");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("is not a price"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_threshold_of_zero_is_refused_because_it_would_gate_nothing() {
        let refusal =
            DistributedReference::new(d("50000"), Decimal::ZERO, t(1_000), Duration::from_secs(60))
                .expect_err("a threshold of zero admits every price");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("gates nothing"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn the_gate_refuses_a_cycle_whose_direction_the_reference_does_not_indicate() {
        // The construction: a region whose local price is above the reference
        // may only sell, whatever the cycle wanted. Without this, two regions
        // reading one dislocation the same way could both buy.
        let refusal = direction_gate(
            &band(),
            d("90"),
            &reference(),
            d("50200"),
            Direction::Buy,
            t(1_010),
        )
        .expect_err("the reference indicates a sell");
        assert_eq!(refusal.code(), "denied");
        assert!(
            refusal.message().contains("permits only sell"),
            "the refusal should name the indicated direction: {}",
            refusal.message()
        );
    }

    #[test]
    fn the_gate_refuses_a_direction_the_band_forbids_even_when_the_reference_indicates_it() {
        // The other half: the reference and the band are two independent
        // conditions and §33.1 requires "both, every time". A region above
        // its target sees a cheap local price and still may not buy.
        let permitted = direction_gate(
            &band(),
            d("90"),
            &reference(),
            d("49800"),
            Direction::Buy,
            t(1_010),
        )
        .expect("below target and the reference indicates a buy");
        assert_eq!(permitted.direction(), Direction::Buy);
        assert_eq!(permitted.posture().state(), RegionState::BelowTarget);

        let refusal = direction_gate(
            &band(),
            d("110"),
            &reference(),
            d("49800"),
            Direction::Buy,
            t(1_010),
        )
        .expect_err("above target the band permits only a sell");
        assert_eq!(refusal.code(), "denied");
        assert!(
            refusal
                .message()
                .contains("may sell_only, so it may not buy"),
            "the refusal should name the band's permission: {}",
            refusal.message()
        );
    }

    #[test]
    fn the_gate_refuses_on_a_stale_reference_before_it_reads_the_band() {
        // Ordering matters for the message an operator gets: a region that
        // is also outside its band would otherwise be told to rebalance when
        // the real problem is that the centre has gone quiet.
        let refusal = direction_gate(
            &band(),
            d("200"),
            &reference(),
            d("49800"),
            Direction::Buy,
            t(2_000),
        )
        .expect_err("the reference has expired");
        assert_eq!(refusal.code(), "denied");
        assert!(
            refusal.message().contains("stopped republishing"),
            "a stale reference should be named before the band: {}",
            refusal.message()
        );
    }

    #[test]
    fn the_permission_rationale_names_the_row_the_holding_matched() {
        let permitted = direction_gate(
            &band(),
            d("101"),
            &reference(),
            d("49800"),
            Direction::Buy,
            t(1_010),
        )
        .expect("at target, either direction");
        let rationale = permitted.rationale();
        assert!(
            rationale.contains("region state at_target_inside_band"),
            "the rationale should name the row: {rationale}"
        );
        assert!(
            rationale.contains("size discipline reduced"),
            "the rationale should carry the size discipline: {rationale}"
        );
    }

    #[test]
    fn a_buy_takes_the_ask_and_a_sell_takes_the_bid() {
        // Stated once in this crate and asserted once. An inverted mapping
        // would flip every direction §31.1 gates, and every other test here
        // would still pass because they never touch a book side.
        assert_eq!(Direction::Buy.taking(), BookSide::Ask);
        assert_eq!(Direction::Sell.taking(), BookSide::Bid);
        assert_eq!(Direction::from_taking(BookSide::Ask), Direction::Buy);
        assert_eq!(Direction::from_taking(BookSide::Bid), Direction::Sell);
        assert_eq!(Direction::Buy.opposite(), Direction::Sell);
    }
}
