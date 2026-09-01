//! The invariants every other subsystem is entitled to rely on.
//!
//! These types are the seams between fifteen crates. A contract that holds
//! only by convention is one that fails at the seam, where the two sides each
//! assumed the other was checking — so each invariant is asserted here once,
//! centrally, rather than trusted fifteen times.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::{CapitalEnvelope, CapitalGrant, Utilisation};
use qip_contracts::edge::{Deduction, DeductionKind, LegPlan, LegStep, NetEdge};
use qip_contracts::feature::{FeatureKey, FeatureValue, FeatureVector, Revision};
use qip_contracts::gate::GateStage;
use qip_contracts::governance::{Approval, Control, Entitlement, Provenance, Severity, Usage};
use qip_contracts::message::{BookSide, TradeCondition};
use qip_contracts::signal::{Conviction, StrategyId};
use qip_contracts::time::{Stamped, Watermark};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

// --- bitemporal truth -------------------------------------------------------

#[test]
fn a_fact_can_never_be_known_before_it_was_true() {
    // The combination has no physical meaning, and every time it appears it is
    // a clock or a parser rather than a prescient feed. Clamping keeps the
    // message and makes the anomaly visible.
    let stamped = Stamped::new("a price", t(100), t(50));
    assert_eq!(stamped.known_at(), t(100));
    assert!(stamped.was_clamped());
    assert!(stamped.latency().as_nanos() >= 0);
}

#[test]
fn a_point_in_time_read_filters_on_known_time_not_valid_time() {
    // The distinction that decides whether a backtest is honest. The fact was
    // true at t=100 and only knowable at t=160; a reader asking "as of 120"
    // must not see it.
    let late = Stamped::new("a revised figure", t(100), t(160));
    assert!(
        !late.was_known_by(t(120)),
        "a look-ahead read was permitted"
    );
    assert!(late.was_known_by(t(160)));
    assert!(late.was_known_by(t(200)));
    assert_eq!(late.valid_at(), t(100), "valid time is preserved as stated");
}

#[test]
fn a_watermark_promises_contiguity_and_never_retreats() {
    // A watermark that can go backwards is not a promise, and whoever trusted
    // the earlier value has already acted on it.
    let mut mark = Watermark::new("xnys/itch-a/0", 100, t(0));
    assert!(mark.advance_to(140, t(1)));
    assert!(!mark.advance_to(120, t(2)), "the watermark retreated");
    assert!(!mark.advance_to(140, t(3)), "a repeat is not an advance");
    assert_eq!(mark.position, 140);
}

#[test]
fn sequence_adjacency_is_scoped_to_one_stream() {
    // Sequence numbers from different partitions are not comparable, and
    // treating them as though they were manufactures gaps that are not there.
    let venue = VenueId::new("XNYS");
    let first = Origin::new(venue.clone(), "itch-a", 0, 41);
    let same_stream = Origin::new(venue.clone(), "itch-a", 0, 42);
    let other_partition = Origin::new(venue.clone(), "itch-a", 1, 42);
    let other_feed = Origin::new(venue, "itch-b", 0, 42);

    assert!(same_stream.directly_follows(&first));
    assert!(!other_partition.directly_follows(&first));
    assert!(!other_feed.directly_follows(&first));
}

// --- net edge ---------------------------------------------------------------

#[test]
fn a_net_edge_is_computed_from_its_parts_and_cannot_be_asserted() -> Result<()> {
    // There is deliberately no constructor accepting a net figure, so a caller
    // cannot report a number its own deductions contradict.
    let edge = NetEdge::gross(dec!("100"), dec!("1000"))?
        .deduct(Deduction::new(
            DeductionKind::Spread,
            dec!("30"),
            "half-spread × 2",
        )?)
        .deduct(Deduction::new(
            DeductionKind::Fees,
            dec!("12"),
            "taker both legs",
        )?);

    assert_eq!(edge.total_deducted(), dec!("42"));
    assert_eq!(edge.net(), dec!("58"));
    assert_eq!(edge.net(), edge.gross_edge() - edge.total_deducted());
    Ok(())
}

#[test]
fn an_edge_that_skipped_a_cost_refuses_to_call_itself_complete() -> Result<()> {
    // The most common way a strategy flatters itself is by not modelling a
    // cost at all, which looks identical to modelling it as zero.
    let partial = NetEdge::gross(dec!("100"), dec!("1000"))?.deduct(Deduction::new(
        DeductionKind::Spread,
        dec!("10"),
        "touch",
    )?);
    let missing = partial.unconsidered();
    assert_eq!(missing.len(), 8, "{missing:?}");
    assert!(partial.require_complete().is_err());

    // The two the platform charges itself are as mandatory as the seven the
    // market charges. An edge that considered every market cost and neither of
    // its own is the exact shape of a strategy that is profitable per trade and
    // loses money per month, so it is refused by name rather than by count.
    let refusal = partial
        .require_complete()
        .expect_err("an edge missing eight deductions is not complete");
    assert!(refusal.message().contains("compute_cost"), "{refusal}");
    assert!(refusal.message().contains("data_cost"), "{refusal}");

    let complete = DeductionKind::all().into_iter().try_fold(
        NetEdge::gross(dec!("100"), dec!("1000"))?,
        |edge, kind| -> Result<NetEdge> {
            Ok(edge.deduct(Deduction::new(kind, dec!("5"), "modelled")?))
        },
    )?;
    complete.require_complete()?;
    assert_eq!(complete.net(), dec!("55"));
    Ok(())
}

#[test]
fn an_edge_that_clears_its_market_costs_but_not_its_compute_cost_is_negative() -> Result<()> {
    // The reason compute cost is a deduction and not a budget line. Every
    // market cost is covered with room to spare, and the decision still lost
    // money because reaching it cost more than it was worth. An accounting
    // that put the inference bill anywhere else would report this as a
    // twelve-unit win.
    let edge = NetEdge::gross(dec!("100"), dec!("1000"))?
        .deduct(Deduction::new(DeductionKind::Spread, dec!("40"), "touch")?)
        .deduct(Deduction::new(DeductionKind::Fees, dec!("20"), "taker")?)
        .deduct(Deduction::new(
            DeductionKind::Latency,
            dec!("10"),
            "round trip",
        )?)
        .deduct(Deduction::new(DeductionKind::Slippage, dec!("8"), "sweep")?)
        .deduct(Deduction::new(DeductionKind::Funding, dec!("4"), "carry")?)
        .deduct(Deduction::new(
            DeductionKind::Collateral,
            dec!("3"),
            "margin",
        )?)
        .deduct(Deduction::new(
            DeductionKind::Uncertainty,
            dec!("3"),
            "haircut",
        )?)
        .deduct(Deduction::new(
            DeductionKind::ComputeCost,
            dec!("25"),
            "a deep model and two agent calls to reach the view",
        )?)
        .deduct(Deduction::new(
            DeductionKind::DataCost,
            dec!("2"),
            "amortised licence cost of the sources read",
        )?);

    edge.require_complete()?;
    assert_eq!(
        edge.gross_edge() - dec!("88"),
        dec!("12"),
        "the market costs alone leave the trade in profit"
    );
    assert_eq!(edge.net(), dec!("-15"));
    assert!(
        !edge.is_positive(),
        "an opportunity that earns less than it cost to find is not an opportunity: {}",
        edge.summarise()
    );
    Ok(())
}

#[test]
fn a_rebate_is_a_smaller_fee_rather_than_a_negative_cost() {
    // Allowing a negative deduction would let one line item manufacture edge,
    // and the sum would stop meaning "everything taken off the top".
    assert!(Deduction::new(DeductionKind::Fees, dec!("-3"), "maker rebate").is_err());
    assert!(Deduction::new(DeductionKind::Fees, dec!("0"), "rebate nets to nil").is_ok());
}

#[test]
fn an_unprofitable_edge_reports_itself_as_such_rather_than_flooring_at_zero() -> Result<()> {
    let edge = NetEdge::gross(dec!("10"), dec!("100"))?.deduct(Deduction::new(
        DeductionKind::Slippage,
        dec!("25"),
        "swept three levels",
    )?);
    assert_eq!(edge.net(), dec!("-15"));
    assert!(!edge.is_positive());
    Ok(())
}

#[test]
fn net_edge_requires_the_size_it_is_quoted_for() {
    // Edge does not scale linearly, so a net figure without a size is not a
    // number anybody can act on.
    assert!(NetEdge::gross(dec!("100"), Decimal::ZERO).is_err());
    assert!(NetEdge::gross(dec!("100"), dec!("-1")).is_err());
}

// --- leg plans --------------------------------------------------------------

fn leg(venue: &str, order: u16, optional: bool, quantity: &str) -> LegStep {
    LegStep {
        object_id: object("ACME"),
        venue: VenueId::new(venue),
        side: BookSide::Bid,
        quantity: Decimal::parse(quantity).expect("a decimal literal"),
        reference_price: dec!("100"),
        priced_in: object("USD"),
        order,
        optional,
    }
}

#[test]
fn a_mixed_currency_residual_is_reported_per_unit_rather_than_as_one_sum() -> Result<()> {
    // The number a leg-risk budget is compared against must have units. The
    // arbitrage planner found this the hard way: a EUR leg and a USD leg
    // summed into one figure is a screen, not a measurement.
    let mut eur_leg = leg("XETR", 2, false, "100");
    eur_leg.priced_in = object("EUR");
    let plan = LegPlan::new(vec![leg("XNYS", 1, false, "100"), eur_leg])?;

    let by_unit = plan.residual_by_unit(0);
    assert_eq!(by_unit.len(), 2, "two currencies collapsed into one figure");
    for (_, amount) in &by_unit {
        assert_eq!(*amount, dec!("10000"));
    }
    // The gross screen still spans both — usable as a refusal bound, and the
    // per-unit breakdown is where the meaning is.
    assert_eq!(plan.residual_after(0), dec!("20000"));
    assert_eq!(
        plan.residual_by_unit(2),
        Vec::new(),
        "a finished plan holds nothing"
    );
    Ok(())
}

#[test]
fn a_leg_plan_orders_its_legs_and_counts_only_what_it_cannot_abandon() -> Result<()> {
    // Residual exposure is what the leg-risk budget is sized against, and an
    // optional leg left undone costs nothing.
    let plan = LegPlan::new(vec![
        leg("XNYS", 2, false, "100"),
        leg("XNAS", 1, false, "100"),
        leg("ARCA", 3, true, "100"),
    ])?;

    let venues: Vec<&str> = plan.venues().into_iter().map(VenueId::as_str).collect();
    assert_eq!(venues, vec!["XNAS", "XNYS", "ARCA"], "legs were not sorted");
    assert_eq!(plan.mandatory().len(), 2);
    assert_eq!(plan.residual_after(0), dec!("20000"));
    assert_eq!(plan.residual_after(1), dec!("10000"));
    assert_eq!(
        plan.residual_after(2),
        Decimal::ZERO,
        "an optional trailing leg is not residual exposure"
    );
    Ok(())
}

#[test]
fn a_plan_with_no_legs_is_refused() {
    assert!(LegPlan::new(Vec::new()).is_err());
}

// --- gates ------------------------------------------------------------------

#[test]
fn no_path_to_capital_skips_running_against_live_data() {
    // A strategy that has never seen live data has not been tested, however
    // good its backtest. Shadow is the rung that cannot be jumped.
    let mut stage = GateStage::Candidate;
    let mut walked = vec![stage];
    while let Some(next) = stage.next() {
        stage = next;
        walked.push(stage);
    }
    assert_eq!(*walked.last().expect("a path"), GateStage::Scaled);
    assert!(walked.contains(&GateStage::Shadow));

    let first_with_capital = walked
        .iter()
        .position(|s| s.holds_capital())
        .expect("some stage holds capital");
    let shadow = walked
        .iter()
        .position(|s| *s == GateStage::Shadow)
        .expect("shadow is on the path");
    assert!(shadow < first_with_capital, "capital before shadow");
}

#[test]
fn every_stage_that_can_lose_money_needs_a_human() {
    for stage in GateStage::all() {
        assert_eq!(
            stage.holds_capital(),
            stage.requires_human_approval(),
            "{} disagrees about whether a person is needed",
            stage.as_str()
        );
        assert_eq!(stage.holds_capital(), stage.may_reach_a_venue());
    }
}

#[test]
fn retirement_is_terminal_so_evidence_must_be_re_earned() {
    assert!(GateStage::Retired.next().is_none());
    assert!(!GateStage::Retired.holds_capital());
}

// --- capital ----------------------------------------------------------------

fn envelope(gross: &str, order: &str, loss: &str) -> Result<CapitalEnvelope> {
    CapitalEnvelope::new(
        StrategyId::new("mean-reversion-1"),
        "cell-nynj",
        Decimal::parse(gross).expect("a decimal literal"),
        Decimal::parse(order).expect("a decimal literal"),
        Decimal::parse(loss).expect("a decimal literal"),
        vec![VenueId::new("XNYS")],
        t(0),
        t(3600),
        "alice@example.com",
        "signature-placeholder",
    )
}

#[test]
fn an_envelope_that_could_be_widened_in_place_would_not_be_a_limit() -> Result<()> {
    // Every bound is private with no setter: widening means going back to the
    // central plane for a new grant, which is the point.
    let grant = envelope("1000000", "100000", "50000")?;
    assert_eq!(grant.gross_limit(), dec!("1000000"));
    assert!(grant.is_live(t(10)));
    assert!(!grant.is_live(t(3600)), "an envelope must expire");
    assert!(!grant.is_live(t(-1)));
    Ok(())
}

#[test]
fn an_envelope_refuses_the_shapes_that_would_make_it_meaningless() {
    assert!(envelope("0", "0", "0").is_err(), "a zero limit");
    assert!(
        envelope("1000", "5000", "100").is_err(),
        "one order committing more than the whole envelope"
    );
    assert!(
        CapitalEnvelope::new(
            StrategyId::new("s"),
            "cell",
            dec!("100"),
            dec!("10"),
            dec!("10"),
            vec![],
            t(0),
            t(3600),
            "   ",
            "sig",
        )
        .is_err(),
        "an unnamed approver"
    );
    assert!(
        CapitalEnvelope::new(
            StrategyId::new("s"),
            "cell",
            dec!("100"),
            dec!("10"),
            dec!("10"),
            vec![],
            t(3600),
            t(0),
            "alice",
            "sig",
        )
        .is_err(),
        "an envelope expiring before it was granted"
    );
}

#[test]
fn an_empty_venue_list_grants_no_venues_rather_than_all_of_them() -> Result<()> {
    // The permissive reading of an empty list is how a grant silently leaks
    // across every venue the platform can reach.
    let grant = CapitalEnvelope::new(
        StrategyId::new("s"),
        "cell",
        dec!("1000"),
        dec!("100"),
        dec!("100"),
        vec![],
        t(0),
        t(3600),
        "alice",
        "sig",
    )?;
    assert!(!grant.permits_venue(&VenueId::new("XNYS")));
    assert!(
        grant
            .admit(
                &VenueId::new("XNYS"),
                dec!("10"),
                &Utilisation::default(),
                t(1)
            )
            .is_refused()
    );
    Ok(())
}

#[test]
fn every_capital_refusal_names_the_bound_that_stopped_it() -> Result<()> {
    // "Refused" without a reason is the least actionable message an execution
    // system can produce at three in the morning.
    let grant = envelope("1000", "400", "100")?;
    let venue = VenueId::new("XNYS");

    match grant.admit(&venue, dec!("10"), &Utilisation::default(), t(4000)) {
        CapitalGrant::Refused(why) => assert!(why.contains("expired"), "{why}"),
        other => panic!("an expired envelope admitted an order: {other:?}"),
    }

    let breached = Utilisation {
        gross_committed: Decimal::ZERO,
        realised_loss: dec!("150"),
        orders_sent: 3,
    };
    match grant.admit(&venue, dec!("10"), &breached, t(10)) {
        CapitalGrant::Refused(why) => assert!(why.contains("loss"), "{why}"),
        other => panic!("a breached loss limit admitted an order: {other:?}"),
    }

    let full = Utilisation {
        gross_committed: dec!("1000"),
        realised_loss: Decimal::ZERO,
        orders_sent: 9,
    };
    match grant.admit(&venue, dec!("10"), &full, t(10)) {
        CapitalGrant::Refused(why) => assert!(why.contains("gross"), "{why}"),
        other => panic!("a fully committed envelope admitted an order: {other:?}"),
    }
    Ok(())
}

#[test]
fn an_oversized_order_is_reduced_rather_than_approved_as_requested() -> Result<()> {
    // A `Reduced` that a caller can mistake for approval is how an order goes
    // out at the size the strategy wanted rather than the size it was allowed.
    let grant = envelope("1000", "400", "100")?;
    let venue = VenueId::new("XNYS");
    let used = Utilisation {
        gross_committed: dec!("800"),
        realised_loss: Decimal::ZERO,
        orders_sent: 1,
    };
    // Headroom is 200, below the 400 per-order cap, so the tighter one binds.
    match grant.admit(&venue, dec!("400"), &used, t(10)) {
        CapitalGrant::Reduced(size) => {
            assert_eq!(size, dec!("200"));
            assert_eq!(
                CapitalGrant::Reduced(size).permitted_quantity(dec!("400")),
                dec!("200")
            );
        }
        other => panic!("expected a reduction, got {other:?}"),
    }
    assert!(matches!(
        grant.admit(&venue, dec!("150"), &used, t(10)),
        CapitalGrant::Full
    ));
    Ok(())
}

#[test]
fn the_signing_payload_covers_every_bound_it_claims_to_authorise() -> Result<()> {
    // A signature that does not cover a limit is a signature over the wrong
    // thing, and the limit it omits is the one that will be edited.
    let base = envelope("1000", "400", "100")?;
    let wider = envelope("2000", "400", "100")?;
    let looser_order = envelope("1000", "500", "100")?;
    let looser_loss = envelope("1000", "400", "900")?;

    assert_ne!(base.signing_payload(), wider.signing_payload());
    assert_ne!(base.signing_payload(), looser_order.signing_payload());
    assert_ne!(base.signing_payload(), looser_loss.signing_payload());
    assert!(base.signing_payload().contains("alice@example.com"));
    Ok(())
}

// --- governance -------------------------------------------------------------

#[test]
fn a_second_approver_who_is_the_first_approver_is_not_a_second_approver() -> Result<()> {
    let approval = Approval::new(
        "promote mean-reversion-1 to pilot",
        "alice@example.com",
        t(0),
        "shadow agreement held for six weeks",
    )?;
    assert!(!approval.is_dual());
    assert!(
        approval
            .clone()
            .countersigned_by("alice@example.com")
            .is_err()
    );
    assert!(approval.countersigned_by("bob@example.com")?.is_dual());
    Ok(())
}

#[test]
fn an_approval_without_a_reviewable_rationale_is_refused() {
    assert!(Approval::new("subject", "alice", t(0), "ok").is_err());
    assert!(Approval::new("subject", "", t(0), "a perfectly good reason").is_err());
}

#[test]
fn a_provenance_cannot_disagree_with_the_bytes_it_describes() -> Result<()> {
    // The digest is computed, never supplied, so the mismatched case has no
    // constructor.
    let artifact = b"model weights v1";
    let signed = Provenance::sign(
        artifact,
        "build-service",
        "hmac",
        t(0),
        vec!["dataset@abc".to_string()],
    )?;
    assert!(signed.matches(artifact));
    assert!(
        !signed.matches(b"model weights v2"),
        "tampering went unnoticed"
    );
    assert!(signed.reference().starts_with("sha256:"));
    assert_eq!(signed.inputs().len(), 1);
    assert!(Provenance::sign(artifact, "  ", "hmac", t(0), vec![]).is_err());
    Ok(())
}

#[test]
fn a_licence_for_research_does_not_licence_a_trade() -> Result<()> {
    // The common real case, and collapsing the two usages is how a licence is
    // breached by a backtest that got promoted.
    let research = Entitlement::Granted {
        dataset: "vendor-alpha".to_string(),
        usage: Usage::Research,
        expires_at: t(3600),
    };
    let trading = Entitlement::Denied {
        dataset: "vendor-alpha".to_string(),
        usage: Usage::Trade,
        reason: "the agreement covers internal research only".to_string(),
    };
    assert!(research.is_granted(t(10)));
    assert!(
        !research.is_granted(t(3600)),
        "an expired licence still granted"
    );
    assert!(!trading.is_granted(t(10)));
    assert!(trading.describe().contains("not licensed"));
    assert_eq!(research.dataset(), trading.dataset());
    Ok(())
}

#[test]
fn severity_decides_what_stops_and_only_observation_stops_nothing() {
    assert!(!Severity::Observation.halts_something());
    for severity in [Severity::Scoped, Severity::Cell, Severity::Global] {
        assert!(severity.halts_something(), "{}", severity.as_str());
    }
    assert!(Severity::Global > Severity::Cell);
    assert!(Severity::Cell > Severity::Scoped);
}

#[test]
fn the_six_controls_are_named_once_so_a_test_can_enumerate_them() {
    // The list is what makes "fully compliant" checkable instead of a claim.
    let controls = Control::all();
    assert_eq!(controls.len(), 6);
    let mut names: Vec<&str> = controls.iter().map(Control::as_str).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 6, "two controls share a name");
}

// --- features ---------------------------------------------------------------

#[test]
fn a_feature_key_is_canonical_however_its_parameters_were_supplied() -> Result<()> {
    // Two strategies asking the same question must produce the same key, or
    // the DAG computes it twice and sharing buys nothing.
    let one = FeatureKey::new("realised_vol", object("ACME"))
        .with("window", 20)
        .with("basis", "log");
    let other = FeatureKey::new("realised_vol", object("ACME"))
        .with("basis", "log")
        .with("window", 20);
    assert_eq!(one.canonical(), other.canonical());
    assert_eq!(one, other);

    let different = FeatureKey::new("realised_vol", object("ACME")).with("window", 30);
    assert_ne!(one.canonical(), different.canonical());
    Ok(())
}

#[test]
fn a_repeated_parameter_does_not_change_the_key() {
    let key = FeatureKey::new("ema", object("ACME"))
        .with("window", 12)
        .with("window", 12);
    assert_eq!(key.parameters.len(), 1);
}

#[test]
fn an_uncomputable_feature_is_undefined_rather_than_zero() {
    // A default that looks like data is how a strategy trades on nothing.
    assert_eq!(FeatureValue::Undefined.as_f64(), None);
    assert_eq!(FeatureValue::Undefined.as_exact(), None);
    assert!(!FeatureValue::Undefined.is_defined());
    assert_eq!(FeatureValue::Statistic(0.0).as_f64(), Some(0.0));
    assert!(FeatureValue::Statistic(0.0).is_defined());
}

#[test]
fn a_statistic_is_not_an_exact_quantity() {
    // The two are separate variants so a caller cannot route a volatility into
    // a price without noticing.
    assert_eq!(
        FeatureValue::Exact(dec!("1.5")).as_exact(),
        Some(dec!("1.5"))
    );
    assert_eq!(FeatureValue::Statistic(1.5).as_exact(), None);
}

#[test]
fn a_vector_reports_exactly_which_inputs_were_missing() {
    let mut vector = FeatureVector::new(t(0));
    let good = FeatureKey::new("ema", object("ACME"));
    let bad = FeatureKey::new("realised_vol", object("ACME"));
    vector.insert(good.clone(), FeatureValue::Statistic(1.2), Revision::new(4));
    vector.insert(bad.clone(), FeatureValue::Undefined, Revision::new(4));

    assert!(!vector.is_complete());
    assert_eq!(vector.undefined(), vec![&bad]);
    assert_eq!(vector.revision_of(&good), Some(Revision::new(4)));

    vector.insert(bad.clone(), FeatureValue::Statistic(0.3), Revision::new(5));
    assert!(vector.is_complete());
    assert_eq!(vector.len(), 2, "re-inserting a key duplicated it");
    assert_eq!(vector.revision_of(&bad), Some(Revision::new(5)));
}

// --- conviction -------------------------------------------------------------

#[test]
fn a_belief_with_no_evidence_shrinks_to_a_coin_flip() {
    // The honest answer for a 0.95 backed by nothing, and the reason the
    // allocator is given the sample size rather than only the probability.
    let unsupported = Conviction::new(0.95, 0);
    assert!((unsupported.shrunk() - 0.5).abs() < 1e-12);
    assert!(!unsupported.clears(0.6));

    let supported = Conviction::new(0.95, 4000);
    assert!(supported.shrunk() > 0.9);
    assert!(supported.clears(0.6));
}

#[test]
fn shrinkage_is_monotone_in_evidence_and_never_overshoots() {
    let mut previous = 0.5;
    for observations in [1_u32, 10, 100, 1_000, 100_000] {
        let shrunk = Conviction::new(0.8, observations).shrunk();
        assert!(shrunk > previous, "shrinkage was not monotone");
        assert!(shrunk <= 0.8 + 1e-12, "shrinkage exceeded the raw belief");
        previous = shrunk;
    }
}

#[test]
fn an_out_of_range_probability_is_clamped_rather_than_dropped() {
    assert!((Conviction::new(1.4, 10).probability() - 1.0).abs() < 1e-12);
    assert!((Conviction::new(-0.2, 10).probability() - 0.0).abs() < 1e-12);
}

// --- venue and message semantics --------------------------------------------

#[test]
fn better_means_the_opposite_thing_on_each_side_of_the_book() {
    // Got wrong at least once in every order book ever written, so it is
    // defined once here and tested rather than re-derived per call site.
    assert!(BookSide::Bid.is_better(dec!("101"), dec!("100")));
    assert!(!BookSide::Bid.is_better(dec!("99"), dec!("100")));
    assert!(BookSide::Ask.is_better(dec!("99"), dec!("100")));
    assert!(!BookSide::Ask.is_better(dec!("101"), dec!("100")));
    assert_eq!(BookSide::Bid.opposite(), BookSide::Ask);
    assert_eq!(BookSide::Ask.opposite().opposite(), BookSide::Ask);
}

#[test]
fn a_correction_does_not_move_the_last_price_or_the_volume() {
    // Treating every print as a trade is how a mid gets dragged by an odd lot.
    assert!(TradeCondition::Regular.updates_last());
    assert!(TradeCondition::Auction.updates_last());
    assert!(!TradeCondition::OddLot.updates_last());
    assert!(!TradeCondition::Reported.updates_last());
    assert!(!TradeCondition::Correction.counts_toward_volume());
    assert!(TradeCondition::OddLot.counts_toward_volume());
}

#[test]
fn a_chain_settled_venue_reports_that_its_quotes_are_not_firm() {
    // The leg planner sizes inventory against this, so it has to be a property
    // of the venue class rather than a comment somebody remembers.
    assert!(!VenueClass::DecentralisedExchange.settles_atomically());
    assert!(!VenueClass::DecentralisedExchange.quotes_are_firm());
    assert!(VenueClass::Exchange.settles_atomically());
    assert!(VenueClass::Exchange.quotes_are_firm());
    assert!(!VenueClass::PredictionMarket.quotes_are_firm());
}

#[test]
fn an_unreachable_venue_is_treated_as_untradeable_and_unpriceable() {
    // Distinct from halted: the venue may well be trading and this cell simply
    // cannot see it, which is the more dangerous of the two.
    assert!(!VenueStatus::Unreachable.accepts_orders());
    assert!(!VenueStatus::Unreachable.prices_are_usable());
    assert!(!VenueStatus::Halted.accepts_orders());
    assert!(VenueStatus::Auction.accepts_orders());
    assert!(VenueStatus::Open.prices_are_usable());
    assert!(!VenueStatus::Closed.accepts_orders());
}

#[test]
fn a_stamped_value_keeps_both_times_through_a_transformation() {
    let stamped = Stamped::new(dec!("100"), t(10), t(12));
    let doubled = stamped.map(|price| price + price);
    assert_eq!(*doubled.value(), dec!("200"));
    assert_eq!(doubled.valid_at(), t(10));
    assert_eq!(doubled.known_at(), t(12));
    assert_eq!(doubled.latency(), Duration::from_secs(2));
}

// --- degradation: blueprint §6.2 -------------------------------------------

use qip_contracts::degradation::{
    AllocationMode, Capability, DegradationState, Freshness, StrategyClass,
};

#[test]
fn an_ingestion_stall_pauses_the_strategies_that_need_the_world_and_no_others() {
    // §6.2 row 1. The negative half is the half worth asserting: "price-only
    // strategies continue unaffected" is a promise the platform makes about an
    // outage it is *not* going to inflict on itself.
    let healthy = DegradationState::fully_available();
    // Premise first. If these already paused, the assertions below would pass
    // while proving that the stall did nothing.
    for class in StrategyClass::all() {
        assert!(
            !healthy.pauses(class),
            "{} pauses with every capability fresh, so this test cannot show \
             that an ingestion stall is what pauses it",
            class.as_str()
        );
    }

    let mut stalled = DegradationState::fully_available();
    stalled.observe(Capability::Ingestion, Freshness::Unavailable);

    assert!(stalled.pauses(StrategyClass::EventDriven));
    assert!(stalled.pauses(StrategyClass::PredictionMarket));
    assert!(
        !stalled.pauses(StrategyClass::PriceOnly),
        "a price-only strategy was paused by an ingestion stall it does not \
         depend on; that is an outage the platform inflicted on itself"
    );
    assert!(!stalled.pauses(StrategyClass::SituationalRecognition));
}

#[test]
fn a_stale_causal_graph_reverts_to_unconditional_allocation_and_sizes_smaller() {
    // §6.2 row 2, both halves: the allocation mode changes *and* size becomes
    // more conservative, because relationships can no longer be reasoned
    // about.
    let healthy = DegradationState::fully_available();
    assert_eq!(healthy.allocation_mode(), AllocationMode::RegimeConditional);
    assert_eq!(healthy.sizing_multiplier(), dec!("1"));

    let mut stale = DegradationState::fully_available();
    stale.observe(Capability::CausalGraph, Freshness::Stale);

    assert_eq!(stale.allocation_mode(), AllocationMode::Unconditional);
    assert!(
        stale.sizing_multiplier() < healthy.sizing_multiplier(),
        "a stale causal graph did not size more conservatively"
    );
    assert_eq!(stale.sizing_multiplier(), dec!("0.75"));
}

#[test]
fn a_belief_state_stale_beyond_its_ttl_falls_back_to_a_fixed_multiplier_and_halts_nothing() {
    // §6.2 row 4. "Nothing halts" is the operative phrase and is asserted
    // rather than assumed.
    let mut stale = DegradationState::fully_available();
    stale.observe(Capability::BeliefState, Freshness::Stale);

    assert_eq!(stale.sizing_multiplier(), dec!("0.5"));
    assert!(!stale.halts());
    for class in StrategyClass::all() {
        assert!(
            !stale.pauses(class),
            "{} paused on a stale belief state; §6.2 says sizing falls back \
             and nothing halts",
            class.as_str()
        );
    }
}

#[test]
fn losing_counterfactual_scoring_changes_no_trading_decision_whatsoever() {
    // §6.2 row 5, stated in the blueprint as strongly as it is stated here:
    // counterfactual scoring is entirely a warm-path function, so its loss
    // slows learning and touches nothing else. This is the test that stops the
    // learning path from acquiring a veto over the trading path by accident.
    let healthy = DegradationState::fully_available();
    let mut lost = DegradationState::fully_available();
    lost.observe(Capability::CounterfactualScoring, Freshness::Unavailable);

    // The premise: something really did change state.
    assert_eq!(
        lost.freshness(Capability::CounterfactualScoring),
        Freshness::Unavailable
    );
    assert!(!lost.narrowed().is_empty());

    assert_eq!(lost.sizing_multiplier(), healthy.sizing_multiplier());
    assert_eq!(lost.allocation_mode(), healthy.allocation_mode());
    assert!(!lost.halts());
    for class in StrategyClass::all() {
        assert_eq!(
            lost.pauses(class),
            healthy.pauses(class),
            "{} changed behaviour when counterfactual scoring was lost",
            class.as_str()
        );
    }
    assert!(!Capability::CounterfactualScoring.affects_trading());
}

#[test]
fn episodic_loss_pauses_only_the_strategies_that_recognise_situations() {
    // §6.2 row 3: "the rest continue".
    let mut lost = DegradationState::fully_available();
    lost.observe(Capability::EpisodicMemory, Freshness::Unavailable);

    assert!(lost.pauses(StrategyClass::SituationalRecognition));
    assert!(!lost.pauses(StrategyClass::PriceOnly));
    assert!(!lost.pauses(StrategyClass::EventDriven));
    assert!(!lost.pauses(StrategyClass::PredictionMarket));
}

#[test]
fn two_degradations_narrow_further_than_either_one_alone() {
    // Compounding rather than competing. A scheme that took the most
    // conservative single rule would let the second independent loss of
    // confidence cost nothing at all.
    let mut causal = DegradationState::fully_available();
    causal.observe(Capability::CausalGraph, Freshness::Stale);
    let mut belief = DegradationState::fully_available();
    belief.observe(Capability::BeliefState, Freshness::Stale);
    let mut both = DegradationState::fully_available();
    both.observe(Capability::CausalGraph, Freshness::Stale);
    both.observe(Capability::BeliefState, Freshness::Stale);

    // Premise: the two single-loss multipliers differ, so "smaller than both"
    // is a real constraint rather than a restatement of one of them.
    assert_ne!(causal.sizing_multiplier(), belief.sizing_multiplier());
    assert!(both.sizing_multiplier() < causal.sizing_multiplier());
    assert!(both.sizing_multiplier() < belief.sizing_multiplier());
    assert_eq!(both.sizing_multiplier(), dec!("0.375"));
}

#[test]
fn a_capability_nobody_has_reported_on_reads_as_unavailable() {
    // Failing closed. A dead reporter must not be indistinguishable from a
    // healthy subsystem — otherwise the platform sizes as though it still knew
    // something it had merely stopped being told.
    let silent = DegradationState::nothing_known();
    for capability in Capability::all() {
        assert_eq!(
            silent.freshness(capability),
            Freshness::Unavailable,
            "{} read as something other than unavailable when nothing had \
             been reported about it",
            capability.as_str()
        );
    }
    // And the consequence, not merely the reading: silence narrows.
    assert!(silent.sizing_multiplier() < DegradationState::fully_available().sizing_multiplier());
    assert_eq!(silent.allocation_mode(), AllocationMode::Unconditional);
    // Still no halt. Narrowing is total here and the platform keeps running.
    assert!(!silent.halts());
}

#[test]
fn an_unknown_freshness_token_is_refused_rather_than_defaulted() -> Result<()> {
    // Refuse rather than clamp. A policy file with a typo must stop, not
    // quietly select the permissive reading.
    assert_eq!(Freshness::parse("fresh")?, Freshness::Fresh);
    assert_eq!(Freshness::parse("stale")?, Freshness::Stale);
    assert_eq!(Freshness::parse("unavailable")?, Freshness::Unavailable);

    let refused = Freshness::parse("Stale");
    assert!(refused.is_err(), "a differently-cased token was accepted");
    let message = format!("{}", refused.expect_err("just asserted an error"));
    assert!(
        message.contains("fresh") && message.contains("unavailable"),
        "the refusal does not name the permitted tokens: {message}"
    );

    // The substring trap this repository has been bitten by: a token that
    // merely contains a valid one is not a valid one.
    assert!(Freshness::parse("very_stale").is_err());
    assert!(Freshness::parse("").is_err());
    Ok(())
}

#[test]
fn nothing_in_the_degradation_table_halts_the_platform() {
    // §6.2's whole premise. Halting belongs to the kill switch, which an
    // operator holds; it is never a consequence of a warm-path job being late.
    let mut worst = DegradationState::nothing_known();
    for capability in Capability::all() {
        worst.observe(capability, Freshness::Unavailable);
    }
    // Premise: this really is the worst state the type can express.
    assert_eq!(worst.narrowed().len(), Capability::all().len());
    assert!(!worst.halts());
    assert!(
        !worst.pauses(StrategyClass::PriceOnly),
        "with every cognitive capability gone, a price-only strategy still \
         trades on prices; pausing it would be halting by another name"
    );
}

#[test]
fn a_freshness_survives_the_round_trip_through_its_own_wire_format() {
    // Principle 6, caught by review: two independent claims about the same
    // fact will disagree, and the louder one will be wrong. Serde's derive
    // emitted `"Stale"` while `Freshness::parse` accepted only `"stale"` and
    // deliberately refused the capitalised form — so a `DegradationState`
    // written to the event log could not be read back through the documented
    // entry point, and the test asserting that refusal locked the
    // incompatibility in place.
    //
    // One fact, one spelling. Asserted in both directions, because a rename
    // that fixed serialisation and broke `parse` would satisfy either half
    // alone.
    for freshness in [Freshness::Fresh, Freshness::Stale, Freshness::Unavailable] {
        let json = serde_json::to_string(&freshness).expect("serialisable");
        let token = json.trim_matches('"');
        assert_eq!(
            token,
            freshness.as_str(),
            "the wire spelling and as_str disagree for {freshness:?}"
        );
        assert_eq!(
            Freshness::parse(token).expect("the wire spelling must parse"),
            freshness,
            "{freshness:?} does not survive its own wire format"
        );
        let decoded: Freshness = serde_json::from_str(&json).expect("deserialisable");
        assert_eq!(decoded, freshness);
    }

    // The whole state, since that is what actually reaches the log.
    let mut state = DegradationState::fully_available();
    state.observe(Capability::CausalGraph, Freshness::Stale);
    state.observe(Capability::EpisodicMemory, Freshness::Unavailable);
    let json = serde_json::to_string(&state).expect("serialisable");
    let decoded: DegradationState = serde_json::from_str(&json).expect("deserialisable");
    assert_eq!(decoded, state);
    // Premise: the state really was degraded, so the round trip carried
    // something rather than trivially matching a default.
    assert_eq!(decoded.narrowed().len(), 2);
    assert_eq!(decoded.sizing_multiplier(), dec!("0.75"));
    assert!(!decoded.analogical_retrieval_available());
}

// --- the twelve-item policy payload: blueprint §41.5 -------------------------

use qip_contracts::policy::{BeliefPriors, PolicyItem, PolicyPayload, Slot};

fn payload_key() -> Vec<u8> {
    b"a-test-trust-root-of-decent-length".to_vec()
}

#[test]
fn a_policy_payload_round_trips_and_an_unknown_field_is_refused() -> Result<()> {
    // The second half is the structural guarantee that matters: the payload
    // cannot carry an autonomy ceiling. The layering already means no type
    // here can *name* one; this asserts a ceiling cannot ride in as an extra
    // key either. The injected field below is exactly the attack — a live
    // level smuggled into policy — and it must fail deserialisation, not be
    // ignored.
    let payload = PolicyPayload::unproduced(1, "cell-1", t(0)).signed(&payload_key())?;
    let json = serde_json::to_string(&payload).expect("serialisable");
    let decoded: PolicyPayload = serde_json::from_str(&json).expect("own wire form decodes");
    assert_eq!(decoded, payload);

    // Premise: the injection point exists and the json was an object.
    assert!(json.ends_with('}'), "the wire form is not a JSON object");
    let smuggled = format!(
        "{},\"autonomy_ceiling\":\"autonomous_live\"}}",
        &json[..json.len() - 1]
    );
    let refused: std::result::Result<PolicyPayload, _> = serde_json::from_str(&smuggled);
    assert!(
        refused.is_err(),
        "a policy payload accepted an unknown field, so an autonomy ceiling \
         could ride into a cell as policy"
    );
    Ok(())
}

#[test]
fn an_unproduced_slot_is_stale_from_birth_and_narrows_like_staleness() -> Result<()> {
    // The design's central fail-closed claim: a capability the platform does
    // not have behaves exactly like one that went stale. A cell that has never
    // received belief priors sizes at the fixed conservative multiplier — the
    // platform's sizing was never belief-weighted, and this makes that fact
    // load-bearing instead of implicit.
    let payload = PolicyPayload::unproduced(1, "cell-1", t(0)).signed(&payload_key())?;
    for item in PolicyItem::all() {
        assert_eq!(
            payload.freshness(item, t(1)),
            Freshness::Unavailable,
            "{} was produced by nothing and does not read as unavailable",
            item.as_str()
        );
    }
    let narrowing = payload.narrowing(t(1));
    // Premise: the mapping actually observed something, or the assertions
    // below describe `nothing_known` rather than the payload.
    assert!(
        !narrowing.narrowed().is_empty(),
        "no capability was observed from the payload, so narrowing tested nothing"
    );
    assert_eq!(narrowing.sizing_multiplier(), dec!("0.375"));
    assert_eq!(narrowing.allocation_mode(), AllocationMode::Unconditional);
    assert!(!narrowing.halts());
    Ok(())
}

#[test]
fn a_produced_slot_is_fresh_within_its_ttl_and_stale_beyond_it() -> Result<()> {
    // Freshness comes from the producer's instant against the item's own TTL,
    // and the payload's overall validity caps it: an old envelope carrying a
    // "fresh" fact is how a replayed payload would smuggle confidence.
    let produced = Slot::produced(
        BeliefPriors {
            priors: std::collections::BTreeMap::from([("subject".to_string(), 0.7)]),
        },
        t(0),
    );
    let mut payload = PolicyPayload::unproduced(2, "cell-1", t(0));
    payload.belief_priors = produced;
    payload.valid_for = Duration::from_secs(10_000);
    let payload = payload.signed(&payload_key())?;

    // Belief priors carry a 300-second TTL, the conservative end of §41.5's
    // "seconds to minutes".
    assert_eq!(
        payload.freshness(PolicyItem::BeliefPriors, t(299)),
        Freshness::Fresh
    );
    assert_eq!(
        payload.freshness(PolicyItem::BeliefPriors, t(301)),
        Freshness::Stale
    );
    // Fresh belief widens the multiplier back to the causal-only narrowing.
    assert_eq!(payload.narrowing(t(299)).sizing_multiplier(), dec!("0.75"));

    // And the payload's own expiry caps the slot, whatever its instant says.
    let mut short = PolicyPayload::unproduced(3, "cell-1", t(0));
    short.belief_priors = Slot::produced(
        BeliefPriors {
            priors: std::collections::BTreeMap::from([("subject".to_string(), 0.7)]),
        },
        t(0),
    );
    short.valid_for = Duration::from_secs(5);
    let short = short.signed(&payload_key())?;
    assert_eq!(
        short.freshness(PolicyItem::BeliefPriors, t(6)),
        Freshness::Stale,
        "an expired payload still reported a fresh slot, which is the replay \
         that smuggles confidence"
    );
    Ok(())
}

#[test]
fn the_policy_signature_covers_every_field_that_changes_what_a_cell_may_do() -> Result<()> {
    // The `CapitalEnvelope` rule, generalised: a signature that does not cover
    // a bound is a signature over the wrong thing. Every mutation below
    // changes what the cell would do, so every one must change the signing
    // payload — including the halt flag, or a replayed payload could un-halt
    // a cell the centre stopped.
    let base = PolicyPayload::unproduced(5, "cell-1", t(100));
    let reference = base.signing_payload()?;

    let mut resequenced = base.clone();
    resequenced.sequence = 6;
    let mut readdressed = base.clone();
    readdressed.cell = "cell-2".to_string();
    let mut unhalted = base.clone();
    unhalted.halted = true;
    let mut reslotted = base.clone();
    reslotted.belief_priors = Slot::produced(
        BeliefPriors {
            priors: std::collections::BTreeMap::new(),
        },
        t(100),
    );
    let mut rewindowed = base.clone();
    rewindowed.valid_for = Duration::from_secs(999_999);

    for (label, mutated) in [
        ("sequence", &resequenced),
        ("cell", &readdressed),
        ("halt flag", &unhalted),
        ("a slot's content", &reslotted),
        ("validity window", &rewindowed),
    ] {
        assert_ne!(
            mutated.signing_payload()?,
            reference,
            "changing the {label} does not change the signing payload, so a \
             signature over one payload authorises the other"
        );
    }

    // An empty key is refused, as it is for capital envelopes.
    assert!(base.clone().signed(&[]).is_err());
    Ok(())
}

#[test]
fn two_different_halts_cannot_share_one_signing_string() {
    // The reviewer's exact collision. With fields joined on a bare `|`, these
    // two commands — different cells, different reasons — serialised to the
    // same bytes and therefore shared one MAC: a signature over one was a
    // signature over the other. Length-prefixing the free-text fields is what
    // makes the string injective, and this is the pair that proves it.
    use qip_contracts::policy::HaltCommand;

    // Raw seconds, not the suite's epoch-offset helper: the collision needs
    // the instant's decimal spelling to be exactly the token the hostile
    // reason begins with, and the first version of this test used the helper,
    // broke that alignment, and passed against the unfixed code — caught by
    // its own mutation run.
    let instant = Timestamp::from_secs(100);
    let first = HaltCommand::new("a", instant, "100|b|c");
    let second = HaltCommand::new("a|100", instant, "b|c");
    // The premise: these are genuinely different commands.
    assert_ne!(first.cell, second.cell);
    assert_ne!(first.reason, second.reason);
    assert_ne!(
        first.signing_payload(),
        second.signing_payload(),
        "two different halt commands share one signing string, so one \
         signature authorises both"
    );

    // The same property for the payload's one free-text field: a cell name
    // that swallows the adjacent numeric field must not collide with the
    // honest spelling.
    let mut plain = PolicyPayload::unproduced(1, "a", t(100));
    plain.sequence = 1;
    let mut tricky = PolicyPayload::unproduced(1, "a|", t(100));
    tricky.sequence = 1;
    assert_ne!(
        plain.signing_payload().expect("serialisable"),
        tricky.signing_payload().expect("serialisable"),
        "two different payload addresses share one signing string"
    );
}

// --- intent netting: blueprint §27 -------------------------------------------

use qip_contracts::intent::{Contributor, Intent, NetIntent, Representation, net, netting_ratio};

fn intent(strategy: &str, object: &str, venue: &str, size: &str) -> Intent {
    Intent::new(
        StrategyId::new(strategy),
        ObjectId::from_string(object),
        VenueId::new(venue),
        Decimal::parse(size).expect("a decimal literal"),
        dec!("100"),
        t(3_600),
    )
    .expect("a non-zero intent")
}

#[test]
fn opposing_intents_cancel_internally_and_neither_reaches_the_venue() -> Result<()> {
    // §27's self-trade row, which is a live defect today: one strategy's buy
    // crossing another's sell is a regulatory problem and a pure loss at once.
    // Netting is signed addition precisely so the cancellation is a sum rather
    // than a conditional somebody can get backwards.
    let nets = net(vec![
        intent("momentum", "ACME", "XLON", "100"),
        intent("reversion", "ACME", "XLON", "-100"),
    ]);
    assert_eq!(
        nets.len(),
        1,
        "two intents on one key produced {} nets",
        nets.len()
    );
    let single = &nets[0];
    // The premise: both strategies really are in the group. A cancellation
    // computed over one contributor would pass this test while proving that
    // netting dropped the other one.
    assert_eq!(single.contributors.len(), 2);
    assert!(single.is_cancelled(), "opposing intents did not cancel");
    assert_eq!(single.order_quantity(), Decimal::ZERO);
    assert_eq!(single.is_buy(), None);
    // Gross survives the cancellation: something was intended, and the netting
    // ratio needs to know it even though nothing was sent.
    assert_eq!(single.gross_size, dec!("200"));
    Ok(())
}

#[test]
fn intents_in_the_same_direction_become_one_order_carrying_both_contributors() -> Result<()> {
    let nets = net(vec![
        intent("momentum", "ACME", "XLON", "60"),
        intent("carry", "ACME", "XLON", "40"),
    ]);
    assert_eq!(nets.len(), 1);
    let single = &nets[0];
    assert_eq!(single.net_size, dec!("100"));
    assert_eq!(single.gross_size, dec!("100"));
    assert_eq!(single.is_buy(), Some(true));
    // The contributor vector sums to the net. This is the property the whole
    // attribution chain rests on: a vector that did not sum would attribute a
    // fill to sizes nobody traded.
    let summed: Decimal = single
        .contributors
        .iter()
        .map(|contributor| contributor.signed_size)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(summed, single.net_size);
    // Premise for that sum: there is more than one contributor, or it holds
    // trivially and proves nothing about netting.
    assert_eq!(single.contributors.len(), 2);
    // Deterministic order, by strategy id ascending.
    assert_eq!(single.contributors[0].strategy.as_str(), "carry");
    assert_eq!(single.contributors[1].strategy.as_str(), "momentum");
    Ok(())
}

#[test]
fn a_cycle_leg_is_never_netted_with_a_directional_intent() -> Result<()> {
    // §27.2: a leg is part of an atomic set, and netting it against a
    // directional intent silently breaks the cycle's economics — the cycle
    // still executes, at sizes that no longer close. The refusal is
    // structural: the leg gets its own group, so there is no code path on
    // which it joins somebody else's net.
    let leg = intent("arb", "ACME", "XLON", "50").as_cycle_leg("cycle-7");
    let nets = net(vec![intent("momentum", "ACME", "XLON", "50"), leg]);
    assert_eq!(
        nets.len(),
        2,
        "a cycle leg was combined with a directional intent on the same key"
    );
    let cycle = nets
        .iter()
        .find(|net| net.cycle_id.is_some())
        .expect("the leg kept its cycle identity");
    assert_eq!(
        cycle.contributors.len(),
        1,
        "the leg absorbed a contributor"
    );
    assert_eq!(cycle.net_size, dec!("50"));
    // And the directional intent is untouched by the leg's presence.
    let directional = nets
        .iter()
        .find(|net| net.cycle_id.is_none())
        .expect("the directional intent still nets");
    assert_eq!(directional.net_size, dec!("50"));
    assert_eq!(directional.contributors.len(), 1);
    Ok(())
}

#[test]
fn two_legs_of_one_cycle_do_not_net_with_each_other_either() -> Result<()> {
    // The subtler half of the same rule. Two legs of one cycle that happen to
    // share an instrument and a venue are still separate legs of an atomic
    // set; combining them is the same mistake as combining one with a
    // directional intent, and a group keyed only on the cycle id would make
    // exactly that mistake.
    let nets = net(vec![
        intent("arb", "ACME", "XLON", "30").as_cycle_leg("cycle-7"),
        intent("arb", "ACME", "XLON", "-30").as_cycle_leg("cycle-7"),
    ]);
    assert_eq!(nets.len(), 2, "two legs of one cycle were netted together");
    assert!(nets.iter().all(|net| !net.is_cancelled()));
    Ok(())
}

#[test]
fn the_netting_key_separates_venues_and_representations() -> Result<()> {
    // §27.2's two "not netted" rows. Different venues are different executions
    // at different prices; different representations of one underlying are
    // different instruments with different risk.
    let venues = net(vec![
        intent("momentum", "ACME", "XLON", "50"),
        intent("momentum", "ACME", "XNYS", "50"),
    ]);
    assert_eq!(venues.len(), 2, "two venues were netted into one order");

    let representations = net(vec![
        intent("momentum", "BTC", "COINBASE", "5"),
        intent("carry", "BTC", "COINBASE", "-5").with_representation(Representation::Perpetual),
    ]);
    assert_eq!(
        representations.len(),
        2,
        "spot and perpetual were netted against each other"
    );
    // Premise: without the representation difference these two would have
    // cancelled, so the separation above is the representation's doing.
    let same = net(vec![
        intent("momentum", "BTC", "COINBASE", "5"),
        intent("carry", "BTC", "COINBASE", "-5"),
    ]);
    assert_eq!(same.len(), 1);
    assert!(same[0].is_cancelled());
    Ok(())
}

#[test]
fn a_fill_splits_across_contributors_and_sums_exactly_to_what_was_traded() -> Result<()> {
    // Exact attribution, at the seam netting creates. A truncating split loses
    // a fraction of every fill and a floating-point one invents fractions;
    // either way the shares stop summing to what was actually traded, and
    // unexplained P&L is what exact attribution exists to make impossible.
    //
    // Thirds are the case that cannot divide evenly, which is why they are
    // the fixture.
    let nets = net(vec![
        intent("alpha", "ACME", "XLON", "1"),
        intent("beta", "ACME", "XLON", "1"),
        intent("gamma", "ACME", "XLON", "1"),
    ]);
    let single = &nets[0];
    assert_eq!(
        single.contributors.len(),
        3,
        "the premise needs three ways to split"
    );

    let filled = dec!("100");
    let shares = single.split_fill(filled);
    assert_eq!(shares.len(), 3);
    let summed: Decimal = shares
        .iter()
        .map(|(_, share)| *share)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(
        summed,
        filled,
        "the split lost or invented {} of the fill",
        filled - summed
    );
    // Every share is a real part of the fill, not a zero standing in for one.
    assert!(shares.iter().all(|(_, share)| share.is_positive()));

    // A partial fill splits by the same rule and still sums exactly.
    let partial = single.split_fill(dec!("7"));
    let partial_sum: Decimal = partial
        .iter()
        .map(|(_, share)| *share)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(partial_sum, dec!("7"));
    Ok(())
}

#[test]
fn the_same_fill_splits_identically_however_the_intents_arrived() -> Result<()> {
    // Determinism, which the remainder rule owes twice over: the same fill
    // must split the same way on every machine and in every replay. Equal
    // remainders are the case where an unstable tie-break would show, so the
    // fixture is three equal contributors whose remainders are identical.
    let forward = net(vec![
        intent("alpha", "ACME", "XLON", "1"),
        intent("beta", "ACME", "XLON", "1"),
        intent("gamma", "ACME", "XLON", "1"),
    ]);
    let reversed = net(vec![
        intent("gamma", "ACME", "XLON", "1"),
        intent("beta", "ACME", "XLON", "1"),
        intent("alpha", "ACME", "XLON", "1"),
    ]);
    let first = forward[0].split_fill(dec!("100"));
    let second = reversed[0].split_fill(dec!("100"));
    assert_eq!(
        first, second,
        "the same fill split differently depending on the order the intents arrived in"
    );
    // The premise: the split really did need a remainder distributed, or the
    // determinism above is about a case the tie-break never reached.
    assert!(
        first.iter().any(|(_, share)| *share != first[0].1),
        "every share was identical, so no remainder was distributed and the \
         tie-break was never exercised"
    );

    // And the tie-break itself, reached directly. `net` sorts contributors by
    // strategy id, so no vector it builds can exercise the tie — but
    // `NetIntent` has public fields and a caller may hand `split_fill` a
    // vector in any order, so the rule has to hold there too. Without the
    // strategy-id tie-break, a stable sort leaves equal remainders in arrival
    // order and the leftover unit follows whoever happened to be first.
    let unsorted = NetIntent {
        object_id: ObjectId::from_string("ACME"),
        venue: VenueId::new("XLON"),
        representation: Representation::Spot,
        net_size: dec!("3"),
        gross_size: dec!("3"),
        contributors: vec![
            Contributor {
                strategy: StrategyId::new("gamma"),
                signed_size: dec!("1"),
                inputs: Vec::new(),
            },
            Contributor {
                strategy: StrategyId::new("alpha"),
                signed_size: dec!("1"),
                inputs: Vec::new(),
            },
            Contributor {
                strategy: StrategyId::new("beta"),
                signed_size: dec!("1"),
                inputs: Vec::new(),
            },
        ],
        reference_price: dec!("100"),
        cycle_id: None,
    };
    let split = unsorted.split_fill(dec!("100"));
    let leader = split
        .iter()
        .max_by(|left, right| left.1.cmp(&right.1))
        .expect("a split");
    assert_eq!(
        leader.0.as_str(),
        "alpha",
        "the leftover unit went to {} rather than to the lowest strategy id, \
         so equal remainders are separated by arrival order",
        leader.0.as_str()
    );
    let unsorted_sum: Decimal = split
        .iter()
        .map(|(_, share)| *share)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(unsorted_sum, dec!("100"));
    Ok(())
}

#[test]
fn the_netting_ratio_reports_diversity_and_refuses_to_invent_one() -> Result<()> {
    // §27 calls this the single best summary of whether a strategy set has
    // genuine diversity. Two strategies wanting the same thing net to one
    // order and a ratio of one; two wanting opposite things send nothing, and
    // the ratio is undefined rather than a sentinel somebody would chart.
    let agreeing = net(vec![
        intent("alpha", "ACME", "XLON", "50"),
        intent("beta", "ACME", "XLON", "50"),
    ]);
    let ratio = netting_ratio(&agreeing).expect("a net was sent");
    assert!(
        (ratio - 1.0).abs() < 1e-9,
        "agreeing strategies gave a ratio of {ratio}"
    );

    let disagreeing = net(vec![
        intent("alpha", "ACME", "XLON", "50"),
        intent("beta", "ACME", "XLON", "-50"),
    ]);
    assert!(
        netting_ratio(&disagreeing).is_none(),
        "a fully cancelled set reported a ratio rather than declining to"
    );
    Ok(())
}

#[test]
fn an_intent_to_trade_nothing_is_refused_rather_than_carried() {
    assert!(
        Intent::new(
            StrategyId::new("alpha"),
            ObjectId::from_string("ACME"),
            VenueId::new("XLON"),
            Decimal::ZERO,
            dec!("100"),
            t(3_600),
        )
        .is_err(),
        "a zero-size intent was admitted into a contributor vector that must sum"
    );
}

#[test]
fn each_contributor_keeps_the_feature_revisions_its_own_strategy_reasoned_from() {
    // Netting is where attribution is most easily lost. Two strategies read
    // different features at different revisions, agree on direction, and
    // become one order; if the net carried the union — or the first
    // contributor's inputs, or none — a later reader could not say which
    // values produced which share of the fill, which is the whole reason
    // `Signal::inputs` exists.
    let first = intent("alpha", "ACME", "XLON", "60").with_inputs(vec![
        ("book_pressure{levels=5}".to_string(), 11),
        ("spread{}".to_string(), 4),
    ]);
    let second =
        intent("beta", "ACME", "XLON", "40").with_inputs(vec![("momentum{}".to_string(), 9)]);

    // The premise: the two really do differ, so a net that copied one onto
    // both would be visible rather than indistinguishable.
    assert_ne!(first.inputs, second.inputs);

    let nets = net(vec![first, second]);
    assert_eq!(nets.len(), 1, "the premise needs the two to have netted");
    let contributors = &nets[0].contributors;
    assert_eq!(contributors.len(), 2);

    let inputs_of = |strategy: &str| -> Vec<(String, u64)> {
        contributors
            .iter()
            .find(|c| c.strategy.as_str() == strategy)
            .map(|c| c.inputs.clone())
            .expect("both strategies contributed")
    };
    assert_eq!(
        inputs_of("alpha"),
        vec![
            ("book_pressure{levels=5}".to_string(), 11),
            ("spread{}".to_string(), 4),
        ],
        "alpha's revisions did not survive netting intact"
    );
    assert_eq!(
        inputs_of("beta"),
        vec![("momentum{}".to_string(), 9)],
        "beta's revisions did not survive netting intact"
    );
    // And neither inherited the other's: a union would make both lists equal
    // and every attribution afterwards would credit both for one signal.
    assert_ne!(inputs_of("alpha"), inputs_of("beta"));
}
