//! The producer of blueprint §25.3's three levels no instrument record
//! carries: per family, per factor and per causal driver.
//!
//! `qip-risk`'s [`SharedCauseExposure`] is the shape; this is the only thing
//! that fills it, because the sources are here. The causal graph is the world
//! model's, the tape is the platform's own, and the strategy registry is the
//! factory's — none of them reaches `qip-risk`, which is a library with no
//! services beneath it.
//!
//! # What is fed, and what is deliberately not
//!
//! **Per causal driver is fed** from `qip_world_model::CausalGraph`. Every
//! edge whose effect the book actually holds charges that position to a bucket
//! named by the edge's *cause*, at the edge's own
//! [`CausalEdge::transmission`] — its strength discounted by the confidence in
//! the claim, which is the graph's own arithmetic and not a second one
//! invented here. The graph is written every cycle: `Platform::stage_discover`
//! runs `discover_temporal_precedence` over the platform's own price history
//! and claims an edge for every ordered pair that clears
//! `qip_world_model::granger`'s bar.
//!
//! **Per factor is fed** from `qip_risk::market_factor::MarketFactor`, the
//! same estimator `Platform::stress_the_book` already stresses against, at the
//! absolute loading. One factor is published because the platform estimates
//! one; a second bucket would be a name with no number behind it.
//!
//! **Per family is deliberately not fed, and shipping a limit for it would be
//! the defect this module exists to close.** A family bucket needs each
//! *position* attributed to a family, and two separate things stop that.
//!
//! `qip_risk::RiskAggregates` keeps its per-strategy positions privately and
//! exposes only `strategies()` and `strategy_gross()` through
//! `AggregateFigures`, so no reader can split one instrument's notional across
//! the strategies holding it. And the two strategy keys production ever passes
//! to `apply_fill` are `DESK_STRATEGY`, for the desk's own fills, and a cell's
//! name, for a cell's — neither of which `Platform::strategy_family` can
//! resolve, because it reads the strategy factory's candidate register and
//! that register holds neither. A family bucket today would therefore hold the
//! whole book under one name or nothing at all, and a cap over it would be a
//! second leverage limit or a control that cannot fire. Check rather than
//! believe:
//!
//! ```text
//! grep -rn 'apply_fill(' backend/crates/runtime/qip-kernel/src --include=*.rs
//! grep -n 'fn strategy_family' -A 6 backend/crates/runtime/qip-kernel/src/platform.rs
//! grep -n 'fn strategies\|fn strategy_gross' backend/crates/libs/qip-risk/src/aggregate.rs
//! ```
//!
//! `LimitSet::conservative_default` therefore ships a cap on the two levels
//! that have a producer and none on the third. `qip_risk::shared_cause`
//! carries the family axis constant so that the level has one spelling when
//! something can fill it, and an empty constant is not a control: nothing
//! declares that axis, so it stays absent from `RiskState::axis_exposures`,
//! which is this mechanism's way of saying "no producer ran".
//!
//! # Why the read happens where the state is built
//!
//! `Platform::risk_state_from` is the one place every reader of the risk state
//! goes through — the pre-trade check on the order path, the monitor on the
//! cycle path. Charging the levels anywhere else would leave the seam that
//! actually vetoes reading a state without them, which is how
//! `MaxCounterpartyExposure` came to be a cap over one order's delta.
//!
//! The cost is a factor estimation and a walk of the causal graph per risk
//! read, both over bounded working sets: `Platform::price_history` is capped
//! per instrument by `push_bounded`, and the graph is the world model's own
//! bounded structure. Neither walk touches the strategy set, so the O(1)-in-
//! strategy-count contract `qip_risk::aggregate` documents is untouched.

use qip_agents::runtime::Upstream;
use qip_core::error::Error;
use qip_risk::limits::RiskState;
use qip_risk::market_factor::{MARKET_FACTOR, MarketFactor};
use qip_risk::shared_cause::{CAUSAL_DRIVER_AXIS, FACTOR_AXIS, SharedCauseExposure};
use qip_world_model::WorldModel;
use qip_world_model::causal::CausalGraph;
use std::collections::{BTreeMap, BTreeSet};

/// The smallest weight this producer will offer.
///
/// Not a materiality judgement — the bucket charges a position's notional *in
/// proportion* to the weight, so a tenth of a percent of a shock contributes a
/// tenth of a percent of the notional and needs no threshold to be honest.
/// The floor exists for one arithmetic reason: `SharedCauseExposure::attribute`
/// refuses a weight that is positive as an `f64` and rounds to nothing once
/// carried as a nine-decimal-place notional, because such a position would
/// leave the bucket while the producer believed it had charged it. Anything at
/// or above this survives that crossing with room to spare, so the refusal
/// arm is reachable only by a genuinely malformed source.
const SMALLEST_OFFERED_WEIGHT: f64 = 1e-6;

/// Charge the shared-cause levels against `state` and return it.
///
/// Takes and returns the state rather than lending it, so that the exposure
/// this computes cannot be built and then dropped — a level computed and
/// ignored is worse than an absent one, because it reads as a control.
pub(crate) fn observe(
    world: &Upstream<WorldModel>,
    tape: &BTreeMap<String, Vec<f64>>,
    state: RiskState,
) -> RiskState {
    let held: BTreeSet<String> = state.position_notionals.keys().cloned().collect();
    let exposure = {
        let reading = world.read();
        exposure_of(reading.causal(), tape, &held)
    };
    exposure.apply(state)
}

/// What the two fed levels find, or a refusal of every level.
///
/// The refusal arm covers this producer offering the levels something they
/// refuse — a causal edge naming no cause — and this producer finding a figure
/// it cannot offer at all: a transmission or a loading that is not a number.
/// The second half was false prose until a review read the code: both
/// non-finite arms were `continue`s, so the name was dropped from its bucket
/// and `SharedCauseExposure::attribute`'s own `!is_finite()` refusal was
/// unreachable from the only production producer. They are refusals now, for
/// the reason the seat functions give: a bucket short of one position reads
/// downstream as a measurement of the whole book.
/// **Every** level is refused then, not only the one that failed, because a
/// producer that got its own walk wrong cannot vouch for the walk it had not
/// reached yet. That blocks orders, which is the fail-closed direction and
/// deliberately louder than an empty axis: an empty axis is a measured
/// statement that the book shares no named cause.
fn exposure_of(
    graph: &CausalGraph,
    tape: &BTreeMap<String, Vec<f64>>,
    held: &BTreeSet<String>,
) -> SharedCauseExposure {
    let mut exposure = SharedCauseExposure::new();
    if let Err(error) = seat_causal_drivers(&mut exposure, graph, held) {
        return SharedCauseExposure::refusing_all(&error);
    }
    if let Err(error) = seat_factor_loadings(&mut exposure, tape, held) {
        return SharedCauseExposure::refusing_all(&error);
    }
    exposure
}

/// Charge every held position to the causes the graph says drive it.
///
/// Declared before anything is attributed, and declared even when the graph
/// has absorbed nothing: a cycle in which no held position sits downstream of
/// a claimed cause must read as *measured and empty*, not as *nothing looked*.
/// The two are the same silence at the venue otherwise, which is the confusion
/// the liquidity floor already shipped once.
///
/// `known_at` is the graph's own `last_updated` rather than a clock, for two
/// reasons. The risk state is built without an instant — `Platform::risk_state`
/// takes none and reads none — and an edge recorded after the graph last
/// absorbed anything cannot exist. So this is "the graph as it stands", which
/// is reproducible from the log, where a wall-clock read would not be.
fn seat_causal_drivers(
    exposure: &mut SharedCauseExposure,
    graph: &CausalGraph,
    held: &BTreeSet<String>,
) -> Result<(), Error> {
    exposure.declare(CAUSAL_DRIVER_AXIS)?;
    let Some(known_at) = graph.last_updated() else {
        return Ok(());
    };
    for edge in graph.edges() {
        if edge.recorded_at > known_at || !held.contains(&edge.effect) {
            continue;
        }
        // The graph's own discount of a claim nobody is sure of, taken
        // whole. Recomputing `strength * confidence` here would be a second
        // derivation of one number, and the two would disagree the first
        // time `CausalEdge::transmission` learned to account for decay.
        let transmission = edge.transmission();
        // Two conditions that read as one and mean opposite things, and they
        // were one line until a review found the doc above describing a
        // refusal this `continue` made unreachable. A transmission below the
        // floor is a materiality judgement — the position contributes in
        // proportion, so leaving it out costs a rounding. A transmission that
        // is not a number is a malformed source, and dropping it understates
        // the bucket by that position's whole notional while the axis reads as
        // measured and complete, so the cap fires later than it should with
        // nothing reading as wrong. Refuse, do not drop.
        if !transmission.is_finite() {
            return Err(Error::numeric(format!(
                "the causal edge {} -> {} carries a transmission of {transmission}, which is not \
                 a share of a notional; the causal-driver level is refused rather than the edge \
                 quietly left out, because a bucket short of one position still reads as a \
                 measurement of the whole book — repair the edge's strength or confidence at its \
                 source",
                edge.cause, edge.effect
            )));
        }
        if transmission < SMALLEST_OFFERED_WEIGHT {
            continue;
        }
        exposure.attribute(CAUSAL_DRIVER_AXIS, &edge.cause, &edge.effect, transmission)?;
    }
    Ok(())
}

/// Charge every held position to the market factor at its absolute loading.
///
/// **Absolute, and never netted.** A long in a beta of 1.2 and a short in a
/// beta of 1.2 are two positions that move on the same systematic shock, and
/// signing them would report a book with no factor exposure at all — the
/// diversification illusion §25.3 calls the concentration that ends firms,
/// written into the control meant to catch it. The two positions do hedge each
/// other's *market* move, and that is what `MaxNetExposure` is for; this level
/// answers a different question.
///
/// An instrument with no beta is charged to nothing rather than to zero, which
/// is the rule `MarketFactor` states for every consumer: a position nobody
/// could model and a position genuinely insensitive to the shock are different
/// facts.
fn seat_factor_loadings(
    exposure: &mut SharedCauseExposure,
    tape: &BTreeMap<String, Vec<f64>>,
    held: &BTreeSet<String>,
) -> Result<(), Error> {
    exposure.declare(FACTOR_AXIS)?;
    if held.is_empty() {
        return Ok(());
    }
    let market = MarketFactor::estimate(tape);
    for instrument in held {
        let Some(beta) = market.beta_of(instrument) else {
            continue;
        };
        let loading = beta.abs();
        // Split for the reason `seat_causal_drivers` splits the same pair. A
        // degenerate covariance over a flat tape returns a beta that is not a
        // number, and dropping those names understated the factor bucket by
        // their notional while the axis read as measured and complete.
        if !loading.is_finite() {
            return Err(Error::numeric(format!(
                "the market factor's loading for {instrument} is {loading}, which is not a share \
                 of a notional; the factor level is refused rather than {instrument} quietly \
                 left out of its bucket, because a bucket short of one position still reads as a \
                 measurement of the whole book — a tape that cannot be regressed is a source to \
                 repair, not a zero exposure"
            )));
        }
        if loading < SMALLEST_OFFERED_WEIGHT {
            continue;
        }
        exposure.attribute(FACTOR_AXIS, MARKET_FACTOR, instrument, loading)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::{Duration, Timestamp};
    use qip_risk::shared_cause::{FAMILY_AXIS, SHARED_CAUSE_AXES};
    use qip_world_model::causal::{CausalEdge, Mechanism};

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn held(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    /// A graph holding one edge, whose `recorded_at` is the graph's own
    /// `last_updated`, so the point-in-time guard in `seat_causal_drivers`
    /// admits it.
    fn graph_of(edge: CausalEdge) -> CausalGraph {
        let mut graph = CausalGraph::new();
        graph.add(edge);
        graph
    }

    fn edge(strength: f64) -> CausalEdge {
        CausalEdge::new(
            "rates",
            "AAA",
            Mechanism::DiscountRate,
            strength,
            Duration::from_days(1),
            at(),
        )
        .with_confidence(1.0)
    }

    /// A tape whose returns are not numbers, so the single-factor regression
    /// answers a beta that is not one either.
    ///
    /// An infinite close is the cheapest reachable route: `returns` divides by
    /// the previous close, so an infinite close makes the next return
    /// `-inf / inf`, the factor series carries the result, and the variance
    /// floor `MarketFactor::estimate` guards with does not exclude a variance
    /// that is not a number. The finding this test exists for named the
    /// realistic route — a degenerate covariance over a flat tape — and the
    /// arithmetic downstream of either is the same.
    fn unmodellable_tape() -> BTreeMap<String, Vec<f64>> {
        let mut steady: Vec<f64> = (0..40).map(|step| 100.0 + step as f64).collect();
        let mut broken = steady.clone();
        broken[20] = f64::INFINITY;
        steady[0] = 100.0;
        BTreeMap::from([
            ("AAA".to_string(), broken),
            ("BBB".to_string(), steady.clone()),
        ])
    }

    #[test]
    fn a_causal_edge_whose_transmission_is_not_a_number_refuses_every_level_rather_than_dropping_the_name()
     {
        // Security review LOW-1. Both non-finite arms in this module were
        // `continue`s, so a malformed edge was dropped from its bucket while
        // the axis went on reading as measured and complete — the bucket
        // understated by that position's whole notional, the cap firing later
        // than it should, and nothing anywhere reading as wrong. This module's
        // own doc comment claimed the refusal arm covered it, which is the
        // worse half: prose describing a control the code did not have.
        let mut malformed = edge(0.5);
        malformed.strength = f64::NAN;
        // The premise: the transmission really is not a number, so the
        // refusal below is this guard and not some other.
        assert!(
            !malformed.transmission().is_finite(),
            "the fixture's transmission is a number, so this test exercises the ordinary path"
        );

        let exposure = exposure_of(
            &graph_of(malformed),
            &BTreeMap::new(),
            &held(&["AAA", "BBB"]),
        );
        let state = exposure.apply(RiskState::default());

        for axis in SHARED_CAUSE_AXES {
            assert!(
                state.unevaluated.contains_key(axis),
                "the {axis} level was not refused, so a producer that got its own walk wrong \
                 still vouched for a level it never reached"
            );
        }
        assert!(
            state.axis_exposures.is_empty(),
            "a refusing producer left buckets behind: {:?}",
            state.axis_exposures.keys().collect::<Vec<_>>()
        );
        let refusal = state
            .unevaluated
            .get(CAUSAL_DRIVER_AXIS)
            .expect("the refused level names the figure it could not compute");
        assert!(
            refusal.contains("not a share of a notional"),
            "the refusal does not name the malformed figure: {refusal}"
        );
    }

    #[test]
    fn a_causal_edge_below_the_offered_floor_is_left_out_without_refusing_the_level() {
        // The other half of the split, and the reason the two conditions are
        // not one line. A weight below the floor is a materiality judgement
        // this module argues for above `SMALLEST_OFFERED_WEIGHT`: the position
        // contributes in proportion, so leaving it out costs a rounding. If
        // this arm refused too, every graph carrying one negligible edge would
        // stop every order.
        let mut immaterial = edge(1.0);
        immaterial.strength = SMALLEST_OFFERED_WEIGHT / 10.0;
        // The premise: the edge really is under the floor and really is
        // finite, so this is the drop arm and not the refuse arm.
        let transmission = immaterial.transmission();
        assert!(
            transmission.is_finite() && transmission < SMALLEST_OFFERED_WEIGHT,
            "the fixture does not sit under the floor at {transmission}"
        );

        let exposure = exposure_of(&graph_of(immaterial), &BTreeMap::new(), &held(&["AAA"]));
        let state = exposure.apply(RiskState::default());

        assert!(
            state.unevaluated.is_empty(),
            "an immaterial edge refused a level: {:?}",
            state.unevaluated
        );
        // Measured and empty rather than absent, which is the distinction the
        // whole mechanism exists for.
        assert!(
            state
                .axis_exposures
                .get(CAUSAL_DRIVER_AXIS)
                .is_some_and(BTreeMap::is_empty),
            "the causal-driver level is not present and empty after a pass that found only an \
             immaterial edge"
        );
        assert!(
            !state.axis_exposures.contains_key(FAMILY_AXIS),
            "a level with no producer was written"
        );
    }

    #[test]
    fn a_factor_loading_that_is_not_a_number_refuses_every_level_rather_than_understating_the_bucket()
     {
        // The same finding on the other fed level, and the one the review
        // described: `MarketFactor::estimate` returning a beta that is not a
        // number for some names and a number for others. Dropping the first
        // group understates the factor bucket by their notional while the axis
        // reads complete, so the cap fires later than it should.
        let tape = unmodellable_tape();
        // The premise: a loading really is offered and really is not a
        // number, so this exercises the guard and not an absent beta.
        let beta = MarketFactor::estimate(&tape).beta_of("AAA");
        assert!(
            beta.is_some_and(|value| !value.is_finite()),
            "the fixture's beta is {beta:?}, so the factor guard under test is never reached"
        );

        let exposure = exposure_of(&CausalGraph::new(), &tape, &held(&["AAA", "BBB"]));
        let state = exposure.apply(RiskState::default());

        for axis in SHARED_CAUSE_AXES {
            assert!(
                state.unevaluated.contains_key(axis),
                "the {axis} level was not refused after a loading that is not a number"
            );
        }
        let refusal = state
            .unevaluated
            .get(FACTOR_AXIS)
            .expect("the refused level names the figure it could not compute");
        assert!(
            refusal.contains("not a share of a notional"),
            "the refusal does not name the malformed figure: {refusal}"
        );
    }

    #[test]
    fn a_well_formed_graph_and_tape_still_charge_both_fed_levels() {
        // The admitting half of both guards above. A producer that refused
        // every book would stop every order for ever, which is a control that
        // cannot be distinguished from a broken platform.
        let state = RiskState {
            equity: qip_core::Decimal::from_int(1_000_000),
            position_notionals: BTreeMap::from([
                ("AAA".to_string(), qip_core::Decimal::from_int(400_000)),
                ("BBB".to_string(), qip_core::Decimal::from_int(200_000)),
            ]),
            ..RiskState::default()
        };
        let tape: BTreeMap<String, Vec<f64>> = BTreeMap::from([
            (
                "AAA".to_string(),
                (0..40).map(|step| 100.0 + (step % 7) as f64).collect(),
            ),
            (
                "BBB".to_string(),
                (0..40).map(|step| 50.0 + (step % 5) as f64).collect(),
            ),
        ]);

        let exposure = exposure_of(&graph_of(edge(0.8)), &tape, &held(&["AAA", "BBB"]));
        let state = exposure.apply(state);

        assert!(
            state.unevaluated.is_empty(),
            "a well-formed graph and tape were refused: {:?}",
            state.unevaluated
        );
        assert_eq!(
            state.axis_exposures[CAUSAL_DRIVER_AXIS]["rates"],
            qip_core::Decimal::from_int(320_000),
            "the causal-driver bucket is not the held notional at the edge's own transmission"
        );
        assert!(
            !state.axis_exposures[FACTOR_AXIS].is_empty(),
            "the factor level charged nothing against a tape both names are modellable on"
        );
    }
}
