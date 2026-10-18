//! The Meta-Intelligence market (§9.12; R12-AMBIENT-7202, 3110, 3111, 3112,
//! 3113, 3114): compute granted by marginal value per unit of cost, a QPU
//! never without its classical baseline, a realised value measured per
//! added unit, unproductive work retired, reputation held per cell, and
//! specialist populations proposed on economic weight.
//!
//! Each test asserts its premise first — that the bids could have been
//! granted — so a market that grants nothing cannot pass a test about what
//! it declines.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)] // integration tests may unwrap; the float comparisons are against values the arithmetic produces exactly

use qip_contracts::ambient::{ComputeResourceType, ModelReputation};
use qip_contracts::control::ComputeBackend;
use qip_core::{Decimal, Timestamp};
use qip_expansion::meta::{
    Decline, DomainResidual, MetaIntelligence, MetaPolicy, REALISATION_PRIOR_BP, ReputationBook,
    ReputationKey, Standing, UNINFORMED_REPUTATION_BP, WorkBid, WorkKind,
};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

fn key(domain: &str) -> ReputationKey {
    ReputationKey::new(domain, "research", "all").unwrap()
}

fn bid(id: &str, information_bp: u32, cost: i64, units: u32) -> WorkBid {
    WorkBid {
        work_id: id.to_string(),
        kind: WorkKind::Question,
        subject: id.to_string(),
        resource: ComputeResourceType::Cpu,
        information_value_bp: information_bp,
        economic_value_bp: 0,
        unit_cost: Decimal::from_int(cost),
        max_units: units,
        reputation: key("execution"),
        classical_baseline_cost: None,
    }
}

fn cpu(units: i64) -> BTreeMap<ComputeResourceType, Decimal> {
    BTreeMap::from([(ComputeResourceType::Cpu, Decimal::from_int(units))])
}

fn market() -> MetaIntelligence {
    MetaIntelligence::new(MetaPolicy::default()).unwrap()
}

/// With room for one unit, the unit goes to the bid worth more per unit of
/// cost and the other is outbid. Mutation: sort the greedy pick ascending
/// (`is_lt` for `is_gt`) — `cheap` is granted and both assertions fail.
#[test]
fn the_next_unit_goes_to_the_work_with_the_higher_marginal_value_per_cost() {
    let mut market = market();
    let rich = bid("rich", 8_000, 2, 1);
    let cheap = bid("cheap", 2_000, 2, 1);
    assert!(
        market.marginal_value_bp(&cheap, 0) / 2.0 >= market.policy().value_floor_bp_per_unit_cost,
        "premise: either bid clears the floor on its own"
    );

    let round = market.allocate(&[cheap, rich], &cpu(2), now()).unwrap();

    assert!(round.grant("rich").is_some(), "{round:?}");
    assert!(round.grant("cheap").is_none(), "{round:?}");
    assert!(
        round
            .declined
            .contains(&("cheap".to_string(), Decline::Outbid)),
        "{round:?}"
    );
}

/// The second unit of one question is worth half the first and the third a
/// third: the market buys breadth before depth. Mutation: drop the
/// `/ (k + 1)` in `marginal_value_bp` — the expected value becomes 3 × the
/// first unit's, and the sum assertion fails.
#[test]
fn each_further_unit_of_one_question_is_worth_less_than_the_last() {
    let mut market = market();
    let deep = bid("deep", 6_000, 1, 3);
    let first = market.marginal_value_bp(&deep, 0);
    assert!(first > 0.0, "premise: the first unit is worth something");

    let round = market.allocate(&[deep], &cpu(3), now()).unwrap();

    let grant = round.grant("deep").unwrap();
    assert_eq!(grant.allocation.units(), 3);
    let expected = first + first / 2.0 + first / 3.0;
    assert!((grant.expected_bp - expected).abs() < 1e-9, "{grant:?}");
}

/// Every grant's rationale states the value it was granted on
/// (R12-AMBIENT-3110), and the figure matches the grant's own. Mutation:
/// replace the rationale with a constant sentence — the `contains` on the
/// formatted figure fails.
#[test]
fn every_allocation_records_the_expected_value_it_was_granted_on() {
    let mut market = market();
    let round = market
        .allocate(&[bid("q", 5_000, 1, 1)], &cpu(1), now())
        .unwrap();
    let grant = round.grant("q").expect("premise: the bid was granted");
    let figure = format!("an expected {:.1} bp", grant.expected_bp);
    assert!(
        grant.allocation.rationale().contains(&figure),
        "{} does not state {figure}",
        grant.allocation.rationale()
    );
    assert!(grant.expected_bp > 0.0);
}

/// A QPU unit is granted only with its classical baseline charged to the
/// CPU budget and named in a validated ComputePlan (ADR 0006). With no CPU
/// to pay for the baseline the QPU bid is declined by name. Mutation:
/// delete the `cpu_left < baseline` check — the first round grants the QPU
/// with an unpaid baseline and the decline assertion fails.
#[test]
fn a_qpu_unit_is_never_granted_without_its_classical_baseline_being_paid() {
    let mut quantum = bid("qaoa", 9_000, 1, 1);
    quantum.resource = ComputeResourceType::Qpu;
    quantum.kind = WorkKind::Solver;
    quantum.classical_baseline_cost = Some(Decimal::from_int(1));
    let qpu_only = BTreeMap::from([(ComputeResourceType::Qpu, Decimal::from_int(5))]);

    let starved = market()
        .allocate(std::slice::from_ref(&quantum), &qpu_only, now())
        .unwrap();
    assert!(
        starved
            .declined
            .contains(&("qaoa".to_string(), Decline::NoClassicalBaseline)),
        "{starved:?}"
    );

    let mut both = qpu_only;
    both.insert(ComputeResourceType::Cpu, Decimal::from_int(1));
    let round = market().allocate(&[quantum], &both, now()).unwrap();
    let grant = round
        .grant("qaoa")
        .expect("a paid baseline admits the QPU unit");
    assert_eq!(
        grant.charged.get(&ComputeResourceType::Cpu),
        Some(&Decimal::from_int(1))
    );
    let plan = grant.plan.as_ref().expect("a QPU grant carries its plan");
    let baseline = plan
        .candidates
        .iter()
        .find(|c| c.id == plan.classical_baseline)
        .unwrap();
    assert_eq!(baseline.backend, ComputeBackend::Cpu);
}

/// A QPU bid that names no baseline is refused before the market runs.
#[test]
fn a_qpu_bid_with_no_classical_baseline_is_refused_outright() {
    let mut quantum = bid("qaoa", 9_000, 1, 1);
    quantum.resource = ComputeResourceType::Qpu;
    let error = market()
        .allocate(&[quantum], &cpu(10), now())
        .expect_err("a QPU bid with no baseline was priced");
    assert!(error.message().contains("ADR 0006"), "{}", error.message());
}

/// Bids sharing a subject are one piece of work: the most valuable is
/// priced and the others are declined naming it. Mutation: skip the
/// redundancy pass (push every non-retired bid to `eligible`) — both are
/// granted and the decline assertion fails.
#[test]
fn bids_sharing_a_subject_are_redundant_and_only_the_most_valuable_is_priced() {
    let mut a = bid("a", 3_000, 1, 1);
    let mut b = bid("b", 6_000, 1, 1);
    a.subject = "venue X".to_string();
    b.subject = "venue X".to_string();

    let round = market().allocate(&[a, b], &cpu(10), now()).unwrap();

    assert!(
        round.grant("b").is_some(),
        "premise: the better bid is granted"
    );
    assert!(
        round.declined.contains(&(
            "a".to_string(),
            Decline::Redundant {
                with: "b".to_string()
            }
        )),
        "{round:?}"
    );
}

/// One window's realised value is recorded per added unit
/// (R12-AMBIENT-3112). Mutation: record `realised_bp` as `per_unit_bp` —
/// the per-unit assertion fails.
#[test]
fn a_marginal_value_is_recorded_per_added_unit() {
    let mut market = market();
    let measured = market
        .record_outcome("q", WorkKind::Simulation, 4, 800.0, 400.0, now())
        .unwrap();
    assert_eq!(measured.per_unit_bp, 100.0);
    assert_eq!(market.marginal_values("q").count(), 1);
}

/// Work that delivered half of what it promised bids lower next round, by
/// exactly the measured ratio under the stated prior: the market learns
/// from what it measured. Mutation: return 1.0 from
/// `WorkHistory::realisation` — the value does not move and the assertion
/// fails.
#[test]
fn work_that_delivered_half_its_promise_bids_lower_by_the_measured_ratio() {
    let mut market = market();
    let q = bid("q", 6_000, 1, 1);
    let before = market.marginal_value_bp(&q, 0);
    market
        .record_outcome("q", WorkKind::Question, 1, 1_000.0, 500.0, now())
        .unwrap();
    let ratio = (500.0 + REALISATION_PRIOR_BP) / (1_000.0 + REALISATION_PRIOR_BP);
    assert!(ratio < 1.0, "premise: half delivery is below full");
    assert!((market.marginal_value_bp(&q, 0) - before * ratio).abs() < 1e-9);
}

/// Work that realises nothing is still granted after one empty window —
/// lowered, not zeroed — so the market keeps measuring it until the
/// retirement rule decides. This is the stall the prior exists to prevent:
/// a zeroed bid is never granted, never measured again, and never retired.
/// Mutation: set `REALISATION_PRIOR_BP` to 0 — the second round declines
/// the bid below the floor and the assertion fails.
#[test]
fn one_empty_window_lowers_a_bid_without_starving_it_before_retirement() {
    let mut market = market();
    let q = bid("q", 6_000, 1, 1);
    market
        .record_outcome("q", WorkKind::Question, 1, 3_000.0, 0.0, now())
        .unwrap();
    assert!(
        !market.is_retired("q"),
        "premise: one window retires nothing"
    );
    let round = market.allocate(&[q], &cpu(10), now()).unwrap();
    assert!(round.grant("q").is_some(), "{round:?}");
}

/// Work unproductive for `retire_after` consecutive windows is retired and
/// declined from then on; one productive window resets the count
/// (R12-AMBIENT-3114). Mutation: never reset `unproductive_streak` on a
/// productive window — `steady` is retired too and its assertion fails.
#[test]
fn work_unproductive_for_the_window_is_retired_and_productive_work_is_not() {
    let mut market = market();
    let after = market.policy().retire_after;
    assert!(after >= 2, "premise: a streak longer than one window");
    for _ in 0..after {
        market
            .record_outcome("idle", WorkKind::Question, 1, 500.0, 0.0, now())
            .unwrap();
    }
    for window in 0..(after * 2) {
        let realised = if window % 2 == 0 { 0.0 } else { 400.0 };
        market
            .record_outcome("steady", WorkKind::Question, 1, 500.0, realised, now())
            .unwrap();
    }
    assert!(market.is_retired("idle"));
    assert!(!market.is_retired("steady"));

    let round = market
        .allocate(
            &[bid("idle", 9_000, 1, 1), bid("steady", 9_000, 1, 1)],
            &cpu(10),
            now(),
        )
        .unwrap();
    assert!(round.grant("steady").is_some(), "{round:?}");
    assert!(
        round
            .declined
            .iter()
            .any(|(id, d)| id == "idle" && matches!(d, Decline::Retired { .. })),
        "{round:?}"
    );
}

/// A reputation cell nobody has recorded reads as uninformed — the stated
/// prior — and not as the average of the domain's other cells
/// (R12-AMBIENT-3111). Mutation: make `standing` fall back to the mean of
/// recorded cells — the unseen regime then reads 9000 and the assertion
/// fails.
#[test]
fn an_unseen_regime_cell_reads_as_uninformed_and_not_as_the_average() {
    let mut book = ReputationBook::default();
    book.record(ModelReputation::new("momentum", "1d", "calm", 9_000, 9_000, 9_000).unwrap())
        .unwrap();
    let seen = ReputationKey::new("momentum", "1d", "calm").unwrap();
    let unseen = ReputationKey::new("momentum", "1d", "crisis").unwrap();
    assert_eq!(
        book.standing(&seen).weight_bp(),
        9_000,
        "premise: the recorded cell reads its own record"
    );
    assert_eq!(book.standing(&unseen), Standing::Uninformed);
    assert_eq!(book.standing(&unseen).weight_bp(), UNINFORMED_REPUTATION_BP);
}

/// A residual whose economic weight clears the bar proposes a specialist
/// population; a larger residual on a trivial exposure does not
/// (R12-AMBIENT-3113). Mutation: compare the residual rather than the
/// weight against the threshold — the trivial-exposure domain is proposed
/// and the length assertion fails.
#[test]
fn a_specialist_population_is_proposed_on_economic_weight_and_not_on_residual_alone() {
    let market = market();
    let threshold = market.policy().specialist_threshold;
    let residuals = vec![
        DomainResidual {
            domain: "rates".to_string(),
            residual: 0.2,
            exposure: Decimal::from_int(1_000_000),
        },
        DomainResidual {
            domain: "tiny".to_string(),
            residual: 0.9,
            exposure: Decimal::from_int(10),
        },
    ];
    assert!(
        Decimal::from_int(10) < threshold,
        "premise: the tiny exposure is below the bar whatever its residual"
    );
    let proposals = market.propose_specialists(&residuals).unwrap();
    assert_eq!(proposals.len(), 1, "{proposals:?}");
    assert_eq!(proposals[0].domain, "rates");
    assert!(proposals[0].economic_weight >= threshold);
}

/// A bid worth less per unit of cost than the floor is declined below the
/// floor even with budget to spare: compute is not spent because it exists.
#[test]
fn a_bid_below_the_value_floor_is_declined_with_budget_to_spare() {
    let mut market = market();
    let faint = bid("faint", 10, 1, 1);
    assert!(
        market.marginal_value_bp(&faint, 0) < market.policy().value_floor_bp_per_unit_cost,
        "premise: the bid is below the floor"
    );
    let round = market.allocate(&[faint], &cpu(100), now()).unwrap();
    assert!(
        round
            .declined
            .contains(&("faint".to_string(), Decline::BelowFloor))
    );
    assert!(round.grants.is_empty());
}

/// R12-AMBIENT-9034: an agent's compute budget is a bound, not advice. The
/// agent bidding as `execution` may be charged three units; its first bid
/// wants five and gets three, and its second bid — a different question,
/// so not redundant, worth less per unit than the first bid's third — is
/// declined `OverAgentBudget`, while another agent's bid in the same round
/// is unaffected. The failure prevented: one
/// well-reputed agent outbids every other for the whole round, because the
/// per-resource budget bounds the platform and nothing bounded the agent.
///
/// Mutation: in `allocate_within`, delete the agent-budget check, and the
/// first bid is granted five units and the second is granted too.
#[test]
fn an_agent_bid_over_its_compute_budget_is_refused() {
    let hungry = bid("hungry", 9_000, 1, 5);
    let late = bid("late", 1_000, 1, 5);
    let mut other = bid("other", 9_000, 1, 2);
    other.reputation = key("macro");
    let bids = [hungry, late, other];

    let unbounded = market().allocate(&bids, &cpu(100), now()).unwrap();
    let units = |round: &qip_expansion::meta::AllocationRound, id: &str| {
        round
            .grant(id)
            .and_then(|g| g.charged.get(&ComputeResourceType::Cpu).copied())
    };
    assert!(
        units(&unbounded, "hungry") > Some(Decimal::from_int(3)),
        "premise: unbounded, the agent takes more than its budget: {unbounded:?}"
    );

    let agents = BTreeMap::from([(key("execution"), Decimal::from_int(3))]);
    let round = market()
        .allocate_within(&bids, &cpu(100), &agents, now())
        .unwrap();
    assert_eq!(
        units(&round, "hungry"),
        Some(Decimal::from_int(3)),
        "{round:?}"
    );
    assert_eq!(units(&round, "late"), None, "{round:?}");
    assert!(
        round
            .declined
            .contains(&("late".to_string(), Decline::OverAgentBudget)),
        "the bid past the budget is declined by name: {round:?}"
    );
    assert_eq!(
        units(&round, "other"),
        Some(Decimal::from_int(2)),
        "{round:?}"
    );

    let negative = BTreeMap::from([(key("execution"), Decimal::from_int(-1))]);
    assert!(
        market()
            .allocate_within(&bids, &cpu(100), &negative, now())
            .is_err(),
        "a negative agent budget is refused, not read as zero"
    );
}
