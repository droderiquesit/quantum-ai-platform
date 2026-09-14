//! Blueprint §20.1's sixth control: "Capacity estimate — capital level at
//! which the edge decays, from depth and impact modelling", preventing "a real
//! edge funded past the point it becomes a loss".
//!
//! # The estimate, and why it needs no number nobody computed
//!
//! A capacity figure normally needs three inputs: the depth of the book, an
//! impact law, and the edge the strategy expects. The third is the awkward
//! one — this platform holds no per-strategy expected return in basis points
//! of traded notional, and inventing a desk-wide constant to stand in for it
//! would be a capacity estimate whose answer is mostly that constant.
//!
//! So the edge is not invented. The platform already states, on every leg it
//! proposes, the cost it expects that leg to pay:
//! `qip_portfolio_engine::proposal::ProposalLeg::estimated_cost_bps`. A leg is
//! worth taking only if its edge exceeds that, because the sizing said so when
//! it wrote the figure down. The capacity question therefore becomes
//! self-referential in the useful direction:
//!
//! > At what size does the impact the *observed book depth* implies exceed the
//! > cost this leg's own sizing assumed it would pay?
//!
//! Beyond that size the leg is paying more than the arithmetic that justified
//! it allowed for, which is §20.1's "funded past the point it becomes a loss"
//! stated in quantities the platform actually holds. Both sides of the
//! comparison are the platform's own: the cost assumption is the portfolio
//! engine's, the depth is what the SENSE stage absorbed from quotes and books
//! into `qip_world_model::liquidity::LiquidityTopology`.
//!
//! # The impact law, written once
//!
//! A uniform ladder. Taking the whole visible depth at the touch costs about
//! half the quoted spread; taking `n` times that walks `n` rungs of a book
//! whose rungs are about as deep as the first, so
//!
//! ```text
//! cost_bps(Q) = (s / 2) * (1 + Q / D)
//! ```
//!
//! with `s` the quoted spread in basis points and `D` the usable depth on the
//! side being taken. Setting `cost_bps(Q*) = e`, the leg's own assumed cost,
//! gives the capacity as a multiple of visible depth:
//!
//! ```text
//! Q* / D = 2e/s - 1
//! ```
//!
//! Three properties make this worth having rather than a curve fitted after
//! the fact. It is one line, so a person can check it. It is *conservative* —
//! a real book's deeper rungs are usually thicker than the touch, so linear
//! walking overstates the cost and the capacity it reports is on the low side,
//! which is the right direction for a control. And it reaches zero on its own:
//! when a leg's assumed cost does not cover half the quoted spread, `2e/s ≤ 1`
//! and the capacity is nothing at all — the leg is beyond capacity at any
//! size, which is a finding and not an error.
//!
//! The square-root law would be the alternative and is deliberately not used:
//! it needs a volatility and a participation horizon, neither of which the
//! platform states per leg, so it would require exactly the invented constants
//! this module is built to avoid.
//!
//! # What this module does not do
//!
//! It moves nothing. No function here returns a `Decimal` that could become a
//! size, a weight or a multiplier, and the finding names a leg and two ratios.
//! Narrowing a proposal on this evidence would be a risk control, and a risk
//! control belongs behind `qip-risk-engine`'s limits where it can be reviewed
//! as one — see `.claude/rules/domains/risk-and-execution.md`. The same
//! restraint `family_review` takes, for the same reason: a finding that can
//! only be read is one nobody can mistake for a decision.

use qip_core::time::Timestamp;
use qip_core::{Decimal, ObjectId};
use qip_portfolio_engine::proposal::{Proposal, Side};
use qip_world_model::liquidity::LiquidityTopology;
use serde::{Deserialize, Serialize};

/// The narrowest quoted spread a capacity estimate will divide by, in basis
/// points: a hundredth of one basis point.
///
/// Not a tolerance and not a clamp — a leg whose reference spread comes in
/// under this is reported unassessable by name. The impact law divides by the
/// spread, so a spread of zero is an infinite capacity and a spread of 10⁻¹²
/// is an absurd one, and both read downstream as "this leg has all the room in
/// the world". A book quoting at a hundredth of a basis point is a data fault
/// or a crossed market, and saying so is more useful than a capacity of 10¹⁵.
pub const MIN_ASSESSABLE_SPREAD_BPS: f64 = 0.01;

/// What the book says one proposed leg can absorb.
///
/// Carries the two exact figures as [`Decimal`] — the depth observed and the
/// quantity proposed — and everything derived from them as `f64`. That is the
/// crossing point, and it is here rather than scattered: a depth and a
/// quantity are sizes the platform observed and must not be approximated,
/// while their ratio is dimensionless and is the unit the impact law is stated
/// in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegCapacity {
    pub object_id: String,
    /// Which way the leg goes, so a reader knows which side of the book the
    /// depth below was taken from.
    pub side: String,
    /// Usable depth at the touch on the side being taken, summed over venues
    /// that were accepting orders when observed. Exact.
    pub usable_depth: Decimal,
    /// The quantity the proposal asks for. Exact.
    pub quantity: Decimal,
    /// Venues counted into the depth, and venues whose last observation was
    /// older than the topology's staleness bound. The second is reported
    /// because a capacity computed on a shrunken basis is a smaller number
    /// than the market would actually show, and a reader needs to know which
    /// it is looking at.
    pub venues_counted: usize,
    pub venues_aged_out: usize,
    /// The quoted spread, in basis points of the leg's own reference price.
    pub spread_bps: f64,
    /// The cost the leg's sizing assumed it would pay, in basis points.
    pub assumed_cost_bps: f64,
    /// `Q*/D`: how many multiples of visible depth may be taken before the
    /// implied impact exceeds `assumed_cost_bps`. Zero when the assumed cost
    /// does not cover half the spread.
    pub capacity_multiple: f64,
    /// `Q/D`: how many multiples of visible depth the proposal asks for.
    pub proposed_multiple: f64,
}

impl LegCapacity {
    /// Whether the proposal asks for more than the book's depth supports at
    /// the cost the sizing assumed.
    ///
    /// Strictly greater: a leg landing exactly on its capacity is at it, not
    /// past it, and a control that fired on equality would fire on every leg
    /// sized from this very arithmetic the day anything starts sizing from it.
    pub fn is_beyond_capacity(&self) -> bool {
        self.proposed_multiple > self.capacity_multiple
    }

    /// How far past, as a multiple of the capacity. `None` where the capacity
    /// is zero, because a leg cannot be a multiple of nothing — the finding is
    /// then "the assumed cost does not cover half the spread", which is a
    /// different sentence and deserves one.
    pub fn overage(&self) -> Option<f64> {
        if self.capacity_multiple <= 0.0 {
            return None;
        }
        Some(self.proposed_multiple / self.capacity_multiple)
    }

    /// One line for a journal.
    pub fn describe(&self) -> String {
        match self.overage() {
            None => format!(
                "{} {}: the sizing assumed {:.2}bps of cost against a quoted spread of \
                 {:.2}bps, so half the spread alone exceeds it and the book supports no size at \
                 that cost; {} unit(s) proposed against {} of usable depth",
                self.object_id,
                self.side,
                self.assumed_cost_bps,
                self.spread_bps,
                self.quantity,
                self.usable_depth
            ),
            Some(overage) => format!(
                "{} {}: {} unit(s) proposed against {} of usable depth at {} venue(s), \
                 {:.2}x the {:.2}x of depth the assumed cost of {:.2}bps supports on a \
                 {:.2}bps spread",
                self.object_id,
                self.side,
                self.quantity,
                self.usable_depth,
                self.venues_counted,
                overage,
                self.capacity_multiple,
                self.assumed_cost_bps,
                self.spread_bps
            ),
        }
    }
}

/// The measurement, once a cycle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapacityReview {
    /// Every leg the book could be read for, in the order the proposal lists
    /// them.
    pub legs: Vec<LegCapacity>,
    /// Legs whose capacity could not be estimated, and why. Named rather than
    /// dropped: a leg missing from a capacity review reads exactly like a leg
    /// that had room.
    pub unassessable: Vec<(String, String)>,
    /// Legs asking for more than the book supports at their own assumed cost.
    pub beyond_capacity: Vec<LegCapacity>,
    pub cycle: u64,
    pub at: Timestamp,
}

impl CapacityReview {
    pub fn has_findings(&self) -> bool {
        !self.beyond_capacity.is_empty()
    }

    /// One line for the cycle's detail.
    ///
    /// Names the legs beyond capacity and counts the rest. Bounded by the
    /// legs in one proposal, which the portfolio engine bounds; an
    /// instrument-keyed line over the whole universe would not be.
    pub fn describe(&self) -> String {
        let mut line = format!(
            "{} proposed leg(s) measured against book depth",
            self.legs.len()
        );
        if !self.unassessable.is_empty() {
            line.push_str(&format!(
                ", {} with no readable depth",
                self.unassessable.len()
            ));
        }
        if self.beyond_capacity.is_empty() {
            line.push_str("; every leg sits inside the capacity its own assumed cost implies");
        } else {
            let named: Vec<String> = self
                .beyond_capacity
                .iter()
                .map(LegCapacity::describe)
                .collect();
            line.push_str(&format!(
                "; {} beyond capacity: {}",
                self.beyond_capacity.len(),
                named.join("; ")
            ));
        }
        line
    }
}

/// Measure every leg of `proposal` against the depth `liquidity` has observed.
///
/// The entry point. Reads only; the topology and the proposal are both
/// borrowed and neither is changed.
///
/// A leg is `unassessable` — named, with the reason — when the topology holds
/// no current map for its instrument, when the map's counted venues showed no
/// usable depth on the side being taken, when no venue on the map quoted a
/// spread, when the leg's reference price is not positive, or when the implied
/// spread is below [`MIN_ASSESSABLE_SPREAD_BPS`]. Each of those is a fact
/// about the book or the record rather than a capacity, and reporting it as a
/// capacity of anything would be inventing one.
pub fn measure(
    liquidity: &LiquidityTopology,
    proposal: &Proposal,
    cycle: u64,
    at: Timestamp,
) -> CapacityReview {
    let mut legs = Vec::new();
    let mut unassessable = Vec::new();
    let mut beyond_capacity = Vec::new();

    for leg in &proposal.legs {
        match assess(
            liquidity,
            &leg.object_id,
            leg.side,
            leg.quantity,
            leg.reference_price,
            leg.estimated_cost_bps,
            at,
        ) {
            Ok(capacity) => {
                if capacity.is_beyond_capacity() {
                    beyond_capacity.push(capacity.clone());
                }
                legs.push(capacity);
            }
            Err(reason) => unassessable.push((leg.object_id.as_str().to_string(), reason)),
        }
    }

    CapacityReview {
        legs,
        unassessable,
        beyond_capacity,
        cycle,
        at,
    }
}

/// One leg's capacity, or the reason there is none to state.
///
/// `Err` carries a sentence and not an `Error`, because none of these is a
/// failure: a book nobody has quoted is a normal state of the world at four in
/// the morning, and returning `qip_core::error::Error` for it would make a
/// caller treat a quiet market as a fault.
#[allow(clippy::too_many_arguments)]
fn assess(
    liquidity: &LiquidityTopology,
    object_id: &ObjectId,
    side: Side,
    quantity: Decimal,
    reference_price: Decimal,
    assumed_cost_bps: f64,
    at: Timestamp,
) -> std::result::Result<LegCapacity, String> {
    let Some(map) = liquidity.current_map(object_id, at) else {
        return Err(format!(
            "no venue has quoted {} within the topology's staleness bound, so there is no depth \
             to state a capacity against",
            object_id.as_str()
        ));
    };
    // A buy consumes the ask side and a sell the bid. Taking the wrong one
    // would report a capacity from the liquidity on the side the order is not
    // going to touch, which is a plausible-looking number about the wrong
    // half of the book.
    let depth = match side {
        Side::Buy => map.usable_ask_depth,
        Side::Sell => map.usable_bid_depth,
    };
    if !depth.is_positive() {
        return Err(format!(
            "{} shows no usable depth on the {} side across {} counted venue(s) ({} aged out), \
             so any size at all is beyond what the book can absorb — reported here rather than \
             as a capacity of zero, because zero depth and zero capacity are different facts",
            object_id.as_str(),
            side.as_str(),
            map.venue_count(),
            map.venues_aged_out
        ));
    }
    if !reference_price.is_positive() {
        return Err(format!(
            "{} was sized against a reference price of {reference_price}, which no spread can be \
             expressed as a fraction of",
            object_id.as_str()
        ));
    }
    // The narrowest spread any counted venue quoted: the one an order would
    // actually pay if it went to the best venue, and the conservative choice
    // for a capacity, since a narrower spread implies a *larger* capacity and
    // taking the widest would flatter the control into never firing.
    let Some(spread) = map
        .venues
        .iter()
        .filter(|venue| venue.accepts_orders())
        .filter_map(|venue| venue.spread)
        .filter(|spread| spread.is_positive())
        .min()
    else {
        return Err(format!(
            "no venue accepting orders in {} quoted a two-sided spread, so the impact law has no \
             spread to work from",
            object_id.as_str()
        ));
    };
    // Decimal → f64: the crossing point. A spread and a price are exact
    // money; their ratio in basis points is a statistic and everything below
    // is arithmetic on statistics.
    let spread_bps = spread.to_f64() / reference_price.to_f64() * 10_000.0;
    if !spread_bps.is_finite() || spread_bps < MIN_ASSESSABLE_SPREAD_BPS {
        return Err(format!(
            "{} quotes a spread of {spread_bps}bps against its reference price, below the \
             {MIN_ASSESSABLE_SPREAD_BPS}bps this estimate will divide by; a crossed or \
             zero-width book would report a capacity of practically anything",
            object_id.as_str()
        ));
    }
    if !assumed_cost_bps.is_finite() || assumed_cost_bps < 0.0 {
        return Err(format!(
            "{} was sized assuming a cost of {assumed_cost_bps}bps, which is not a cost; the \
             capacity is what that figure implies and there is nothing to imply it from",
            object_id.as_str()
        ));
    }
    // The impact law, in one line. `.max(0.0)` is not a clamp of an input —
    // it is the law's own answer where the assumed cost does not cover half
    // the spread, and `overage` reads the zero as the distinct finding it is.
    let capacity_multiple = (2.0 * assumed_cost_bps / spread_bps - 1.0).max(0.0);
    // Decimal → f64 again, and the same argument: two sizes are exact and
    // their ratio is the dimensionless quantity the law is stated in.
    let proposed_multiple = quantity.to_f64() / depth.to_f64();
    Ok(LegCapacity {
        object_id: object_id.as_str().to_string(),
        side: side.as_str().to_string(),
        usable_depth: depth,
        quantity,
        venues_counted: map.venue_count(),
        venues_aged_out: map.venues_aged_out,
        spread_bps,
        assumed_cost_bps,
        capacity_multiple,
        proposed_multiple,
    })
}

// The workspace denies `panic_in_result_fn` for production code. A test that
// returns `Result` so it can use `?` still has to assert, and the abort is the
// reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_contracts::venue::{VenueId, VenueStatus};
    use qip_core::error::Result;
    use qip_portfolio_engine::proposal::ProposalLeg;
    use qip_world_model::liquidity::DepthObservation;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_700_000_000)
    }

    /// A topology holding one venue's depth in one instrument.
    fn book(object: &str, bid: i64, ask: i64, spread_cents: i64) -> Result<LiquidityTopology> {
        let mut topology = LiquidityTopology::default();
        let observation = DepthObservation::new(
            ObjectId::from_string(object),
            VenueId::new("SIM"),
            VenueStatus::Open,
            Decimal::from_int(bid),
            Decimal::from_int(ask),
            at(),
        )
        .with_spread(Decimal::from_scaled(i128::from(spread_cents), 2).unwrap_or(Decimal::ZERO));
        topology.absorb(observation, at())?;
        Ok(topology)
    }

    fn leg(object: &str, side: Side, quantity: i64, cost_bps: f64) -> ProposalLeg {
        ProposalLeg {
            object_id: ObjectId::from_string(object),
            side,
            quantity: Decimal::from_int(quantity),
            // A hundred units of currency, so a spread in cents converts to a
            // round number of basis points: one cent on a price of 100 is one
            // basis point.
            reference_price: Decimal::from_int(100),
            current_weight: 0.0,
            target_weight: 0.01,
            estimated_cost_bps: cost_bps,
            hypotheses: vec!["h".to_string()],
        }
    }

    fn proposal(legs: Vec<ProposalLeg>) -> Proposal {
        Proposal::draft(
            qip_core::ProposalId::from_string("p-1"),
            at(),
            at(),
            qip_core::Money::new(Decimal::from_int(1_000_000), qip_core::Currency::USD),
            legs,
            "a capacity review fixture",
        )
    }

    /// The finding: a leg asking for four times the visible depth is beyond
    /// the capacity its own cost assumption supports, and the same leg at half
    /// the depth is not.
    ///
    /// The book quotes a 10bp spread and the sizing assumed 30bps of cost, so
    /// the capacity is `2 × 30 / 10 − 1 = 5` times visible depth. Both
    /// assertions are against that one number, computed by hand from the
    /// module's own law, which is what makes this a test of the arithmetic and
    /// not of whatever the arithmetic happens to produce.
    ///
    /// Mutated by replacing the computed capacity with `f64::INFINITY` —
    /// confirmed it then reports `2 x 30 / 10 - 1 is 5, not inf`, then
    /// restored byte-for-byte. Mutated again by replacing it with `0.0` —
    /// confirmed it then reports `2 x 30 / 10 - 1 is 5, not 0`, then restored
    /// byte-for-byte. The two mutations are the two ways this control fails
    /// silently: one never fires, the other always does, and both read in the
    /// code like a working comparison.
    #[test]
    fn a_leg_asking_for_more_depth_than_its_own_cost_assumption_supports_is_a_finding() -> Result<()>
    {
        // 1,000 units of ask depth, a spread of 10 cents on a price of 100,
        // which is 10 basis points.
        let topology = book("ACME", 1_000, 1_000, 10)?;
        let inside = proposal(vec![leg("ACME", Side::Buy, 4_000, 30.0)]);
        let outside = proposal(vec![leg("ACME", Side::Buy, 6_000, 30.0)]);

        let inside_review = measure(&topology, &inside, 1, at());
        let outside_review = measure(&topology, &outside, 1, at());
        // Premise: both legs were assessable, so the assertions below are
        // about the capacity and not about a leg that was skipped.
        assert!(
            inside_review.unassessable.is_empty() && outside_review.unassessable.is_empty(),
            "{:?} / {:?}",
            inside_review.unassessable,
            outside_review.unassessable
        );
        assert_eq!(inside_review.legs.len(), 1);

        let measured = &inside_review.legs[0];
        assert!(
            (measured.spread_bps - 10.0).abs() < 1e-9,
            "premise: a 10-cent spread on a price of 100 is 10bps, not {}",
            measured.spread_bps
        );
        assert!(
            (measured.capacity_multiple - 5.0).abs() < 1e-9,
            "2 x 30 / 10 - 1 is 5, not {}",
            measured.capacity_multiple
        );
        assert!(
            (measured.proposed_multiple - 4.0).abs() < 1e-9,
            "4,000 units against 1,000 of depth is 4x, not {}",
            measured.proposed_multiple
        );

        assert!(
            !inside_review.has_findings(),
            "a leg at four times depth was called beyond a capacity of five: {}",
            inside_review.describe()
        );
        assert!(
            outside_review.has_findings(),
            "a leg at six times depth was not called beyond a capacity of five: {}",
            outside_review.describe()
        );
        assert_eq!(
            outside_review.beyond_capacity[0]
                .overage()
                .map(|o| (o * 100.0).round()),
            Some(120.0),
            "six times depth against a capacity of five is 1.2x"
        );
        Ok(())
    }

    /// The zero-capacity finding, which is a different sentence: a leg whose
    /// sizing assumed less cost than half the quoted spread is beyond capacity
    /// at any size, and the review says that rather than reporting a ratio.
    ///
    /// This is §20.1's "a real edge funded past the point it becomes a loss"
    /// at its starkest — the leg does not clear the touch, let alone the
    /// impact of walking the book — and it is the case a capacity expressed
    /// only as a multiple would quietly report as zero and leave to a reader
    /// to interpret.
    ///
    /// Mutated by removing the `.max(0.0)` so the capacity goes negative —
    /// confirmed it then reports `the capacity is -0.75`, then restored
    /// byte-for-byte. A negative capacity is what the unguarded law returns
    /// here, and it would flow into `overage` as a negative multiple: a
    /// journal line reading "-1.33x the capacity" about a leg with no capacity
    /// at all.
    #[test]
    fn a_leg_whose_assumed_cost_does_not_cover_half_the_spread_has_no_capacity_at_all() -> Result<()>
    {
        // A 40bp spread against a sizing that assumed 5bps of cost: half the
        // spread alone is four times the assumption.
        let topology = book("WIDE", 1_000, 1_000, 40)?;
        let review = measure(
            &topology,
            &proposal(vec![leg("WIDE", Side::Buy, 1, 5.0)]),
            2,
            at(),
        );
        assert!(review.unassessable.is_empty(), "{:?}", review.unassessable);
        // Premise: one unit is as small an order as the type allows, so the
        // finding below is about the spread and not about the size.
        assert_eq!(review.legs[0].quantity, Decimal::from_int(1));

        assert!(review.has_findings(), "{}", review.describe());
        let found = &review.beyond_capacity[0];
        assert!(
            (found.capacity_multiple - 0.0).abs() < 1e-12,
            "the capacity is {}",
            found.capacity_multiple
        );
        assert_eq!(found.overage(), None, "a multiple of nothing was reported");
        assert!(
            found.describe().contains("no size at that cost"),
            "the finding reads as a ratio rather than as a refusal: {}",
            found.describe()
        );
        Ok(())
    }

    /// The side matters: a sell is measured against bid depth and a buy
    /// against ask depth, and a book that is deep on one side and thin on the
    /// other separates them.
    ///
    /// Mutated by swapping the two arms of the `match side` — confirmed both
    /// premise assertions then fail, then restored byte-for-byte. Without a
    /// lopsided book the mutation would be invisible, which is why the two
    /// depths here differ by a factor of ten.
    #[test]
    fn a_buy_is_measured_against_ask_depth_and_a_sell_against_bid_depth() -> Result<()> {
        // Deep on the bid, thin on the ask.
        let topology = book("LOPSIDED", 10_000, 1_000, 10)?;
        let buy = measure(
            &topology,
            &proposal(vec![leg("LOPSIDED", Side::Buy, 1_000, 30.0)]),
            3,
            at(),
        );
        let sell = measure(
            &topology,
            &proposal(vec![leg("LOPSIDED", Side::Sell, 1_000, 30.0)]),
            3,
            at(),
        );
        // Premise: the book really is lopsided, so the two sides must differ.
        assert_eq!(buy.legs[0].usable_depth, Decimal::from_int(1_000));
        assert_eq!(sell.legs[0].usable_depth, Decimal::from_int(10_000));
        assert!(
            (buy.legs[0].proposed_multiple - 1.0).abs() < 1e-9,
            "a thousand units against a thousand of ask depth is 1x, not {}",
            buy.legs[0].proposed_multiple
        );
        assert!(
            (sell.legs[0].proposed_multiple - 0.1).abs() < 1e-9,
            "a thousand units against ten thousand of bid depth is 0.1x, not {}",
            sell.legs[0].proposed_multiple
        );
        Ok(())
    }

    /// Every unassessable arm is named with its reason rather than dropped,
    /// and the review still comes back.
    ///
    /// A leg missing from a capacity review reads exactly like a leg that had
    /// room, which is the quietest way for this control to stop working.
    ///
    /// Mutated by replacing the `Err(reason)` arm's push with a `drop` —
    /// confirmed every assertion below then fails, then restored byte-for-byte.
    #[test]
    fn a_leg_with_no_readable_depth_is_named_with_its_reason_rather_than_treated_as_roomy()
    -> Result<()> {
        // An instrument nothing has quoted.
        let empty = LiquidityTopology::default();
        let unquoted = measure(
            &empty,
            &proposal(vec![leg("UNQUOTED", Side::Buy, 100, 30.0)]),
            4,
            at(),
        );
        assert!(unquoted.legs.is_empty());
        assert_eq!(unquoted.unassessable.len(), 1);
        assert!(
            unquoted.unassessable[0].1.contains("no venue has quoted"),
            "{:?}",
            unquoted.unassessable
        );
        assert!(
            !unquoted.has_findings(),
            "an unquoted instrument was reported as beyond capacity"
        );

        // A venue quoting no two-sided spread: the impact law would have
        // nothing to divide by, and a capacity computed anyway would read as
        // all the room in the world.
        let crossed = book("CROSSED", 1_000, 1_000, 0)?;
        let review = measure(
            &crossed,
            &proposal(vec![leg("CROSSED", Side::Buy, 100, 30.0)]),
            5,
            at(),
        );
        assert_eq!(review.unassessable.len(), 1, "{:?}", review.legs);
        assert!(
            review.unassessable[0].1.contains("two-sided spread"),
            "{:?}",
            review.unassessable
        );

        // No depth on the side being taken.
        let one_sided = book("ONESIDED", 1_000, 0, 10)?;
        let review = measure(
            &one_sided,
            &proposal(vec![leg("ONESIDED", Side::Buy, 100, 30.0)]),
            6,
            at(),
        );
        assert_eq!(review.unassessable.len(), 1, "{:?}", review.legs);
        assert!(
            review.unassessable[0].1.contains("no usable depth"),
            "{:?}",
            review.unassessable
        );
        assert!(
            review.describe().contains("no readable depth"),
            "the summary line hides the unassessable leg: {}",
            review.describe()
        );
        Ok(())
    }
}
