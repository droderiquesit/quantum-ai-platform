//! Tests for the REASON stage.
//!
//! The properties worth defending here are all about not being fooled: by
//! correlated evidence dressed as confirmation, by a confidence that drifted
//! from its evidence, by a mechanism that quietly reverses sign, and by a red
//! team that approves everything.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_agents::finding::{AgentFinding, Direction, NumericFact};
use qip_core::error::Result;
use qip_core::ids::{AgentRunId, ChallengeId, EvidenceId, HypothesisId, ObjectId};
use qip_core::testing::is_exactly_zero;
use qip_core::time::{Duration, Timestamp};
use qip_reasoning_engine::bayes::{BaseRate, EvidenceStrength, from_log_odds, to_log_odds, update};
use qip_reasoning_engine::engine::{ReasoningEngine, SynthesisInput};
use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet, Stance};
use qip_reasoning_engine::hypothesis::{
    CausalChain, CausalStep, Claim, Hypothesis, HypothesisDraft, HypothesisStatus,
};
use qip_reasoning_engine::redteam::{ChallengeKind, RedTeam, ReviewPolicy};
use qip_world_model::causal::Mechanism;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn evidence(
    id: &str,
    kind: EvidenceKind,
    stance: Stance,
    origin: &str,
    reliability: f64,
    diagnosticity: f64,
) -> Evidence {
    Evidence::new(
        EvidenceId::from_string(id),
        kind,
        stance,
        format!("statement for {id}"),
        format!("rec-{id}"),
        origin,
        now(),
        now(),
    )
    .with_reliability(reliability)
    .with_diagnosticity(diagnosticity)
}

/// A two-step chain that preserves sign, both steps well evidenced.
fn sound_chain() -> CausalChain {
    CausalChain::new(vec![
        CausalStep::new(
            "policy-rate",
            "funding-cost",
            Mechanism::CreditConditions,
            "commercial paper rates rise",
            Duration::from_days(2),
            0.85,
        ),
        CausalStep::new(
            "funding-cost",
            "obj-ACME",
            Mechanism::CreditConditions,
            "gross margin compresses in the next quarterly report",
            Duration::from_days(20),
            0.75,
        ),
    ])
}

fn draft(evidence: EvidenceSet, chain: CausalChain) -> HypothesisDraft {
    HypothesisDraft {
        hypothesis_id: HypothesisId::from_string("hyp-1"),
        opportunity_id: None,
        formed_at: now(),
        as_of: now(),
        class: "funding-cost-pass-through".to_string(),
        claim: Claim::Overvalued,
        statement: "ACME's margin guidance does not reflect its floating-rate funding".to_string(),
        subjects: vec![object("ACME")],
        chain,
        evidence,
        prior: 0.25,
        falsifiers: vec![
            "the next quarterly report shows flat gross margin".to_string(),
            "the company discloses a funding hedge covering the exposure".to_string(),
        ],
        leading_alternative:
            "the market already knows the funding structure and has priced the margin path"
                .to_string(),
        horizon: Duration::from_days(60),
        contributors: Vec::new(),
        models: Vec::new(),
    }
}

fn well_supported() -> EvidenceSet {
    EvidenceSet::from_items(vec![
        evidence(
            "e1",
            EvidenceKind::Filing,
            Stance::Supports,
            "sec-edgar",
            0.95,
            0.8,
        ),
        evidence(
            "e2",
            EvidenceKind::MarketObservation,
            Stance::Supports,
            "exchange",
            0.9,
            0.6,
        ),
        evidence(
            "e3",
            EvidenceKind::Computation,
            Stance::Supports,
            "credit-model",
            0.85,
            0.5,
        ),
    ])
}

// --- log-odds arithmetic ----------------------------------------------------

#[test]
fn log_odds_round_trips() {
    for p in [0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99] {
        let back = from_log_odds(to_log_odds(p));
        assert!((back - p).abs() < 1e-9, "{p} became {back}");
    }
}

#[test]
fn no_finite_evidence_drives_a_belief_to_certainty() {
    // A belief pinned at 1.0 could never be revised by later evidence, which
    // is exactly the state a learning system must not be able to reach.
    let overwhelming: Vec<EvidenceStrength> = (0..50)
        .map(|i| EvidenceStrength {
            id: format!("e{i}"),
            signed_weight: 0.99,
            origin: format!("origin-{i}"),
        })
        .collect();
    let result = update(0.5, &overwhelming);
    assert!(result.posterior < 1.0);
    assert!(result.posterior > 0.999);
}

#[test]
fn correlated_evidence_moves_a_belief_less_than_independent_evidence() {
    // Five reports from one newsroom are not five confirmations.
    let independent: Vec<EvidenceStrength> = (0..5)
        .map(|i| EvidenceStrength {
            id: format!("e{i}"),
            signed_weight: 0.6,
            origin: format!("origin-{i}"),
        })
        .collect();
    let correlated: Vec<EvidenceStrength> = (0..5)
        .map(|i| EvidenceStrength {
            id: format!("e{i}"),
            signed_weight: 0.6,
            origin: "one-newsroom".to_string(),
        })
        .collect();

    let a = update(0.25, &independent);
    let b = update(0.25, &correlated);
    assert!(
        a.posterior > b.posterior + 0.05,
        "independent {:.3} vs correlated {:.3}",
        a.posterior,
        b.posterior
    );
    assert_eq!(a.distinct_origins, 5);
    assert_eq!(b.distinct_origins, 1);
    assert!((b.independence_discount - (1.0f64 / 5.0).sqrt()).abs() < 1e-12);
}

#[test]
fn contradicting_evidence_lowers_a_belief() {
    let mixed = vec![
        EvidenceStrength {
            id: "for".into(),
            signed_weight: 0.7,
            origin: "a".into(),
        },
        EvidenceStrength {
            id: "against".into(),
            signed_weight: -0.7,
            origin: "b".into(),
        },
    ];
    let result = update(0.4, &mixed);
    // Symmetric evidence must leave the prior where it was.
    assert!(
        (result.posterior - 0.4).abs() < 1e-9,
        "{:?}",
        result.posterior
    );
}

#[test]
fn one_item_cannot_carry_a_belief_from_doubt_to_certainty() {
    let single = vec![EvidenceStrength {
        id: "e".into(),
        signed_weight: 0.999,
        origin: "a".into(),
    }];
    let result = update(0.10, &single);
    assert!(
        result.posterior < 0.5,
        "one item took the belief to {:.3}",
        result.posterior
    );
    assert!((result.dominance() - 1.0).abs() < 1e-12);
}

#[test]
fn a_base_rate_from_a_thin_sample_is_shrunk_toward_a_coin_flip() {
    let thin = BaseRate::new("rare-event", 0.9, 5, "internal study");
    let solid = BaseRate::new("common-event", 0.9, 500, "twenty years of history");
    assert!(thin.shrunk() < 0.65, "{}", thin.shrunk());
    assert!(solid.shrunk() > 0.88);
    assert!(!thin.is_well_established());
    assert!(solid.is_well_established());
}

// --- evidence ---------------------------------------------------------------

#[test]
fn a_rumour_cannot_be_promoted_to_the_reliability_of_a_filing() {
    let rumour = evidence(
        "r",
        EvidenceKind::Rumour,
        Stance::Supports,
        "chat",
        0.99,
        1.0,
    );
    assert!(rumour.reliability <= EvidenceKind::Rumour.reliability_ceiling());
    assert!(rumour.validate().is_ok());

    // Constructed by hand rather than through the clamping setter, which is
    // how a bad value would actually arrive: through deserialisation.
    let mut forged = rumour.clone();
    forged.reliability = 0.99;
    assert!(forged.validate().unwrap_err().message().contains("ceiling"));
}

#[test]
fn evidence_cannot_be_known_before_it_was_true() {
    let mut item = evidence("e", EvidenceKind::Filing, Stance::Supports, "sec", 0.9, 0.5);
    item.known_at = item.valid_at.saturating_sub(Duration::from_days(1));
    assert!(
        item.validate()
            .unwrap_err()
            .message()
            .contains("before it was true")
    );
}

#[test]
fn evidence_is_read_point_in_time() {
    let mut set = well_supported();
    let mut future = evidence(
        "late",
        EvidenceKind::Filing,
        Stance::Supports,
        "sec",
        0.9,
        0.9,
    );
    future.valid_at = now().saturating_add(Duration::from_days(3));
    future.known_at = now().saturating_add(Duration::from_days(3));
    set.push(future);

    assert_eq!(set.len(), 4);
    assert_eq!(
        set.as_of(now()).len(),
        3,
        "tomorrow's filing cannot inform today"
    );
}

#[test]
fn independent_weight_collapses_reports_from_one_origin() {
    let three_from_one = EvidenceSet::from_items(vec![
        evidence("a", EvidenceKind::News, Stance::Supports, "wire", 0.7, 0.8),
        evidence("b", EvidenceKind::News, Stance::Supports, "wire", 0.7, 0.8),
        evidence("c", EvidenceKind::News, Stance::Supports, "wire", 0.7, 0.8),
    ]);
    let three_origins = EvidenceSet::from_items(vec![
        evidence(
            "a",
            EvidenceKind::News,
            Stance::Supports,
            "wire-1",
            0.7,
            0.8,
        ),
        evidence(
            "b",
            EvidenceKind::News,
            Stance::Supports,
            "wire-2",
            0.7,
            0.8,
        ),
        evidence(
            "c",
            EvidenceKind::News,
            Stance::Supports,
            "wire-3",
            0.7,
            0.8,
        ),
    ]);
    let collapsed = three_from_one.independent_weight(Stance::Supports);
    let genuine = three_origins.independent_weight(Stance::Supports);
    assert!(collapsed < 0.5 * genuine, "{collapsed} vs {genuine}");
    assert!((three_from_one.concentration() - 1.0).abs() < 1e-12);
    assert!((three_origins.concentration() - 1.0 / 3.0).abs() < 1e-9);
}

#[test]
fn a_thesis_on_news_alone_has_no_primary_source() {
    let secondary = EvidenceSet::from_items(vec![
        evidence("a", EvidenceKind::News, Stance::Supports, "wire", 0.7, 0.8),
        evidence(
            "b",
            EvidenceKind::ThirdPartyResearch,
            Stance::Supports,
            "broker",
            0.6,
            0.7,
        ),
    ]);
    assert!(!secondary.has_primary_source());
    assert!(well_supported().has_primary_source());
}

// --- causal chains ----------------------------------------------------------

#[test]
fn a_broken_chain_is_rejected() {
    let disconnected = CausalChain::new(vec![
        CausalStep::new(
            "a",
            "b",
            Mechanism::CreditConditions,
            "b moves",
            Duration::from_days(1),
            0.9,
        ),
        CausalStep::new(
            "c",
            "d",
            Mechanism::CreditConditions,
            "d moves",
            Duration::from_days(1),
            0.9,
        ),
    ]);
    assert!(
        disconnected
            .validate()
            .unwrap_err()
            .message()
            .contains("broken")
    );
}

#[test]
fn a_step_with_nothing_observable_cannot_be_checked_and_is_rejected() {
    let vague = CausalChain::new(vec![CausalStep::new(
        "a",
        "b",
        Mechanism::CreditConditions,
        "  ",
        Duration::from_days(1),
        0.9,
    )]);
    assert!(
        vague
            .validate()
            .unwrap_err()
            .message()
            .contains("observable")
    );
}

#[test]
fn chain_confidence_is_multiplicative_so_long_stories_are_weak() {
    let long = CausalChain::new(
        (0..5)
            .map(|i| {
                CausalStep::new(
                    format!("n{i}"),
                    format!("n{}", i + 1),
                    Mechanism::CreditConditions,
                    "something moves",
                    Duration::from_days(1),
                    0.8,
                )
            })
            .collect(),
    );
    // Five individually plausible steps at 0.8 hold together only 33% of the time.
    assert!((long.confidence() - 0.8_f64.powi(5)).abs() < 1e-12);
    assert!(long.confidence() < 0.35);
}

#[test]
fn a_substitution_step_flips_the_sign_and_two_flip_it_back() {
    let single = CausalChain::new(vec![CausalStep::new(
        "a",
        "b",
        Mechanism::CompetitiveSubstitution,
        "share shifts",
        Duration::from_days(5),
        0.8,
    )]);
    assert!(!single.preserves_sign());

    let double = CausalChain::new(vec![
        CausalStep::new(
            "a",
            "b",
            Mechanism::CompetitiveSubstitution,
            "share shifts",
            Duration::from_days(5),
            0.8,
        ),
        CausalStep::new(
            "b",
            "c",
            Mechanism::CompetitiveSubstitution,
            "share shifts back",
            Duration::from_days(5),
            0.8,
        ),
    ]);
    assert!(double.preserves_sign());
}

// --- hypothesis formation ---------------------------------------------------

#[test]
fn confidence_is_computed_from_evidence_not_asserted() {
    let hypothesis = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    assert!(hypothesis.confidence > hypothesis.prior);
    // The stated confidence must equal the arithmetic, exactly.
    assert!(hypothesis.validate().is_ok());

    let mut tampered = hypothesis.clone();
    tampered.confidence = 0.99;
    assert!(
        tampered
            .validate()
            .unwrap_err()
            .message()
            .contains("implies"),
        "editing a confidence in isolation must be rejected"
    );
}

#[test]
fn adding_evidence_is_the_only_way_confidence_moves() {
    let mut hypothesis = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    let before = hypothesis.confidence;
    hypothesis
        .add_evidence(evidence(
            "e4",
            EvidenceKind::Filing,
            Stance::Supports,
            "companies-house",
            0.95,
            0.8,
        ))
        .unwrap();
    assert!(hypothesis.confidence > before);
    assert!(hypothesis.validate().is_ok());
}

#[test]
fn evidence_from_after_the_as_of_time_is_refused() {
    let mut hypothesis = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    let mut future = evidence(
        "late",
        EvidenceKind::Filing,
        Stance::Supports,
        "sec",
        0.95,
        0.9,
    );
    future.valid_at = now().saturating_add(Duration::from_days(5));
    future.known_at = now().saturating_add(Duration::from_days(5));
    let error = hypothesis.add_evidence(future).unwrap_err();
    assert!(error.message().contains("after the hypothesis as-of"));
}

#[test]
fn a_hypothesis_without_a_falsifier_is_refused() {
    let mut d = draft(well_supported(), sound_chain());
    d.falsifiers.clear();
    assert!(
        Hypothesis::form(d)
            .unwrap_err()
            .message()
            .contains("falsifier")
    );
}

#[test]
fn a_hypothesis_with_no_considered_alternative_is_refused() {
    let mut d = draft(well_supported(), sound_chain());
    d.leading_alternative = String::new();
    assert!(
        Hypothesis::form(d)
            .unwrap_err()
            .message()
            .contains("alternative")
    );
}

#[test]
fn a_hypothesis_with_no_mechanism_is_refused() {
    let mut d = draft(well_supported(), CausalChain::default());
    d.leading_alternative = "the market has priced it".to_string();
    let error = Hypothesis::form(d).unwrap_err();
    assert!(error.message().contains("correlation with an opinion"));
}

#[test]
fn a_hypothesis_with_only_contradicting_evidence_is_refused() {
    let against = EvidenceSet::from_items(vec![evidence(
        "e1",
        EvidenceKind::Filing,
        Stance::Contradicts,
        "sec-edgar",
        0.95,
        0.8,
    )]);
    assert!(
        Hypothesis::form(draft(against, sound_chain()))
            .unwrap_err()
            .message()
            .contains("no supporting evidence")
    );
}

#[test]
fn a_weak_mechanism_caps_confidence_however_good_the_evidence() {
    let weak = CausalChain::new(vec![CausalStep::new(
        "a",
        "obj-ACME",
        Mechanism::Sentiment,
        "sentiment indices move together",
        Duration::from_days(1),
        0.35,
    )]);
    let strong_evidence = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    let weak_mechanism = Hypothesis::form(draft(well_supported(), weak)).unwrap();
    assert!(
        weak_mechanism.confidence < strong_evidence.confidence,
        "a weak causal story must cap confidence: {} vs {}",
        weak_mechanism.confidence,
        strong_evidence.confidence
    );
}

#[test]
fn a_weak_mechanism_never_takes_confidence_below_the_base_rate() {
    // Regression: multiplying the posterior by the chain confidence made a
    // long causal story assert the claim was *less* likely than its own base
    // rate. A weak mechanism means the evidence tells you less, which pulls
    // confidence back toward the prior — never underneath it.
    let very_long = CausalChain::new(
        (0..8)
            .map(|i| {
                CausalStep::new(
                    if i == 0 {
                        "a".to_string()
                    } else {
                        format!("n{i}")
                    },
                    if i == 7 {
                        "obj-ACME".to_string()
                    } else {
                        format!("n{}", i + 1)
                    },
                    Mechanism::Sentiment,
                    "something moves",
                    Duration::from_hours(6),
                    0.6,
                )
            })
            .collect(),
    );
    assert!(
        very_long.confidence() < 0.02,
        "the fixture must be a weak chain"
    );

    let hypothesis = Hypothesis::form(draft(well_supported(), very_long)).unwrap();
    assert!(
        hypothesis.confidence >= hypothesis.prior,
        "confidence {:.4} fell below the prior of {:.4}",
        hypothesis.confidence,
        hypothesis.prior
    );
    // And it must still be lower than the same evidence on a sound mechanism.
    let sound = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    assert!(hypothesis.confidence < sound.confidence);
}

#[test]
fn confidence_is_monotone_in_the_strength_of_the_mechanism() {
    let mut previous = 0.0;
    for step_confidence in [0.4, 0.6, 0.8, 0.95] {
        let chain = CausalChain::new(vec![CausalStep::new(
            "policy-rate",
            "obj-ACME",
            Mechanism::CreditConditions,
            "funding cost rises",
            Duration::from_days(3),
            step_confidence,
        )]);
        let hypothesis = Hypothesis::form(draft(well_supported(), chain)).unwrap();
        assert!(
            hypothesis.confidence > previous,
            "a stronger mechanism must not lower confidence: {step_confidence} gave {:.4} after {previous:.4}",
            hypothesis.confidence
        );
        previous = hypothesis.confidence;
    }
}

#[test]
fn concentrated_support_reduces_effective_confidence() {
    let one_origin = EvidenceSet::from_items(vec![
        evidence(
            "a",
            EvidenceKind::Filing,
            Stance::Supports,
            "sec-edgar",
            0.95,
            0.8,
        ),
        evidence(
            "b",
            EvidenceKind::Filing,
            Stance::Supports,
            "sec-edgar",
            0.95,
            0.8,
        ),
    ]);
    let concentrated = Hypothesis::form(draft(one_origin, sound_chain())).unwrap();
    assert!(concentrated.effective_confidence() < concentrated.confidence);
    let diverse = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    assert!(
        (diverse.effective_confidence() - diverse.confidence).abs() < 1e-12,
        "evidence spread across origins should not be penalised"
    );
}

// --- the red team -----------------------------------------------------------

fn ids() -> impl FnMut() -> ChallengeId {
    let mut n = 0;
    move || {
        n += 1;
        ChallengeId::from_string(format!("ch-{n}"))
    }
}

#[test]
fn a_sound_hypothesis_survives_review() {
    let hypothesis = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    let mut team = RedTeam::new(ReviewPolicy::default());
    let outcome = team.review(&hypothesis, Some(0.2), now(), &mut ids());
    assert!(
        outcome.approved(),
        "rejected a sound thesis: {} / {:?}",
        outcome.rationale,
        outcome.kinds()
    );
    assert!(outcome.fatal().is_empty());
}

#[test]
fn a_thesis_that_is_already_priced_is_rejected_outright() {
    let hypothesis = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    let mut team = RedTeam::new(ReviewPolicy::default());
    let outcome = team.review(&hypothesis, Some(0.95), now(), &mut ids());
    assert!(!outcome.approved());
    assert!(outcome.kinds().contains(&ChallengeKind::AlreadyPriced));
    assert!(is_exactly_zero(outcome.confidence_after));
}

#[test]
fn a_thesis_whose_mechanism_is_slower_than_its_horizon_is_rejected() {
    let mut d = draft(well_supported(), sound_chain());
    // The chain's own lags total 22 days; a five-day horizon cannot contain it.
    d.horizon = Duration::from_days(5);
    let hypothesis = Hypothesis::form(d).unwrap();
    let mut team = RedTeam::new(ReviewPolicy::default());
    let outcome = team.review(&hypothesis, Some(0.1), now(), &mut ids());
    assert!(!outcome.approved());
    assert!(outcome.kinds().contains(&ChallengeKind::HorizonMismatch));
}

#[test]
fn a_thesis_resting_entirely_on_rumour_is_rejected() {
    let gossip = EvidenceSet::from_items(vec![
        evidence(
            "r1",
            EvidenceKind::Rumour,
            Stance::Supports,
            "chat-a",
            0.25,
            0.9,
        ),
        evidence(
            "r2",
            EvidenceKind::Rumour,
            Stance::Supports,
            "chat-b",
            0.25,
            0.9,
        ),
    ]);
    let hypothesis = Hypothesis::form(draft(gossip, sound_chain())).unwrap();
    let mut team = RedTeam::new(ReviewPolicy::default());
    let outcome = team.review(&hypothesis, Some(0.1), now(), &mut ids());
    assert!(!outcome.approved());
    assert!(
        outcome
            .fatal()
            .iter()
            .any(|c| c.kind == ChallengeKind::NoPrimarySource)
    );
}

#[test]
fn a_thesis_whose_support_comes_from_one_origin_is_challenged() {
    let one_origin = EvidenceSet::from_items(vec![
        evidence(
            "a",
            EvidenceKind::Filing,
            Stance::Supports,
            "sec-edgar",
            0.95,
            0.8,
        ),
        evidence(
            "b",
            EvidenceKind::News,
            Stance::Supports,
            "sec-edgar",
            0.7,
            0.7,
        ),
    ]);
    let hypothesis = Hypothesis::form(draft(one_origin, sound_chain())).unwrap();
    let mut team = RedTeam::new(ReviewPolicy::default());
    let outcome = team.review(&hypothesis, Some(0.1), now(), &mut ids());
    assert!(outcome.kinds().contains(&ChallengeKind::CorrelatedEvidence));
}

#[test]
fn every_challenge_says_what_would_resolve_it() {
    // A challenge without a resolution is a complaint, and complaints do not
    // make a thesis better.
    let thin = EvidenceSet::from_items(vec![evidence(
        "a",
        EvidenceKind::News,
        Stance::Supports,
        "wire",
        0.7,
        0.8,
    )]);
    let hypothesis = Hypothesis::form(draft(thin, sound_chain())).unwrap();
    let mut team = RedTeam::new(ReviewPolicy::default());
    let outcome = team.review(&hypothesis, None, now(), &mut ids());
    assert!(!outcome.challenges.is_empty());
    for challenge in &outcome.challenges {
        assert!(
            !challenge.resolution.trim().is_empty(),
            "{} has no resolution",
            challenge.kind
        );
        assert!(!challenge.finding.trim().is_empty());
    }
}

#[test]
fn the_red_team_rejects_often_enough_to_be_doing_something() {
    let mut team = RedTeam::new(ReviewPolicy::default());
    let mut generator = ids();

    // A deliberately mixed batch: sound, priced-in, rumour-only, slow.
    let sound = Hypothesis::form(draft(well_supported(), sound_chain())).unwrap();
    team.review(&sound, Some(0.15), now(), &mut generator);
    team.review(&sound, Some(0.95), now(), &mut generator);

    let gossip = EvidenceSet::from_items(vec![evidence(
        "r",
        EvidenceKind::Rumour,
        Stance::Supports,
        "chat",
        0.25,
        0.9,
    )]);
    let weak = Hypothesis::form(draft(gossip, sound_chain())).unwrap();
    team.review(&weak, Some(0.1), now(), &mut generator);

    let mut slow = draft(well_supported(), sound_chain());
    slow.horizon = Duration::from_days(3);
    let slow = Hypothesis::form(slow).unwrap();
    team.review(&slow, Some(0.1), now(), &mut generator);

    assert_eq!(team.reviewed(), 4);
    let rate = team.rejection_rate().unwrap();
    assert!((rate - 0.75).abs() < 1e-12, "rejection rate was {rate}");
}

#[test]
fn the_action_bar_refuses_a_thesis_with_no_primary_source() {
    let secondary = EvidenceSet::from_items(vec![
        evidence(
            "a",
            EvidenceKind::News,
            Stance::Supports,
            "wire-1",
            0.7,
            0.8,
        ),
        evidence(
            "b",
            EvidenceKind::News,
            Stance::Supports,
            "wire-2",
            0.7,
            0.8,
        ),
        evidence(
            "c",
            EvidenceKind::ThirdPartyResearch,
            Stance::Supports,
            "broker",
            0.6,
            0.8,
        ),
    ]);
    let mut hypothesis = Hypothesis::form(draft(secondary, sound_chain())).unwrap();
    hypothesis.status = HypothesisStatus::Approved;
    let refusal = hypothesis.meets_action_bar(0.1).unwrap_err();
    assert!(refusal.contains("primary source"));
}

// --- the engine -------------------------------------------------------------

fn finding(agent: &str, direction: Direction, conviction: f64, records: usize) -> AgentFinding {
    AgentFinding::new(
        AgentRunId::from_string(format!("run-{agent}")),
        agent,
        now(),
        now(),
        format!("{agent} view on ACME funding costs"),
    )
    .with_direction(direction, conviction)
    .with_evidence((0..records).map(|i| format!("rec-{agent}-{i}")).collect())
    .with_falsifiers(vec!["the margin holds".to_string()])
}

fn synthesis(findings: Vec<AgentFinding>, priced_in: Option<f64>) -> SynthesisInput {
    SynthesisInput {
        hypothesis_id: HypothesisId::from_string("hyp-e1"),
        opportunity_id: None,
        as_of: now(),
        now: now(),
        class: "funding-cost-pass-through".to_string(),
        claim: Claim::Overvalued,
        statement: "ACME's guidance does not reflect its floating-rate funding".to_string(),
        subjects: vec![object("ACME")],
        chain: sound_chain(),
        findings,
        direct_evidence: well_supported(),
        prior: 0.25,
        falsifiers: vec!["the next quarterly report shows flat gross margin".to_string()],
        leading_alternative:
            "the market already knows the funding structure and has priced the margin path"
                .to_string(),
        horizon: Duration::from_days(60),
        market_priced_in: priced_in,
        models: Vec::new(),
    }
}

#[test]
fn the_engine_forms_and_reviews_a_hypothesis_from_agent_findings() -> Result<()> {
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let outcome = engine.reason(synthesis(
        vec![
            // Overvalued is a negative claim, so a Negative finding supports it.
            finding("credit", Direction::Negative, 0.8, 3),
            finding("equity", Direction::Negative, 0.7, 2),
        ],
        Some(0.2),
    ))?;
    assert_eq!(outcome.hypothesis.status, HypothesisStatus::Approved);
    assert!(outcome.dissenters.is_empty());
    assert!(outcome.narrate().contains("Mechanism:"));
    Ok(())
}

#[test]
fn disagreement_is_carried_through_rather_than_averaged_away() -> Result<()> {
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let agreeing = engine.reason(synthesis(
        vec![finding("credit", Direction::Negative, 0.8, 3)],
        Some(0.2),
    ))?;
    let contested = engine.reason(synthesis(
        vec![
            finding("credit", Direction::Negative, 0.8, 3),
            finding("equity", Direction::Positive, 0.8, 3),
        ],
        Some(0.2),
    ))?;

    assert_eq!(contested.dissenters, vec!["equity".to_string()]);
    assert!(
        contested.hypothesis.confidence < agreeing.hypothesis.confidence,
        "a contested thesis must be less confident: {:.3} vs {:.3}",
        contested.hypothesis.confidence,
        agreeing.hypothesis.confidence
    );
    Ok(())
}

#[test]
fn an_agent_that_defers_is_recorded_and_not_counted_as_agreement() -> Result<()> {
    let deferred = AgentFinding::deferred(
        AgentRunId::from_string("run-macro"),
        "macro",
        now(),
        now(),
        "single-name credit is outside the macro remit",
    );
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let with_deferral = engine.reason(synthesis(
        vec![finding("credit", Direction::Negative, 0.8, 3), deferred],
        Some(0.2),
    ))?;
    let without = engine.reason(synthesis(
        vec![finding("credit", Direction::Negative, 0.8, 3)],
        Some(0.2),
    ))?;

    assert_eq!(with_deferral.deferred_by, vec!["macro".to_string()]);
    assert!(
        (with_deferral.hypothesis.confidence - without.hypothesis.confidence).abs() < 1e-12,
        "a deferral must not change the conclusion"
    );
    Ok(())
}

#[test]
fn a_better_grounded_finding_counts_for_more() -> Result<()> {
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let thin = engine.reason(synthesis(
        vec![finding("credit", Direction::Negative, 0.8, 0)],
        Some(0.2),
    ))?;
    let grounded = engine.reason(synthesis(
        vec![finding("credit", Direction::Negative, 0.8, 6)],
        Some(0.2),
    ))?;
    assert!(
        grounded.hypothesis.confidence > thin.hypothesis.confidence,
        "grounding must matter: {:.4} vs {:.4}",
        grounded.hypothesis.confidence,
        thin.hypothesis.confidence
    );
    Ok(())
}

#[test]
fn the_engine_refuses_a_finding_from_the_future() {
    let mut ahead = finding("credit", Direction::Negative, 0.8, 3);
    ahead.as_of = now().saturating_add(Duration::from_days(1));
    ahead.produced_at = ahead.as_of;
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let error = engine
        .reason(synthesis(vec![ahead], Some(0.2)))
        .unwrap_err();
    assert!(error.message().contains("after this hypothesis's"));
}

#[test]
fn the_engine_refuses_to_reason_about_a_time_it_has_not_reached() {
    let mut input = synthesis(
        vec![finding("credit", Direction::Negative, 0.8, 3)],
        Some(0.2),
    );
    input.as_of = now().saturating_add(Duration::from_days(1));
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    assert!(
        engine
            .reason(input)
            .unwrap_err()
            .message()
            .contains("in its future")
    );
}

#[test]
fn agent_facts_are_preserved_with_their_provenance() -> Result<()> {
    // The finding's own numbers must not be lost on the way into a thesis:
    // they are what makes the conclusion checkable.
    let with_fact =
        finding("credit", Direction::Negative, 0.8, 2).with_fact(NumericFact::computed(
            "margin_compression_bps",
            85.0,
            "bps",
            "funding-model",
            vec!["floating_debt".into(), "rate_delta".into()],
        ));
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());
    let outcome = engine.reason(synthesis(vec![with_fact], Some(0.2)))?;
    assert!(
        outcome
            .hypothesis
            .contributors
            .contains(&"run-credit".to_string())
    );
    Ok(())
}
