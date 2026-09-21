//! Episodic memory: bitemporal recall, bounded capacity, approximate
//! retrieval that re-ranks exactly, and determinism across constructions.
//!
//! Each test names the failure it prevents. The class that matters most is
//! leakage: an episode recalled before its outcome was knowable makes every
//! backtest that touches it a lie, and no test on the reasoning side can see
//! that from where it stands.

// Exact float comparisons are deliberate where two constructions must agree
// bit for bit; a "close enough" replay is not a replay.
#![allow(clippy::float_cmp)]

use qip_ai::memory::{
    AnalystStance, CausalContextEdge, ClaimRecord, DecisionTaken, EPISODE_DIMENSIONS,
    EPISODE_ENCODING, Episode, EpisodeGrade, EpisodeOutcome, EpisodeQuery, EpisodeSampler,
    EpisodicMemory, FindingsSummary, HIGH_SURPRISE_BPS, MarketState, PrecedentDigest, RegimeLabel,
    StanceDirection, TAIL_RESERVE_DIVISOR,
};
use qip_core::time::{Duration, Timestamp};

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 9, 1)
}

fn regime(market: &str, volatility: &str) -> RegimeLabel {
    RegimeLabel {
        market: market.to_string(),
        volatility: volatility.to_string(),
    }
}

fn claim(label: &str, direction: f64, confidence: f64) -> ClaimRecord {
    ClaimRecord {
        class: "price_move".to_string(),
        claim: label.to_string(),
        direction,
        confidence,
    }
}

fn stance(agent: &str, direction: StanceDirection, conviction: f64) -> AnalystStance {
    AnalystStance {
        agent_id: agent.to_string(),
        direction,
        conviction,
    }
}

/// A resolved episode: true at `at`, knowable one day later.
fn episode(id: &str, instrument: &str, market: &str, at: Timestamp, move_bps: f64) -> Episode {
    Episode {
        episode_id: id.to_string(),
        instrument: instrument.to_string(),
        regime: regime(market, "normal"),
        state: None,
        causal_context: Vec::new(),
        findings: FindingsSummary {
            runs: 4,
            findings: 3,
            coverage: 0.75,
            contested: false,
        },
        stances: vec![
            stance("analyst-a", StanceDirection::Positive, 0.6),
            stance("analyst-b", StanceDirection::Positive, 0.7),
        ],
        claim: claim("undervalued", 1.0, 0.62),
        horizon: Duration::from_days(5),
        decision: DecisionTaken::Approved,
        outcome: Some(EpisodeOutcome {
            resolved_at: at.saturating_add(Duration::from_days(1)),
            realised_move_bps: move_bps,
            realised_pnl: 0.0,
            expected_move_bps: None,
        }),
        at,
        known_at: at.saturating_add(Duration::from_days(1)),
    }
}

#[test]
fn an_episode_is_not_recalled_before_its_known_at() {
    // The failure: a Monday backtest recalling a Tuesday resolution. The
    // memory's only read path takes `now` and filters on `known_at`, so the
    // same query asked at the instant and just after it must give different
    // answers — and at the instant itself the answer is nothing, because a
    // record stamped `now` is not yet knowledge on a clock that can hand two
    // cycles the same reading.
    let mut memory = EpisodicMemory::new(16, 16).expect("non-zero bounds");
    let formed = start();
    let one = episode("ep-1", "obj-AAA", "trending", formed, 120.0);
    let known_at = one.known_at;
    assert!(
        known_at > formed,
        "the fixture must become knowable after it was true"
    );
    memory
        .remember(one.clone())
        .expect("a valid episode is remembered");
    assert_eq!(memory.len(), 1, "premise: the memory holds the episode");

    let query = one.as_query();
    for (label, now) in [
        (
            "a minute before",
            known_at.saturating_sub(Duration::from_mins(1)),
        ),
        ("at the instant of", known_at),
    ] {
        let before = memory.recall(&query, now, 5);
        assert!(
            before.nearest.is_empty(),
            "recalled {} episode(s) {label} their known_at",
            before.nearest.len()
        );
        assert_eq!(
            before.examined, 0,
            "an unknowable episode must not even occupy a candidate slot ({label})"
        );
    }

    let at = memory.recall(&query, known_at.saturating_add(Duration::from_nanos(1)), 5);
    assert_eq!(
        at.nearest.len(),
        1,
        "retrievable once its known_at has passed"
    );
    assert_eq!(at.nearest[0].episode.episode_id, "ep-1");
    assert!(
        (at.nearest[0].similarity - 1.0).abs() < 1e-6,
        "an episode is its own nearest neighbour at similarity 1, got {}",
        at.nearest[0].similarity
    );
}

#[test]
fn the_plain_iterator_hides_an_episode_whose_known_at_has_not_passed() {
    // The failure this guards, found in review: `episodes()` returned every
    // stored episode regardless of `known_at`, while the module doc said no
    // read path ignored it. `recall` was honest and the iterator beside it
    // was not, so anything that walked memory rather than querying it — a
    // digest, a report, a future backtest — would have read Tuesday's
    // resolution on Monday. The iterator now takes `now` and applies the
    // same strict rule as `recall`.
    let mut memory = EpisodicMemory::new(16, 16).expect("non-zero bounds");
    let day = Duration::from_days(1);
    let old = episode("ep-old", "obj-AAA", "trending", start(), 120.0);
    let new = episode(
        "ep-new",
        "obj-AAA",
        "trending",
        start().saturating_add(day * 10),
        80.0,
    );
    let (old_known, new_known) = (old.known_at, new.known_at);
    assert!(
        old_known < new_known,
        "the fixture orders the two by known_at"
    );
    memory.remember(old).expect("valid");
    memory.remember(new).expect("valid");
    assert_eq!(memory.len(), 2, "premise: the memory holds both episodes");
    // Premise: with everything knowable, the iterator yields both — so the
    // absence below is the filter and not an empty store.
    let all: Vec<&str> = memory
        .episodes(Timestamp::MAX)
        .map(|e| e.episode_id.as_str())
        .collect();
    assert_eq!(all, vec!["ep-old", "ep-new"]);

    // Strictly before: at the instant of the newer episode's `known_at` it
    // is not yet knowable, and a nanosecond later it is.
    for (label, now) in [
        (
            "a minute before",
            new_known.saturating_sub(Duration::from_mins(1)),
        ),
        ("at the instant of", new_known),
    ] {
        let visible: Vec<&str> = memory
            .episodes(now)
            .map(|e| e.episode_id.as_str())
            .collect();
        assert_eq!(
            visible,
            vec!["ep-old"],
            "the iterator yielded an episode {label} its known_at"
        );
    }
    let visible: Vec<&str> = memory
        .episodes(new_known.saturating_add(Duration::from_nanos(1)))
        .map(|e| e.episode_id.as_str())
        .collect();
    assert_eq!(visible, vec!["ep-old", "ep-new"]);
    // And before either was knowable, nothing at all.
    assert_eq!(memory.episodes(old_known).count(), 0);
}

#[test]
fn a_record_knowable_before_it_was_true_is_refused_not_corrected() {
    // The other half of the leakage guard: an episode whose `known_at`
    // precedes its `at` would be retrievable for a situation that had not
    // yet happened. Clamping `known_at` up to `at` would hide the caller bug.
    let mut memory = EpisodicMemory::new(16, 16).expect("non-zero bounds");
    let mut bad = episode("ep-bad", "obj-AAA", "quiet", start(), 10.0);
    bad.known_at = bad.at.saturating_sub(Duration::from_days(1));
    let refused = memory
        .remember(bad)
        .expect_err("a leaking record must be refused");
    assert!(
        refused
            .message()
            .contains("cannot be knowable before it was true"),
        "the refusal must name the leak: {}",
        refused.message()
    );
    assert!(memory.is_empty(), "a refused episode must not be kept");
}

#[test]
fn the_capacity_bound_evicts_the_oldest_known_episode_first() {
    // The failure: an unbounded working set. With capacity three and four
    // episodes remembered, the one known earliest — not the one inserted
    // first — must be gone, because age is a fact about the record, not
    // about the order a replay happened to feed it in.
    let mut memory = EpisodicMemory::new(3, 16).expect("non-zero bounds");
    let day = Duration::from_days(1);
    // Inserted out of known_at order on purpose.
    let order = [
        ("ep-day3", start().saturating_add(day * 3)),
        ("ep-day1", start().saturating_add(day)),
        ("ep-day4", start().saturating_add(day * 4)),
        ("ep-day2", start().saturating_add(day * 2)),
    ];
    for (id, at) in order {
        memory
            .remember(episode(id, "obj-AAA", "quiet", at, 50.0))
            .expect("valid");
    }
    assert_eq!(memory.capacity(), 3);
    assert_eq!(
        memory.len(),
        3,
        "the bound must hold after the fourth insert"
    );
    assert!(
        !memory.contains("ep-day1"),
        "the oldest-known episode survived eviction"
    );
    for kept in ["ep-day2", "ep-day3", "ep-day4"] {
        assert!(
            memory.contains(kept),
            "{kept} was evicted though newer ones exist"
        );
    }
    // And the evicted episode is gone from the index too, not only the
    // store: a dangling bucket entry would be recalled as nothing.
    let recall = memory.recall(
        &episode("q", "obj-AAA", "quiet", start(), 0.0).as_query(),
        Timestamp::MAX,
        10,
    );
    assert_eq!(recall.nearest.len(), 3);
    assert!(
        recall
            .nearest
            .iter()
            .all(|r| r.episode.episode_id != "ep-day1")
    );
}

#[test]
fn lsh_recall_returns_the_exact_nearest_of_the_candidates_and_never_more_than_the_bound() {
    // Two failures. First, an approximate index that returns a merely-close
    // episode ahead of an identical one, or hands back candidates in bucket
    // order without re-ranking: the twin of the query lands in the query's
    // own bucket, which is probed first, so it must come back first at
    // similarity one, and everything after it in exact cosine order. Second,
    // an index that walks the whole store on a query: `examined` must never
    // exceed the bound, however many episodes are eligible.
    let bound = 4;
    let mut memory = EpisodicMemory::new(64, bound).expect("non-zero bounds");
    let day = Duration::from_days(1);
    // Twenty near-duplicates of one situation, so the home bucket alone
    // holds more than the bound, and one exact twin among them. The
    // newest near-duplicate is the *least* similar and the oldest the most,
    // so an index that merely returned candidates newest-first — which is
    // the order they are gathered in — would be caught by the ranking
    // assertion below rather than passing by coincidence.
    for i in 0..20 {
        let at = start().saturating_add(day * i);
        let mut near = episode(&format!("ep-near-{i}"), "obj-AAA", "trending", at, 80.0);
        near.claim.confidence = 0.59 - 0.01 * f64::from(i as u8);
        memory.remember(near).expect("valid");
    }
    let twin_at = start().saturating_add(day * 30);
    let twin = episode("ep-twin", "obj-AAA", "trending", twin_at, 80.0);
    memory.remember(twin.clone()).expect("valid");
    assert_eq!(memory.len(), 21, "premise: every episode was kept");
    let twin_buckets = memory.buckets_of(&twin.embedding());
    let sharing = memory
        .episodes(Timestamp::MAX)
        .filter(|e| e.episode_id != "ep-twin")
        .filter(|e| memory.buckets_of(&e.embedding())[0] == twin_buckets[0])
        .count();
    assert!(
        sharing > bound,
        "premise: only {sharing} near-duplicates share the twin's home bucket; the bound of \
         {bound} would not bind"
    );

    let now = Timestamp::MAX;
    let recall = memory.recall(&twin.as_query(), now, 10);
    assert_eq!(
        recall.examined, bound,
        "premise: the bound must bind — examined {} against {bound}",
        recall.examined
    );
    assert!(
        recall.nearest.len() <= bound,
        "returned {} against a bound of {bound}",
        recall.nearest.len()
    );
    assert!(
        recall.nearest.iter().any(|r| r.similarity < 1.0 - 1e-6),
        "premise: every candidate is identical to the twin, so ranking has nothing to do"
    );
    assert_eq!(
        recall.nearest[0].episode.episode_id,
        "ep-twin",
        "the exact twin must rank first; got {:?}",
        recall
            .nearest
            .iter()
            .map(|r| (r.episode.episode_id.clone(), r.similarity))
            .collect::<Vec<_>>()
    );
    assert!((recall.nearest[0].similarity - 1.0).abs() < 1e-6);
    // Re-ranking is exact: similarities are non-increasing down the list,
    // and strictly so somewhere, since the candidates differ.
    for pair in recall.nearest.windows(2) {
        assert!(
            pair[0].similarity >= pair[1].similarity,
            "not re-ranked by exact cosine: {} before {}",
            pair[0].similarity,
            pair[1].similarity
        );
    }
    // The gathered order is newest first — twin, then near-19, 18, 17 —
    // and near-19 is the least similar of those, so a correct re-rank must
    // move it to the back.
    assert_eq!(
        recall.nearest.last().map(|r| r.episode.episode_id.as_str()),
        Some("ep-near-19"),
        "the least similar candidate must rank last: {:?}",
        recall
            .nearest
            .iter()
            .map(|r| (r.episode.episode_id.clone(), r.similarity))
            .collect::<Vec<_>>()
    );
    // And `k` still caps the answer below the bound.
    let two = memory.recall(&twin.as_query(), now, 2);
    assert_eq!(two.nearest.len(), 2);
}

#[test]
fn two_constructions_from_the_same_episodes_recall_identically() {
    // The failure: hyperplanes drawn from entropy, so a replayed process
    // buckets — and therefore recalls — differently from the live one. Both
    // the bucket assignment and the ranked answer must agree exactly.
    let build = || {
        let mut memory = EpisodicMemory::new(32, 8).expect("non-zero bounds");
        let day = Duration::from_days(1);
        for (i, (instrument, market)) in [
            ("obj-AAA", "trending"),
            ("obj-BBB", "quiet"),
            ("obj-AAA", "crisis"),
            ("obj-CCC", "mean_reverting"),
            ("obj-AAA", "trending"),
            ("obj-BBB", "trending"),
        ]
        .into_iter()
        .enumerate()
        {
            let at = start().saturating_add(day * i as i64);
            memory
                .remember(episode(
                    &format!("ep-{i}"),
                    instrument,
                    market,
                    at,
                    10.0 * i as f64,
                ))
                .expect("valid");
        }
        memory
    };
    let first = build();
    let second = build();
    assert_eq!(first.len(), 6, "premise: the fixture remembered six");

    let query = EpisodeQuery {
        instrument: "obj-AAA".to_string(),
        regime: regime("trending", "high"),
        state: None,
        causal_context: Vec::new(),
        claim: Some(claim("undervalued", 1.0, 0.5)),
        findings: None,
        stances: Vec::new(),
        horizon: Duration::from_days(3),
    };
    let embedding = query.embedding();
    assert_eq!(embedding.dimensions(), EPISODE_DIMENSIONS);
    assert_eq!(embedding.model, EPISODE_ENCODING);
    assert_eq!(
        first.buckets_of(&embedding),
        second.buckets_of(&embedding),
        "the index itself differs between constructions"
    );
    let a = first.recall(&query, Timestamp::MAX, 4);
    let b = second.recall(&query, Timestamp::MAX, 4);
    assert!(
        !a.nearest.is_empty(),
        "premise: the query recalls something"
    );
    assert_eq!(a, b, "two constructions recalled differently");
    for stored in first.episodes(Timestamp::MAX) {
        let (x, y) = (
            first.buckets_of(&stored.embedding()),
            second.buckets_of(&stored.embedding()),
        );
        assert_eq!(x, y, "{} bucketed differently", stored.episode_id);
    }
}

#[test]
fn the_precedent_digest_counts_agreement_only_over_resolved_signed_outcomes() {
    // The failure: a digest reporting zero agreement when nothing had
    // resolved, which reads as "precedent says no" when the truth is "no
    // precedent". Unresolved and zero-move episodes are excluded from the
    // denominator, and an empty denominator is `None`.
    let mut memory = EpisodicMemory::new(16, 16).expect("non-zero bounds");
    let day = Duration::from_days(1);
    memory
        .remember(episode("agree", "obj-AAA", "quiet", start(), 90.0))
        .expect("valid");
    memory
        .remember(episode(
            "disagree",
            "obj-AAA",
            "quiet",
            start().saturating_add(day),
            -40.0,
        ))
        .expect("valid");
    let mut flat = episode(
        "flat",
        "obj-AAA",
        "quiet",
        start().saturating_add(day * 2),
        0.0,
    );
    flat.outcome = None;
    memory.remember(flat).expect("valid");

    let recall = memory.recall(
        &episode("q", "obj-AAA", "quiet", start(), 0.0).as_query(),
        Timestamp::MAX,
        10,
    );
    assert_eq!(recall.nearest.len(), 3, "premise: all three recalled");
    let digest = PrecedentDigest::of(&recall.nearest, 1.0);
    assert_eq!(digest.nearest, 3);
    assert_eq!(digest.resolved, 2, "the unresolved episode must not count");
    assert_eq!(digest.agreeing, 1);
    assert_eq!(digest.agreement, Some(0.5));

    let none = PrecedentDigest::of(&[], 1.0);
    assert_eq!(none.agreement, None, "no precedent is not zero agreement");
    let directionless = PrecedentDigest::of(&recall.nearest, 0.0);
    assert_eq!(
        directionless.agreement, None,
        "a directionless claim cannot be agreed with"
    );
}

// --- §10.1: the state, the causal context and the surprise -------------------

/// A measured state: ratios that were formed, a book a tenth down, and a
/// window that ran up two hundred basis points.
fn measured_state() -> MarketState {
    MarketState {
        drawdown: 0.10,
        volatility_ratio: Some(1.8),
        spread_ratio: Some(1.2),
        recent_return_bps: Some(200.0),
        observations: 120,
    }
}

fn edge(cause: &str, transmission: f64, established: bool) -> CausalContextEdge {
    CausalContextEdge {
        cause: cause.to_string(),
        mechanism: "supply_chain".to_string(),
        transmission,
        established,
    }
}

#[test]
fn a_state_whose_ratio_is_not_a_real_number_is_refused_and_a_measured_one_is_admitted() {
    // The failure: a degenerate series divides by zero, the quotient is NaN,
    // and the encoder's bounded map sends NaN to 0.0 — which is exactly the
    // value an *unmeasured* state encodes as. A broken measurement would
    // reach the index labelled "nothing was measured", and no reader of the
    // record or of the vector could tell the two apart.
    let mut admitted = episode("ok", "obj-AAA", "quiet", start(), 10.0);
    admitted.state = Some(measured_state());
    assert!(
        admitted.validate().is_ok(),
        "premise: a state whose every figure was measured is admitted, so the refusals below \
         are about the value and not about the field existing"
    );

    for (label, bad) in [
        (
            "volatility_ratio",
            MarketState {
                volatility_ratio: Some(f64::NAN),
                ..measured_state()
            },
        ),
        (
            "spread_ratio",
            MarketState {
                spread_ratio: Some(-0.5),
                ..measured_state()
            },
        ),
    ] {
        let mut refused = episode("bad", "obj-AAA", "quiet", start(), 10.0);
        refused.state = Some(bad);
        let error = refused
            .validate()
            .expect_err("a ratio that is not a real number was admitted");
        assert!(
            error.message().contains(label),
            "the refusal must name the field that was wrong; got: {}",
            error.message()
        );
    }

    // A drawdown is a fraction of the high-water mark. One above 1 is a
    // reading from a broken capital state, not a very bad day.
    let mut deep = episode("deep", "obj-AAA", "quiet", start(), 10.0);
    deep.state = Some(MarketState {
        drawdown: 1.4,
        ..measured_state()
    });
    let error = deep
        .validate()
        .expect_err("a drawdown above one was admitted");
    assert!(
        error.message().contains("drawdown"),
        "got: {}",
        error.message()
    );

    // A ratio nobody could form is absent, and absent is admitted: the
    // platform is allowed to say it did not measure.
    let mut cold = episode("cold", "obj-AAA", "quiet", start(), 10.0);
    cold.state = Some(MarketState {
        volatility_ratio: None,
        spread_ratio: None,
        recent_return_bps: None,
        observations: 0,
        drawdown: 0.0,
    });
    assert!(
        cold.validate().is_ok(),
        "a cold start must be recordable; refusing it would force a caller to invent a ratio"
    );
}

#[test]
fn a_causal_edge_whose_transmission_is_outside_the_unit_interval_is_refused_and_one_inside_it_is_admitted()
 {
    // The failure, and it has already happened once in the world model: a
    // NaN strength reached `transmission`, `partial_cmp` answered `None`, and
    // every ordering that touched it silently stopped ordering. Here the same
    // number reaches the mean the encoder takes over these edges and makes
    // every episode in the store unorderable against every other.
    let mut admitted = episode("ok", "obj-AAA", "quiet", start(), 10.0);
    admitted.causal_context = vec![edge("obj-BBB", 0.42, true)];
    assert!(
        admitted.validate().is_ok(),
        "premise: an edge with a real transmission is admitted"
    );

    for bad in [f64::NAN, 1.5, -0.1] {
        let mut refused = episode("bad", "obj-AAA", "quiet", start(), 10.0);
        refused.causal_context = vec![edge("obj-BBB", bad, true)];
        let error = refused
            .validate()
            .expect_err("a transmission outside the unit interval was admitted");
        assert!(
            error.message().contains("transmission"),
            "got: {}",
            error.message()
        );
    }

    let mut unnamed = episode("unnamed", "obj-AAA", "quiet", start(), 10.0);
    unnamed.causal_context = vec![edge("  ", 0.5, true)];
    assert!(
        unnamed.validate().is_err(),
        "an edge from an unnamed cause is a bucket nobody can read"
    );
}

#[test]
fn the_surprise_is_the_gap_from_what_was_claimed_to_what_happened_and_is_absent_where_nothing_was_claimed()
 {
    // The failure: reporting a surprise of zero for a claim that never
    // stated a magnitude. Zero surprise reads as "we called it exactly",
    // which is the strongest possible statement about the platform's
    // judgement, and it would be made on the strength of no claim at all.
    let mut resolved = episode("ep", "obj-AAA", "quiet", start(), 120.0);
    assert_eq!(
        resolved.surprise_bps(),
        None,
        "premise: an outcome with no expectation states no surprise"
    );

    if let Some(outcome) = resolved.outcome.as_mut() {
        outcome.expected_move_bps = Some(50.0);
    }
    assert_eq!(
        resolved.surprise_bps(),
        Some(70.0),
        "the move overshot a 50bp claim by 70bp"
    );

    // Signed, because the sign is a different lesson: falling short of a
    // claim and blowing through it are not the same mistake.
    if let Some(outcome) = resolved.outcome.as_mut() {
        outcome.expected_move_bps = Some(300.0);
    }
    assert_eq!(
        resolved.surprise_bps(),
        Some(-180.0),
        "a move that fell short must report a negative surprise, not its magnitude"
    );

    let mut open = episode("open", "obj-AAA", "quiet", start(), 0.0);
    open.outcome = None;
    assert_eq!(
        open.surprise_bps(),
        None,
        "an unresolved episode cannot be surprising yet"
    );
}

#[test]
fn the_state_a_market_was_in_ranks_the_neighbours_and_never_moves_the_bucket_they_are_found_in() {
    // The failure this guards is not hypothetical and was found by the
    // kernel's own suite rather than reasoned out: bucketing on the state put
    // the same claim about the same name in a different bucket once the tape
    // had swung, the one-bit probe missed it, and memory answered "no
    // precedent" in exactly the situation precedent is asked for. The index
    // gathers on the question; the cosine ranks on the state.
    let quiet_state = MarketState {
        volatility_ratio: Some(0.9),
        recent_return_bps: Some(10.0),
        ..measured_state()
    };
    let violent_state = MarketState {
        volatility_ratio: Some(4.0),
        recent_return_bps: Some(1_500.0),
        ..measured_state()
    };

    let mut quiet = episode("quiet", "obj-AAA", "quiet", start(), 30.0);
    quiet.state = Some(quiet_state);
    let mut violent = episode(
        "violent",
        "obj-AAA",
        "quiet",
        start().saturating_add(Duration::from_days(1)),
        30.0,
    );
    violent.state = Some(violent_state);

    let mut memory = EpisodicMemory::new(16, 16).expect("non-zero bounds");
    let mut bare = episode("bare", "obj-AAA", "quiet", start(), 30.0);
    bare.state = None;
    bare.causal_context = Vec::new();
    let stateless = memory.buckets_of(&bare.embedding());
    assert_eq!(
        memory.buckets_of(&quiet.embedding()),
        stateless,
        "a state block moved the bucket; the index must gather on the question alone"
    );
    assert_eq!(
        memory.buckets_of(&violent.embedding()),
        stateless,
        "a violent state moved the bucket away from a quiet one's"
    );

    memory.remember(quiet).expect("valid");
    memory.remember(violent).expect("valid");

    // Ranking, though, is the state's business. A question asked on a
    // violent tape must bring the violent episode back first.
    let mut query = episode("q", "obj-AAA", "quiet", start(), 0.0).as_query();
    query.state = Some(violent_state);
    let recall = memory.recall(&query, Timestamp::MAX, 2);
    assert_eq!(recall.nearest.len(), 2, "premise: both were recalled");
    assert_eq!(
        recall.nearest[0].episode.episode_id, "violent",
        "the episode formed in the state being asked about did not rank first"
    );

    let mut calm_query = episode("q", "obj-AAA", "quiet", start(), 0.0).as_query();
    calm_query.state = Some(quiet_state);
    let calm = memory.recall(&calm_query, Timestamp::MAX, 2);
    assert_eq!(calm.nearest.len(), 2, "premise: both were recalled");
    assert_eq!(
        calm.nearest[0].episode.episode_id, "quiet",
        "the ranking did not follow the state at all, so it is not ranking on it"
    );
}

#[test]
fn the_precedent_digest_reports_the_worst_surprise_among_the_neighbours_and_none_where_none_is_gradeable()
 {
    // The failure: a mean over the neighbours. §10.2 calls high-surprise
    // moments the most informative and the rarest, and a mean over five
    // buries the one that was rare — the exact reading the statistic exists
    // to surface.
    let mut memory = EpisodicMemory::new(16, 16).expect("non-zero bounds");
    let day = Duration::from_days(1);
    for (id, at, realised, expected) in [
        ("small", start(), 60.0, Some(50.0)),
        ("large", start().saturating_add(day), 40.0, Some(900.0)),
        ("ungraded", start().saturating_add(day * 2), 30.0, None),
    ] {
        let mut entry = episode(id, "obj-AAA", "quiet", at, realised);
        if let Some(outcome) = entry.outcome.as_mut() {
            outcome.expected_move_bps = expected;
        }
        memory.remember(entry).expect("valid");
    }

    let recall = memory.recall(
        &episode("q", "obj-AAA", "quiet", start(), 0.0).as_query(),
        Timestamp::MAX,
        10,
    );
    assert_eq!(recall.nearest.len(), 3, "premise: all three recalled");
    let digest = PrecedentDigest::of(&recall.nearest, 1.0);
    assert_eq!(
        digest.surprising, 2,
        "the episode with no expectation must not be in the denominator"
    );
    assert_eq!(
        digest.worst_surprise_bps,
        Some(-860.0),
        "the worst surprise is the largest by magnitude, reported with its sign"
    );

    let nothing = PrecedentDigest::of(&[], 1.0);
    assert_eq!(nothing.surprising, 0);
    assert_eq!(
        nothing.worst_surprise_bps, None,
        "no precedent is not a surprise of zero"
    );
}
// ---------------------------------------------------------------------------
// Blueprint §54.2's episode sampling: dense at high surprise, sparse in calm,
// and bounded in both directions.
//
// These tests drive `EpisodicMemory` rather than `EpisodeSampler` directly
// wherever the property is about what the store retains, because the sampler
// answering correctly and the store spending the episode it named are two
// different facts and only the second one protects a recall.
// ---------------------------------------------------------------------------

/// An episode whose claim was written down in a gradeable form, so
/// `surprise_bps` is `realised - expected` and the sampler can grade it.
///
/// The fixture above deliberately leaves `expected_move_bps` at `None` —
/// every episode it builds is ungradeable and so grades `Calm`, which is why
/// the older capacity test still reads as pure oldest-first.
fn graded(
    id: &str,
    instrument: &str,
    at: Timestamp,
    realised_bps: f64,
    expected_bps: f64,
) -> Episode {
    let mut episode = episode(id, instrument, "quiet", at, realised_bps);
    episode.outcome = Some(EpisodeOutcome {
        resolved_at: at.saturating_add(Duration::from_days(1)),
        realised_move_bps: realised_bps,
        realised_pnl: 0.0,
        expected_move_bps: Some(expected_bps),
    });
    episode
}

#[test]
fn a_high_surprise_episode_outlives_a_full_capacity_of_calm_ones_that_arrive_after_it() {
    // The failure this prevents, and it is the whole point of §54.2: a memory
    // that evicts oldest-first spends the rare episode to make room for the
    // ordinary one, purely for being older. "Most information is in the
    // tails" is exactly the claim that trade is backwards.
    //
    // Capacity four, reserve two. The surprising episode is inserted first
    // and is therefore the oldest-known, so under the policy this replaced it
    // would be the very first thing evicted. Then a full capacity of calm
    // episodes arrives after it.
    let mut memory = EpisodicMemory::new(4, 16).expect("non-zero bounds");
    let day = Duration::from_days(1);
    let tail = graded("ep-tail", "obj-AAA", start(), 900.0, 40.0);
    let tail_surprise = tail.surprise_bps().expect("the fixture is gradeable");
    assert!(
        tail_surprise.abs() >= HIGH_SURPRISE_BPS,
        "premise: the fixture must actually be a tail episode, not merely \
         intended as one - it surprised by {tail_surprise} bps against a \
         threshold of {HIGH_SURPRISE_BPS}"
    );
    let tail_known_at = tail.known_at;
    memory.remember(tail).expect("valid");

    for n in 1..=8u32 {
        let at = start().saturating_add(day * i64::from(n));
        let calm = graded(&format!("ep-calm-{n}"), "obj-AAA", at, 45.0, 40.0);
        assert!(
            calm.known_at > tail_known_at,
            "premise: every calm episode must be newer than the tail one, or \
             oldest-first would have kept the tail one anyway and this test \
             would prove nothing"
        );
        assert!(
            calm.surprise_bps()
                .is_some_and(|s| s.abs() < HIGH_SURPRISE_BPS),
            "premise: the calm fixture must grade calm"
        );
        memory.remember(calm).expect("valid");
    }

    assert_eq!(memory.len(), 4, "the capacity bound must still hold");
    assert!(
        memory.contains("ep-tail"),
        "the surprising episode was evicted by eight ordinary ones that \
         arrived after it; oldest-first is exactly what §54.2 refuses"
    );
    // Sparse in calm: what was given up is the ordinary history, oldest
    // first, and the most recent calm episodes are what remain.
    for spent in [
        "ep-calm-1",
        "ep-calm-2",
        "ep-calm-3",
        "ep-calm-4",
        "ep-calm-5",
    ] {
        assert!(
            !memory.contains(spent),
            "{spent} survived though five newer calm episodes exist"
        );
    }
    for kept in ["ep-calm-6", "ep-calm-7", "ep-calm-8"] {
        assert!(
            memory.contains(kept),
            "{kept} was spent before an older one"
        );
    }
    assert_eq!(
        memory.held_at(EpisodeGrade::Tail),
        1,
        "one tail episode was remembered and one must be held"
    );
    // And the index agrees with the store: a dangling bucket entry would
    // recall an episode the store no longer has.
    let recall = memory.recall(
        &graded("q", "obj-AAA", start(), 0.0, 0.0).as_query(),
        Timestamp::MAX,
        10,
    );
    assert_eq!(recall.nearest.len(), 4);
    assert!(
        recall
            .nearest
            .iter()
            .any(|r| r.episode.episode_id == "ep-tail"),
        "the tail episode is in the store but unreachable through the index"
    );
}

#[test]
fn a_stream_of_nothing_but_surprises_never_grows_the_memory_past_its_capacity() {
    // The failure: "protect the tail" written without a second bound. Every
    // episode is eventually a tail episode of something, so a rule that
    // declines to evict a surprising episode is an episodic memory that grows
    // with the stream - the defect the whole capacity exists to prevent, and
    // one that would read as the sampler working.
    let mut memory = EpisodicMemory::new(4, 16).expect("non-zero bounds");
    let day = Duration::from_days(1);
    for n in 1..=32u32 {
        let at = start().saturating_add(day * i64::from(n));
        let episode = graded(&format!("ep-{n}"), "obj-AAA", at, 900.0, 40.0);
        assert!(
            episode
                .surprise_bps()
                .is_some_and(|s| s.abs() >= HIGH_SURPRISE_BPS),
            "premise: every episode in this stream must grade tail, or the \
             test is not exercising the tail bound at all"
        );
        memory.remember(episode).expect("valid");
        assert!(
            memory.len() <= memory.capacity(),
            "the memory held {} episodes against a capacity of {} after {n} \
             inserts",
            memory.len(),
            memory.capacity()
        );
    }
    assert_eq!(memory.len(), 4, "the bound must bind, not merely not break");
    for kept in ["ep-29", "ep-30", "ep-31", "ep-32"] {
        assert!(
            memory.contains(kept),
            "{kept} was spent though older tail episodes exist; over its \
             reserve the tail is ordinary and goes oldest-first"
        );
    }
    assert!(
        !memory.contains("ep-1"),
        "the oldest tail episode survived thirty-one newer ones"
    );
}

#[test]
fn a_stream_carrying_both_grades_settles_at_exactly_the_reserved_number_of_surprises() {
    // The failure on the other side: a reserve that is a share of the store
    // rather than a seat count would let the tail crowd out the calm until a
    // recall on an ordinary morning returned only disasters. The steady state
    // is the assertion - `reserve` tail seats, the rest calm - and it is
    // reached from a stream that offers far more tail episodes than seats.
    let mut memory = EpisodicMemory::new(8, 16).expect("non-zero bounds");
    let reserve = memory.sampler().reserve(memory.capacity());
    assert_eq!(reserve, 4, "premise: capacity eight over a divisor of two");
    let day = Duration::from_days(1);
    for n in 1..=40u32 {
        let at = start().saturating_add(day * i64::from(n));
        // Alternating, so twenty of each are offered to eight seats.
        let episode = if n % 2 == 0 {
            graded(&format!("ep-tail-{n}"), "obj-AAA", at, 900.0, 40.0)
        } else {
            graded(&format!("ep-calm-{n}"), "obj-AAA", at, 45.0, 40.0)
        };
        memory.remember(episode).expect("valid");
    }
    assert_eq!(memory.len(), 8, "the capacity bound must hold");
    assert_eq!(
        memory.held_at(EpisodeGrade::Tail),
        reserve,
        "the tail took more than its reserved seats from a stream that \
         offered twenty surprises to four seats"
    );
    assert_eq!(
        memory.held_at(EpisodeGrade::Calm),
        memory.capacity() - reserve,
        "the calm half of the memory is not what is left over by accident; \
         it is the other side of the same bound"
    );
}

#[test]
fn two_memories_fed_the_same_episodes_retain_the_same_set() {
    // The failure: a sample whose contents depend on something the event log
    // does not hold. A memory retaining a different set on a replay makes
    // every recall irreproducible, and nothing downstream could tell that
    // from a genuine difference in the episodes. The sampler reads only
    // `realised_move_bps` and `expected_move_bps` and makes no random choice,
    // so the only way this can fail is an unordered index inside the store -
    // which is why the grades are held in `BTreeSet`s.
    let day = Duration::from_days(1);
    let feed = |memory: &mut EpisodicMemory| {
        for n in 1..=40u32 {
            let at = start().saturating_add(day * i64::from(n));
            let next = match n % 3 {
                0 => graded(&format!("ep-{n}"), "obj-AAA", at, 900.0, 40.0),
                1 => graded(&format!("ep-{n}"), "obj-BBB", at, 45.0, 40.0),
                _ => episode(&format!("ep-{n}"), "obj-CCC", "quiet", at, 50.0),
            };
            memory.remember(next).expect("valid");
        }
    };
    let mut first = EpisodicMemory::new(9, 16).expect("non-zero bounds");
    let mut second = EpisodicMemory::new(9, 16).expect("non-zero bounds");
    feed(&mut first);
    feed(&mut second);

    let ids = |memory: &EpisodicMemory| -> Vec<String> {
        memory
            .episodes(Timestamp::MAX)
            .map(|episode| episode.episode_id.clone())
            .collect()
    };
    let retained = ids(&first);
    assert_eq!(
        retained.len(),
        9,
        "premise: the bound must have bound, or two empty memories would \
         agree and prove nothing"
    );
    assert!(
        retained.iter().any(|id| id != &retained[0]),
        "premise: the retained set must hold more than one episode"
    );
    assert_eq!(
        retained,
        ids(&second),
        "two memories fed identical episodes retained different sets, in the \
         same order - the sample depends on something outside the record"
    );
    assert_eq!(
        first.held_at(EpisodeGrade::Tail),
        second.held_at(EpisodeGrade::Tail),
        "the two memories disagree on how much of the sample is tail"
    );
}

#[test]
fn an_episode_whose_claim_named_no_expectation_is_calm_rather_than_surprising() {
    // The failure: treating an ungradeable outcome as a large surprise. A
    // `RegimeShift` names no direction and so carries no `expected_move_bps`;
    // an absence is not a magnitude, and reserving a seat for one would fill
    // the tail with records that hold no tail - a reserve that reads as
    // protecting the rare and is full of the unmeasurable.
    let sampler = EpisodeSampler::default();
    let ungradeable = episode("ep-none", "obj-AAA", "quiet", start(), 5_000.0);
    assert!(
        ungradeable.surprise_bps().is_none(),
        "premise: the fixture must carry no expectation to be surprised \
         against, despite a realised move far past the threshold"
    );
    assert_eq!(
        sampler.grade(&ungradeable),
        EpisodeGrade::Calm,
        "an outcome nobody could grade for surprise took a reserved seat"
    );

    let unresolved = {
        let mut open = episode("ep-open", "obj-AAA", "quiet", start(), 0.0);
        open.outcome = None;
        open
    };
    assert_eq!(
        sampler.grade(&unresolved),
        EpisodeGrade::Calm,
        "an episode with no outcome at all took a reserved seat"
    );

    // And the boundary is inclusive on the threshold itself, which is the
    // arm the `>=` in `grade` is there for.
    assert_eq!(
        sampler.grade(&graded("ep-at", "obj-AAA", start(), 140.0, 40.0)),
        EpisodeGrade::Tail,
        "an episode exactly at the threshold graded calm"
    );
    assert_eq!(
        sampler.grade(&graded("ep-under", "obj-AAA", start(), 139.9, 40.0)),
        EpisodeGrade::Calm,
        "an episode below the threshold graded tail"
    );
    // Signed on the record, compared on magnitude: falling a per cent short
    // of a claim is as informative as overshooting it by the same.
    assert_eq!(
        sampler.grade(&graded("ep-short", "obj-AAA", start(), -60.0, 40.0)),
        EpisodeGrade::Tail,
        "a surprise to the downside was not graded on its magnitude"
    );
}

#[test]
fn a_sampler_given_a_threshold_or_a_reserve_that_means_nothing_refuses_rather_than_corrects() {
    // The failure: a silently corrected sampling policy. A platform running
    // every cycle on a policy nobody chose, with no complaint, is the clamping
    // this workspace refuses - and here it would quietly change what the
    // memory remembers for the life of the process.
    for bad in [f64::NAN, f64::INFINITY, -1.0] {
        let refused = EpisodeSampler::new(bad, TAIL_RESERVE_DIVISOR);
        assert!(
            refused.is_err(),
            "a high-surprise threshold of {bad} was accepted"
        );
    }
    let refused = EpisodeSampler::new(HIGH_SURPRISE_BPS, 0)
        .expect_err("a reserve divisor of zero must be refused");
    assert!(
        refused.message().contains("divisor of zero"),
        "the refusal must name what is wrong: {}",
        refused.message()
    );
    // And a good value is admitted - a gate that refuses everything is not a
    // gate.
    let admitted = EpisodeSampler::new(250.0, 4).expect("a stated policy is admitted");
    assert_eq!(admitted.high_surprise_bps(), 250.0);
    assert_eq!(admitted.reserve(4_096), 1_024);
}

#[test]
fn a_memory_given_a_stated_sampler_uses_it_and_not_the_default() {
    // The failure: a policy parameter nothing reads. `with_sampler` that set
    // a field the store never consulted would read as configurable sampling
    // and behave as the default forever.
    let mut memory = EpisodicMemory::new(4, 16)
        .expect("non-zero bounds")
        // Reserve zero: no episode is ever held past the point recency would
        // have spent it, which is precisely the oldest-first policy the
        // sampler replaced.
        .with_sampler(EpisodeSampler::new(HIGH_SURPRISE_BPS, 8).expect("a stated policy"));
    assert_eq!(
        memory.sampler().reserve(memory.capacity()),
        0,
        "premise: four over eight is no reserved seat at all"
    );
    let day = Duration::from_days(1);
    memory
        .remember(graded("ep-tail", "obj-AAA", start(), 900.0, 40.0))
        .expect("valid");
    for n in 1..=8u32 {
        let at = start().saturating_add(day * i64::from(n));
        memory
            .remember(graded(&format!("ep-calm-{n}"), "obj-AAA", at, 45.0, 40.0))
            .expect("valid");
    }
    assert_eq!(memory.len(), 4);
    assert!(
        !memory.contains("ep-tail"),
        "the stated sampler reserved no seat, so the oldest episode must have \
         been spent first - the default's reserve was used instead"
    );
}
