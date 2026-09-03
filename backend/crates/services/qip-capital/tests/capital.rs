//! Tests for global capital and risk.
//!
//! The properties here are the ones that go wrong quietly: a budget that is
//! exceeded by a rounding step per strategy, a grant that never expires, a
//! concentration that no individual cell can see, an allocation that grows as
//! a drawdown deepens. Each is asserted as a property over a swept range
//! rather than at a single point, because every one of them is a bug that
//! passes a spot check.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::allocation::{
    AllocationLimits, CapitalAllocator, DrawdownSchedule, StrategyProposal,
};
use qip_capital::capacity::{CapacityBound, CapacityModel};
use qip_capital::envelope::{EnvelopeIssuer, EnvelopeTerms, MAXIMUM_ENVELOPE_VALIDITY};
use qip_capital::exposure::{AggregateExposure, CellPosition, ConcentrationLimits};
use qip_capital::margin::{MarginModel, assess_liquidity};
use qip_capital::recall::{RecallReason, RecallRegister, RecallState};
use qip_capital::reservation::ReservationLedger;
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::{CapitalEnvelope, Utilisation};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Currency, Decimal, Duration, Timestamp, dec};
use qip_financial::asset_class::Sector;
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use std::collections::BTreeMap;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn venue() -> VenueId {
    VenueId::new("XNYS")
}

fn approval(at: Timestamp) -> Result<Approval> {
    Approval::new(
        "capital grant",
        "alice.chen",
        at,
        "the allocation committee approved this envelope",
    )?
    .countersigned_by("bram.oduya")
}

fn capacity_model(daily_volume: i64, alpha_bps: f64, turnover: f64) -> Result<CapacityModel> {
    CapacityModel::new(
        LiquidityProfile::listed(Decimal::from_int(daily_volume), 5.0),
        TransactionCostModel::listed(5.0),
        alpha_bps,
        dec!("100"),
        turnover,
    )
}

fn proposal(
    name: &str,
    cell: &str,
    sharpe: f64,
    standard_error: f64,
    daily_volume: i64,
) -> Result<StrategyProposal> {
    Ok(StrategyProposal {
        strategy: StrategyId::new(name),
        cell: cell.to_string(),
        venue: venue(),
        expected_sharpe: sharpe,
        sharpe_standard_error: standard_error,
        capacity: capacity_model(daily_volume, 50.0, 0.5)?,
        capacity_uncertainty: 0.1,
    })
}

fn limits() -> Result<AllocationLimits> {
    AllocationLimits::new(
        dec!("100000000"),
        dec!("30000000"),
        dec!("60000000"),
        dec!("90000000"),
    )
}

fn allocator() -> Result<CapitalAllocator> {
    Ok(CapitalAllocator::new(
        limits()?,
        DrawdownSchedule::default(),
    ))
}

#[test]
fn allocations_never_sum_above_the_total_budget() -> Result<()> {
    // Swept over randomly generated books rather than asserted once: the
    // failure this guards against is a fractional overshoot that only appears
    // when the shares do not divide evenly, which a hand-written fixture will
    // not produce.
    let mut rng = Xoshiro256::seeded(0x5EED_C0DE);
    // Tighter cell and venue limits than the default fixture, so all five
    // constraints actually bind somewhere in the sweep. A property test where
    // one limit is never reached proves nothing about that limit, so the
    // counters below assert the sweep exercised each one.
    let allocator = CapitalAllocator::new(
        AllocationLimits::new(
            dec!("100000000"),
            dec!("30000000"),
            dec!("20000000"),
            dec!("40000000"),
        )?,
        DrawdownSchedule::default(),
    );
    let mut bound_by: BTreeMap<&str, usize> = BTreeMap::new();
    let mut exactly_full = 0usize;

    for case in 0..200u64 {
        let count = 1 + (rng.next_u64() % 12) as usize;
        let mut proposals = Vec::with_capacity(count);
        for index in 0..count {
            proposals.push(StrategyProposal {
                strategy: StrategyId::new(format!("strategy-{case}-{index}")),
                cell: format!("cell-{}", index % 3),
                venue: VenueId::new(format!("venue-{}", index % 2)),
                expected_sharpe: rng.next_f64() * 3.0,
                sharpe_standard_error: rng.next_f64() * 0.5,
                capacity: capacity_model(
                    1_000 + (rng.next_u64() % 5_000_000) as i64,
                    10.0 + rng.next_f64() * 60.0,
                    0.1 + rng.next_f64(),
                )?,
                capacity_uncertainty: rng.next_f64() * 0.5,
            });
        }
        let drawdown = rng.next_f64() * 0.25;
        let plan = allocator.allocate(&proposals, drawdown, start())?;

        assert!(
            plan.is_within_budget(),
            "case {case}: allocated {} against a {} budget",
            plan.allocated(),
            plan.budget
        );
        // Exact, not approximate. The comparison is over i128 fixed point, so
        // a single unit of overshoot fails this.
        assert!(plan.allocated() <= plan.total_budget);

        if plan.allocated() == plan.budget {
            exactly_full += 1;
        }

        // The per-axis limits hold jointly, not merely one at a time.
        for allocation in &plan.allocations {
            assert!(allocation.notional <= allocator.limits().per_strategy);
            for reason in &allocation.binding_constraints {
                for label in [
                    "total budget",
                    "modelled capacity",
                    "per-strategy",
                    "at cell",
                    "at venue",
                ] {
                    if reason.contains(label) {
                        *bound_by.entry(label).or_default() += 1;
                    }
                }
            }
        }
        for cell in ["cell-0", "cell-1", "cell-2"] {
            assert!(plan.for_cell(cell) <= allocator.limits().cell_limit(cell));
        }
        for name in ["venue-0", "venue-1"] {
            let venue = VenueId::new(name);
            assert!(plan.for_venue(&venue) <= allocator.limits().venue_limit(&venue));
        }
    }

    for label in [
        "total budget",
        "modelled capacity",
        "per-strategy",
        "at cell",
        "at venue",
    ] {
        assert!(
            bound_by.get(label).is_some_and(|count| *count > 0),
            "the sweep never exercised the {label} constraint, so it proves nothing about it"
        );
    }
    assert!(
        exactly_full > 10,
        "only {exactly_full} plans landed exactly on the budget; a rounding leak shows up on \
         the boundary, so the sweep has to reach it"
    );
    Ok(())
}

#[test]
fn a_strategy_with_a_wider_error_bar_is_allocated_less_than_its_point_estimate_suggests()
-> Result<()> {
    // Identical in every respect except the standard error on the estimate.
    let confident = proposal("confident", "cell-a", 2.0, 0.1, 50_000_000)?;
    let uncertain = proposal("uncertain", "cell-a", 2.0, 1.5, 50_000_000)?;
    let plan = allocator()?.allocate(&[confident, uncertain], 0.0, start())?;

    let confident = plan
        .for_strategy(&StrategyId::new("confident"))
        .ok_or_else(|| qip_core::error::Error::not_found("confident allocation"))?;
    let uncertain = plan
        .for_strategy(&StrategyId::new("uncertain"))
        .ok_or_else(|| qip_core::error::Error::not_found("uncertain allocation"))?;

    assert!(
        uncertain.notional < confident.notional,
        "the same point estimate with more uncertainty must be sized smaller: {} vs {}",
        uncertain.notional,
        confident.notional
    );
    assert!(uncertain.risk_adjusted_edge < confident.risk_adjusted_edge);
    Ok(())
}

#[test]
fn a_strategy_whose_edge_vanishes_under_its_own_uncertainty_is_refused_with_a_reason() -> Result<()>
{
    let plan = allocator()?.allocate(
        &[
            proposal("solid", "cell-a", 2.0, 0.1, 50_000_000)?,
            proposal("noise", "cell-a", 0.4, 2.0, 50_000_000)?,
        ],
        0.0,
        start(),
    )?;

    assert!(plan.for_strategy(&StrategyId::new("noise")).is_none());
    let (_, reason) = plan
        .refusals
        .iter()
        .find(|(strategy, _)| strategy.as_str() == "noise")
        .ok_or_else(|| qip_core::error::Error::not_found("refusal"))?;
    assert!(reason.contains("standard error"), "{reason}");
    Ok(())
}

#[test]
fn allocation_beyond_modelled_capacity_is_reduced_and_the_capacity_is_named() -> Result<()> {
    // A strong strategy in a thin name: its share of the budget is far more
    // than the market will absorb.
    let thin = StrategyProposal {
        strategy: StrategyId::new("thin"),
        cell: "cell-a".to_string(),
        venue: venue(),
        expected_sharpe: 3.0,
        sharpe_standard_error: 0.05,
        capacity: capacity_model(20_000, 50.0, 1.0)?,
        capacity_uncertainty: 0.0,
    };
    let capacity = thin.capacity.capacity();
    let plan = allocator()?.allocate(&[thin], 0.0, start())?;

    let allocation = plan
        .for_strategy(&StrategyId::new("thin"))
        .ok_or_else(|| qip_core::error::Error::not_found("thin allocation"))?;
    assert!(
        allocation.notional < allocation.indicated,
        "the indicated size must be cut"
    );
    assert!(allocation.notional <= capacity.notional);
    assert!(
        allocation
            .binding_constraints
            .iter()
            .any(|reason| reason.contains("modelled capacity")),
        "the reason must name capacity: {:?}",
        allocation.binding_constraints
    );
    Ok(())
}

#[test]
fn capital_beyond_capacity_carries_negative_expected_edge() -> Result<()> {
    let model = capacity_model(1_000_000, 50.0, 0.5)?;
    let capacity = model.capacity();
    assert!(capacity.notional.is_positive());

    // Inside capacity the edge is positive; far outside it the strategy pays
    // more in impact than it expects to earn. An allocator extrapolating
    // linearly from the point estimate would never see this sign change.
    let half = capacity
        .notional
        .checked_div(Decimal::from_int(2))
        .ok_or_else(|| qip_core::error::Error::numeric("half capacity"))?;
    assert!(model.net_edge_bps(half) > 0.0);
    assert!(!model.is_beyond_capacity(half));

    let far_beyond = capacity
        .notional
        .checked_mul(Decimal::from_int(50))
        .ok_or_else(|| qip_core::error::Error::numeric("fifty times capacity"))?;
    assert!(
        model.net_edge_bps(far_beyond) < 0.0,
        "beyond capacity extra capital must be negative edge, not smaller positive edge"
    );
    assert!(model.is_beyond_capacity(far_beyond));

    // And net edge is monotonically decreasing in size, so there is no size
    // above capacity at which the strategy becomes good again.
    let mut previous = f64::INFINITY;
    for step in 1..=40i64 {
        let notional = Decimal::from_int(step * 250_000);
        let edge = model.net_edge_bps(notional);
        assert!(
            edge <= previous,
            "net edge increased with size at {notional}"
        );
        previous = edge;
    }
    Ok(())
}

#[test]
fn a_strategy_with_no_tradeable_volume_has_no_capacity_and_the_bound_is_named() -> Result<()> {
    let negotiated = CapacityModel::new(
        LiquidityProfile::illiquid(21.0),
        TransactionCostModel::negotiated(),
        50.0,
        dec!("100"),
        0.5,
    )?;
    assert_eq!(negotiated.capacity().binding, CapacityBound::NoVolume);
    assert!(negotiated.capacity().notional.is_zero());

    // And a strategy whose alpha does not cover the spread has no capacity at
    // any size, which is a different finding from having no volume.
    let unprofitable = capacity_model(1_000_000, 1.0, 0.5)?;
    assert_eq!(
        unprofitable.capacity().binding,
        CapacityBound::NoEdgeAtAnySize
    );
    Ok(())
}

#[test]
fn a_deeper_drawdown_never_increases_allocation() -> Result<()> {
    let allocator = allocator()?;
    let proposals = [
        proposal("alpha", "cell-a", 2.0, 0.2, 50_000_000)?,
        proposal("beta", "cell-b", 1.5, 0.2, 50_000_000)?,
        proposal("gamma", "cell-a", 1.1, 0.3, 50_000_000)?,
    ];

    let mut previous_total = Decimal::MAX;
    let mut previous_per_strategy: BTreeMap<String, Decimal> = BTreeMap::new();
    // Swept in fine steps across and past the whole schedule, so a
    // non-monotone step between two thresholds cannot hide between samples.
    for step in 0..=300u32 {
        let drawdown = f64::from(step) / 1000.0;
        let plan = allocator.allocate(&proposals, drawdown, start())?;

        assert!(
            plan.allocated() <= previous_total,
            "allocation grew at a {drawdown:.3} drawdown: {} after {}",
            plan.allocated(),
            previous_total
        );
        for allocation in &plan.allocations {
            let name = allocation.strategy.as_str().to_string();
            if let Some(previous) = previous_per_strategy.get(&name) {
                assert!(
                    allocation.notional <= *previous,
                    "{name} grew at a {drawdown:.3} drawdown"
                );
            }
            previous_per_strategy.insert(name, allocation.notional);
        }
        // A strategy dropped from the plan is at zero, which is still a
        // decrease — record that so a later step cannot resurrect it.
        for proposal in &proposals {
            let name = proposal.strategy.as_str().to_string();
            if plan.for_strategy(&proposal.strategy).is_none() {
                previous_per_strategy.insert(name, Decimal::ZERO);
            }
        }
        previous_total = plan.allocated();
    }

    // Monotone is only interesting if the number actually moves.
    let at_the_high_water_mark = allocator.allocate(&proposals, 0.0, start())?.allocated();
    let in_drawdown = allocator.allocate(&proposals, 0.12, start())?.allocated();
    assert!(
        in_drawdown < at_the_high_water_mark,
        "a 12% drawdown must shrink the book: {in_drawdown} against {at_the_high_water_mark}"
    );

    // Past the end of the schedule nothing is allocated at all.
    let stopped = allocator.allocate(&proposals, 0.35, start())?;
    assert!(stopped.allocated().is_zero());
    assert!(!stopped.refusals.is_empty());
    Ok(())
}

#[test]
fn a_drawdown_schedule_that_allocates_more_as_losses_deepen_cannot_be_constructed() {
    let procyclical = DrawdownSchedule::new(vec![(0.00, dec!("0.5")), (0.10, dec!("1.0"))]);
    assert!(
        procyclical.is_err(),
        "a schedule that grows with drawdown must be refused at construction"
    );

    // The shipped schedule is monotone at every step.
    let schedule = DrawdownSchedule::default();
    let mut previous = Decimal::MAX;
    for (_, multiplier) in schedule.steps() {
        assert!(*multiplier <= previous);
        previous = *multiplier;
    }
}

fn issuer() -> Result<EnvelopeIssuer> {
    EnvelopeIssuer::new(vec![0x42u8; 32], "capital-signing-key-1")
}

fn terms(gross: Decimal, validity: Duration) -> EnvelopeTerms {
    EnvelopeTerms {
        strategy: StrategyId::new("momentum-v3"),
        cell: "cell-lon-1".to_string(),
        gross_limit: gross,
        order_fraction: dec!("0.1"),
        loss_fraction: dec!("0.2"),
        venues: vec![venue()],
        validity,
    }
}

#[test]
fn an_envelope_always_expires_and_never_outlives_the_ceiling() -> Result<()> {
    let issuer = issuer()?;
    for hours in 1..=12i64 {
        let validity = Duration::from_hours(hours);
        let envelope = issuer.issue(
            &terms(dec!("1000000"), validity),
            &approval(start())?,
            start(),
        )?;

        assert!(envelope.is_live(start()));
        assert!(
            !envelope.is_live(envelope.expires_at()),
            "an envelope must not be live at its own expiry"
        );
        assert!(!envelope.is_live(envelope.expires_at().saturating_add(Duration::from_secs(1))));
        assert!(envelope.expires_at().since(start()) <= MAXIMUM_ENVELOPE_VALIDITY);

        // And the cell refuses orders past expiry on its own authority, with
        // no message from the central plane.
        let after = envelope.expires_at().saturating_add(Duration::from_mins(1));
        let grant = envelope.admit(&venue(), dec!("1"), &Utilisation::default(), after);
        assert!(grant.is_refused());
    }

    // A grant that would outlive the ceiling is refused rather than truncated,
    // so nobody believes they were given what they asked for.
    let error = issuer
        .issue(
            &terms(dec!("1000000"), Duration::from_days(7)),
            &approval(start())?,
            start(),
        )
        .expect_err("a week-long grant must be refused");
    assert!(error.message().contains("backstop"), "{error:?}");
    Ok(())
}

#[test]
fn an_unsigned_or_tampered_envelope_is_rejected() -> Result<()> {
    let issuer = issuer()?;
    let genuine = issuer.issue(
        &terms(dec!("1000000"), Duration::from_hours(6)),
        &approval(start())?,
        start(),
    )?;
    assert!(issuer.verify(&genuine, start()).is_ok());

    // Unsigned.
    let unsigned = CapitalEnvelope::new(
        StrategyId::new("momentum-v3"),
        "cell-lon-1",
        dec!("1000000"),
        dec!("100000"),
        dec!("200000"),
        vec![venue()],
        start(),
        start().saturating_add(Duration::from_hours(6)),
        "alice.chen",
        "",
    )?;
    assert!(issuer.verify(&unsigned, start()).is_err());

    // Tampered: the same signature over a wider grant. The payload covers the
    // gross limit, so the recomputed MAC does not match.
    let widened = CapitalEnvelope::new(
        StrategyId::new("momentum-v3"),
        "cell-lon-1",
        dec!("100000000"),
        dec!("100000"),
        dec!("200000"),
        vec![venue()],
        start(),
        start().saturating_add(Duration::from_hours(6)),
        "alice.chen",
        genuine.signature(),
    )?;
    let error = issuer
        .verify(&widened, start())
        .expect_err("a widened grant must not verify");
    assert!(error.message().contains("altered"), "{error:?}");

    // Every other field the payload covers is equally protected.
    for (label, envelope) in [
        (
            "expiry",
            CapitalEnvelope::new(
                StrategyId::new("momentum-v3"),
                "cell-lon-1",
                dec!("1000000"),
                dec!("100000"),
                dec!("200000"),
                vec![venue()],
                start(),
                start().saturating_add(Duration::from_hours(11)),
                "alice.chen",
                genuine.signature(),
            )?,
        ),
        (
            "venue",
            CapitalEnvelope::new(
                StrategyId::new("momentum-v3"),
                "cell-lon-1",
                dec!("1000000"),
                dec!("100000"),
                dec!("200000"),
                vec![VenueId::new("XNAS")],
                start(),
                start().saturating_add(Duration::from_hours(6)),
                "alice.chen",
                genuine.signature(),
            )?,
        ),
        (
            "cell",
            CapitalEnvelope::new(
                StrategyId::new("momentum-v3"),
                "cell-nyc-9",
                dec!("1000000"),
                dec!("100000"),
                dec!("200000"),
                vec![venue()],
                start(),
                start().saturating_add(Duration::from_hours(6)),
                "alice.chen",
                genuine.signature(),
            )?,
        ),
    ] {
        assert!(
            issuer.verify(&envelope, start()).is_err(),
            "a changed {label} must invalidate the signature"
        );
    }

    // A different key does not verify a grant it did not sign.
    let other = EnvelopeIssuer::new(vec![0x99u8; 32], "capital-signing-key-2")?;
    assert!(!other.is_valid(&genuine, start()));
    Ok(())
}

#[test]
fn issuing_capital_needs_two_approvers() -> Result<()> {
    let single = Approval::new(
        "capital grant",
        "alice.chen",
        start(),
        "I am confident about this allocation",
    )?;
    let error = issuer()?
        .issue(
            &terms(dec!("1000000"), Duration::from_hours(6)),
            &single,
            start(),
        )
        .expect_err("one approver cannot move capital");
    assert!(error.message().contains("two approvers"), "{error:?}");
    Ok(())
}

/// Three cells that have each independently accumulated the same name.
fn crowded_book() -> Vec<CellPosition> {
    let position = |cell: &str, strategy: &str, instrument: &str, quantity: &str| CellPosition {
        cell: cell.to_string(),
        strategy: StrategyId::new(strategy),
        instrument: instrument.to_string(),
        sector: if instrument == "ACME" {
            Sector::InformationTechnology
        } else {
            Sector::Energy
        },
        venue: venue(),
        currency: Currency::USD,
        quantity: Decimal::parse(quantity).unwrap_or(Decimal::ZERO),
        price: dec!("100"),
    };
    vec![
        // Each cell holds ACME at a size that is unremarkable on its own.
        position("cell-lon-1", "momentum-v3", "ACME", "30000"),
        position("cell-nyc-2", "reversion-v1", "ACME", "25000"),
        position("cell-tok-3", "carry-v7", "ACME", "20000"),
        // …alongside genuinely separate books.
        position("cell-lon-1", "momentum-v3", "BOREAS", "10000"),
        position("cell-nyc-2", "reversion-v1", "CYGNUS", "12000"),
        position("cell-tok-3", "carry-v7", "DORADO", "9000"),
    ]
}

#[test]
fn aggregate_exposure_sums_positions_three_cells_took_independently() -> Result<()> {
    let positions = crowded_book();
    let aggregate = AggregateExposure::of(&positions);

    // 30k + 25k + 20k units at 100 is 7.5m in one name, which no cell can see.
    assert_eq!(aggregate.by_instrument.net_of("ACME"), dec!("7500000"));
    assert_eq!(aggregate.by_instrument.gross_of("ACME"), dec!("7500000"));
    assert_eq!(aggregate.gross(), dec!("10600000"));
    assert_eq!(aggregate.net(), dec!("10600000"));

    // The cells are named, so an operator can call the right three teams.
    assert_eq!(
        aggregate.cells_holding("ACME"),
        vec!["cell-lon-1", "cell-nyc-2", "cell-tok-3"]
    );

    let crowded = aggregate.crowded(2);
    let acme = crowded
        .first()
        .ok_or_else(|| qip_core::error::Error::not_found("crowded position"))?;
    assert_eq!(acme.instrument, "ACME");
    assert_eq!(acme.cells.len(), 3);
    assert!(!acme.cells_disagree);
    assert!(acme.share_of_gross > 0.7);

    // A name held by one cell is not crowded, however large.
    assert!(crowded.iter().all(|p| p.instrument != "BOREAS"));

    // And the concentration is a finding, not merely a number.
    let findings = aggregate.concentrations(&ConcentrationLimits::default());
    assert!(
        findings
            .iter()
            .any(|f| f.axis == "instrument" && f.bucket == "ACME"),
        "{findings:?}"
    );
    Ok(())
}

#[test]
fn cells_taking_opposite_sides_of_the_same_name_are_flagged_even_when_they_net_to_nothing()
-> Result<()> {
    let mut positions = crowded_book();
    positions[1].quantity = dec!("-30000");
    positions[2].quantity = dec!("0");
    let aggregate = AggregateExposure::of(&positions);

    // Flat on net, six million gross: the firm is paying to hold both sides.
    assert!(aggregate.by_instrument.net_of("ACME").is_zero());
    assert_eq!(aggregate.by_instrument.gross_of("ACME"), dec!("6000000"));

    let acme = aggregate
        .crowded(2)
        .into_iter()
        .find(|p| p.instrument == "ACME")
        .ok_or_else(|| qip_core::error::Error::not_found("ACME"))?;
    assert!(
        acme.cells_disagree,
        "two cells on opposite sides of one name is a finding, not a hedge"
    );
    Ok(())
}

#[test]
fn margin_rises_with_concentration_at_the_same_notional() -> Result<()> {
    let model = MarginModel::default();
    let concentrated = AggregateExposure::of(&crowded_book());

    // The same gross, spread over ten names instead of four.
    let spread: Vec<CellPosition> = (0..10)
        .map(|i| CellPosition {
            cell: format!("cell-{i}"),
            strategy: StrategyId::new(format!("s-{i}")),
            instrument: format!("NAME{i}"),
            sector: Sector::Industrials,
            venue: venue(),
            currency: Currency::USD,
            quantity: dec!("10600"),
            price: dec!("100"),
        })
        .collect();
    let diversified = AggregateExposure::of(&spread);
    assert_eq!(concentrated.gross(), diversified.gross());

    let collateral = dec!("6000000");
    let concentrated = model.require(&concentrated, collateral)?;
    let diversified = model.require(&diversified, collateral)?;

    assert!(
        concentrated.initial > diversified.initial,
        "the same notional in one name must cost more margin than in ten"
    );
    assert!(concentrated.concentration_add_on.is_positive());
    assert!(diversified.concentration_add_on.is_zero());

    // And the accounting against collateral is exact and signed the right way.
    assert_eq!(concentrated.excess(), collateral - concentrated.maintenance);
    assert!(!concentrated.is_call());
    let under = model.require(&AggregateExposure::of(&crowded_book()), dec!("100000"))?;
    assert!(under.is_call());
    assert!(!under.can_open());
    Ok(())
}

#[test]
fn a_position_that_takes_weeks_to_exit_is_distinguished_from_one_that_takes_hours() -> Result<()> {
    let positions = vec![
        CellPosition {
            cell: "cell-lon-1".to_string(),
            strategy: StrategyId::new("momentum-v3"),
            instrument: "LIQUID".to_string(),
            sector: Sector::InformationTechnology,
            venue: venue(),
            currency: Currency::USD,
            quantity: dec!("100000"),
            price: dec!("100"),
        },
        CellPosition {
            cell: "cell-nyc-2".to_string(),
            strategy: StrategyId::new("carry-v7"),
            instrument: "THIN".to_string(),
            sector: Sector::RealEstate,
            venue: venue(),
            currency: Currency::USD,
            quantity: dec!("100000"),
            price: dec!("100"),
        },
        CellPosition {
            cell: "cell-tok-3".to_string(),
            strategy: StrategyId::new("private-credit"),
            instrument: "NEGOTIATED".to_string(),
            sector: Sector::Financials,
            venue: venue(),
            currency: Currency::USD,
            quantity: dec!("100000"),
            price: dec!("100"),
        },
    ];
    let mut profiles = BTreeMap::new();
    profiles.insert(
        "LIQUID".to_string(),
        LiquidityProfile::listed(Decimal::from_int(20_000_000), 2.0),
    );
    profiles.insert(
        "THIN".to_string(),
        LiquidityProfile::listed(Decimal::from_int(35_000), 40.0),
    );
    profiles.insert("NEGOTIATED".to_string(), LiquidityProfile::illiquid(60.0));

    let assessment = assess_liquidity(&positions, &profiles, 0.10)?;

    // The same ten million of notional in each name, and completely different
    // exits: half a session against nearly thirty.
    let horizon = |instrument: &str| {
        assessment
            .horizons
            .iter()
            .find(|h| h.instrument == instrument)
            .and_then(|h| h.days)
    };
    let liquid = horizon("LIQUID").unwrap_or(f64::INFINITY);
    let thin = horizon("THIN").unwrap_or(f64::INFINITY);
    assert!(liquid < 1.0, "{liquid}");
    assert!(thin > 20.0, "{thin}");
    assert!(
        horizon("NEGOTIATED").is_none(),
        "a negotiated name has no volume estimate"
    );

    assert!(assessment.worst_days > 20.0);
    assert_eq!(assessment.unquantifiable_gross, dec!("10000000"));

    // Two thirds of the book cannot be out inside a week — counting the
    // position with no estimate as slow rather than fast.
    let slow = assessment.share_slower_than(5.0);
    assert!(slow > 0.6 && slow < 0.7, "{slow}");
    Ok(())
}

#[test]
fn a_recall_from_an_unreachable_cell_is_still_bounded_by_the_envelopes_expiry() -> Result<()> {
    let issuer = issuer()?;
    let envelope = issuer.issue(
        &terms(dec!("5000000"), Duration::from_hours(4)),
        &approval(start())?,
        start(),
    )?;
    let strategy = StrategyId::new("momentum-v3");

    let mut register = RecallRegister::new();
    let order = register.issue(
        &envelope,
        RecallReason::RiskReduction,
        "the book breached its drawdown limit",
        Duration::from_mins(5),
        start(),
    )?;
    assert_eq!(order.backstop_expiry, envelope.expires_at());
    assert!(order.reason.requires_immediate_flatten());

    // The cell never answers.
    let late = start().saturating_add(Duration::from_mins(30));
    let unreachable = register.expire_unacknowledged(late);
    assert_eq!(unreachable.len(), 1);
    assert!(matches!(
        register.state("cell-lon-1", &strategy),
        Some(RecallState::Unreachable { .. })
    ));

    // While the grant is live, the whole grant is at risk — not an estimate of
    // what the cell is probably holding, because there is nobody to ask.
    assert_eq!(register.outstanding_exposure(late), dec!("5000000"));
    assert!(order.unbounded_window(late) > Duration::ZERO);

    // Past expiry the exposure is bounded at zero without anyone having become
    // reachable, because the cell enforces expiry against its own clock.
    let after = envelope.expires_at().saturating_add(Duration::from_secs(1));
    assert!(register.outstanding_exposure(after).is_zero());
    assert!(register.outstanding(after).is_empty());
    assert_eq!(order.unbounded_window(after), Duration::ZERO);
    assert!(
        envelope
            .admit(&venue(), dec!("1"), &Utilisation::default(), after)
            .is_refused()
    );

    // And the incident has a stated end time even with nobody answering.
    assert_eq!(register.bounded_until(), Some(envelope.expires_at()));
    Ok(())
}

#[test]
fn an_acknowledged_recall_stops_counting_against_outstanding_exposure() -> Result<()> {
    let issuer = issuer()?;
    let envelope = issuer.issue(
        &terms(dec!("5000000"), Duration::from_hours(4)),
        &approval(start())?,
        start(),
    )?;
    let strategy = StrategyId::new("momentum-v3");
    let mut register = RecallRegister::new();
    register.issue(
        &envelope,
        RecallReason::Reallocation,
        "the capital is going to a better use",
        Duration::from_mins(5),
        start(),
    )?;

    let at = start().saturating_add(Duration::from_mins(2));
    register.acknowledge("cell-lon-1", &strategy, dec!("120000"), at)?;
    assert!(register.outstanding_exposure(at).is_zero());
    assert!(register.bounded_until().is_none());
    assert!(matches!(
        register.state("cell-lon-1", &strategy),
        Some(RecallState::Acknowledged { .. })
    ));

    // Acknowledging a recall nobody issued is an error, not a silent success.
    assert!(
        register
            .acknowledge("cell-nyc-2", &strategy, Decimal::ZERO, at)
            .is_err()
    );
    Ok(())
}

#[test]
fn an_allocation_plan_can_be_turned_into_grants_that_together_stay_inside_the_budget() -> Result<()>
{
    let plan = allocator()?.allocate(
        &[
            proposal("alpha", "cell-a", 2.0, 0.2, 50_000_000)?,
            proposal("beta", "cell-b", 1.5, 0.2, 50_000_000)?,
        ],
        0.02,
        start(),
    )?;
    let issuer = issuer()?;

    let mut granted = Decimal::ZERO;
    for allocation in &plan.allocations {
        let envelope = issuer.issue(
            &EnvelopeTerms::from_allocation(allocation, Duration::from_hours(8)),
            &approval(start())?,
            start(),
        )?;
        issuer.verify(&envelope, start())?;
        assert_eq!(envelope.gross_limit(), allocation.notional);
        assert!(envelope.order_limit() <= envelope.gross_limit());
        assert!(envelope.permits_venue(&allocation.venue));
        // An empty venue list would be the permissive reading; the contract
        // treats it as no venues, and the grant here names one.
        assert!(!envelope.permits_venue(&VenueId::new("XLON")));
        granted += envelope.gross_limit();
    }
    assert_eq!(granted, plan.allocated());
    assert!(granted <= plan.budget);
    Ok(())
}

// --- capital reservations ---------------------------------------------------
//
// Gap-matrix item 10. Before the ledger existed, passing a capital check held
// nothing, so two proposals sized in the same cycle each passed against the
// same free balance and their sum was a position nobody approved. These tests
// pin the property that closes it: the check and the hold are one operation.

#[test]
fn a_second_proposal_against_the_same_free_balance_is_refused_while_the_first_holds_it()
-> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    let each = dec!("600000");

    // Premise: either proposal alone fits the free balance, and together they
    // do not. Without this the refusal below could be a plain overdraft
    // rather than the double-spend the ledger exists to prevent.
    assert!(each <= ledger.free(start()));
    assert!(each + each > ledger.free(start()));

    ledger.reserve("proposal-1", each, start(), Duration::from_hours(1))?;
    // Premise: the first check actually holds the capital.
    let held = ledger
        .reservation("proposal-1")
        .expect("the first reservation must exist");
    assert_eq!(held.amount, each);
    assert_eq!(ledger.free(start()), dec!("400000"));

    // The property: the second proposal is refused — not clamped to the
    // 400,000 left, not queued — and the refusal names a way out.
    let refused = ledger
        .reserve("proposal-2", each, start(), Duration::from_hours(1))
        .expect_err("the second proposal passed against capital the first already holds");
    assert_eq!(refused.code(), "denied");
    assert!(
        refused.message().contains("release"),
        "the refusal must name what to do instead: {refused}"
    );
    assert!(ledger.reservation("proposal-2").is_none());

    // And the gate admits a good value: once the first hold is gone, the
    // same second proposal passes. A gate that refuses everything is not a
    // control either.
    ledger.release("proposal-1", start())?;
    ledger.reserve("proposal-2", each, start(), Duration::from_hours(1))?;
    Ok(())
}

#[test]
fn a_reservation_larger_than_the_free_balance_is_refused_rather_than_clamped() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("500000"))?;
    let refused = ledger
        .reserve(
            "proposal-1",
            dec!("500001"),
            start(),
            Duration::from_hours(1),
        )
        .expect_err("an oversized reservation was granted");
    assert_eq!(refused.code(), "denied");
    // Nothing was silently taken: a clamp here would be a caller bug that
    // survives, holding a number nobody asked for.
    assert!(ledger.reservation("proposal-1").is_none());
    assert_eq!(ledger.free(start()), dec!("500000"));
    Ok(())
}

#[test]
fn committing_a_reservation_spends_the_capital_rather_than_returning_it() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve(
        "proposal-1",
        dec!("600000"),
        start(),
        Duration::from_hours(1),
    )?;
    // Premise: the hold exists and the free balance reflects it.
    assert!(ledger.reservation("proposal-1").is_some());
    assert_eq!(ledger.free(start()), dec!("400000"));

    let committed = ledger.commit("proposal-1", start())?;
    assert_eq!(committed, dec!("600000"));
    // The capital left the ledger; it did not quietly return to free, which
    // would let the next proposal spend the same money the fill is spending.
    assert_eq!(ledger.free(start()), dec!("400000"));
    assert_eq!(ledger.committed_total(), dec!("600000"));
    assert!(ledger.reservation("proposal-1").is_none());
    Ok(())
}

#[test]
fn releasing_a_reservation_returns_its_capital_to_the_free_balance() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve(
        "proposal-1",
        dec!("600000"),
        start(),
        Duration::from_hours(1),
    )?;
    // Premise: the hold reduced the free balance before the release.
    assert_eq!(ledger.free(start()), dec!("400000"));

    let returned = ledger.release("proposal-1", start())?;
    assert_eq!(returned, dec!("600000"));
    assert_eq!(ledger.free(start()), dec!("1000000"));
    assert_eq!(ledger.committed_total(), Decimal::ZERO);
    Ok(())
}

#[test]
fn a_committed_reservation_cannot_be_committed_or_released_a_second_time() -> Result<()> {
    // The failure this guards: `commit` removing the reservation on success is
    // what makes a second commit impossible. If a future change made `commit`
    // leave the entry in place (say, to preserve it for an audit read), the
    // same id would spend its capital twice — once per call — while
    // `committed_total` silently doubled and the free balance never noticed.
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve(
        "proposal-1",
        dec!("600000"),
        start(),
        Duration::from_hours(1),
    )?;
    let first = ledger.commit("proposal-1", start())?;
    assert_eq!(first, dec!("600000"));
    assert_eq!(ledger.committed_total(), dec!("600000"));

    // Premise held: the hold is gone and the capital already left the ledger.
    assert!(ledger.reservation("proposal-1").is_none());

    let second_commit = ledger
        .commit("proposal-1", start())
        .expect_err("committed the same capital twice");
    assert_eq!(second_commit.code(), "denied");
    let second_release = ledger
        .release("proposal-1", start())
        .expect_err("released capital that was never held free again");
    assert_eq!(second_release.code(), "denied");

    // Neither refused call moved a single unit: one commit's worth stayed
    // committed, and nothing came back to free.
    assert_eq!(ledger.committed_total(), dec!("600000"));
    assert_eq!(ledger.free(start()), dec!("400000"));
    Ok(())
}

#[test]
fn a_released_reservation_cannot_be_committed_or_released_a_second_time() -> Result<()> {
    // The mirror of the commit case: `release` also removes the entry on
    // success, so a second release or a late commit against the same id finds
    // nothing rather than crediting the free balance twice.
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve(
        "proposal-1",
        dec!("600000"),
        start(),
        Duration::from_hours(1),
    )?;
    let first = ledger.release("proposal-1", start())?;
    assert_eq!(first, dec!("600000"));
    assert_eq!(ledger.free(start()), dec!("1000000"));

    let second_release = ledger
        .release("proposal-1", start())
        .expect_err("released the same capital twice");
    assert_eq!(second_release.code(), "denied");
    let second_commit = ledger
        .commit("proposal-1", start())
        .expect_err("committed a hold that had already been released");
    assert_eq!(second_commit.code(), "denied");

    // Still exactly the one credit the first release made — not two.
    assert_eq!(ledger.free(start()), dec!("1000000"));
    assert_eq!(ledger.committed_total(), Decimal::ZERO);
    Ok(())
}

#[test]
fn an_unclaimed_reservation_returns_its_capital_at_expiry() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve(
        "proposal-1",
        dec!("600000"),
        start(),
        Duration::from_hours(1),
    )?;

    // Premise: one instant before expiry the hold still binds — a proposal
    // for the full balance is still refused.
    let just_before = start().saturating_add(Duration::from_hours(1) - Duration::from_nanos(1));
    assert_eq!(ledger.free(just_before), dec!("400000"));

    // At expiry the capital is free again, without anyone calling release:
    // an abandoned proposal must not pin capital forever.
    let at_expiry = start().saturating_add(Duration::from_hours(1));
    assert_eq!(ledger.free(at_expiry), dec!("1000000"));
    assert!(ledger.reservation("proposal-1").is_none());
    ledger.reserve(
        "proposal-2",
        dec!("1000000"),
        at_expiry,
        Duration::from_hours(1),
    )?;
    Ok(())
}

#[test]
fn an_expired_reservation_cannot_be_committed() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve(
        "proposal-1",
        dec!("600000"),
        start(),
        Duration::from_hours(1),
    )?;
    // Premise: before expiry the same commit would have succeeded, so the
    // refusal below is about the clock and nothing else.
    assert!(
        !ledger
            .reservation("proposal-1")
            .expect("the reservation must exist")
            .is_expired(start())
    );

    let at_expiry = start().saturating_add(Duration::from_hours(1));
    let refused = ledger
        .commit("proposal-1", at_expiry)
        .expect_err("a lapsed hold was committed; the free balance already counts that capital");
    assert_eq!(refused.code(), "denied");
    assert!(
        refused.message().contains("expired"),
        "the refusal must say the hold lapsed: {refused}"
    );
    // Fail closed and account honestly: nothing was committed, and the
    // capital is back where the free balance already said it was.
    assert_eq!(ledger.committed_total(), Decimal::ZERO);
    assert_eq!(ledger.free(at_expiry), dec!("1000000"));
    Ok(())
}

// --- every capital gate rule, both halves -----------------------------------
//
// | Rule | Pass fixture | Veto fixture |
// |---|---|---|
// | reservation fits the free balance | `a_second_proposal_against_the_same_free_balance_is_refused_while_the_first_holds_it` (release half) | the same test, and `a_reservation_larger_than_the_free_balance_is_refused_rather_than_clamped` |
// | reservation names an id | `a_reservation_that_names_nothing_is_refused` (premise) | the same test |
// | reservation holds a positive amount | `a_reservation_for_a_non_positive_amount_is_refused` (premise) | the same test |
// | reservation has a positive validity | `a_reservation_with_no_validity_is_refused` (premise) | the same test |
// | one live hold per id | `a_second_reservation_under_the_same_id_is_refused_until_the_first_ends` | the same test |
// | ledger opens over a non-negative balance | `a_ledger_cannot_open_over_a_negative_balance` | the same test |
// | commit and release need a live hold | `committing_a_reservation_spends_the_capital_rather_than_returning_it` | `committing_or_releasing_a_reservation_nobody_made_is_refused`, `an_expired_reservation_cannot_be_committed` |
// | free = equity − holds | `resyncing_free_to_equity_reports_a_shortfall_and_floors_free_at_zero` | the same test |
// | envelope needs two approvers | `an_allocation_plan_can_be_turned_into_grants_that_together_stay_inside_the_budget` | `issuing_capital_needs_two_approvers` |
// | envelope validity positive and under the ceiling | `an_envelope_always_expires_and_never_outlives_the_ceiling` | the same test, and `an_envelope_with_no_validity_period_is_refused` |
// | envelope fractions in (0, 1] | `an_envelope_fraction_outside_zero_to_one_is_refused` (premise) | the same test |
// | signing key length and id | `an_issuer_with_a_short_key_or_no_key_id_is_refused` (premise) | the same test |
// | envelope verifies and is live | `an_unsigned_or_tampered_envelope_is_rejected` (genuine half) | the same test, and `an_expired_envelope_does_not_verify` |
// | per-strategy, per-cell, per-venue and total allocation limits | `allocations_never_sum_above_the_total_budget` | `each_allocation_limit_binds_and_is_named` |
// | allocation limits are positive and non-negative | `allocation_limits_refuse_a_non_positive_budget_or_a_negative_limit` (premise) | the same test |
// | edge after uncertainty | `a_strategy_with_a_wider_error_bar_is_allocated_less_than_its_point_estimate_suggests` | `a_strategy_whose_edge_vanishes_under_its_own_uncertainty_is_refused_with_a_reason` |
// | capacity | `allocation_beyond_modelled_capacity_is_reduced_and_the_capacity_is_named` | the same test, and `a_strategy_with_no_tradeable_volume_has_no_capacity_and_the_bound_is_named` |
// | drawdown schedule monotone | `a_deeper_drawdown_never_increases_allocation` | `a_drawdown_schedule_that_allocates_more_as_losses_deepen_cannot_be_constructed` |
// | concentration per axis | `a_diversified_book_produces_no_concentration_finding` | `aggregate_exposure_sums_positions_three_cells_took_independently` |
// | margin call and room to open | `margin_rises_with_concentration_at_the_same_notional` | the same test |
// | participation rate for a liquidity assessment | `a_liquidity_assessment_needs_a_participation_rate_in_the_unit_interval` (premise) | the same test |
// | recall window and acknowledgement target | `a_recall_needs_a_positive_window_and_an_outstanding_order_to_acknowledge` (premise) | the same test |

#[test]
fn a_reservation_that_names_nothing_is_refused() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve("named", dec!("1"), start(), Duration::from_hours(1))?;
    let refused = ledger
        .reserve("   ", dec!("1"), start(), Duration::from_hours(1))
        .expect_err("a nameless reservation was held");
    assert_eq!(refused.code(), "invalid");
    assert_eq!(
        ledger.free(start()),
        dec!("999999"),
        "a refusal moved the balance"
    );
    Ok(())
}

#[test]
fn a_reservation_for_a_non_positive_amount_is_refused() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve("positive", dec!("1"), start(), Duration::from_hours(1))?;
    for amount in [Decimal::ZERO, dec!("-1")] {
        let refused = ledger
            .reserve("nothing", amount, start(), Duration::from_hours(1))
            .expect_err("a reservation of nothing was held");
        assert_eq!(refused.code(), "invalid");
    }
    assert!(ledger.reservation("nothing").is_none());
    assert_eq!(ledger.free(start()), dec!("999999"));
    Ok(())
}

#[test]
fn a_reservation_with_no_validity_is_refused() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve("timed", dec!("1"), start(), Duration::from_secs(1))?;
    let refused = ledger
        .reserve("untimed", dec!("1"), start(), Duration::ZERO)
        .expect_err("a reservation that expires at its own creation was held");
    assert_eq!(refused.code(), "invalid");
    assert!(ledger.reservation("untimed").is_none());
    Ok(())
}

#[test]
fn a_second_reservation_under_the_same_id_is_refused_until_the_first_ends() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve("p", dec!("1000"), start(), Duration::from_hours(1))?;
    let refused = ledger
        .reserve("p", dec!("1000"), start(), Duration::from_hours(1))
        .expect_err("the same id held twice");
    assert_eq!(refused.code(), "invalid");
    assert_eq!(
        ledger.free(start()),
        dec!("999000"),
        "the second hold took capital"
    );
    ledger.release("p", start())?;
    ledger.reserve("p", dec!("1000"), start(), Duration::from_hours(1))?;
    Ok(())
}

#[test]
fn a_ledger_cannot_open_over_a_negative_balance() -> Result<()> {
    assert_eq!(
        ReservationLedger::new(dec!("-1"))
            .expect_err("opened")
            .code(),
        "invalid"
    );
    // Zero is a ledger that refuses everything, which is the right answer
    // for an empty book — not an error.
    let mut empty = ReservationLedger::new(Decimal::ZERO)?;
    assert!(
        empty
            .reserve("p", dec!("1"), start(), Duration::from_hours(1))
            .is_err()
    );
    Ok(())
}

#[test]
fn committing_or_releasing_a_reservation_nobody_made_is_refused() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve("real", dec!("1000"), start(), Duration::from_hours(1))?;
    assert_eq!(
        ledger
            .commit("ghost", start())
            .expect_err("committed nothing")
            .code(),
        "denied"
    );
    assert_eq!(
        ledger
            .release("ghost", start())
            .expect_err("released nothing")
            .code(),
        "denied"
    );
    assert_eq!(ledger.committed_total(), Decimal::ZERO);
    assert_eq!(ledger.free(start()), dec!("999000"));
    Ok(())
}

#[test]
fn resyncing_free_to_equity_reports_a_shortfall_and_floors_free_at_zero() -> Result<()> {
    let mut ledger = ReservationLedger::new(dec!("1000000"))?;
    ledger.reserve("p", dec!("600000"), start(), Duration::from_hours(1))?;
    // Equity above the holds: free is the identity, exactly.
    ledger.resync_free(dec!("900000"), start())?;
    assert_eq!(ledger.free(start()), dec!("300000"));

    // A drawdown below the holds: the shortfall is reported, free is zero,
    // and the next reservation is refused.
    let shortfall = ledger
        .resync_free(dec!("500000"), start())
        .expect_err("holds exceeding equity went unreported");
    assert_eq!(shortfall.code(), "invalid");
    assert_eq!(ledger.free(start()), Decimal::ZERO);
    assert!(
        ledger
            .reserve("q", dec!("1"), start(), Duration::from_hours(1))
            .is_err()
    );
    Ok(())
}

#[test]
fn an_envelope_with_no_validity_period_is_refused() -> Result<()> {
    let issuer = issuer()?;
    issuer.issue(
        &terms(dec!("1000000"), Duration::from_hours(1)),
        &approval(start())?,
        start(),
    )?;
    let refused = issuer
        .issue(
            &terms(dec!("1000000"), Duration::ZERO),
            &approval(start())?,
            start(),
        )
        .expect_err("an envelope that grants nothing was issued");
    assert_eq!(refused.code(), "invalid");
    // The contract's constructor refuses an expiry at or before the grant
    // too, so the code alone cannot tell the issuer's rule from the
    // contract's; the message can.
    assert!(refused.message().contains("grants nothing"), "{refused}");
    Ok(())
}

#[test]
fn an_envelope_fraction_outside_zero_to_one_is_refused() -> Result<()> {
    let issuer = issuer()?;
    let base = terms(dec!("1000000"), Duration::from_hours(1));
    // The boundary is inclusive at one: a grant whose single order may be
    // the whole grant is a legitimate term.
    issuer.issue(
        &base.clone().with_order_fraction(Decimal::ONE),
        &approval(start())?,
        start(),
    )?;
    for (label, terms) in [
        (
            "zero order",
            base.clone().with_order_fraction(Decimal::ZERO),
        ),
        (
            "order above one",
            base.clone().with_order_fraction(dec!("1.5")),
        ),
        (
            "negative loss",
            base.clone().with_loss_fraction(dec!("-0.1")),
        ),
        ("loss above one", base.clone().with_loss_fraction(dec!("2"))),
    ] {
        let refused = issuer
            .issue(&terms, &approval(start())?, start())
            .expect_err(label);
        assert_eq!(refused.code(), "invalid", "{label}");
    }
    Ok(())
}

#[test]
fn an_issuer_with_a_short_key_or_no_key_id_is_refused() {
    assert!(EnvelopeIssuer::new(vec![7u8; 32], "k").is_ok());
    assert_eq!(
        EnvelopeIssuer::new(vec![7u8; 31], "k")
            .expect_err("a guessable key was accepted")
            .code(),
        "denied"
    );
    assert!(EnvelopeIssuer::new(vec![7u8; 32], "  ").is_err());
}

#[test]
fn an_expired_envelope_does_not_verify() -> Result<()> {
    let issuer = issuer()?;
    let envelope = issuer.issue(
        &terms(dec!("1000000"), Duration::from_hours(1)),
        &approval(start())?,
        start(),
    )?;
    issuer.verify(&envelope, start())?;
    let refused = issuer
        .verify(&envelope, envelope.expires_at())
        .expect_err("a grant verified at its own expiry");
    assert!(refused.message().contains("expired"), "{refused}");
    Ok(())
}

#[test]
fn each_allocation_limit_binds_and_is_named() -> Result<()> {
    // Deep volume so capacity does not bind and the four capital limits are
    // the only constraints in play; the premise below checks that.
    let deep = |name: &str, cell: &str| proposal(name, cell, 2.0, 0.1, 5_000_000_000);
    let plan = |limits: AllocationLimits, proposals: &[StrategyProposal]| {
        CapitalAllocator::new(limits, DrawdownSchedule::default()).allocate(proposals, 0.0, start())
    };
    let named = |plan: &qip_capital::allocation::AllocationPlan, strategy: &str| -> Vec<String> {
        plan.for_strategy(&StrategyId::new(strategy))
            .map(|a| a.binding_constraints.clone())
            .or_else(|| {
                plan.refusals
                    .iter()
                    .find(|(id, _)| id.as_str() == strategy)
                    .map(|(_, reason)| vec![reason.clone()])
            })
            .unwrap_or_default()
    };

    // Per strategy: one proposal takes the whole budget and is cut to 30m.
    let plan_a = plan(limits()?, &[deep("solo", "cell-a")?])?;
    let solo = plan_a
        .for_strategy(&StrategyId::new("solo"))
        .expect("allocated");
    assert!(solo.indicated > solo.notional, "premise: something bound");
    assert!(
        !named(&plan_a, "solo")
            .iter()
            .any(|r| r.contains("capacity")),
        "premise failed: capacity bound, so the limit under test may not have: {:?}",
        named(&plan_a, "solo")
    );
    assert_eq!(solo.notional, dec!("30000000"));
    assert!(
        named(&plan_a, "solo")
            .iter()
            .any(|r| r.contains("per-strategy"))
    );

    // Per cell: three in one 60m cell, the third has no headroom.
    let plan_b = plan(
        limits()?,
        &[
            deep("c1", "cell-a")?,
            deep("c2", "cell-a")?,
            deep("c3", "cell-a")?,
        ],
    )?;
    assert_eq!(plan_b.for_cell("cell-a"), dec!("60000000"));
    assert!(
        named(&plan_b, "c3")
            .iter()
            .any(|r| r.contains("cell cell-a")),
        "{:?}",
        named(&plan_b, "c3")
    );

    // Per venue: four cells at one 90m venue, the fourth has no headroom.
    let plan_c = plan(
        limits()?,
        &[
            deep("v1", "cell-a")?,
            deep("v2", "cell-b")?,
            deep("v3", "cell-c")?,
            deep("v4", "cell-d")?,
        ],
    )?;
    assert_eq!(plan_c.for_venue(&venue()), dec!("90000000"));
    assert!(
        named(&plan_c, "v4").iter().any(|r| r.contains("venue")),
        "{:?}",
        named(&plan_c, "v4")
    );

    // Total: the backstop for the rounding step the module documents. Seven
    // equal shares of a seventh round up at nine decimals, so the seven
    // indicated sizes sum to 100,000,000.1 against a 100m budget, and the
    // seventh is cut by the 0.1 the others already took. With the venue
    // widened nothing else can bind.
    let plan_d = plan(
        limits()?.with_venue_limit(venue(), dec!("200000000")),
        &[
            deep("t1", "cell-a")?,
            deep("t2", "cell-b")?,
            deep("t3", "cell-c")?,
            deep("t4", "cell-d")?,
            deep("t5", "cell-e")?,
            deep("t6", "cell-f")?,
            deep("t7", "cell-g")?,
        ],
    )?;
    let t7 = plan_d
        .for_strategy(&StrategyId::new("t7"))
        .expect("allocated");
    assert_eq!(
        t7.indicated,
        dec!("14285714.3"),
        "premise: the share rounded up"
    );
    assert_eq!(t7.notional, dec!("14285714.2"));
    assert!(
        t7.binding_constraints
            .iter()
            .any(|r| r.contains("total budget")),
        "{:?}",
        t7.binding_constraints
    );
    assert_eq!(plan_d.allocated(), dec!("100000000"));
    assert!(plan_d.is_within_budget());
    Ok(())
}

#[test]
fn allocation_limits_refuse_a_non_positive_budget_or_a_negative_limit() -> Result<()> {
    assert!(AllocationLimits::new(dec!("1"), Decimal::ZERO, Decimal::ZERO, Decimal::ZERO).is_ok());
    assert!(AllocationLimits::new(Decimal::ZERO, dec!("1"), dec!("1"), dec!("1")).is_err());
    assert!(AllocationLimits::new(dec!("1"), dec!("-1"), dec!("1"), dec!("1")).is_err());
    assert!(AllocationLimits::new(dec!("1"), dec!("1"), dec!("-1"), dec!("1")).is_err());
    assert!(AllocationLimits::new(dec!("1"), dec!("1"), dec!("1"), dec!("-1")).is_err());
    assert!(allocator()?.with_uncertainty_penalty(0.5).is_ok());
    assert!(allocator()?.with_uncertainty_penalty(-0.5).is_err());
    assert!(allocator()?.with_uncertainty_penalty(f64::NAN).is_err());
    Ok(())
}

#[test]
fn a_diversified_book_produces_no_concentration_finding() {
    let sectors = [
        Sector::InformationTechnology,
        Sector::Energy,
        Sector::Financials,
        Sector::Industrials,
    ];
    let positions: Vec<CellPosition> = (0..12)
        .map(|i| CellPosition {
            cell: format!("cell-{}", i % 3),
            strategy: StrategyId::new(format!("s-{i}")),
            instrument: format!("NAME{i:02}"),
            sector: sectors[i % 4],
            venue: VenueId::new(format!("VEN{}", i % 3)),
            currency: if i % 2 == 0 {
                Currency::USD
            } else {
                Currency::EUR
            },
            quantity: dec!("1000"),
            price: dec!("100"),
        })
        .collect();
    let aggregate = AggregateExposure::of(&positions);
    // Premise: the book has exposure, and the rule reads it — a tightened
    // instrument limit produces findings on the same book.
    assert!(aggregate.gross().is_positive());
    let tightened = ConcentrationLimits {
        per_instrument: 0.05,
        ..ConcentrationLimits::default()
    };
    assert!(!aggregate.concentrations(&tightened).is_empty());

    let findings = aggregate.concentrations(&ConcentrationLimits::default());
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_liquidity_assessment_needs_a_participation_rate_in_the_unit_interval() -> Result<()> {
    let positions = crowded_book();
    let profiles: BTreeMap<String, LiquidityProfile> = ["ACME", "BOREAS", "CYGNUS", "DORADO"]
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                LiquidityProfile::listed(Decimal::from_int(1_000_000), 5.0),
            )
        })
        .collect();
    assess_liquidity(&positions, &profiles, 0.1)?;
    for rate in [0.0, -0.1, 1.5, f64::NAN] {
        assert!(
            assess_liquidity(&positions, &profiles, rate).is_err(),
            "a participation rate of {rate} was accepted"
        );
    }
    Ok(())
}

#[test]
fn a_recall_needs_a_positive_window_and_an_outstanding_order_to_acknowledge() -> Result<()> {
    let issuer = issuer()?;
    let envelope = issuer.issue(
        &terms(dec!("1000000"), Duration::from_hours(6)),
        &approval(start())?,
        start(),
    )?;
    let mut register = RecallRegister::new();
    assert!(
        register
            .issue(
                &envelope,
                RecallReason::Precautionary,
                "drill",
                Duration::ZERO,
                start()
            )
            .is_err()
    );
    assert!(
        register.orders().next().is_none(),
        "a refused recall was recorded"
    );

    // Acknowledging a recall nobody issued is refused, not invented.
    assert!(
        register
            .acknowledge(
                "cell-lon-1",
                &StrategyId::new("momentum-v3"),
                Decimal::ZERO,
                start()
            )
            .is_err()
    );
    register.issue(
        &envelope,
        RecallReason::Precautionary,
        "drill",
        Duration::from_mins(5),
        start(),
    )?;
    register.acknowledge(
        "cell-lon-1",
        &StrategyId::new("momentum-v3"),
        Decimal::ZERO,
        start(),
    )?;
    assert!(matches!(
        register.state("cell-lon-1", &StrategyId::new("momentum-v3")),
        Some(RecallState::Acknowledged { .. })
    ));
    Ok(())
}
