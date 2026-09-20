//! Episodic memory in the cycle: REASON recalls what LEARN resolved, and
//! records it as precedent without touching the confidence.
//!
//! The failure the suite guards has two halves. A memory that fills from
//! nothing — `EpisodicMemory` with no production writer — would be the
//! blueprint's §10 in name only. And a precedent that quietly moved the
//! confidence would put an unreviewed statistic one governed approval away
//! from a position size, which is exactly what ADR 0005's evidence
//! arithmetic exists to prevent.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
// Exact float comparison is deliberate: the claim under test is that the
// confidence with a precedent is the same number as the confidence without
// one, bit for bit. A tolerance would let a small leak through.
#![allow(clippy::float_cmp)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

// --- fixtures, the same shape `learning.rs` feeds -----------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    fixture_liquidity(),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
}

fn fresh() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

/// A price series with a jump partway through, so the detectors have
/// something real to find.
fn bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// Twenty swinging bars up to `horizon`, which move every observable a
/// claim here can name far enough that the verdict is informative.
fn swings(symbol: &str, horizon: Timestamp) -> Vec<SensedRecord> {
    (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 150.0)
            } else {
                (150.0, 100.0)
            };
            let at = horizon.saturating_sub(Duration::from_mins((20 - i) * 60));
            bar(symbol, at, open, close)
        })
        .collect()
}

/// Drive a platform through the three cycles the suite is about: form a
/// claim, resolve it, reason again in the same name. `second_at` is when
/// the resolving cycle runs and `third_at` when the next REASON asks for
/// precedent; the tape is identical whatever the two instants are.
fn three_cycles(second_at: Timestamp, third_at: Timestamp) -> Result<(Platform, Timestamp)> {
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());
    assert!(
        !platform.predictions().is_empty(),
        "premise: the first cycle made a claim:\n{}",
        first.summarise()
    );
    let horizon = platform.predictions()[0].proposition.resolves_at;
    assert!(
        second_at > horizon,
        "the fixture's resolving cycle must run after the horizon"
    );
    platform.observe(swings("AAA", horizon));
    let second = platform.run_cycle(second_at);
    let learn = second.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("episode(s) remembered"),
        "LEARN did not remember the resolved thesis as an episode: {}",
        learn.detail
    );
    assert!(
        platform.predictions()[0].verdict.is_some(),
        "premise: the first claim was settled"
    );
    let third = platform.run_cycle(third_at);
    let reason = third.stage(Stage::Reason).expect("reason ran");
    assert!(
        reason.detail.contains("hypothesis"),
        "premise: the third cycle formed a hypothesis: {}",
        reason.detail
    );
    Ok((platform, horizon))
}

/// The two instants `three_cycles` is driven with: the resolving cycle a
/// minute past the first claim's horizon, and the next REASON a second later.
fn resolution_instants() -> Result<(Timestamp, Timestamp)> {
    let probe = {
        let mut platform = fresh()?;
        platform.observe(bars("AAA", 120));
        platform.run_cycle(start());
        platform.predictions()[0].proposition.resolves_at
    };
    let t2 = probe.saturating_add(Duration::from_mins(1));
    Ok((t2, t2.saturating_add(Duration::from_secs(1))))
}

#[test]
fn an_episode_the_cycle_wrote_carries_the_market_and_world_state_measured_when_it_reasoned()
-> Result<()> {
    // §10.1's `state_vector`. The failure this guards: a `MarketState` type
    // that exists, validates and encodes, and that nothing in the cycle ever
    // fills — the `MaxExpectedShortfall` shape, where a field reads as a
    // record of the situation and is `None` on every episode the platform
    // ever wrote. So this asserts against the episode the production cycle
    // put into memory, not against one the test built.
    let (t2, t3) = resolution_instants()?;
    let (platform, _) = three_cycles(t2, t3)?;

    let remembered: Vec<_> = platform.remembered_episodes(t3).collect();
    assert_eq!(
        remembered.len(),
        1,
        "premise: LEARN moved exactly one resolved episode into memory"
    );
    let state = remembered[0]
        .state
        .as_ref()
        .expect("the cycle wrote an episode with no state at all");

    // The platform observed 120 bars of AAA before the first cycle, so every
    // figure the state names was formable. A `None` here is the wiring gone,
    // not a thin tape.
    assert!(
        state.observations >= 120,
        "the state rests on {} observations though the fixture fed 120 bars",
        state.observations
    );
    let volatility = state
        .volatility_ratio
        .expect("120 bars is enough history to form a volatility ratio");
    assert!(
        volatility > 0.0 && volatility.is_finite(),
        "the volatility ratio is {volatility}, which no series produces"
    );
    assert!(
        state
            .recent_return_bps
            .is_some_and(|bps| bps.is_finite() && bps != 0.0),
        "the window's return is {:?}; the fixture's tape moves, so a zero or an absence here \
         is the measurement not happening",
        state.recent_return_bps
    );
    assert!(
        (0.0..=1.0).contains(&state.drawdown),
        "the drawdown is {}, outside the fraction it is defined as",
        state.drawdown
    );

    // And it is the state at formation, not at resolution. `three_cycles`
    // feeds twenty bars swinging between 100 and 150 before the resolving
    // cycle, so a state measured at LEARN would carry a volatility ratio
    // from that tape rather than from the quiet 120 that preceded the claim.
    let at_resolution = platform
        .remembered_episodes(t3)
        .next()
        .and_then(|episode| episode.state.as_ref())
        .and_then(|state| state.volatility_ratio);
    assert_eq!(
        at_resolution,
        Some(volatility),
        "the episode's state must be the one instant it was reasoned at"
    );
    assert!(
        remembered[0].at < remembered[0].known_at,
        "premise: the episode was true before it was knowable, so formation and resolution are \
         different instants and the assertion above is not vacuous"
    );
    Ok(())
}

#[test]
fn a_resolved_episode_records_how_far_the_outcome_was_from_what_the_claim_expected() -> Result<()> {
    // §10.1's `surprise`. The failure: LEARN writes the outcome and drops the
    // expectation, so the platform can say what happened and can never say
    // whether it was surprised — which is the one thing §10.2 calls the most
    // informative and the rarest. The expectation exists at exactly one
    // instant, beside the outcome in `calibrate_resolved`, and this asserts it
    // survived the journey onto the record.
    let (t2, t3) = resolution_instants()?;
    let (platform, _) = three_cycles(t2, t3)?;

    let remembered: Vec<_> = platform.remembered_episodes(t3).collect();
    assert_eq!(remembered.len(), 1, "premise: one episode was remembered");
    let outcome = remembered[0]
        .outcome
        .as_ref()
        .expect("premise: a remembered episode is a resolved one");
    let expected = outcome
        .expected_move_bps
        .expect("the claim stated a magnitude and the episode did not keep it");
    assert!(
        expected.is_finite() && expected != 0.0,
        "the expectation is {expected}; a claim written with no magnitude cannot be graded and \
         should not have been recorded as one"
    );

    let surprise = remembered[0]
        .surprise_bps()
        .expect("an episode holding both an outcome and an expectation must state a surprise");
    assert_eq!(
        surprise,
        outcome.realised_move_bps - expected,
        "the surprise is not the gap between the two numbers on the record"
    );

    // The expectation must be *this* thesis's, which is what the id match in
    // `remember_resolved` is for. The claim the learning engine graded is the
    // same one, so the two figures have to agree.
    let claim = platform.predictions()[0]
        .claim
        .as_ref()
        .expect("premise: the first cycle's claim was written down");
    assert_eq!(
        claim.expected_move_bps, expected,
        "the episode kept an expectation that is not the one the claim stated"
    );
    Ok(())
}

#[test]
fn the_kernel_records_precedents_on_a_hypothesis_once_prior_episodes_resolved_and_leaves_the_confidence_alone()
-> Result<()> {
    // Platform A: the claim resolves at `t2`, and one second later REASON
    // asks again in the same name. The episode LEARN stamped at `t2` is
    // known before `t2 + 1s`, so it must come back as precedent.
    let probe = {
        let mut platform = fresh()?;
        platform.observe(bars("AAA", 120));
        platform.run_cycle(start());
        platform.predictions()[0].proposition.resolves_at
    };
    let t2 = probe.saturating_add(Duration::from_mins(1));
    let t3 = t2.saturating_add(Duration::from_secs(1));
    let (with_memory, _) = three_cycles(t2, t3)?;

    let precedents = with_memory.precedents();
    assert_eq!(
        precedents.len(),
        3,
        "premise: every hypothesis carries a precedent record"
    );
    // The first two REASONs ran before anything resolved, so they saw an
    // empty memory and say so — "no precedent" is `None`, not zero.
    for earlier in &precedents[..2] {
        assert_eq!(
            earlier.memory_size, 0,
            "{}: memory was not empty",
            earlier.hypothesis_id
        );
        assert!(earlier.nearest.is_empty());
        assert_eq!(earlier.digest.agreement, None);
    }
    let third = &precedents[2];
    assert_eq!(third.cycle, 3);
    assert_eq!(
        third.memory_size, 1,
        "the resolved episode did not enter memory"
    );
    assert!(
        !third.nearest.is_empty(),
        "the third REASON recalled nothing though a resolved episode in the same name was \
         known before it ran"
    );
    let recalled = &third.nearest[0];
    assert_eq!(recalled.instrument, "obj-AAA");
    assert_eq!(
        recalled.episode_id, "ep-hyp-1-obj-AAA",
        "the precedent must be the first cycle's episode"
    );
    assert!(
        recalled.known_at < t3 && recalled.known_at == t2,
        "the precedent's known_at is the resolution instant; got {} against t2 {}",
        recalled.known_at.to_rfc3339(),
        t2.to_rfc3339()
    );
    assert!(
        recalled.realised_move_bps.is_some(),
        "a precedent without an outcome is not a precedent"
    );
    assert!(
        third.examined <= 256 && third.examined >= 1,
        "examined {} candidates",
        third.examined
    );
    assert_eq!(third.digest.nearest, third.nearest.len());
    assert!(
        third.digest.agreement.is_some(),
        "a resolved, signed outcome must yield an agreement share"
    );

    // Platform B: the same tape and the same three REASONs, except that the
    // resolving cycle and the next one share the clock reading `t3`. The
    // episode is then stamped known at `t3`, which is not before `t3`, so
    // recall is empty by the point-in-time rule — and everything else about
    // the third REASON is the same question asked at the same instant on
    // the same history. That makes it the control: whatever the precedent
    // digest says, the confidence review produced must be identical.
    let (without_memory, _) = three_cycles(t3, t3)?;
    let control = &without_memory.precedents()[2];
    assert_eq!(control.cycle, 3);
    assert_eq!(
        control.memory_size, 1,
        "premise: the control also resolved and remembered the episode"
    );
    assert!(
        control.nearest.is_empty(),
        "an episode stamped at the instant of the question was recalled"
    );
    assert_eq!(control.digest.agreement, None);
    assert_ne!(
        third.digest, control.digest,
        "premise: the two platforms saw different precedent"
    );
    assert_eq!(
        third.confidence, control.confidence,
        "precedent moved the confidence: {} with a precedent of {:?} against {} without",
        third.confidence, third.digest, control.confidence
    );
    // And the confidence the record carries is the one the claim was
    // written at — the number calibration grades — not a copy taken
    // somewhere else in the stage.
    let claim = with_memory.predictions()[2]
        .claim
        .as_ref()
        .expect("a claim records its confidence");
    assert_eq!(claim.hypothesis_id, third.hypothesis_id);
    assert_eq!(claim.confidence, third.confidence);
    Ok(())
}

#[test]
fn the_panel_is_briefed_on_the_recalled_precedent_through_the_typed_field_and_only_when_one_was_recalled()
-> Result<()> {
    // The failure this guards: a precedent recorded beside the hypothesis
    // but never shown to the panel is memory nobody consults, and a
    // precedent shown as prose in `context` is memory the reviewer's lesson
    // matcher would count. The brief every agent ran under is in the audit
    // trail, so this reads the brief the panel actually received rather
    // than what the stage says it sent.
    use qip_investment_agents::ids;

    let probe = {
        let mut platform = fresh()?;
        platform.observe(bars("AAA", 120));
        platform.run_cycle(start());
        platform.predictions()[0].proposition.resolves_at
    };
    let t2 = probe.saturating_add(Duration::from_mins(1));
    let t3 = t2.saturating_add(Duration::from_secs(1));
    let (with_memory, _) = three_cycles(t2, t3)?;

    let audit = with_memory.organisation().audit();
    let reviews = audit.for_agent(ids::ADVERSARIAL);
    assert_eq!(
        reviews.len(),
        3,
        "premise: the reviewer ran once per REASON"
    );
    for earlier in &reviews[..2] {
        assert!(
            earlier.brief.precedent.is_none(),
            "a REASON that ran before anything resolved was briefed on a precedent"
        );
    }
    let briefed = reviews[2]
        .brief
        .precedent
        .as_ref()
        .expect("the third REASON recalled a precedent and the panel was not briefed on it");
    let recorded = &with_memory.precedents()[2];
    assert_eq!(
        briefed.digest(),
        &recorded.digest,
        "the panel was briefed on a different digest from the one recorded beside the hypothesis"
    );
    assert_eq!(
        briefed.similarity(),
        recorded.nearest[0].similarity,
        "the brief's similarity is not the nearest episode's"
    );
    assert_eq!(briefed.prior_outcome(), recorded.nearest[0].agreed);
    assert_eq!(
        briefed.age(),
        t3.since(recorded.nearest[0].known_at),
        "the brief's age is not measured from the nearest episode's known_at"
    );
    assert!(
        briefed.age().as_nanos() > 0,
        "the precedent must be knowable strictly before the question"
    );
    // The channel is the typed field and nothing else: the free-text
    // context the lesson matcher reads carries no precedent block.
    assert!(
        !reviews[2].brief.context.contains("precedent"),
        "the precedent reached the free-text context: {}",
        reviews[2].brief.context
    );
    // And the reviewer cited it in narrative, which is what the channel is
    // for.
    let finding = reviews[2]
        .finding
        .as_ref()
        .expect("the reviewer produced a finding");
    assert!(
        finding.claim.contains("precedent:")
            || finding.caveats.iter().any(|c| c.contains("precedent:")),
        "the reviewer did not cite the precedent it was briefed on: {}",
        finding.claim
    );

    // The control: the resolving cycle and the next share the clock reading,
    // so nothing is knowable before the question and no brief carries one.
    let (without_memory, _) = three_cycles(t3, t3)?;
    let control = without_memory
        .organisation()
        .audit()
        .for_agent(ids::ADVERSARIAL);
    assert_eq!(
        control.len(),
        3,
        "premise: the control reviewer ran three times"
    );
    assert!(
        control.iter().all(|run| run.brief.precedent.is_none()),
        "a brief carried a precedent that was not knowable before the question"
    );
    Ok(())
}

#[test]
fn the_question_reason_asks_is_encoded_in_the_state_it_is_asked_in_and_not_only_in_the_claim()
-> Result<()> {
    // The half of §10.1 a record-side test cannot see. An episode may carry a
    // state block while the *query* leaves it at zero, and nothing about the
    // stored record would look wrong: recall would still return the episode,
    // the precedent would still be recorded, and every assertion about what
    // memory holds would pass. What would be silently true is that cosine
    // counted every state dimension against every candidate, so the episodes
    // richest in state ranked worst — retrieval made worse by the field added
    // to improve it.
    //
    // So this drives two platforms that differ only in the tape seen *after*
    // the episode was written, and asserts the recorded similarity moved. In
    // this fixture the regime label cannot move — `market_regime` falls
    // through to `Quiet` with no fundamentals and no spreads, and
    // `volatility_regime` to `Normal` — so a difference here is the state
    // block and not the one-hots, which the premises below check rather than
    // assume.
    let (t2, t3) = resolution_instants()?;
    let (quiet, _) = three_cycles(t2, t3)?;

    let violent = {
        let mut platform = fresh()?;
        platform.observe(bars("AAA", 120));
        platform.run_cycle(start());
        let horizon = platform.predictions()[0].proposition.resolves_at;
        platform.observe(swings("AAA", horizon));
        platform.run_cycle(t2);
        // The extra tape: a hundred more swings between the resolution and
        // the question, which moves every figure the state names and nothing
        // else the encoding reads.
        for step in 0..5 {
            platform.observe(swings("AAA", t2.saturating_add(Duration::from_mins(step))));
        }
        platform.run_cycle(t3);
        platform
    };

    let (left, right) = (&quiet.precedents()[2], &violent.precedents()[2]);
    assert!(
        !left.nearest.is_empty() && !right.nearest.is_empty(),
        "premise: both platforms recalled the episode, so the comparison below is between two \
         similarities and not between a similarity and nothing"
    );
    assert_eq!(
        left.nearest[0].episode_id, right.nearest[0].episode_id,
        "premise: both recalled the same episode"
    );
    assert_eq!(
        left.nearest[0].claim, right.nearest[0].claim,
        "premise: the recalled episode's claim block is identical, so a difference in similarity \
         is not the claim"
    );
    assert_ne!(
        left.nearest[0].similarity, right.nearest[0].similarity,
        "the same episode was equally similar to a question asked on a quiet tape and to one \
         asked after a hundred swings; the query is not encoding the state it is asked in"
    );
    Ok(())
}

/// A tape for `symbol` that leads `bars("AAA", count)` by one step: its
/// return at `i` is AAA's return at `i + 1`, so AAA is a lagged copy of it
/// and the temporal-precedence pass has a real relationship to find.
fn leading_bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let aaa_return = |i: usize| {
        let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
        let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
        noise + jump
    };
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let open = price;
            price *= 1.0
                + if i + 1 < count {
                    aaa_return(i + 1)
                } else {
                    0.0
                };
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

#[test]
fn an_episode_records_the_causal_edges_the_graph_held_into_its_instrument_when_it_was_reasoned()
-> Result<()> {
    // §10.1's `causal_context`, "which edges were active and their strength".
    // The failure: a field the type carries and the cycle never fills, so
    // every episode the platform ever wrote says the graph knew nothing —
    // which is indistinguishable from a graph that knew nothing, and is the
    // reason this test drives a tape the temporal-precedence pass actually
    // finds a relationship in rather than asserting on an empty graph.
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));
    platform.observe(leading_bars("BBB", 120));

    let first = platform.run_cycle(start());
    let held: Vec<_> = platform
        .world()
        .causal()
        .edges()
        .iter()
        .filter(|edge| edge.effect == "obj-AAA")
        .map(|edge| (edge.cause.clone(), edge.transmission()))
        .collect();
    assert_eq!(
        held.len(),
        1,
        "premise: the cycle's temporal-precedence pass claimed exactly one edge into obj-AAA, \
         so an empty context below is the wiring and not the graph:\n{}",
        first.summarise()
    );
    assert!(
        !platform.predictions().is_empty(),
        "premise: the first cycle made a claim to resolve"
    );

    // Resolve it, so the episode REASON drafted moves into memory where it
    // can be read.
    let horizon = platform.predictions()[0].proposition.resolves_at;
    let t2 = horizon.saturating_add(Duration::from_mins(1));
    let t3 = t2.saturating_add(Duration::from_secs(1));
    platform.observe(swings("AAA", horizon));
    platform.run_cycle(t2);

    let remembered: Vec<_> = platform.remembered_episodes(t3).collect();
    assert_eq!(remembered.len(), 1, "premise: one episode was remembered");
    let context = &remembered[0].causal_context;
    assert_eq!(
        context.len(),
        1,
        "the episode recorded {} edges against a graph holding {} into its instrument",
        context.len(),
        held.len()
    );
    assert_eq!(
        context[0].cause, held[0].0,
        "the episode named a cause the graph does not run from"
    );
    assert_eq!(
        context[0].transmission, held[0].1,
        "the episode recorded a strength that is not the graph's own transmission"
    );
    assert!(
        !context[0].mechanism.is_empty(),
        "an edge with no mechanism is a relationship nobody can argue with"
    );

    // §10.2's fifth trigger, "every veto and near-miss", and the reason this
    // assertion lives here rather than in a fixture of its own: this tape is
    // the one that gets a hypothesis *rejected* on review, so the episode
    // memory holds is a veto. The register said vetoes reached the
    // counterfactual queue and not memory; they reach both, because
    // `record_precedent` is called on every arm of the decision and the
    // prediction a rejected hypothesis still writes is what later resolves
    // it. What a veto episode must not do is arrive labelled as a decision
    // the platform took.
    assert_eq!(
        remembered[0].decision,
        qip_ai::memory::DecisionTaken::RejectedOnReview,
        "the review rejected this hypothesis and the episode records it as {:?}; an episode that \
         cannot say the platform declined is a memory of successes",
        remembered[0].decision
    );
    Ok(())
}
#[test]
fn a_precedent_says_what_the_platform_declined_on_the_episodes_it_recalled_and_whether_it_should_have()
-> Result<()> {
    // §10.3's last query — "what did we decline in situations like this, and
    // should we have?" — as the REASON stage answers it.
    //
    // The failure this guards is the one the sibling lane found on the
    // recall side and is worth stating again: every record-side test can
    // pass while the read path asks nothing. The twin has priced declined
    // paths since `b9e2242` and the memory has held episodes since the
    // §10.1 lane, and until the join below existed no code could put the
    // two in one sentence. A mutation that stops `record_precedent` calling
    // the join, or that hands it the wrong hypotheses, leaves every other
    // test in this file green and fails this one.
    let (t2, t3) = resolution_instants()?;
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));
    let first = platform.run_cycle(start());
    assert!(
        !platform.predictions().is_empty(),
        "premise: the first cycle made a claim:\n{}",
        first.summarise()
    );
    let horizon = platform.predictions()[0].proposition.resolves_at;
    let hypothesis = platform
        .precedents()
        .first()
        .expect("premise: the first cycle recorded a precedent, so it named a hypothesis")
        .hypothesis_id
        .clone();

    // A refused order that names that hypothesis. The quantity is far beyond
    // anything the fixture's limits admit, so a control refuses it before it
    // reaches a venue — which is the fact this query is about.
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000000"),
        dec!("100"),
        "prop-declined",
        vec![hypothesis.clone()],
        start(),
    );
    let order_id = order.order_id.clone();
    assert!(
        platform.submit_order(order, start()).is_err(),
        "premise: the order was refused; an accepted one is not a decline"
    );
    assert_eq!(
        platform.declined_awaiting_score(),
        1,
        "premise: the refusal is queued for the twin, so there will be a score to join"
    );

    // The cycle that resolves the claim also prices the refusal.
    platform.observe(swings("AAA", horizon));
    let second = platform.run_cycle(t2);
    let learn = second.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("episode(s) remembered"),
        "premise: LEARN remembered the resolved thesis as an episode: {}",
        learn.detail
    );
    let scores = platform.declined_scores();
    assert_eq!(
        scores.len(),
        1,
        "premise: the twin priced the refusal:\n{}",
        second.summarise()
    );
    assert_eq!(
        scores[0].order_id, order_id,
        "premise: the score is the one for the order that named the hypothesis"
    );
    let gate = scores[0].gate.clone();
    let regretted = usize::from(scores[0].regret);

    // And the next REASON recalls that episode and says what was declined on
    // it.
    let third = platform.run_cycle(t3);
    let episode_id = format!("ep-{hypothesis}");
    let precedent = platform
        .precedents()
        .iter()
        .find(|precedent| {
            precedent.cycle == 3
                && precedent
                    .nearest
                    .iter()
                    .any(|entry| entry.episode_id == episode_id)
        })
        .unwrap_or_else(|| {
            panic!(
                "premise: the third cycle recalled {episode_id} as a precedent:\n{}",
                third.summarise()
            )
        });

    assert_eq!(
        precedent.declines.analogues,
        precedent.nearest.len(),
        "the join was asked about a different set of analogues than the one recalled"
    );
    assert_eq!(
        precedent.declines.declines, 1,
        "the refusal scored on the recalled episode's own hypothesis was not joined to it: {:?}",
        precedent.declines
    );
    assert_eq!(precedent.declines.analogues_matched, 1);
    assert_eq!(
        precedent.declines.regretted, regretted,
        "the twin's regret bit and the precedent's disagree about the same refusal"
    );
    assert_eq!(
        precedent
            .declines
            .by_gate
            .get(&gate)
            .map(|charged| charged.declines),
        Some(1),
        "the refusal is not charged to {gate}, the control that made it: {:?}",
        precedent.declines.by_gate
    );
    assert!(
        precedent.declines.was_answerable(),
        "a join that found a refusal must read as answerable"
    );
    Ok(())
}
#[test]
fn the_learn_stage_reads_regime_experience_from_memory_and_journals_it_with_the_blind_spots()
-> Result<()> {
    // Blueprint §13.1's regime-experience and blind-spot rows, proven
    // through `run_cycle` rather than the store's own method: the failure
    // this guards is a report that exists and is read by nothing, which is
    // exactly what the self-model's other five dimensions were until this
    // lane. Two things have to hold. Before anything resolves, the stage
    // still says so — twenty blind spots and no experience, rather than
    // silence — and once the resolving cycle has moved an episode into
    // memory, the next cycle's LEARN reads it back, names the regime it was
    // reasoned in, and puts the report on the sealed journal entry.
    let (second_at, third_at) = resolution_instants()?;

    // Premise one: on a fresh platform the line is present and empty. The
    // first cycle's LEARN runs before any thesis has resolved.
    let mut fresh_platform = fresh()?;
    fresh_platform.observe(bars("AAA", 120));
    let first = fresh_platform.run_cycle(start());
    let learn = first.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn
            .detail
            .contains("regime experience: none, memory holds no knowable episode")
            && learn.detail.contains("20 of 20 regime(s) are blind spots"),
        "a platform that has resolved nothing must say it has no experience, not nothing: {}",
        learn.detail
    );
    let entries = fresh_platform.journal_entries()?;
    let sealed = entries.last().expect("one cycle was journalled");
    let report = sealed
        .experience
        .as_ref()
        .expect("the empty report is journalled, because 'no experience' is a finding");
    assert_eq!(report.episodes_examined, 0);
    assert_eq!(
        report.blind_spots.len(),
        20,
        "five market by four volatility regimes"
    );
    assert_eq!(report.regimes_traded(), 0);

    // Then the resolving cycle and the one after it. `three_cycles` asserts
    // its own premise: the second cycle's LEARN said "episode(s) remembered",
    // so by the third cycle memory holds at least one knowable episode.
    let (platform, _) = three_cycles(second_at, third_at)?;

    let entries = platform.journal_entries()?;
    let sealed = entries.last().expect("three cycles were journalled");
    let report = sealed
        .experience
        .as_ref()
        .expect("LEARN journals the experience report on every cycle");
    let knowable = report.episodes_examined;
    assert!(
        knowable >= 1,
        "the report is over what memory holds knowable at the stage instant, and the \
         resolving cycle put one episode there"
    );
    assert_eq!(
        report.regimes.len(),
        1,
        "one instrument, one regime label: {:?}",
        report.regimes
    );
    assert!(
        report.unknown.is_empty(),
        "an episode stamped by `regime_label` names a regime the enum product knows: {:?}",
        report.unknown
    );
    assert_eq!(
        report.blind_spots.len(),
        19,
        "every regime but the one reasoned in is a blind spot: {:?}",
        report.blind_spots
    );
    let (key, experience) = report.regimes.iter().next().expect("one regime");
    assert!(
        !report.blind_spots.contains(key),
        "a regime with an episode cannot also be a blind spot"
    );
    assert_eq!(experience.episodes, knowable);
    // The label reads as the enums spell it, which is how a reader would
    // grep the cost router for it.
    assert!(
        key.split_once('/').is_some_and(|(market, volatility)| {
            qip_cost_router::MarketRegime::ALL
                .iter()
                .any(|m| m.as_str() == market)
                && qip_cost_router::VolatilityRegime::ALL
                    .iter()
                    .any(|v| v.as_str() == volatility)
        }),
        "the regime key {key} is not a market/volatility pair the enums spell"
    );

    // And the stage detail carries the same finding, so an operator reading
    // the cycle summary and one reading the journal see one fact.
    let learned = platform
        .journal_entries()?
        .last()
        .map(|entry| entry.summary.clone())
        .expect("a summary");
    assert!(
        learned.contains("regime experience: traded through")
            && learned.contains("(reasoned in 1)")
            && learned.contains("19 blind spot(s)"),
        "the LEARN detail does not carry the experience line: {learned}"
    );
    Ok(())
}
