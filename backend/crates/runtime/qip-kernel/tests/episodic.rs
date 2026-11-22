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
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

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

/// A day bar opening at `at`.
fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    bar_over(symbol, at, Interval::Day, open, close)
}

/// An hour bar opening at `at`, so it closes — and becomes knowable — an hour
/// later.
///
/// R12-TICK-2077: the fixtures that feed bars an hour apart used to stamp
/// them `Interval::Day`, which describes twenty overlapping day buckets, the
/// last closing twenty-three hours after the horizon the claim is graded at.
/// While the as-of rule read a bar from its open time that mislabel was
/// invisible; a rule on the close time rightly leaves such a bar out. The
/// fixture is corrected to say what it always meant — hourly bars — rather
/// than the rule being loosened to admit a bar that had not closed.
fn hourly_bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    bar_over(symbol, at, Interval::Hour, open, close)
}

fn bar_over(
    symbol: &str,
    at: Timestamp,
    interval: Interval,
    open: f64,
    close: f64,
) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval,
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
            hourly_bar(symbol, at, open, close)
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

/// R12-WORLD-3032: the research organisation's memory — what the reviewer
/// reads to object to a poor analyst or an applicable lesson — had no writer
/// in any binary. The LEARN pass that remembers a resolved thesis now charges
/// each analyst that leaned one way or the other to it, through the same
/// slot the desk's gate reads.
#[test]
fn the_learn_pass_that_remembers_a_resolved_thesis_charges_its_analysts_to_research_memory()
-> Result<()> {
    let (t2, t3) = resolution_instants()?;
    let (platform, _) = three_cycles(t2, t3)?;
    let leaning: usize = platform
        .remembered_episodes(t3)
        .map(|episode| {
            episode
                .stances
                .iter()
                .filter(|stance| {
                    matches!(
                        stance.direction,
                        qip_ai::memory::StanceDirection::Positive
                            | qip_ai::memory::StanceDirection::Negative
                    )
                })
                .count()
        })
        .sum();
    // The premise: the resolved thesis had analysts who took a side, so an
    // empty research memory below would be a missing writer, not a quiet panel.
    assert!(
        leaning > 0,
        "premise: no analyst took a side on the resolved thesis"
    );
    let memory = platform.research_memory();
    assert_eq!(
        memory.episode_count(),
        leaning,
        "one research episode per directional stance on the resolved thesis"
    );
    assert!(
        memory
            .episodes()
            .iter()
            .all(|episode| episode.outcome.is_some())
    );
    Ok(())
}

/// R12-AMBIENT-3024: the Meta Brain says which analyst is reliable in which
/// regime, and it can only say so if every call the kernel charges to research
/// memory carries the regime it was made in. Each research episode must carry
/// exactly the `{market}/{volatility}` key the resolved kernel episode was
/// stamped with, and `reliability_by_regime` must then hold one cell per
/// (analyst, regime) the resolved thesis named.
///
/// Mutation: write `regime: None` in `write_research_memory` — every research
/// episode carries no regime and the per-regime record is empty.
#[test]
fn every_call_charged_to_research_memory_carries_the_regime_it_was_made_in() -> Result<()> {
    use std::collections::BTreeSet;
    let (t2, t3) = resolution_instants()?;
    let (platform, _) = three_cycles(t2, t3)?;
    let regimes: BTreeSet<String> = platform
        .remembered_episodes(t3)
        .filter(|episode| episode.outcome.is_some())
        .map(|episode| format!("{}/{}", episode.regime.market, episode.regime.volatility))
        .collect();
    assert!(
        !regimes.is_empty(),
        "premise: a thesis resolved, so some kernel episode carries a regime"
    );
    let memory = platform.research_memory();
    assert!(
        memory.episode_count() > 0,
        "premise: the resolved thesis charged its analysts to research memory"
    );
    for episode in memory.episodes() {
        let regime = episode
            .regime
            .as_ref()
            .unwrap_or_else(|| panic!("{} was charged with no regime", episode.agent_id));
        assert!(
            regimes.contains(regime),
            "{} was charged under {regime}, not a regime a resolved episode was stamped with \
             ({regimes:?})",
            episode.agent_id
        );
    }
    let cells = memory.reliability_by_regime(t3);
    assert!(!cells.is_empty(), "no per-regime record was formed");
    for (agent, regime) in cells.keys() {
        assert!(
            regimes.contains(regime),
            "{agent} has a record under {regime}, which no resolved episode carried"
        );
    }
    let resolved: usize = cells.values().map(|record| record.resolved).sum();
    assert_eq!(
        resolved,
        memory.episode_count(),
        "every resolved call is counted once"
    );
    Ok(())
}

/// R12-AMBIENT-3024 at the kernel seam: the Meta Brain is constructed in the
/// production organisation and runs in the REASON stage's dispatch, after the
/// panel, under the regime in force for the brief's instrument — so by the
/// third cycle, once a thesis has resolved and its analysts have been charged
/// to research memory under their regime, its finding reports the record it
/// read rather than a pooled rate.
///
/// The regime is asserted as a value, not as punctuation: until 2026-11-22
/// this test asked that the claim contain `"; in "` and a `/`, both literals
/// of `MetaBrain::arbitrate`'s own format string, which a brief carrying no
/// regime at all satisfies whenever any other clause follows. It now reads
/// the brief the audit trail recorded for the Meta Brain's run, and the
/// delimited regime in the claim, against `Platform::regime_context`.
///
/// Mutations: drop the `in_regime` call where `stage_reason` builds the brief
/// — the recorded brief names no regime; drop `with_arbiters` from
/// `Organisation::standard` — the meta brain never runs.
#[test]
fn the_reason_stage_runs_the_meta_brain_under_the_regime_in_force() -> Result<()> {
    let (t2, t3) = resolution_instants()?;
    let (platform, _) = three_cycles(t2, t3)?;
    let regime = platform.regime_context(object("AAA").as_str());
    assert!(
        regime
            .split_once('/')
            .is_some_and(|(market, volatility)| !market.is_empty() && !volatility.is_empty()),
        "premise: the platform keys a regime as market/volatility: {regime}"
    );
    let (brief, last) = last_meta_run(&platform);
    assert_eq!(
        brief.as_of, t3,
        "premise: the run read is the third cycle's REASON"
    );
    assert_eq!(
        brief.regime.as_deref(),
        Some(regime.as_str()),
        "the Meta Brain was not briefed under the regime in force"
    );
    assert_eq!(
        regime_the_claim_names(&last.claim),
        Some(regime.as_str()),
        "the claim does not name the regime in force: {}",
        last.claim
    );
    Ok(())
}

/// The brief and the finding of the Meta Brain's most recent run that
/// produced a finding, as the organisation's audit trail holds them.
fn last_meta_run(
    platform: &Platform,
) -> (
    &qip_agents::finding::AgentBrief,
    &qip_agents::finding::AgentFinding,
) {
    platform
        .organisation()
        .audit()
        .records()
        .iter()
        .rev()
        .filter(|run| run.agent_id == qip_investment_agents::ids::META)
        .find_map(|run| run.finding.as_ref().map(|finding| (&run.brief, finding)))
        .expect("the meta brain ran in a REASON dispatch and produced a finding")
}

/// The regime a Meta Brain claim says its counts are for: the text between
/// `in ` and the comma in the clause `in {regime}, N specialist(s) measured`,
/// whole — never a substring test, which a neighbouring regime would pass.
fn regime_the_claim_names(claim: &str) -> Option<&str> {
    claim.split("; ").find_map(|clause| {
        let (regime, rest) = clause.strip_prefix("in ")?.split_once(", ")?;
        rest.contains("specialist(s) measured").then_some(regime)
    })
}

/// Every fact of `finding` whose label starts with `prefix`, by label.
fn facts_under(
    finding: &qip_agents::finding::AgentFinding,
    prefix: &str,
) -> std::collections::BTreeMap<String, f64> {
    finding
        .facts
        .iter()
        .filter(|fact| fact.label.starts_with(prefix))
        .map(|fact| (fact.label.clone(), fact.value))
        .collect()
}

/// One fact of `finding` by its exact label.
fn fact_of(finding: &qip_agents::finding::AgentFinding, label: &str) -> Option<f64> {
    finding
        .facts
        .iter()
        .find(|fact| fact.label == label)
        .map(|fact| fact.value)
}

/// R12-AMBIENT-3024, the models half, through the cycle on
/// `PlatformConfig::default`: the Meta Brain measures which *models* are
/// reliable in which regime, not only which analysts. The model tournament
/// files every scored forecast under the regime its question was opened in;
/// before 2026-11-22 nothing handed that record to the Meta Brain, which read
/// research memory alone, so a detector the platform had scored for months
/// was never named in the one finding whose scope is reliability by regime.
///
/// Three cycles: the first opens a question every forecaster answers, the
/// second resolves it in LEARN, and the third's REASON briefs the desk. The
/// Meta Brain's finding from that REASON must carry one cell per (forecaster,
/// regime) the tournament scored — every one, at the tournament's own figure
/// read here through `Tournament::scorecards`, a different path from the one
/// the kernel feeds the desk by — and must count as measured in the regime in
/// force exactly the forecasters scored in that regime.
///
/// Mutations: delete `self.hand_the_arbiters_the_models_record();` from
/// `stage_reason` — no cell is reported; file every cell under one fixed
/// regime in `Tournament::regime_briers` — the cells no longer match the
/// scorecards' regimes.
#[test]
fn the_meta_brain_reports_every_forecasters_record_under_the_regime_the_tournament_scored_it_in()
-> Result<()> {
    let (t2, t3) = resolution_instants()?;
    // `three_cycles`, with the forecasters read off the open questions after
    // the first cycle and again after the third: the detector that answers
    // the third cycle's question need not be the class that answered the
    // first, and the first cycle's is the one LEARN has scored.
    let asked = |platform: &Platform| -> Vec<String> {
        platform
            .model_tournament()
            .open_questions()
            .flat_map(|question| question.forecasts().keys().cloned())
            .collect()
    };
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));
    platform.run_cycle(start());
    let mut forecasters: std::collections::BTreeSet<String> =
        asked(&platform).into_iter().collect();
    let horizon = platform.predictions()[0].proposition.resolves_at;
    assert!(
        t2 > horizon,
        "premise: the second cycle runs past the horizon"
    );
    platform.observe(swings("AAA", horizon));
    platform.run_cycle(t2);
    platform.run_cycle(t3);
    forecasters.extend(asked(&platform));
    let tournament = platform.model_tournament();
    assert!(
        forecasters.iter().any(|f| f.starts_with("detector:"))
            && forecasters.iter().any(|f| f.starts_with("analyst:")),
        "premise: a detector and an analyst both forecast: {forecasters:?}"
    );
    // What the tournament holds for each, by regime, through its scorecards.
    let mut expected_brier = std::collections::BTreeMap::new();
    let mut expected_scored = std::collections::BTreeMap::new();
    let regime_in_force = platform.regime_context(object("AAA").as_str());
    let mut in_force = 0usize;
    let mut scored_forecasters = 0usize;
    for forecaster in &forecasters {
        let cards = tournament.scorecards(forecaster);
        let mut any = false;
        for card in &cards {
            let Some(brier) = card.brier.value() else {
                continue;
            };
            any = true;
            let cell = format!("{forecaster}@{}", card.regime);
            expected_brier.insert(format!("model_brier:{cell}"), brier);
            expected_scored.insert(format!("model_scored:{cell}"), card.resolved as f64);
            if card.regime == regime_in_force {
                in_force += 1;
            }
        }
        scored_forecasters += usize::from(any);
    }
    assert!(
        expected_brier.len() >= 2,
        "premise: the second cycle's LEARN scored the first cycle's forecasters: \
         {expected_brier:?}"
    );
    // The premise the count below stands on. Without it `in_force` could be
    // zero and a Meta Brain that counted no model in any regime would agree.
    assert!(
        in_force >= 2,
        "premise: forecasters are scored in the regime in force ({regime_in_force}): \
         {expected_brier:?}"
    );
    assert_eq!(
        last_meta_run(&platform).0.regime.as_deref(),
        Some(regime_in_force.as_str()),
        "premise: the Meta Brain was briefed under `Platform::regime_context`'s key"
    );
    assert!(
        expected_brier
            .keys()
            .any(|cell| cell.starts_with("model_brier:detector:")),
        "premise: a detector, not only analysts, holds a scored cell: {expected_brier:?}"
    );

    let runs = platform
        .organisation()
        .audit()
        .for_agent(qip_investment_agents::ids::META);
    let last = runs
        .iter()
        .filter_map(|run| run.finding.as_ref())
        .next_back()
        .expect("the meta brain's run produced a finding");
    let facts = |prefix: &str| -> std::collections::BTreeMap<String, f64> {
        last.facts
            .iter()
            .filter(|fact| fact.label.starts_with(prefix))
            .map(|fact| (fact.label.clone(), fact.value))
            .collect()
    };
    assert_eq!(
        facts("model_brier:"),
        expected_brier,
        "the Meta Brain's model cells are not the tournament's: {}",
        last.claim
    );
    assert_eq!(facts("model_scored:"), expected_scored, "{}", last.claim);
    let fact = |label: &str| {
        last.facts
            .iter()
            .find(|fact| fact.label == label)
            .map(|fact| fact.value)
    };
    assert_eq!(
        fact("model_regime_reliability_cells"),
        Some(expected_brier.len() as f64)
    );
    assert_eq!(
        fact("models_measured_in_regime"),
        Some(in_force as f64),
        "in {regime_in_force}: {}",
        last.claim
    );
    assert_eq!(
        fact("models_unmeasured_in_regime"),
        Some((scored_forecasters - in_force) as f64),
        "in {regime_in_force}: {}",
        last.claim
    );
    eprintln!("evidence: {}", last.claim);
    Ok(())
}

/// R12-REASON-3109 at the kernel seam: a hypothesis the red team refuses on
/// review never reaches DECIDE. The refusal was proven inside
/// `qip-reasoning-engine`, where the review verdict is formed, and nothing
/// proved that the platform honours it — that a rejected hypothesis is not
/// still turned into a thesis and sized. This tape is the one the review
/// rejects (see the episode test above); the cycle runs every stage, and the
/// assertion is on what DECIDE sized, not on what REASON said.
///
/// Mutation: in `Platform`'s REASON arm, take the thesis branch whatever the
/// verdict (`if false && !approved {` in place of `if !approved {`) — REASON
/// queues a thesis "sized on conviction 0.396" and DECIDE weighs obj-AAA, so
/// the thesis assertion fires. Nothing is sized even then, because the action
/// bar refuses so weak a thesis, which is why the assertion is on the queue
/// and not only on the proposals.
#[test]
fn a_hypothesis_refused_on_review_forms_no_thesis_and_decide_sizes_nothing_for_it() -> Result<()> {
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));
    platform.observe(leading_bars("BBB", 120));
    let report = platform.run_cycle(start());

    let reason = report
        .stage(Stage::Reason)
        .expect("premise: the cycle reports its REASON stage");
    // The verdict as the reasoning engine stated it, read from the stage
    // detail its own summary writes, rather than from the problem the
    // kernel's rejection arm adds — the arm under test.
    assert!(
        reason
            .detail
            .contains("; hypothesis rejected at confidence "),
        "premise: the review rejected a hypothesis on this tape:\n{}",
        report.summarise()
    );
    let decide = report
        .stage(Stage::Decide)
        .expect("premise: the cycle reports its DECIDE stage");
    assert!(decide.ran, "premise: DECIDE ran:\n{}", report.summarise());
    // No thesis was queued for DECIDE: the clause REASON writes when it
    // queues one is absent, and DECIDE has no clause about the instrument.
    assert!(
        !reason.detail.contains("; sized on conviction "),
        "REASON queued a thesis for a hypothesis the review refused:\n{}",
        report.summarise()
    );
    assert!(
        !decide
            .detail
            .split("; ")
            .any(|clause| clause.starts_with("obj-AAA: ")),
        "DECIDE weighed the refused hypothesis's instrument:\n{}",
        report.summarise()
    );
    let sized: Vec<String> = platform
        .proposals()
        .iter()
        .flat_map(|p| p.legs.iter().map(|leg| leg.object_id.to_string()))
        .collect();
    assert!(
        sized.is_empty(),
        "DECIDE sized {sized:?} on a cycle whose only hypothesis the review refused:\n{}",
        report.summarise()
    );
    Ok(())
}

/// R12-WORLD-3033: an episode says what the platform did about its claim, not
/// only what it claimed. A thesis whose order filled remembers that fill, by
/// order id, quantity and price; the same thesis with no order remembers none,
/// so the field cannot be filled by default. Mutations: leave the vector empty
/// in `remember_resolved` (the filled case fails); match the belief key
/// against a hypothesis nothing cites in `fills_expressing` (no fill is
/// found, the same assertion fails).
#[test]
fn a_resolved_episode_remembers_the_fills_on_the_orders_that_expressed_its_claim() -> Result<()> {
    let (t2, t3) = resolution_instants()?;

    // The control: no order was ever sent for the thesis.
    let (idle, _) = three_cycles(t2, t3)?;
    let remembered: Vec<_> = idle.remembered_episodes(t3).collect();
    assert_eq!(remembered.len(), 1, "premise: one episode was remembered");
    assert!(
        remembered[0]
            .outcome
            .as_ref()
            .is_some_and(|o| o.fills.is_empty()),
        "an episode with no order remembered a fill"
    );

    // The same tape, with an order citing the hypothesis that fills before the
    // claim resolves.
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));
    platform.run_cycle(start());
    let claim = platform.predictions()[0].clone();
    let hypothesis = claim
        .claim
        .as_ref()
        .map(|c| c.hypothesis_id.clone())
        .expect("premise: the claim was written down");
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("10"),
        dec!("100"),
        "prop-episode",
        vec![hypothesis],
        start(),
    );
    let order_id = order.order_id.clone();
    platform.submit_order(order, start())?;
    let filled: Vec<_> = platform
        .orders()
        .fills()
        .into_iter()
        .filter(|fill| fill.order_id == order_id)
        .cloned()
        .collect();
    assert!(!filled.is_empty(), "premise: the accepted order filled");

    platform.observe(swings("AAA", claim.proposition.resolves_at));
    platform.run_cycle(t2);
    let remembered: Vec<_> = platform.remembered_episodes(t3).collect();
    assert_eq!(remembered.len(), 1, "premise: one episode was remembered");
    let fills = &remembered[0]
        .outcome
        .as_ref()
        .expect("a remembered episode is a resolved one")
        .fills;
    assert_eq!(fills.len(), filled.len(), "{fills:?}");
    for (remembered, fill) in fills.iter().zip(&filled) {
        assert_eq!(remembered.order_id, order_id.as_str());
        assert_eq!(remembered.quantity, fill.quantity);
        assert_eq!(remembered.price, fill.price);
    }
    Ok(())
}

/// A quote on `symbol` whose spread is `bps` basis points of a hundred-dollar
/// mid — the fixture `regime_boundary.rs` turns a subject illiquid with.
fn quote(symbol: &str, at: Timestamp, bps: f64) -> SensedRecord {
    let half = 100.0 * bps / 20_000.0;
    SensedRecord::Quote(qip_market::quote::Quote {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        at,
        capture_time: at,
        sequence: None,
        bid: Decimal::from_f64(100.0 - half).expect("a price"),
        ask: Decimal::from_f64(100.0 + half).expect("a price"),
        bid_size: dec!("1000"),
        ask_size: dec!("1000"),
        quality: DataQuality::default(),
    })
}

/// The latest horizon among the claims still open, where the next resolving
/// cycle must run past.
fn latest_open_horizon(platform: &Platform) -> Timestamp {
    platform
        .predictions()
        .iter()
        .filter(|prediction| prediction.verdict.is_none())
        .map(|prediction| prediction.proposition.resolves_at)
        .max()
        .expect("premise: the cycle left a claim open")
}

/// What the tournament holds per (forecaster, regime), read through
/// `Tournament::scorecards` — a different path from `regime_briers`, the one
/// the kernel hands the Meta Brain by — as `model_brier:` and `model_scored:`
/// facts would state it, with the forecasters scored in each regime.
#[derive(Debug, Default, PartialEq)]
struct TournamentRecord {
    brier: std::collections::BTreeMap<String, f64>,
    scored: std::collections::BTreeMap<String, f64>,
    by_regime: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

impl TournamentRecord {
    fn models(&self) -> std::collections::BTreeSet<&String> {
        self.by_regime.values().flatten().collect()
    }
    fn measured_in(&self, regime: &str) -> usize {
        self.by_regime.get(regime).map_or(0, |models| models.len())
    }
}

fn tournament_record(
    platform: &Platform,
    forecasters: &std::collections::BTreeSet<String>,
) -> TournamentRecord {
    let mut record = TournamentRecord::default();
    for forecaster in forecasters {
        for card in platform.model_tournament().scorecards(forecaster) {
            let Some(brier) = card.brier.value() else {
                continue;
            };
            let cell = format!("{forecaster}@{}", card.regime);
            record.brier.insert(format!("model_brier:{cell}"), brier);
            record
                .scored
                .insert(format!("model_scored:{cell}"), card.resolved as f64);
            record
                .by_regime
                .entry(card.regime.clone())
                .or_default()
                .insert(forecaster.clone());
        }
    }
    record
}

/// R12-AMBIENT-3024, "which models are reliable **in which regimes**",
/// through the production producer: `Tournament::regime_briers` read by
/// `Platform::stage_reason` and handed to the Meta Brain, on
/// `PlatformConfig::default`, across a regime that actually turns. The test
/// beside this one runs on a tape that never leaves one regime, so a
/// tournament that pooled a forecaster's scores across regimes and filed the
/// pool under every label, or a Meta Brain that counted a model measured
/// wherever it had any record, passed it — which is what two independent
/// readers found on 2026-11-22.
///
/// Five cycles on one name. The first two open and resolve questions in the
/// regime the tape starts in. A quote thirty times wider than the name's
/// median then turns it illiquid, and stays: the third cycle's REASON runs in
/// the new regime with every model's record earned in the old one, so the
/// Meta Brain must count **none** measured in force and all of them
/// unmeasured. The third and fourth cycles' questions open in the new regime
/// and resolve, so by the fifth REASON the forecasters hold a cell in each,
/// and every cell must be the tournament's own figure for that regime alone —
/// one scored forecast in the old regime is not two because the model has
/// since been scored in the new one.
///
/// Mutations: pool every regime's scores under each label in
/// `Tournament::regime_briers`; count every model as measured whatever the
/// regime (drop the `wanted == c.regime` filter in `MetaBrain::arbitrate`);
/// delete the hand-off from `stage_reason`.
#[test]
fn a_model_scored_in_one_regime_is_unmeasured_when_the_tape_turns_and_is_never_pooled_across_them()
-> Result<()> {
    let name = object("AAA");
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));
    // Three tight quotes, so the median the spread is compared against is a
    // tight one; fewer and the regime could never turn on liquidity.
    for day in 0..3 {
        platform.observe(vec![quote(
            "AAA",
            start().saturating_sub(Duration::from_days(3 - day)),
            10.0,
        )]);
    }
    let mut forecasters = std::collections::BTreeSet::new();
    let mut cycle = |platform: &mut Platform, at: Timestamp| {
        platform.run_cycle(at);
        forecasters.extend(
            platform
                .model_tournament()
                .open_questions()
                .flat_map(|question| question.forecasts().keys().cloned()),
        );
        let horizon = latest_open_horizon(platform);
        platform.observe(swings("AAA", horizon));
        (
            forecasters.clone(),
            horizon.saturating_add(Duration::from_mins(1)),
        )
    };

    let (_, t2) = cycle(&mut platform, start());
    let settled = platform.regime_context(name.as_str());
    let (known, t3) = cycle(&mut platform, t2);
    assert_eq!(
        platform.regime_context(name.as_str()),
        settled,
        "premise: the first two cycles ran in one regime"
    );
    let before_the_turn = tournament_record(&platform, &known);
    assert!(
        before_the_turn.measured_in(&settled) >= 2 && before_the_turn.by_regime.len() == 1,
        "premise: the second cycle scored the first's forecasters, all in {settled}: \
         {before_the_turn:?}"
    );

    // The turn.
    platform.observe(vec![quote("AAA", t3, 300.0)]);
    let (_, t4) = cycle(&mut platform, t3);
    let turned = platform.regime_context(name.as_str());
    assert_ne!(
        settled, turned,
        "premise: the regime key changed; without that nothing here discriminates"
    );
    {
        let (brief, finding) = last_meta_run(&platform);
        assert_eq!(brief.as_of, t3, "premise: the third cycle's REASON");
        assert_eq!(brief.regime.as_deref(), Some(turned.as_str()));
        assert_eq!(
            regime_the_claim_names(&finding.claim),
            Some(turned.as_str())
        );
        assert_eq!(
            facts_under(finding, "model_brier:"),
            before_the_turn.brier,
            "{}",
            finding.claim
        );
        // Every record was earned in the regime the tape has left.
        assert_eq!(
            fact_of(finding, "models_measured_in_regime"),
            Some(0.0),
            "a record earned in {settled} was read as a record in {turned}: {}",
            finding.claim
        );
        assert_eq!(
            fact_of(finding, "models_unmeasured_in_regime"),
            Some(before_the_turn.models().len() as f64),
            "{}",
            finding.claim
        );
    }

    let (known, t5) = cycle(&mut platform, t4);
    assert_eq!(
        platform.regime_context(name.as_str()),
        turned,
        "premise: the name stayed in the regime it turned to"
    );
    // What the fifth REASON can know: everything LEARN had scored by the end
    // of the fourth cycle, so the snapshot is taken before the fifth runs.
    let both = tournament_record(&platform, &known);
    platform.run_cycle(t5);
    assert!(
        both.by_regime.contains_key(&settled) && both.by_regime.contains_key(&turned),
        "premise: forecasts were scored in both regimes: {both:?}"
    );
    let in_both: Vec<&String> = both.by_regime[&settled]
        .intersection(&both.by_regime[&turned])
        .collect();
    assert!(
        in_both.iter().any(|model| model.starts_with("detector:"))
            && in_both.iter().any(|model| model.starts_with("analyst:")),
        "premise: a detector and an analyst each hold a cell in both regimes: {in_both:?}"
    );
    assert!(
        in_both.iter().any(|model| {
            both.brier[&format!("model_brier:{model}@{settled}")]
                != both.brier[&format!("model_brier:{model}@{turned}")]
        }),
        "premise: some model's record differs between the regimes, so a pooled figure is \
         neither: {both:?}"
    );
    assert!(
        both.measured_in(&turned) < both.models().len(),
        "premise: some model is scored only in {settled}, so a regime-blind count differs: \
         {both:?}"
    );

    let (brief, finding) = last_meta_run(&platform);
    assert_eq!(brief.as_of, t5, "premise: the fifth cycle's REASON");
    assert_eq!(brief.regime.as_deref(), Some(turned.as_str()));
    assert_eq!(
        facts_under(finding, "model_brier:"),
        both.brier,
        "a cell is not the tournament's figure for its own regime: {}",
        finding.claim
    );
    assert_eq!(
        facts_under(finding, "model_scored:"),
        both.scored,
        "{}",
        finding.claim
    );
    assert_eq!(
        fact_of(finding, "model_regime_reliability_cells"),
        Some(both.brier.len() as f64)
    );
    assert_eq!(
        fact_of(finding, "models_measured_in_regime"),
        Some(both.measured_in(&turned) as f64),
        "in {turned}: {}",
        finding.claim
    );
    assert_eq!(
        fact_of(finding, "models_unmeasured_in_regime"),
        Some((both.models().len() - both.measured_in(&turned)) as f64),
        "in {turned}: {}",
        finding.claim
    );
    eprintln!("evidence: {}", finding.claim);
    Ok(())
}

/// Twenty swinging bars up to `horizon` that end high where `swings` ends
/// low, so the tape the next REASON reads has just risen by half.
fn swings_ending_high(symbol: &str, horizon: Timestamp) -> Vec<SensedRecord> {
    (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (150.0, 100.0)
            } else {
                (100.0, 150.0)
            };
            let at = horizon.saturating_sub(Duration::from_mins((20 - i) * 60));
            hourly_bar(symbol, at, open, close)
        })
        .collect()
}

/// R12-AMBIENT-3024, "allocating research attention" and "resolving
/// disagreements", through `Platform::run_cycle` on `PlatformConfig::default`.
/// Both were proven on `Organisation::standard` with a research memory the
/// test wrote; nothing showed the cycle's own REASON reaching either with the
/// memory the cycle's own LEARN had charged. The register recorded that the
/// production panel is not contested on the populated fixture. In the cycle
/// it is: the first panel on a fresh name disagrees (with no record to weigh
/// it by, which the test asserts is said rather than resolved), and once two
/// theses have resolved a tape that has just risen sets the equity analyst
/// against the simulation analyst again, this time with both on the record.
///
/// The expected figures are arithmetic done here from two things the Meta
/// Brain did not write: the per-(agent, regime) record research memory held
/// before the cycle ran, and the panel's own findings as the audit trail
/// holds them.
///
/// Mutations: weigh the contested panel by conviction alone (drop
/// `* reliability` in `reliability_weighted_balance`); give every measured
/// specialist an even share (`1.0 / measured.len()` for `reliability / total`
/// in `MetaBrain::arbitrate`).
#[test]
fn the_cycle_resolves_its_own_contested_panel_and_allocates_attention_by_the_record_it_charged()
-> Result<()> {
    use qip_agents::finding::Direction;
    let name = object("AAA");
    let mut platform = fresh()?;
    platform.observe(bars("AAA", 120));

    // Cycle one: contested, and nobody has a record.
    let first = platform.run_cycle(start());
    assert!(
        reason_clauses(&first).contains(&"contested".to_string()),
        "premise: the first panel is contested: {:?}",
        reason_clauses(&first)
    );
    {
        let (_, finding) = last_meta_run(&platform);
        assert_eq!(fact_of(finding, "reliability_weighted_balance"), None);
        assert_eq!(fact_of(finding, "attention_allocated"), None);
        assert!(
            finding
                .caveats
                .iter()
                .any(|caveat| caveat.starts_with("contested, unresolved:")),
            "a contested panel nobody's record can weigh was not reported unresolved: {:?}",
            finding.caveats
        );
    }

    // Two resolutions, then a third and fourth REASON on a tape that ends
    // high. The fourth is the one read: by then both dissenting authors hold
    // a record that is neither nothing nor everything.
    let mut at = start();
    for round in 0..3 {
        let horizon = latest_open_horizon(&platform);
        platform.observe(if round == 0 {
            swings("AAA", horizon)
        } else {
            swings_ending_high("AAA", horizon)
        });
        at = horizon.saturating_add(Duration::from_mins(1));
        if round < 2 {
            platform.run_cycle(at);
        }
    }
    let regime = platform.regime_context(name.as_str());
    // The record REASON will read: what memory holds before the cycle runs.
    let record: std::collections::BTreeMap<String, f64> = platform
        .research_memory()
        .reliability_by_regime(at)
        .into_iter()
        .filter(|((_, cell_regime), _)| *cell_regime == regime)
        .filter_map(|((agent, _), cell)| Some((agent, cell.hit_rate()?)))
        .collect();
    assert!(
        record.len() >= 2,
        "premise: LEARN charged at least two analysts in {regime}: {record:?}"
    );
    let fourth = platform.run_cycle(at);
    assert!(
        reason_clauses(&fourth).contains(&"contested".to_string()),
        "premise: the fourth panel is contested: {:?}",
        reason_clauses(&fourth)
    );
    let (brief, finding) = last_meta_run(&platform);
    assert_eq!(brief.as_of, at, "premise: the fourth cycle's REASON");
    assert_eq!(brief.regime.as_deref(), Some(regime.as_str()));

    // The panel of that dispatch, from the audit trail.
    let panel: Vec<&qip_agents::finding::AgentFinding> = platform
        .organisation()
        .audit()
        .records()
        .iter()
        .filter(|run| run.brief.as_of == at && run.brief.question == brief.question)
        .filter(|run| run.agent_id != qip_investment_agents::ids::META)
        .filter_map(|run| run.finding.as_ref())
        .collect();

    // Disagreement: sign x effective conviction x hit rate in the regime,
    // over the authors that took a side and hold a record.
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    let mut signs = std::collections::BTreeSet::new();
    for author in &panel {
        let sign = match author.direction {
            Direction::Positive => 1.0,
            Direction::Negative => -1.0,
            _ => continue,
        };
        let Some(rate) = record.get(&author.agent_id) else {
            continue;
        };
        let weight = author.effective_conviction() * rate;
        numerator += sign * weight;
        denominator += weight;
        if weight > 0.0 {
            signs.insert(sign > 0.0);
        }
    }
    assert_eq!(
        signs.len(),
        2,
        "premise: authors with a record weigh in on both sides, so the balance is not ±1"
    );
    let expected = numerator / denominator;
    let conviction_only: f64 = {
        let (n, d) = panel
            .iter()
            .filter(|f| record.contains_key(&f.agent_id))
            .fold((0.0, 0.0), |(n, d), f| match f.direction {
                Direction::Positive => (n + f.effective_conviction(), d + f.effective_conviction()),
                Direction::Negative => (n - f.effective_conviction(), d + f.effective_conviction()),
                _ => (n, d),
            });
        n / d
    };
    assert!(
        (expected - conviction_only).abs() > 1e-3,
        "premise: the record moves the balance ({expected} against {conviction_only} unweighed)"
    );
    let resolved =
        fact_of(finding, "reliability_weighted_balance").expect("the contested panel was resolved");
    assert!(
        (resolved - expected).abs() < 1e-12,
        "resolved to {resolved}, expected {expected}: {}",
        finding.claim
    );

    // Attention: each measured author's hit rate over the measured total.
    let authors: std::collections::BTreeSet<&String> = panel.iter().map(|f| &f.agent_id).collect();
    let measured: std::collections::BTreeMap<&String, f64> = record
        .iter()
        .filter(|(agent, _)| authors.contains(agent))
        .map(|(agent, rate)| (agent, *rate))
        .collect();
    let total: f64 = measured.values().sum();
    let rates: std::collections::BTreeSet<String> =
        measured.values().map(|rate| format!("{rate:.6}")).collect();
    assert!(
        measured.len() >= 2 && rates.len() >= 2,
        "premise: two measured authors with different records, so an even split is wrong: \
         {measured:?}"
    );
    assert_eq!(
        fact_of(finding, "attention_allocated"),
        Some(measured.len() as f64),
        "{}",
        finding.claim
    );
    let weights = facts_under(finding, "attention_weight:");
    assert_eq!(
        weights.keys().cloned().collect::<Vec<_>>(),
        measured
            .keys()
            .map(|agent| format!("attention_weight:{agent}"))
            .collect::<Vec<_>>(),
        "attention went to someone other than the measured authors"
    );
    for (agent, rate) in &measured {
        let weight = weights[&format!("attention_weight:{agent}")];
        assert!(
            (weight - rate / total).abs() < 1e-12,
            "{agent} was given {weight}, its record is {rate} of {total}"
        );
    }
    eprintln!("evidence: {}", finding.claim);
    Ok(())
}

/// The REASON stage's detail split into its clauses, so `contested` is
/// matched as a whole clause and not as a substring of another.
fn reason_clauses(report: &qip_kernel::cycle::CycleReport) -> Vec<String> {
    report
        .stage(Stage::Reason)
        .expect("REASON ran")
        .detail
        .split([',', ';'])
        .map(|clause| clause.trim().to_string())
        .collect()
}
