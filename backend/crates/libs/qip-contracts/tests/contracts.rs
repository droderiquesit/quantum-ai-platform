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
