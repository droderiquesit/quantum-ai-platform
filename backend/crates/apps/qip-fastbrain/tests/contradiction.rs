//! R12-WORLD-1181 on the committed demonstration tape, through the fast
//! brain's own step and `PlatformConfig::default()`: what the platform does
//! when two world models contradict each other, and whether the margin that
//! decides what counts as a contradiction separates anything on the feed a
//! deployment would read.
//!
//! Two failures this file exists to stop. A contradiction was journaled on
//! every pass and nothing read it, so a thesis on an instrument two members
//! called in opposite directions was admitted at whatever confidence the
//! panel reached, as if the federation had agreed. And `CONTRADICTION_MARGIN`
//! was a chosen 0.1 nobody had measured against a feed: a margin that cut
//! through the middle of the opposed pairs a deployment sees would make the
//! record depend on noise either side of it.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::{Clock, Timestamp};
use qip_fastbrain::feed::Feed;
use qip_fastbrain::node;
use qip_fastbrain::roster::MAXIMUM_BUDGET;
use qip_kernel::federation_stage::{self, CONTRADICTED_CONFIDENCE_CAP, CONTRADICTION_MARGIN};
use qip_kernel::{Platform, PlatformConfig, Stage};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::path::PathBuf;
use std::sync::Arc;

const PERIODS: usize = 600;

fn tape_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../data/datasets/loop-demonstration-tape.json")
}

/// The tape run to its end the way `node::run` runs it, with every REASON
/// detail the cycle reported.
fn run_the_tape() -> (Platform, Vec<String>) {
    let mut feed = Feed::tape(&tape_path().display().to_string()).expect("the tape feed opens");
    let tape_clock = feed.owned_clock().expect("a tape owns its clock");
    let config = PlatformConfig::default();
    let clock: Arc<dyn Clock> = tape_clock;
    let context = qip_core::Context::new(clock, config.seed);
    let catalogue =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../data/datasets/universe.json");
    let text = std::fs::read_to_string(&catalogue).expect("the committed catalogue reads");
    let universe = qip_financial::catalogue::load(&text, qip_core::SystemClock.now())
        .expect("the committed catalogue loads")
        .universe;
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
    .expect("the platform assembles");
    let wall = qip_core::SystemClock;
    let mut details = Vec::new();
    while !feed.is_exhausted() {
        let now: Timestamp = feed
            .cycle_instant(&wall)
            .expect("an unexhausted tape has a next period");
        let outcome =
            node::step(&mut platform, &mut feed, now, MAXIMUM_BUDGET).expect("a step runs");
        if let Some(reason) = outcome.report.stage(Stage::Reason) {
            details.push(reason.detail.clone());
        }
    }
    (platform, details)
}

/// The number printed after `marker` in `detail`, if the marker is there.
fn number_after(detail: &str, marker: &str) -> Option<f64> {
    let rest = detail.split(marker).nth(1)?;
    rest.chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .parse()
        .ok()
}

/// The measurement `CONTRADICTION_MARGIN`'s doc cites. For every arbitration
/// the tape's cycles journaled, the opposition depth of its views: how far
/// from even odds the nearer of the most-for and most-against views sits,
/// where the views straddle even odds at all. A margin separates
/// disagreement from noise on this feed when no opposed pair sits at a
/// depth inside the margin and every opposed pair past it is the one the
/// review recorded; the histogram is printed so the distribution is on the
/// record, not only the verdict.
///
/// Measured 2026-11-28 on this tape: 600 reviews, 5,324 arbitrations, 2,320
/// of them with opposed views, every one at a depth of at least 0.2 (one in
/// [0.200, 0.225), the rest from 0.25 up, 1,421 at 0.5), and 2,320
/// contradictions recorded. Nothing sits in [0, 0.2), so any margin up to
/// 0.2 records the same contradictions, and 0.1 sits in the middle of an
/// empty band rather than on the edge of a populated one.
///
/// Mutation: `contradictions_in` records nothing -- the recorded count is
/// zero against 2,320 opposed pairs past the margin and the equality fails.
#[test]
fn the_contradiction_margin_sits_in_an_empty_band_of_the_shipped_feeds_opposed_views() {
    let (platform, _) = run_the_tape();
    let reviews = federation_stage::replay(&platform).expect("every review replays");
    assert_eq!(
        reviews.len(),
        PERIODS,
        "premise: one review per tape period"
    );
    let mut depths: Vec<f64> = Vec::new();
    let mut recorded = 0usize;
    for review in &reviews {
        recorded += review.contradictions.len();
        for a in &review.arbitrations {
            let views = &a.disagreement.views;
            let most_for = views
                .iter()
                .map(|v| v.probability)
                .fold(f64::NEG_INFINITY, f64::max);
            let most_against = views
                .iter()
                .map(|v| v.probability)
                .fold(f64::INFINITY, f64::min);
            if most_for > 0.5 && most_against < 0.5 {
                depths.push((most_for - 0.5).min(0.5 - most_against));
            }
        }
    }
    let mut histogram = [0usize; 21];
    for depth in &depths {
        histogram[((depth / 0.025) as usize).min(20)] += 1;
    }
    for (bucket, count) in histogram.iter().enumerate() {
        eprintln!(
            "opposition depth [{:.3}, {:.3}): {count}",
            bucket as f64 * 0.025,
            (bucket + 1) as f64 * 0.025
        );
    }
    // Premise: the feed holds opposed views at all, or the band is empty
    // because there is nothing in it to measure.
    assert!(
        depths.len() > 100,
        "premise: only {} opposed pair(s) on the tape",
        depths.len()
    );
    let inside: Vec<f64> = depths
        .iter()
        .copied()
        .filter(|d| *d < 2.0 * CONTRADICTION_MARGIN)
        .collect();
    assert!(
        inside.is_empty(),
        "{} opposed pair(s) sit within twice the margin of even odds ({:?}); 0.1 no longer sits \
         in an empty band on this feed and must be re-measured",
        inside.len(),
        inside.iter().take(5).collect::<Vec<_>>()
    );
    let past = depths
        .iter()
        .filter(|d| **d >= CONTRADICTION_MARGIN)
        .count();
    assert_eq!(
        recorded, past,
        "every opposed pair past the margin is a recorded contradiction, and nothing else is"
    );
}

/// The reaction, on the feed a deployment reads: a thesis on an instrument
/// the same cycle's federation recorded a contradiction on is held to at
/// most even odds, and the review that recorded the contradiction journaled
/// the request naming both members.
///
/// Mutation: drop the `min(CONTRADICTED_CONFIDENCE_CAP)` in REASON -- a
/// thesis the panel reached above even odds reports its confidence above the
/// cap and the per-thesis assertion fails.
#[test]
fn a_thesis_on_a_contradicted_instrument_is_held_to_even_odds_on_the_shipped_tape() {
    let (platform, details) = run_the_tape();
    let marker = "; world models contradict on ";
    let contradicted: Vec<&String> = details.iter().filter(|d| d.contains(marker)).collect();
    // Premise: theses were formed on contradicted instruments on this feed.
    assert!(
        !contradicted.is_empty(),
        "premise: no thesis on the tape was formed on a contradicted instrument"
    );
    // Premise: the cap binds on this feed, not only in principle. Without a
    // thesis the panel reached above the cap, holding it there changes
    // nothing and the assertion below could not fail.
    let bound = contradicted
        .iter()
        .filter(|d| {
            number_after(d, ", confidence ").is_some_and(|held| held > CONTRADICTED_CONFIDENCE_CAP)
        })
        .count();
    assert!(
        bound >= 1,
        "premise: no contradicted thesis on the tape was reached above {CONTRADICTED_CONFIDENCE_CAP}"
    );
    for detail in &contradicted {
        let reported = number_after(detail, " at confidence ")
            .unwrap_or_else(|| panic!("a formed thesis reports its confidence: {detail}"));
        assert!(
            reported <= CONTRADICTED_CONFIDENCE_CAP,
            "a thesis on a contradicted instrument was admitted at {reported}: {detail}"
        );
    }
    let reviews = federation_stage::replay(&platform).expect("every review replays");
    let with_requests = reviews
        .iter()
        .filter(|r| !r.contradictions.is_empty())
        .inspect(|r| {
            assert_eq!(r.contradiction_requests.len(), r.contradictions.len());
            for (c, asked) in r.contradictions.iter().zip(&r.contradiction_requests) {
                assert_eq!(asked.proposition, c.proposition);
                assert_eq!(
                    asked.models,
                    vec![c.for_model.clone(), c.against_model.clone()]
                );
            }
        })
        .count();
    assert!(
        with_requests > 0,
        "premise: no review recorded a contradiction"
    );
}
