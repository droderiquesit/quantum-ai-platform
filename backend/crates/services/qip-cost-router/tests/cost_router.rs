//! The properties the cost engine and the router are supposed to have.
//!
//! Each one is a way the platform loses money quietly rather than loudly: an
//! edge that never accounted for what it cost to find, a risk check answered by
//! a model, a decision that spent more on inference than it could ever earn, an
//! escalation that climbed without bound, a model trusted in a regime it has
//! never seen. None of them fails a spot check — every one of them passes right
//! up until it matters — so each is asserted as a property over a range rather
//! than at a point.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_agents::Budget;
use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry};
use qip_contracts::edge::{Deduction, DeductionKind, NetEdge};
use qip_contracts::signal::Conviction;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ModelId, Timestamp, dec};
use qip_cost_router::{
    ComputeLedger, Conditions, CostEngine, DataCostModel, DataReads, DataSource, DecisionContext,
    Determinism, Escalation, EscalationLimits, Horizon, IntelligenceTier, Judgement, MarketRegime,
    Region, ReputationBook, Router, Routing, TierVerdict, VolatilityRegime,
};
use qip_financial::asset_class::AssetClass;
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn conditions(regime: MarketRegime) -> Conditions {
    Conditions::new(
        AssetClass::Equity,
        Region::new("us-east"),
        regime,
        VolatilityRegime::Normal,
        Horizon::Intraday,
    )
}

fn context(
    value: Decimal,
    latency: Duration,
    confidence: f64,
    determinism: Determinism,
) -> DecisionContext {
    DecisionContext::new(
        "is the dislocation real",
        value,
        latency,
        confidence,
        determinism,
        conditions(MarketRegime::Trending),
    )
}

/// A feed billed at 1000 for the period, read `reads` times over it.
fn feed(reads: u64) -> Result<DataSource> {
    DataSource::new("tick-feed", dec!("1000"), Duration::from_days(30), reads)
}

/// The seven deductions the market charges, so a test can concentrate on the
/// two the platform charges itself.
fn market_costs(edge: NetEdge, each: Decimal) -> Result<NetEdge> {
    let mut edge = edge;
    for kind in [
        DeductionKind::Spread,
        DeductionKind::Fees,
        DeductionKind::Latency,
        DeductionKind::Slippage,
        DeductionKind::Funding,
        DeductionKind::Collateral,
        DeductionKind::Uncertainty,
    ] {
        edge = edge.deduct(Deduction::new(kind, each, "modelled by the cell")?);
    }
    Ok(edge)
}

// --- the ladder -------------------------------------------------------------

#[test]
fn the_ladder_is_ordered_by_cost_latency_and_strength_at_once() {
    // The router walks the ladder from the bottom and stops at the first rung
    // it can justify, then refuses outright when a rung is unaffordable or too
    // slow rather than looking higher. That shortcut is only sound while all
    // three quantities rise together, so the ordering is asserted here rather
    // than assumed at every call site.
    let ladder = IntelligenceTier::LADDER;
    for pair in ladder.windows(2) {
        let (lower, upper) = (pair[0], pair[1]);
        assert!(
            lower.cost() < upper.cost(),
            "{} does not cost less than {}",
            lower.as_str(),
            upper.as_str()
        );
        assert!(
            lower.latency().as_nanos() < upper.latency().as_nanos(),
            "{} is not faster than {}",
            lower.as_str(),
            upper.as_str()
        );
        assert!(
            lower.resolving_power_f64() < upper.resolving_power_f64(),
            "{} is not weaker than {}",
            lower.as_str(),
            upper.as_str()
        );
        assert_eq!(lower.next(), Some(upper));
        assert_eq!(lower.rung() + 1, upper.rung());
    }
    assert_eq!(ladder[8].next(), None, "the ladder must terminate");
}

#[test]
fn exactly_one_rung_is_deterministic_and_it_is_the_bottom_one() {
    // The determinism rule is built on this being true of the ladder, not on it
    // happening to be true today.
    let deterministic: Vec<IntelligenceTier> = IntelligenceTier::LADDER
        .into_iter()
        .filter(IntelligenceTier::is_deterministic)
        .collect();
    assert_eq!(deterministic, vec![IntelligenceTier::DeterministicCode]);

    for tier in IntelligenceTier::LADDER {
        assert_eq!(
            tier.model_tier().is_none(),
            tier.is_deterministic(),
            "{} disagrees with itself about whether it can be wrong",
            tier.as_str()
        );
    }
}

// --- the cost engine --------------------------------------------------------

#[test]
fn an_edge_without_a_compute_and_data_cost_is_refused_by_name() -> Result<()> {
    // The whole point of the two new deductions: an edge that considered every
    // cost the market charges and neither of the costs the platform charges
    // itself is incomplete, and says so with the names of what is missing
    // rather than with a count.
    let edge = market_costs(NetEdge::gross(dec!("100"), dec!("1000"))?, dec!("5"))?;
    let refusal = edge
        .require_complete()
        .expect_err("seven of nine is not complete");
    assert!(refusal.message().contains("compute_cost"), "{refusal}");
    assert!(refusal.message().contains("data_cost"), "{refusal}");

    // And with them, it is.
    let engine = CostEngine::new(DataCostModel::new().with_source(feed(1_000_000)?));
    let mut ledger = ComputeLedger::new("is the dislocation real", Budget::research_default())?;
    ledger.charge(IntelligenceTier::DeterministicCode)?;
    let reads = DataReads::new().record("tick-feed", 12);
    let complete = engine.charge_edge(edge, &ledger, &reads)?;
    complete.require_complete()?;
    assert!(complete.unconsidered().is_empty());
    Ok(())
}

#[test]
fn an_opportunity_that_clears_its_market_costs_but_not_its_compute_cost_is_negative() -> Result<()>
{
    // The failure the cost engine exists to make visible. Every market cost is
    // covered with room to spare and the decision still lost money, because
    // reaching it took a deep model and a simulation. An accounting that put
    // the inference bill in an infrastructure budget would report this as a
    // five-unit win and would keep reporting it until the month closed.
    let engine = CostEngine::new(DataCostModel::new().with_source(feed(1_000)?));
    let mut ledger = ComputeLedger::new("is the dislocation real", Budget::deep_research())?;
    ledger.charge(IntelligenceTier::DeepModel)?;
    ledger.charge(IntelligenceTier::Simulation)?;
    let reads = DataReads::new().record("tick-feed", 2);

    let market_only = market_costs(NetEdge::gross(dec!("10"), dec!("100"))?, dec!("0.7"))?;
    assert_eq!(
        market_only.net(),
        dec!("5.1"),
        "the market costs alone leave the trade in profit"
    );

    let edge = engine.charge_edge(market_only, &ledger, &reads)?;
    edge.require_complete()?;
    // 0.90 for the deep model, 4.00 for the simulation, 2 reads of a 1000-unit
    // subscription that saw a thousand reads.
    assert_eq!(edge.net(), dec!("-1.8"));
    assert!(
        !edge.is_positive(),
        "an opportunity that earns less than it cost to find is not an opportunity: {}",
        edge.summarise()
    );
    Ok(())
}

#[test]
fn a_per_read_data_cost_falls_exactly_as_the_read_volume_rises() -> Result<()> {
    // A source read once a day and one read a million times cost the same to
    // licence. Charging a decision the subscription rather than its share of it
    // makes every high-frequency strategy look uneconomic and every low-volume
    // one look free, and both errors are large.
    let mut previous: Option<Decimal> = None;
    for reads in [1u64, 10, 100, 1_000, 10_000, 100_000, 1_000_000] {
        let per_read = feed(reads)?.cost_per_read()?;
        if let Some(previous) = previous {
            assert!(
                per_read < previous,
                "{reads} reads did not amortise further than the volume before it"
            );
        }
        previous = Some(per_read);
    }

    // Exactly, not approximately: doubling the reads halves the per-read cost.
    assert_eq!(feed(1_000)?.cost_per_read()?, dec!("1"));
    assert_eq!(feed(2_000)?.cost_per_read()?, dec!("0.5"));
    assert_eq!(feed(4_000)?.cost_per_read()?, dec!("0.25"));
    assert_eq!(feed(8_000)?.cost_per_read()?, dec!("0.125"));

    // And the amortisation is a partition: the reads over the period pay for
    // the subscription and nothing more.
    let source = feed(2_500)?;
    assert_eq!(
        source.cost_per_read()? * dec!("2500"),
        source.subscription_cost
    );

    // A source nobody read has no per-read cost. Answering zero would say the
    // licence was free, which is the opposite of what an unread licence is.
    assert!(feed(0)?.cost_per_read().is_err());
    // And an unlicensed source is unknown, not free.
    let model = DataCostModel::new().with_source(feed(10)?);
    let unknown = model
        .charge(&DataReads::new().record("someone-elses-feed", 1))
        .expect_err("an unlicensed read must not be charged nothing");
    assert!(
        unknown.message().contains("someone-elses-feed"),
        "{unknown}"
    );
    Ok(())
}

#[test]
fn a_compute_ledger_charges_every_rung_and_defers_agent_accounting_to_the_agent_budget()
-> Result<()> {
    // The ledger owns the money; `qip_agents::BudgetLedger` owns the agent's
    // allowance. Restating the second inside the first is how the two drift
    // until an agent quietly exceeds a budget the ledger thought it was
    // enforcing.
    let mut ledger = ComputeLedger::new("panel review", Budget::research_default())?;
    ledger.charge(IntelligenceTier::MultiAgentReasoning)?;

    let spend = ledger.spend();
    assert_eq!(
        spend.language_model_calls, 3,
        "an adversarial panel is three seats, and each is a completion the agent budget must see"
    );
    assert_eq!(
        spend.tokens,
        IntelligenceTier::MultiAgentReasoning.tokens(),
        "the seats must sum to the rung's tokens, not to the nearest multiple of the fan-out"
    );
    assert_eq!(
        spend.cost_micros, 300_000,
        "the seats must sum to the rung's stated cost exactly"
    );
    assert_eq!(
        ledger.total_cost(),
        IntelligenceTier::MultiAgentReasoning.cost()
    );
    Ok(())
}

#[test]
fn a_fast_path_ledger_refuses_the_first_rung_that_would_wait_on_a_model() -> Result<()> {
    // The Fast Brain must never wait on a model, and `Budget::fast_path` says
    // so in the only place that binds: the allowance. A ledger opened on it
    // charges deterministic code happily and fails on the rung above.
    let mut ledger = ComputeLedger::fast_path("pre-trade risk check")?;
    ledger.charge(IntelligenceTier::DeterministicCode)?;
    assert_eq!(
        ledger.total_cost(),
        IntelligenceTier::DeterministicCode.cost()
    );

    let refusal = ledger
        .charge(IntelligenceTier::TinyModel)
        .expect_err("the fast path may not call a model");
    assert!(
        refusal.message().contains("language_model_calls"),
        "{refusal}"
    );
    assert_eq!(
        ledger.charges().len(),
        1,
        "a rung that ran out of budget did not answer and must not be charged for one"
    );
    Ok(())
}

// --- the router -------------------------------------------------------------

#[test]
fn a_decision_that_must_be_deterministic_cannot_be_routed_to_any_model_tier() -> Result<()> {
    // The rule stated structurally. `Router::select` returns
    // `Routing::Deterministic`, which holds a `DeterministicRouting` — a type
    // with no field a `ModelTier` fits in. The match below is exhaustive, so
    // the compiler is what rules out the model branch; the sweep only shows
    // that no combination of value, urgency or demanded confidence reaches it.
    let router = Router::default();
    for value in [dec!("0.01"), dec!("1000"), dec!("100000000")] {
        for latency in [
            Duration::from_micros(1),
            Duration::from_secs(1),
            Duration::from_hours(1),
        ] {
            for confidence in [0.01, 0.5, 0.9, 1.0] {
                let context = context(value, latency, confidence, Determinism::Required);
                let routing = router.select(&context)?;

                match routing {
                    Routing::Deterministic(determined) => {
                        assert_eq!(
                            determined.charge().tier,
                            IntelligenceTier::DeterministicCode,
                            "the only rung a determined decision can occupy"
                        );
                    }
                    Routing::Judged(judged) => panic!(
                        "a decision requiring determinism reached {}",
                        judged.tier().as_str()
                    ),
                }

                let routing = router.select(&context)?;
                assert!(
                    routing.model_tier().is_none(),
                    "there is no field on a deterministic routing for a model tier to come from"
                );
                assert_eq!(routing.tier(), IntelligenceTier::DeterministicCode);
                assert_eq!(routing.tiers(), vec![IntelligenceTier::DeterministicCode]);
            }
        }
    }
    Ok(())
}

#[test]
fn a_tier_costing_more_than_the_value_at_stake_is_refused_with_the_reason_named() -> Result<()> {
    // Spending more on inference than the opportunity can earn is the failure
    // this crate exists to prevent, and it has to be refused at the rung: by
    // the time it is a deduction the money is gone and the deduction is only
    // how the loss gets reported.
    let router = Router::default();
    let tiny = context(
        dec!("0.10"),
        Duration::from_hours(1),
        0.85,
        Determinism::NotRequired,
    );

    let verdict = router.assess(IntelligenceTier::Agent, &tiny)?;
    assert!(
        matches!(verdict, TierVerdict::Unaffordable { .. }),
        "{verdict:?}"
    );
    assert!(!verdict.can_climb(), "nothing above an agent is cheaper");

    let refusal = router
        .select(&tiny)
        .expect_err("a decision worth a tenth of a unit cannot buy an agent");
    assert!(refusal.message().contains("agent"), "{refusal}");
    assert!(refusal.message().contains("costs"), "{refusal}");
    assert!(
        refusal.message().contains("every rung above it costs more"),
        "the refusal must say why looking higher would not help: {refusal}"
    );

    // The absolute form of the rule, swept: a rung that costs more than the
    // whole decision is worth is never usable, whatever the policy says.
    for tier in IntelligenceTier::LADDER {
        let value = tier.cost() - Decimal::from_raw(1);
        if value <= Decimal::ZERO {
            continue;
        }
        let context = context(
            value,
            Duration::from_hours(1),
            0.01,
            Determinism::NotRequired,
        );
        assert!(
            !router.assess(tier, &context)?.is_usable(),
            "{} was usable for a decision worth less than it costs",
            tier.as_str()
        );
    }
    Ok(())
}

#[test]
fn a_decision_routes_to_the_cheapest_rung_that_is_affordable_fast_enough_and_strong_enough()
-> Result<()> {
    let router = Router::default();

    // Weak bar, plenty of money, plenty of time: the bottom rung answers.
    let easy = context(
        dec!("100000"),
        Duration::from_hours(1),
        0.4,
        Determinism::NotRequired,
    );
    assert_eq!(
        router.select(&easy)?.tier(),
        IntelligenceTier::DeterministicCode
    );

    // Raise the bar and the router climbs to the cheapest rung that reaches it,
    // never past it.
    for (confidence, expected) in [
        (0.55, IntelligenceTier::StatisticalModel),
        (0.65, IntelligenceTier::TinyModel),
        (0.75, IntelligenceTier::SpecialistModel),
        (0.85, IntelligenceTier::Agent),
        (0.90, IntelligenceTier::MultiAgentReasoning),
        (0.95, IntelligenceTier::Simulation),
    ] {
        let context = context(
            dec!("100000"),
            Duration::from_hours(1),
            confidence,
            Determinism::NotRequired,
        );
        let routing = router.select(&context)?;
        assert_eq!(
            routing.tier(),
            expected,
            "a {confidence} bar should buy exactly {} and no more",
            expected.as_str()
        );
        assert_eq!(routing.total_cost(), expected.cost());
    }

    // A tight latency budget refuses rather than routing to something that
    // would answer after the decision stopped mattering.
    let urgent = context(
        dec!("100000"),
        Duration::from_millis(1),
        0.9,
        Determinism::NotRequired,
    );
    let refusal = router
        .select(&urgent)
        .expect_err("no rung reaching 0.9 answers inside a millisecond");
    assert!(refusal.message().contains("slower"), "{refusal}");
    Ok(())
}

#[test]
fn a_tolerated_estimate_that_settles_on_deterministic_code_is_still_judged_and_escalatable()
-> Result<()> {
    // `DeterministicRouting` is the type that proves no model was consulted,
    // and it must stay reserved for `Determinism::Required`. A decision that
    // merely tolerates an estimate (`NotRequired`) can still land on the
    // `DeterministicCode` rung — its 0.50 resolving power is a coin flip
    // against a judgement question, not a computed certainty, so a weak
    // enough confidence bar accepts it. That outcome has to stay a
    // `JudgedRouting`: an estimate that could be wrong, and one `escalate`
    // can still climb past. Collapsing it into `DeterministicRouting` would
    // hand a decision the type-level "this cannot be wrong" guarantee for a
    // coin flip nobody required.
    let router = Router::default();
    let weak = context(
        dec!("100000"),
        Duration::from_hours(1),
        0.4,
        Determinism::NotRequired,
    );
    let routing = router.select(&weak)?;
    let Routing::Judged(judged) = &routing else {
        panic!("a NotRequired decision must never produce Routing::Deterministic");
    };
    assert_eq!(judged.tier(), IntelligenceTier::DeterministicCode);
    assert!(
        judged.model_tier().is_none(),
        "deterministic code still reports no model, from inside the Judged variant"
    );

    // Being Judged rather than Deterministic is not cosmetic: it is what lets
    // an unconvincing answer keep climbing. `DeterministicRouting` has no
    // `escalate` path at all — there is no value of it to hand in.
    let limits = EscalationLimits::new(IntelligenceTier::StatisticalModel, dec!("10"))?;
    let unconvincing = Conviction::new(0.0, 1_000);
    let escalation = router.escalate(judged, &weak, unconvincing, &limits)?;
    assert!(
        escalation.climbed(),
        "a coin flip that misses a 0.4 bar must still be able to climb"
    );
    assert_eq!(
        escalation.routing().tier(),
        IntelligenceTier::StatisticalModel
    );
    Ok(())
}

#[test]
fn routing_is_deterministic_given_the_same_context() -> Result<()> {
    // A routing decision that varied run to run would make a replay reproduce a
    // different reasoning trace for the same decision, and the attribution that
    // follows a fill would be attributing to the wrong thing.
    let context = context(
        dec!("5000"),
        Duration::from_secs(60),
        0.85,
        Determinism::NotRequired,
    );
    let first = Router::default().select(&context)?;
    for _ in 0..32 {
        assert_eq!(Router::default().select(&context)?, first);
    }
    assert_eq!(
        first.rationale(),
        Router::default().select(&context)?.rationale()
    );
    Ok(())
}

#[test]
fn escalation_charges_every_rung_it_used_and_stops_at_the_ceiling() -> Result<()> {
    // Escalation is the expensive path, so it is the one that has to be
    // bounded. A rung that answered badly was still paid for; leaving it off
    // the bill makes climbing look free and makes the decision that climbed six
    // times look as cheap as the one that got it right first time.
    let router = Router::default();
    let context = context(
        dec!("100000"),
        Duration::from_secs(300),
        0.60,
        Determinism::NotRequired,
    );
    let limits = EscalationLimits::new(IntelligenceTier::DeepModel, dec!("10"))?;
    // Better than a coin flip, backed by a thousand observations, and still
    // short of the bar. Shrinkage is what stops a lucky handful from ending the
    // climb early — see `Conviction::shrunk`.
    let unconvincing = Conviction::new(0.55, 1_000);

    let Routing::Judged(mut routing) = router.select(&context)? else {
        panic!("a decision that tolerates an estimate must be judged");
    };
    assert_eq!(routing.tier(), IntelligenceTier::StatisticalModel);

    let expected = [
        IntelligenceTier::TinyModel,
        IntelligenceTier::SpecialistModel,
        IntelligenceTier::Agent,
        IntelligenceTier::MultiAgentReasoning,
        IntelligenceTier::DeepModel,
    ];
    for (climbed, tier) in expected.into_iter().enumerate() {
        let escalation = router.escalate(&routing, &context, unconvincing, &limits)?;
        assert!(escalation.climbed(), "an unconvincing answer must climb");
        routing = escalation.routing().clone();
        assert_eq!(routing.tier(), tier);
        assert_eq!(routing.escalations(), climbed + 1);
    }

    // Every rung it stood on is on the bill, in order, once.
    assert_eq!(
        routing.tiers(),
        vec![
            IntelligenceTier::StatisticalModel,
            IntelligenceTier::TinyModel,
            IntelligenceTier::SpecialistModel,
            IntelligenceTier::Agent,
            IntelligenceTier::MultiAgentReasoning,
            IntelligenceTier::DeepModel,
        ]
    );
    let summed = routing
        .tiers()
        .into_iter()
        .fold(Decimal::ZERO, |sum, tier| sum + tier.cost());
    assert_eq!(routing.total_cost(), summed);
    assert_eq!(routing.total_cost(), dec!("1.25442"));

    // And the ceiling refuses rather than clipping. Returning the ceiling
    // rung's answer as though it were the answer asked for is how a caller ends
    // up acting on a confidence nobody ever reached.
    let refusal = router
        .escalate(&routing, &context, unconvincing, &limits)
        .expect_err("the ceiling must refuse, not silently stop");
    assert!(refusal.message().contains("ceiling"), "{refusal}");
    assert!(refusal.message().contains("simulation"), "{refusal}");

    // The ledger agrees with the routing about what was spent.
    let mut ledger = ComputeLedger::new("is the dislocation real", Budget::deep_research())?;
    assert_eq!(ledger.charge_all(&routing.tiers())?, routing.total_cost());
    Ok(())
}

#[test]
fn escalation_refuses_to_exceed_the_total_spend_it_was_granted() -> Result<()> {
    let router = Router::default();
    let context = context(
        dec!("100000"),
        Duration::from_secs(300),
        0.60,
        Determinism::NotRequired,
    );
    // Enough for the cheap rungs, not enough to reach an agent.
    let limits = EscalationLimits::new(IntelligenceTier::Solver, dec!("0.05"))?;
    let unconvincing = Conviction::new(0.5, 1_000);

    let Routing::Judged(mut routing) = router.select(&context)? else {
        panic!("a decision that tolerates an estimate must be judged");
    };
    for _ in 0..2 {
        routing = router
            .escalate(&routing, &context, unconvincing, &limits)?
            .routing()
            .clone();
    }
    assert_eq!(routing.tier(), IntelligenceTier::SpecialistModel);

    let refusal = router
        .escalate(&routing, &context, unconvincing, &limits)
        .expect_err("the spend limit must refuse, not silently overrun");
    assert!(refusal.message().contains("0.05"), "{refusal}");
    assert!(refusal.message().contains("agent"), "{refusal}");
    assert_eq!(
        routing.total_cost(),
        dec!("0.00442"),
        "a refused escalation charges nothing"
    );
    Ok(())
}

#[test]
fn an_answer_that_clears_the_bar_settles_instead_of_climbing() -> Result<()> {
    // The other half of bounding: escalation only happens when the answer was
    // actually insufficient. A ladder that climbed on every call would spend
    // the whole budget on decisions it had already made.
    let router = Router::default();
    let context = context(
        dec!("100000"),
        Duration::from_secs(300),
        0.60,
        Determinism::NotRequired,
    );
    let limits = EscalationLimits::new(IntelligenceTier::Solver, dec!("100"))?;

    let Routing::Judged(routing) = router.select(&context)? else {
        panic!("a decision that tolerates an estimate must be judged");
    };
    let convincing = Conviction::new(0.95, 5_000);
    let escalation = router.escalate(&routing, &context, convincing, &limits)?;
    assert!(matches!(escalation, Escalation::Settled(_)));
    assert_eq!(escalation.routing().total_cost(), routing.total_cost());
    assert_eq!(escalation.routing().escalations(), 0);
    Ok(())
}

// --- contextual reputation --------------------------------------------------

fn production_model(registry: &mut ModelRegistry, name: &str, at: Timestamp) -> Result<String> {
    let mut card = ModelCard::new(
        ModelId::from_string(name),
        name,
        "1.0",
        "quant.research",
        at,
    );
    card.stage = qip_ai::registry::ModelStage::Validation;
    let reference = card.reference();
    registry.register(card);
    registry.record_evaluation(
        &reference,
        EvaluationRecord {
            evaluated_at: at,
            dataset: "held-out".to_string(),
            metrics: BTreeMap::new(),
            passed: true,
        },
    )?;
    registry.promote(&reference, at)?;
    Ok(reference)
}

/// Mint a judgement the way the platform has to: rank under the conditions the
/// decision is being made in, and take the token from the model that was
/// chosen.
///
/// There is deliberately no shortcut, here or anywhere. A helper that built a
/// `Judgement` from a bare model name and a bare `Conditions` would let every
/// test below pass while the production caller keyed its scores on whatever
/// market it was looking at when the outcome came back, which is the failure
/// the type exists to make unwritable.
fn judgement_for(
    book: &ReputationBook,
    registry: &ModelRegistry,
    reference: &str,
    under: &Conditions,
) -> Result<Judgement> {
    book.rank(registry, under, now())
        .into_iter()
        .find(|rated| rated.card.reference() == reference)
        .expect("the model is registered and eligible to decide")
        .judgement()
}

#[test]
fn a_model_with_no_observations_in_a_regime_does_not_read_as_competent_there() -> Result<()> {
    // The failure contextual reputation exists to prevent: a model that has
    // been right for two years in a trending tape has said nothing about what
    // it will do in a crisis, and a single global accuracy reports exactly that
    // as competence.
    let mut registry = ModelRegistry::new();
    let reference = production_model(&mut registry, "dislocation-classifier", now())?;

    let mut book = ReputationBook::new();
    let trending_judgement = judgement_for(
        &book,
        &registry,
        &reference,
        &conditions(MarketRegime::Trending),
    )?;
    for _ in 0..40 {
        book.observe(&trending_judgement, true);
    }

    let trending = book.competence(&reference, &conditions(MarketRegime::Trending));
    assert_eq!(trending.observations(), 40);
    assert!(
        trending.clears(0.7),
        "forty out of forty is real evidence and should read as such"
    );

    // The same model, the same day, a regime it has never been tried in.
    let crisis = book.competence(&reference, &conditions(MarketRegime::Crisis));
    assert_eq!(crisis.observations(), 0);
    assert!(
        (crisis.shrunk() - 0.5).abs() < f64::EPSILON,
        "an unseen regime must read as a coin flip, not as the model's average elsewhere"
    );
    assert!(!crisis.clears(0.55), "{}", crisis.shrunk());

    let refusal = book
        .select(&registry, &conditions(MarketRegime::Crisis), 0.7, now())
        .expect_err("no model has earned the right to decide in a regime it has not seen");
    assert!(refusal.message().contains("crisis"), "{refusal}");
    assert!(refusal.message().contains("0 observations"), "{refusal}");

    // Shrinkage, not just absence: a perfect record of two is not a reputation.
    let mut thin = ReputationBook::new();
    let crisis_judgement = judgement_for(
        &thin,
        &registry,
        &reference,
        &conditions(MarketRegime::Crisis),
    )?;
    for _ in 0..2 {
        thin.observe(&crisis_judgement, true);
    }
    let lucky = thin.competence(&reference, &conditions(MarketRegime::Crisis));
    assert!(
        (lucky.probability() - 1.0).abs() < f64::EPSILON,
        "the raw hit rate really is perfect"
    );
    assert!(
        !lucky.clears(0.6),
        "two for two shrinks to {} and must not clear a 0.6 bar",
        lucky.shrunk()
    );
    Ok(())
}

#[test]
fn a_model_the_registry_refuses_is_never_ranked_however_good_its_record() -> Result<()> {
    // Governance is `ModelCard::decision_eligibility` and this crate does not
    // get a second opinion on it. A retired model with a spotless record is
    // still retired.
    let mut registry = ModelRegistry::new();
    let good = production_model(&mut registry, "regime-classifier", now())?;
    let retired = production_model(&mut registry, "old-classifier", now())?;

    let mut book = ReputationBook::new();
    let trending = conditions(MarketRegime::Trending);
    let retired_judgement = judgement_for(&book, &registry, &retired, &trending)?;
    let good_judgement = judgement_for(&book, &registry, &good, &trending)?;
    for _ in 0..500 {
        book.observe(&retired_judgement, true);
    }
    for _ in 0..100 {
        book.observe(&good_judgement, true);
    }

    let before = book.rank(&registry, &conditions(MarketRegime::Trending), now());
    assert_eq!(before.len(), 2);
    assert_eq!(
        before[0].card.reference(),
        retired,
        "the better record leads"
    );

    registry.retire(&retired, now())?;
    let after = book.rank(&registry, &conditions(MarketRegime::Trending), now());
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].card.reference(), good);
    assert_eq!(
        book.select(&registry, &conditions(MarketRegime::Trending), 0.7, now())?
            .card
            .reference(),
        good
    );
    Ok(())
}

#[test]
fn ranking_two_models_with_identical_records_is_stable() -> Result<()> {
    // A tie broken arbitrarily would make the routing decision depend on
    // iteration order, which is not a property of the decision.
    let mut registry = ModelRegistry::new();
    let left = production_model(&mut registry, "alpha-classifier", now())?;
    let right = production_model(&mut registry, "beta-classifier", now())?;

    let mut book = ReputationBook::new();
    let quiet = conditions(MarketRegime::Quiet);
    let left_judgement = judgement_for(&book, &registry, &left, &quiet)?;
    let right_judgement = judgement_for(&book, &registry, &right, &quiet)?;
    for _ in 0..50 {
        book.observe(&left_judgement, true);
        book.observe(&right_judgement, true);
    }

    let first = book.rank(&registry, &conditions(MarketRegime::Quiet), now());
    for _ in 0..16 {
        let again = book.rank(&registry, &conditions(MarketRegime::Quiet), now());
        assert_eq!(
            again.iter().map(|r| r.card.reference()).collect::<Vec<_>>(),
            first.iter().map(|r| r.card.reference()).collect::<Vec<_>>()
        );
    }
    assert_eq!(first[0].card.reference(), left, "ties break by reference");
    assert_eq!(first[1].card.reference(), right);
    Ok(())
}

// --- the conditions a decision was made under, kept as data ------------------

#[test]
fn a_routing_record_carries_the_conditions_it_was_made_under_on_both_arms() -> Result<()> {
    // `Conditions` doubles as the routing record and as the key a model's
    // record is kept under, and the crate says so. It only holds if the routing
    // record actually carries the value: reciting it inside the rationale
    // sentence leaves a later caller with prose to parse or a market to guess
    // from, and it will guess.
    let router = Router::default();
    let under = Conditions::new(
        AssetClass::Equity,
        Region::new("apac"),
        MarketRegime::Crisis,
        VolatilityRegime::Extreme,
        Horizon::Strategic,
    );
    // Assert the premise: this is not the conditions the other helpers build,
    // so a routing that reported some default would not match by accident.
    assert_ne!(under, conditions(MarketRegime::Crisis));

    for determinism in [Determinism::Required, Determinism::NotRequired] {
        let context = DecisionContext::new(
            "is the dislocation real",
            dec!("100000"),
            Duration::from_secs(300),
            0.60,
            determinism,
            under.clone(),
        );
        let routing = router.select(&context)?;
        assert_eq!(
            routing.conditions(),
            &under,
            "the {determinism:?} arm lost the conditions its decision was made under"
        );
    }
    Ok(())
}

#[test]
fn an_escalated_routing_keeps_the_conditions_the_decision_started_under() -> Result<()> {
    // The failure this prevents, which is a live one rather than a
    // hypothetical: `escalate` is handed a `&DecisionContext` by the caller,
    // and a caller climbing a rung after a slow first answer may well have
    // rebuilt that context from a fresher market. If the climbed routing took
    // its conditions from the context, the record of what the platform believed
    // the market was doing would move underneath the decision, and the model
    // that answered in a crisis would be scored as having answered in whatever
    // came next.
    let router = Router::default();
    let started_in = conditions(MarketRegime::Crisis);
    let started = DecisionContext::new(
        "is the dislocation real",
        dec!("100000"),
        Duration::from_secs(300),
        0.60,
        Determinism::NotRequired,
        started_in.clone(),
    );
    // The same decision, re-read from a market that has moved on.
    let moved_on = conditions(MarketRegime::Quiet);
    assert_ne!(started_in, moved_on, "the two markets must actually differ");
    let refreshed = DecisionContext::new(
        "is the dislocation real",
        dec!("100000"),
        Duration::from_secs(300),
        0.60,
        Determinism::NotRequired,
        moved_on.clone(),
    );

    let Routing::Judged(routing) = router.select(&started)? else {
        panic!("a decision that tolerates an estimate must be judged");
    };
    assert_eq!(
        routing.conditions(),
        &started_in,
        "premise: it started here"
    );

    let limits = EscalationLimits::new(IntelligenceTier::DeepModel, dec!("10"))?;
    let unconvincing = Conviction::new(0.55, 1_000);
    let escalation = router.escalate(&routing, &refreshed, unconvincing, &limits)?;
    assert!(
        escalation.climbed(),
        "premise: the answer was unconvincing and had to climb"
    );
    assert_eq!(
        escalation.routing().conditions(),
        &started_in,
        "the climbed rung adopted the refreshed market as the conditions of a decision that was made before it"
    );
    assert_ne!(escalation.routing().conditions(), &moved_on);
    Ok(())
}

#[test]
fn a_models_score_lands_in_the_cell_it_was_chosen_in_not_the_one_its_outcome_resolved_in()
-> Result<()> {
    // The reason `ReputationBook::observe` takes a `Judgement` and not a model
    // name and a `Conditions`. An outcome is resolved at the end of the
    // decision's horizon — this one is `Horizon::Intraday`, and a strategic
    // call resolves months out — and the caller that learns whether the answer
    // was right is looking at a different market. Given a free `Conditions`
    // parameter it will fill it from the market in front of it, and the book
    // will then say the model is good at quiet tapes when what it survived was
    // a crisis.
    let mut registry = ModelRegistry::new();
    let reference = production_model(&mut registry, "dislocation-classifier", now())?;
    let mut book = ReputationBook::new();

    let decided_in = conditions(MarketRegime::Crisis);
    let resolved_in = conditions(MarketRegime::Quiet);
    assert_ne!(
        decided_in, resolved_in,
        "the two cells must actually differ"
    );

    // Premise: neither cell has anything in it, so an assertion that one of
    // them moved is an assertion about this observation and not about setup.
    assert_eq!(book.competence(&reference, &decided_in).observations(), 0);
    assert_eq!(book.competence(&reference, &resolved_in).observations(), 0);

    // The decision: the model is chosen under the crisis, and the token is
    // minted there.
    let judgement = judgement_for(&book, &registry, &reference, &decided_in)?;
    assert_eq!(judgement.conditions(), &decided_in);
    assert_eq!(judgement.model(), reference);

    // Much later, in a quiet market, the outcome comes back. The caller has
    // nothing to key it on but the token.
    book.observe(&judgement, true);

    assert_eq!(
        book.competence(&reference, &decided_in).observations(),
        1,
        "the score belongs in the cell the decision was made in"
    );
    assert_eq!(
        book.competence(&reference, &resolved_in).observations(),
        0,
        "the quiet tape the outcome happened to resolve in earned the model nothing"
    );
    Ok(())
}

// --- the second mint: reading a judgement back from the log ------------------
//
// `Judgement`'s doc claimed the two ways to obtain one "both carry
// decision-time conditions by construction, so there is no expression a
// resolution-time caller can write that keys a score on the market it happens
// to be looking at". `#[derive(Deserialize)]` was that expression, and it was
// available from any crate carrying `serde_json` — including `qip-kernel`,
// which is where `ReputationBook`'s wiring is destined. A resolution-time
// caller built a `Conditions` from the market in front of it (`Conditions::new`
// is public), round-tripped it through `serde_json::from_value::<Judgement>`
// and handed the result to `observe`, which took it — for a model reference no
// `ModelRegistry` ever held, and for the empty string.
//
// What follows is what the fix does establish. What it cannot establish — that
// the conditions in a document were captured at the decision — is stated on the
// type rather than asserted here, because no test in this crate could prove it.

/// A judgement minted the way the platform mints one, and the document it
/// serialises to.
///
/// Each forgery below is this document with one field edited, so what is proved
/// is the edit rather than a difference between a hand-written blob and a real
/// token.
fn genuine_judgement() -> Result<(Judgement, serde_json::Value)> {
    let mut registry = ModelRegistry::new();
    let reference = production_model(&mut registry, "dislocation-classifier", now())?;
    let book = ReputationBook::new();
    let judgement = judgement_for(
        &book,
        &registry,
        &reference,
        &conditions(MarketRegime::Crisis),
    )?;
    let document = serde_json::to_value(&judgement)
        .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    Ok((judgement, document))
}

#[test]
fn a_judgement_read_back_from_a_document_meets_the_same_check_the_mint_ran() -> Result<()> {
    let (judgement, document) = genuine_judgement()?;

    // Premise, and the half that separates a working gate from one that refuses
    // everything: a judgement the platform minted survives the round trip and
    // compares equal, so the refusals below are caused by the edit. A token the
    // decision path can write and the replay path refuses would be a decision
    // that cannot be scored twice the same way.
    let round_tripped: Judgement = serde_json::from_value(document.clone())
        .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    assert_eq!(round_tripped, judgement);
    assert_eq!(round_tripped.conditions(), judgement.conditions());

    // The forgery as it was actually written: a bare model id with no version.
    // It is not a `ModelCard::reference`, so `ReputationBook::competence` —
    // which is always called with a `card.reference()` — can never read the
    // cell it would book into. An accumulating record nobody reads is worse
    // than none, because it looks like a reputation.
    let mut forged = document.clone();
    assert!(
        forged.get("model").is_some(),
        "the document must have a model field for editing it to forge anything"
    );
    forged["model"] = serde_json::json!("some-vendor/some-model");
    let refusal = serde_json::from_value::<Judgement>(forged)
        .expect_err("a document naming something that is not a model reference must be refused");
    assert!(
        refusal.to_string().contains("is not a model reference"),
        "the refusal must name what is wrong with it, said: {refusal}"
    );
    assert!(
        refusal.to_string().contains("supply name@version"),
        "the refusal must name what to supply instead, said: {refusal}"
    );

    // The empty string, which booked a cell keyed on "" that nothing looks up.
    let mut ghost = document.clone();
    ghost["model"] = serde_json::json!("");
    let refusal = serde_json::from_value::<Judgement>(ghost)
        .expect_err("a document naming no model must be refused");
    assert!(
        refusal.to_string().contains("a judgement names no model"),
        "the refusal must say the model is missing, said: {refusal}"
    );

    // Padding is its own refusal and not a trim. `" name@1 "` and `"name@1"`
    // are different keys in the book, so a whitespace-padded reference books a
    // cell adjacent to the real one and invisible from it. Note the substring
    // trap this avoids: the padded string *contains* a valid reference, so a
    // check that looked for one would pass.
    let mut padded = document.clone();
    padded["model"] = serde_json::json!(format!(" {} ", judgement.model()));
    let refusal = serde_json::from_value::<Judgement>(padded)
        .expect_err("a padded reference must be refused rather than trimmed");
    assert!(
        refusal.to_string().contains("with surrounding whitespace"),
        "the refusal must name the padding, said: {refusal}"
    );

    // Half a reference either side of the separator.
    for half in ["@1.0.0", "dislocation-classifier@"] {
        let mut broken = document.clone();
        broken["model"] = serde_json::json!(half);
        let refusal = serde_json::from_value::<Judgement>(broken)
            .expect_err("half a model reference must be refused");
        assert!(
            refusal.to_string().contains("supply name@version"),
            "the refusal must name the shape, said: {refusal}"
        );
    }

    // The mint and the replay must agree on every string
    // `ModelCard::reference` can produce, and it can produce a great many:
    // `reference` is `format!("{name}@{version}")` over two free-text fields.
    // Hence the split at the *last* separator. Splitting at the first would
    // refuse a card whose name contains one — a token the platform itself
    // minted, refused a year later on replay, which is precisely the asymmetry
    // the shared constructor exists to remove. Driven through a real card
    // rather than an edited document so the mint side is exercised too.
    let mut registry = ModelRegistry::new();
    let awkward = production_model(&mut registry, "@vendor/dislocation@classifier", now())?;
    assert_eq!(
        awkward, "@vendor/dislocation@classifier@1.0",
        "the premise is a reference whose name contains the separator and begins with it"
    );
    let book = ReputationBook::new();
    let minted = judgement_for(
        &book,
        &registry,
        &awkward,
        &conditions(MarketRegime::Crisis),
    )?;
    let text = serde_json::to_value(&minted)
        .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    let read_back: Judgement =
        serde_json::from_value(text).map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    assert_eq!(read_back, minted);
    assert_eq!(read_back.model(), awkward);
    Ok(())
}

/// A judgement inside an internally-tagged enum — the route `qip-financial`'s
/// `Extension` takes, where serde buffers the content and re-deserialises from
/// the buffer. That buffering is where a `serde(try_from)` is most likely to be
/// silently dropped, so the fix is attacked through it deliberately.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TaggedOutcome {
    Scored { judgement: Judgement, correct: bool },
}

/// The same, untagged: serde tries each variant against a buffered copy.
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum UntaggedOutcome {
    Scored(Judgement),
}

#[test]
fn a_forged_judgement_is_refused_through_every_container_that_buffers_it() -> Result<()> {
    let (judgement, document) = genuine_judgement()?;
    let mut forged = document.clone();
    forged["model"] = serde_json::json!("some-vendor/some-model");

    // A list, and a map keyed by decision id — the shape a journal of open
    // decisions would take.
    let accepted: Vec<Judgement> = serde_json::from_value(serde_json::json!([document.clone()]))
        .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    assert_eq!(accepted, vec![judgement.clone()]);
    assert!(
        serde_json::from_value::<Vec<Judgement>>(serde_json::json!([forged.clone()])).is_err(),
        "a forged judgement inside a list must be refused"
    );

    let accepted: BTreeMap<String, Judgement> =
        serde_json::from_value(serde_json::json!({ "decision-1": document.clone() }))
            .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    assert_eq!(accepted.len(), 1, "the genuine map must be readable");
    assert!(
        serde_json::from_value::<BTreeMap<String, Judgement>>(
            serde_json::json!({ "decision-1": forged.clone() })
        )
        .is_err(),
        "a forged judgement inside a map must be refused"
    );

    let accepted: Option<Judgement> = serde_json::from_value(document.clone())
        .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    assert_eq!(accepted, Some(judgement.clone()));
    assert!(
        serde_json::from_value::<Option<Judgement>>(forged.clone()).is_err(),
        "a forged judgement inside an Option must be refused"
    );

    // The internally-tagged route, which is how an outcome record would most
    // naturally carry one.
    let genuine_tagged = serde_json::json!({
        "kind": "scored", "judgement": document.clone(), "correct": true,
    });
    let forged_tagged = serde_json::json!({
        "kind": "scored", "judgement": forged.clone(), "correct": true,
    });
    let accepted: TaggedOutcome = serde_json::from_value(genuine_tagged)
        .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    let TaggedOutcome::Scored {
        judgement: read_back,
        correct,
    } = accepted;
    assert_eq!(read_back, judgement);
    assert!(correct);
    let refusal = serde_json::from_value::<TaggedOutcome>(forged_tagged)
        .expect_err("a forged judgement inside an internally-tagged enum must be refused");
    assert!(
        refusal.to_string().contains("is not a model reference"),
        "the tagged route must keep the refusal's own words, said: {refusal}"
    );

    // The untagged route refuses too, but serde reports "did not match any
    // variant" and the refusal's words are lost. Asserted rather than left
    // unsaid: an untagged container is a bad place for a judgement, because the
    // record to correct cannot be named from the error.
    let genuine_untagged: UntaggedOutcome = serde_json::from_value(document)
        .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    let UntaggedOutcome::Scored(read_back) = genuine_untagged;
    assert_eq!(read_back, judgement);
    assert!(
        serde_json::from_value::<UntaggedOutcome>(forged).is_err(),
        "a forged judgement inside an untagged enum must be refused"
    );
    Ok(())
}

#[test]
fn a_card_with_no_name_or_no_version_cannot_mint_a_judgement_the_replay_would_refuse() -> Result<()>
{
    // `ModelCard::new` does not require a name or a version, so a card carrying
    // neither produces a reference of `@` — and the decision path used to mint
    // a token from it happily, leaving the refusal for whoever replayed the log
    // a year later. One predicate, both mints: the refusal arrives at the
    // decision, where the corrupt card can still be fixed.
    let mut registry = ModelRegistry::new();
    let good = production_model(&mut registry, "dislocation-classifier", now())?;

    let mut nameless = ModelCard::new(
        ModelId::from_string("m-nameless"),
        "",
        "1.0",
        "quant.research",
        now(),
    );
    nameless.stage = qip_ai::registry::ModelStage::Validation;
    let nameless_reference = nameless.reference();
    assert_eq!(
        nameless_reference, "@1.0",
        "a card with no name must produce exactly the reference the mint has to refuse"
    );
    registry.register(nameless);
    registry.record_evaluation(
        &nameless_reference,
        EvaluationRecord {
            evaluated_at: now(),
            dataset: "held-out".to_string(),
            metrics: BTreeMap::new(),
            passed: true,
        },
    )?;
    registry.promote(&nameless_reference, now())?;

    let book = ReputationBook::new();
    let ranked = book.rank(&registry, &conditions(MarketRegime::Quiet), now());
    // Premise: both cards are eligible and ranked, so the refusal below is
    // about the reference and not about the card being filtered out first.
    assert_eq!(ranked.len(), 2, "both cards must be eligible to decide");

    let mut minted = 0;
    let mut refused = 0;
    for rated in &ranked {
        match rated.judgement() {
            Ok(token) => {
                assert_eq!(token.model(), good);
                minted += 1;
            }
            Err(refusal) => {
                assert!(
                    refusal.message().contains("which has no name"),
                    "the refusal must name the missing half, said: {}",
                    refusal.message()
                );
                refused += 1;
            }
        }
    }
    assert_eq!(minted, 1, "the sound card must still mint a judgement");
    assert_eq!(refused, 1, "the nameless card must not mint one");
    Ok(())
}
