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
    AnalystStance, ClaimRecord, DecisionTaken, EPISODE_DIMENSIONS, EPISODE_ENCODING, Episode,
    EpisodeOutcome, EpisodeQuery, EpisodicMemory, FindingsSummary, PrecedentDigest, RegimeLabel,
    StanceDirection,
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
