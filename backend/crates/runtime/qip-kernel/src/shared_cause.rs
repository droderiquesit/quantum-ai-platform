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
/// refuse — a causal edge naming no cause, a loading that is not a number.
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
        if !transmission.is_finite() || transmission < SMALLEST_OFFERED_WEIGHT {
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
        if !loading.is_finite() || loading < SMALLEST_OFFERED_WEIGHT {
            continue;
        }
        exposure.attribute(FACTOR_AXIS, MARKET_FACTOR, instrument, loading)?;
    }
    Ok(())
}
