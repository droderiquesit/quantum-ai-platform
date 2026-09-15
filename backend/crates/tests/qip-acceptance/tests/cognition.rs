//! Cross-cutting assertions about the cognition stack — blueprint §8.2's
//! graph queries and §9.1–§9.4's causal layer.
//!
//! These live here rather than in any one crate's tests because each spans a
//! seam no single crate can see both sides of: the statistics in
//! `qip-numerics`, the establishment method and the graph in
//! `qip-world-model`, and the review pass in `qip-kernel`.
//!
//! # The claim this suite exists to keep honest
//!
//! `Platform::discover_temporal_precedence` is a real production writer of
//! the causal graph, and it runs an **uncontrolled** lead-lag test. §9.2's
//! method is "Granger-style lead-lag *with controls* — temporal precedence
//! with confounders explicitly adjusted", and the difference is not
//! cosmetic: a single persistent driver shared by a book manufactures an
//! edge between very many of the pairs it touches, each significant, each
//! spurious, and all of them spurious together. The end-to-end test below
//! walks that from ingested-shaped price history to the edges the platform
//! would write to the finding a control produces, so that "the graph has a
//! writer" is never read as "the graph is trustworthy".

use std::collections::{BTreeMap, BTreeSet};

use qip_core::{Duration, Timestamp};
use qip_kernel::causal_review::{self, MIN_FACTOR_CONSTITUENTS};
use qip_world_model::causal::{CausalEdge, CausalGraph, EdgeStanding, Mechanism};
use qip_world_model::confounder::{Confounder, ConfounderSet};
use qip_world_model::exposure;
use qip_world_model::granger;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

fn stream(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((state >> 33) as f64 / (1u64 << 31) as f64) - 0.5
    }
}

/// A book of instruments moved by one persistent common driver and nothing
/// else — no lagged relationship between any two of them exists in the
/// generating process.
///
/// Shaped like `Platform::price_history`: a `BTreeMap` from instrument id to
/// a series, which is what the production writer scans.
fn book_driven_by_one_factor(names: &[&str], bars: usize) -> BTreeMap<String, Vec<f64>> {
    let mut noise = stream(0x2545_F491_4F6C_DD1D);
    let mut driver = Vec::with_capacity(bars);
    let mut level = 0.0;
    for _ in 0..bars {
        level = 0.8 * level + noise();
        driver.push(level);
    }
    let mut book = BTreeMap::new();
    for name in names {
        let series: Vec<f64> = (0..bars).map(|t| driver[t] + 0.6 * noise()).collect();
        book.insert((*name).to_string(), series);
    }
    book
}

/// Exactly what `Platform::discover_temporal_precedence` does: scan ordered
/// pairs and write every edge that clears the uncontrolled bar.
///
/// Reproduced here rather than called, because the production method is a
/// private method on `Platform` and the point of the test is what the
/// *method* produces, not what one struct's private function does.
fn discover_uncontrolled(book: &BTreeMap<String, Vec<f64>>, now: Timestamp) -> CausalGraph {
    let mut graph = CausalGraph::new();
    let subjects: Vec<&String> = book.keys().collect();
    for cause in &subjects {
        for effect in &subjects {
            if cause == effect {
                continue;
            }
            if let Ok(Some(edge)) = granger::establish_temporal_precedence(
                cause,
                &book[*cause],
                effect,
                &book[*effect],
                Duration::from_days(1),
                now,
            ) {
                graph.add(edge);
            }
        }
    }
    graph
}

#[test]
fn an_uncontrolled_pairwise_scan_of_a_book_with_one_common_driver_writes_edges_a_control_removes() {
    let names = ["AAA", "BBB", "CCC", "DDD", "EEE", "FFF", "GGG", "HHH"];
    let book = book_driven_by_one_factor(&names, 400);
    let graph = discover_uncontrolled(&book, at(1_000));

    // The premise, and the finding at once. If this were zero the audit
    // below would be auditing an empty graph, and an empty `unsupported`
    // list would mean nothing — the control-that-cannot-fire shape.
    assert!(
        !graph.is_empty(),
        "the premise: an uncontrolled scan of a book driven by one factor writes edges. \
         Nothing in the generating process links any pair, so every edge here is spurious."
    );

    let audit = causal_review::audit_controls(&graph, &book, Duration::from_days(1), at(2_000));
    assert!(
        audit.was_answerable(),
        "the audit must have judged something; it examined {} and audited {}",
        audit.edges_examined,
        audit.edges_audited
    );
    assert!(
        !audit.unsupported.is_empty(),
        "edges that are nothing but a shared driver must not survive a control over the rest \
         of the book; {} edge(s) audited, none found unsupported",
        audit.edges_audited
    );
    // The explanation an operator reads must name both ends, so the finding
    // is actionable rather than a count.
    let explained = audit.unsupported[0].explain();
    assert!(
        explained.contains(&audit.unsupported[0].cause)
            && explained.contains(&audit.unsupported[0].effect),
        "the finding must name the edge it questions; got: {explained}"
    );
    assert!(
        audit.summary().is_some(),
        "and an answerable audit offers a line for the stage report"
    );
}

#[test]
fn a_book_the_platform_cannot_build_a_factor_from_yields_no_finding_and_says_so() {
    // Two instruments: excluding the pair leaves nothing to average, so no
    // edge in this graph can be judged at all.
    let book = book_driven_by_one_factor(&["AAA", "BBB"], 400);
    let graph = discover_uncontrolled(&book, at(1_000));
    assert!(
        !graph.is_empty(),
        "the premise: the uncontrolled scan still writes edges in a two-name book"
    );

    let audit = causal_review::audit_controls(&graph, &book, Duration::from_days(1), at(2_000));
    assert!(
        audit.edges_examined > 0,
        "the premise: the edges were in scope"
    );
    assert_eq!(audit.edges_audited, 0, "none could be judged");
    assert!(audit.unsupported.is_empty());

    // The distinction that makes this a control rather than decoration: an
    // audit that judged nothing must not read as an audit that found
    // nothing wrong. This is the `MaxExpectedShortfall` failure shape —
    // a control that reads as protection and cannot fire — caught by type
    // rather than by a reader noticing.
    assert!(
        !audit.was_answerable(),
        "an audit with a universe too thin to control against is not evidence of a clean graph"
    );
    assert!(
        audit.summary().is_none(),
        "and it must write no reassuring line into a stage report"
    );
    assert!(
        audit.unauditable > 0,
        "and it must say how many it could not judge"
    );
}

#[test]
fn a_hidden_concentration_over_a_graph_the_platform_wrote_is_found_and_names_its_positions() {
    let names = ["AAA", "BBB", "CCC", "DDD", "EEE", "FFF"];
    let book = book_driven_by_one_factor(&names, 400);
    let graph = discover_uncontrolled(&book, at(1_000));
    assert!(
        !graph.is_empty(),
        "the premise: the graph the platform wrote is not empty"
    );

    // §8.2 asks which held positions share an exposure the desk has *not*
    // counted, so the book must hold the positions and not the driver. Take
    // a driver the platform's own graph gives two or more effects for, hold
    // those effects, and leave the driver out.
    //
    // Built from the graph rather than hand-picked: the point of the test is
    // that the query fires on edges the establishment method wrote, not on a
    // shape the test arranged. An earlier version filtered to effects that
    // are never a cause, which is empty in a dense pairwise scan — the
    // premise assertion below caught it, which is why it is here.
    let mut by_cause: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in graph.edges() {
        by_cause
            .entry(edge.cause.clone())
            .or_default()
            .insert(edge.effect.clone());
    }
    let (driver, effects) = by_cause
        .iter()
        .find(|(_, effects)| effects.len() >= 2)
        .expect("the premise: some driver in the written graph reaches two names");
    let held: BTreeSet<String> = effects.clone();
    assert!(
        !held.contains(driver),
        "the premise: the driver is not one of its own effects"
    );

    let report = causal_review::concentration(&graph, &held, at(2_000));
    // Premise first: the question was answerable. Without this an empty
    // `drivers` list would be indistinguishable from an empty book.
    assert!(
        report.was_answerable(),
        "the premise: {} position(s) and {} edge(s) were examined",
        report.positions_examined,
        report.edges_considered
    );
    assert!(
        !report.is_clean(),
        "a driver reaching {} held positions, itself unheld, is exactly §8.2's hidden \
         concentration and must be surfaced",
        held.len()
    );
    let finding = &report.drivers[0];
    assert!(
        finding.positions.len() >= 2,
        "a concentration names at least two positions, or it is a position"
    );
    assert!(
        !held.contains(&finding.driver),
        "a driver the desk holds is counted exposure, not hidden concentration"
    );
}

#[test]
fn an_unobserved_confounder_cannot_reach_a_regression_across_the_crate_boundary() {
    // The structural half of §9.4's handling: recording a confounder the
    // platform cannot measure must never read as having adjusted for it.
    // Proven here rather than only in the world model's own tests because
    // the regression lives in a different crate, and a type that leaked an
    // unobserved confounder would leak it across exactly this seam.
    let set = ConfounderSet::new()
        .with(Confounder::unobserved("crowding", "positioning nobody can see").unwrap())
        .unwrap();
    assert_eq!(set.len(), 1, "the premise: the set is not empty");
    assert!(
        set.observed_series().is_empty(),
        "an unobserved confounder offers the regression nothing, by type rather than by filter"
    );

    let mut noise = stream(29);
    let cause: Vec<f64> = (0..200).map(|_| noise()).collect();
    let effect: Vec<f64> = (0..200)
        .map(|t| {
            if t == 0 {
                noise()
            } else {
                0.7 * cause[t - 1] + 0.3 * noise()
            }
        })
        .collect();

    let edge = granger::establish_temporal_precedence_controlling_for(
        "CAUSE",
        &cause,
        "EFFECT",
        &effect,
        &set,
        Duration::from_days(1),
        at(1_000),
    )
    .expect("well-formed series")
    .expect("the premise: this pair clears the bar");

    assert_eq!(
        edge.standing(),
        EdgeStanding::Suggestive,
        "§9.4: a plausible unobserved confounder makes the edge suggestive, not established"
    );
    assert!(
        edge.adjusted_for.is_empty(),
        "and it must never be recorded as something that was adjusted for"
    );
}

#[test]
fn the_causal_graph_still_has_a_production_writer_that_reaches_ingested_bars() {
    // A regression guard on ADR 0054's contribution rather than a new
    // claim. The graph's own queries — propagation, explanation, and the
    // concentration query this suite adds — are each a control that cannot
    // fire over an empty graph, so the establishment method being reachable
    // from ordinary return history is a precondition of all of them.
    let book = book_driven_by_one_factor(&["AAA", "BBB", "CCC", "DDD", "EEE"], 400);
    let subjects: Vec<&String> = book.keys().collect();
    let wrote = subjects.iter().any(|cause| {
        subjects.iter().any(|effect| {
            cause != effect
                && matches!(
                    granger::establish_temporal_precedence(
                        cause,
                        &book[*cause],
                        effect,
                        &book[*effect],
                        Duration::from_days(1),
                        at(1_000),
                    ),
                    Ok(Some(_))
                )
        })
    });
    assert!(
        wrote,
        "the establishment method the UNDERSTAND stage calls must be able to write an edge \
         from plain return history; if it cannot, every graph query is a control that \
         cannot fire"
    );
}

#[test]
fn the_factor_floor_admits_the_smallest_real_universe_and_refuses_a_thinner_one() {
    // A gate that refuses everything is not a working gate — both halves
    // asserted, because only the second distinguishes the two.
    let big = book_driven_by_one_factor(&["AAA", "BBB", "CCC", "DDD", "EEE"], 200);
    let mut graph = CausalGraph::new();
    graph.add(
        CausalEdge::new(
            "AAA",
            "BBB",
            Mechanism::TemporalPrecedence,
            0.3,
            Duration::from_days(1),
            at(1_000),
        )
        .expect("a strength in [0, 1] is admitted"),
    );
    let wide = causal_review::audit_controls(&graph, &big, Duration::from_days(1), at(2_000));
    assert_eq!(
        wide.edges_audited, 1,
        "five names leave {MIN_FACTOR_CONSTITUENTS} constituents after the pair, which is \
         enough to judge the edge"
    );

    let thin = book_driven_by_one_factor(&["AAA", "BBB", "CCC", "DDD"], 200);
    let narrow = causal_review::audit_controls(&graph, &thin, Duration::from_days(1), at(2_000));
    assert_eq!(
        narrow.edges_audited, 0,
        "four names leave two, which is not an average and must not be used as one"
    );
    assert_eq!(narrow.unauditable, 1);
}

#[test]
fn every_new_cognition_surface_is_read_only_and_names_no_venue_or_order() {
    // The paper-trading boundary, restated where this lane could have
    // weakened it. None of these types can name a venue, an order, or a
    // side; the causal layer constrains sizing and explanation and never
    // generates a trade (§9.4's fourth handling). This is a structural
    // assertion rather than a grep: the functions below return findings, and
    // there is no constructor anywhere in them that produces an order.
    let mut graph = CausalGraph::new();
    graph.add(
        CausalEdge::new(
            "DRIVER",
            "AAA",
            Mechanism::TemporalPrecedence,
            0.5,
            Duration::from_days(1),
            at(1_000),
        )
        .expect("a strength in [0, 1] is admitted"),
    );
    graph.add(
        CausalEdge::new(
            "DRIVER",
            "BBB",
            Mechanism::TemporalPrecedence,
            0.5,
            Duration::from_days(1),
            at(1_000),
        )
        .expect("a strength in [0, 1] is admitted"),
    );
    let held: BTreeSet<String> = ["AAA".to_string(), "BBB".to_string()].into_iter().collect();

    let before = graph.len();
    let report = exposure::hidden_concentration(&graph, &held, at(2_000));
    assert_eq!(
        graph.len(),
        before,
        "a query must not write to the graph it reads; a second writer would be a second \
         story about what the platform believes"
    );
    assert_eq!(
        report.drivers.len(),
        1,
        "the premise: the query did find something"
    );

    // The deliberate non-action: the audit reports and never retracts.
    let book = book_driven_by_one_factor(&["AAA", "BBB", "CCC", "DDD", "EEE"], 200);
    let audit = causal_review::audit_controls(&graph, &book, Duration::from_days(1), at(2_000));
    assert_eq!(
        graph.len(),
        before,
        "the control audit marks and never mutates — the same argument `decayed_at` makes for \
         being a mark rather than an attenuation"
    );
    let _ = audit;
}
