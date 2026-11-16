//! R12-TICK-2064: a market-state embedding is fitted, versioned by a card and
//! judged against a baseline, and no card exists for one that loses.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use qip_ai::registry::ModelRegistry;
use qip_ai::state_embedding::{EmbeddingEvaluation, StateEmbedder};
use qip_core::{ModelId, Timestamp};

/// A fixed pseudo-random stream in `[-1, 1)`; no clock, no entropy.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

fn names() -> Vec<String> {
    (0..6).map(|i| format!("f{i}")).collect()
}

/// Two latent drivers `a`, `b`. Columns 0 and 1 are mostly noise; columns
/// 2..6 are mixtures of the drivers. The outcome is `a + 0.5 b`, so the
/// first two raw columns, which the baseline retrieves on, say little about
/// it and the embedding's two leading directions say most of it.
fn states(n: usize, seed: u64) -> Vec<(Vec<f64>, f64)> {
    let mut g = Lcg(seed);
    (0..n)
        .map(|_| {
            let (a, b) = (g.next(), g.next());
            let row = vec![
                g.next(),
                g.next(),
                a + 0.05 * g.next(),
                b + 0.05 * g.next(),
                a + b + 0.05 * g.next(),
                a - b + 0.05 * g.next(),
            ];
            (row, a + 0.5 * b)
        })
        .collect()
}

type Labelled = Vec<(Vec<f64>, f64)>;

fn fit() -> (StateEmbedder, Labelled, Labelled) {
    let train = states(300, 7);
    let held = states(100, 99);
    let rows: Vec<Vec<f64>> = train.iter().map(|(r, _)| r.clone()).collect();
    (
        StateEmbedder::fit(names(), &rows, 2).expect("fits"),
        train,
        held,
    )
}

#[test]
fn a_fitted_embedding_beats_the_raw_feature_baseline_and_is_versioned_by_a_card() {
    let (embedder, train, held) = fit();
    assert_eq!(train.len(), 300, "premise: there is training data");
    assert_eq!(held.len(), 100, "premise: there is held-out data");
    assert_eq!(embedder.embed(&train[0].0).expect("embeds").len(), 2);
    let evaluation = embedder.evaluate(&train, &held).expect("evaluates");
    assert!(
        evaluation.baseline_error > 0.0,
        "premise: the baseline is not trivially perfect"
    );
    assert!(
        evaluation.beats_baseline(),
        "embedding {} vs baseline {}",
        evaluation.embedding_error,
        evaluation.baseline_error
    );
    let card = embedder
        .issue_card(
            ModelId::from_string("MDL0000000000000000000064"),
            "1",
            "learning-desk",
            "synthetic-states",
            &evaluation,
            Timestamp::from_nanos(1),
        )
        .expect("a winning embedding is issued a card");
    assert_eq!(
        card.artifact_digest,
        Some(embedder.digest().expect("digest"))
    );
    assert_eq!(card.benchmarks, vec!["raw_first_k_nearest_neighbour"]);
    let reference = card.reference();
    let mut registry = ModelRegistry::new();
    registry.register(card).expect("registers");
    assert_eq!(registry.len(), 1, "premise: one card was registered");
    assert_eq!(
        registry
            .get(&reference)
            .and_then(|c| c.artifact_digest.clone()),
        Some(embedder.digest().expect("digest")),
        "the registry holds the card bound to this artefact"
    );
}

#[test]
fn the_same_rows_give_the_same_artefact_digest_and_other_rows_do_not() {
    let (first, ..) = fit();
    let (second, ..) = fit();
    assert_eq!(first.digest().unwrap(), second.digest().unwrap());
    let other: Vec<Vec<f64>> = states(300, 8).into_iter().map(|(r, _)| r).collect();
    let refit = StateEmbedder::fit(names(), &other, 2).unwrap();
    assert_ne!(first.digest().unwrap(), refit.digest().unwrap());
}

#[test]
fn an_embedding_that_does_not_beat_its_baseline_is_issued_no_card() {
    let (embedder, ..) = fit();
    let tie = EmbeddingEvaluation {
        held_out: 10,
        embedding_error: 0.5,
        baseline_error: 0.5,
    };
    let refused = embedder.issue_card(
        ModelId::from_string("MDL0000000000000000000064"),
        "1",
        "learning-desk",
        "synthetic-states",
        &tie,
        Timestamp::from_nanos(1),
    );
    assert!(refused.is_err(), "a tie is not a win");
}

#[test]
fn a_fit_refuses_what_it_would_otherwise_have_to_guess() {
    let rows: Vec<Vec<f64>> = states(50, 3).into_iter().map(|(r, _)| r).collect();
    assert!(
        StateEmbedder::fit(names(), &rows, 0).is_err(),
        "zero dimensions"
    );
    assert!(
        StateEmbedder::fit(names(), &rows, 7).is_err(),
        "more than the features"
    );
    assert!(
        StateEmbedder::fit(names(), &rows[..6], 2).is_err(),
        "too few rows"
    );
    let mut constant = rows.clone();
    for r in &mut constant {
        r[3] = 1.0;
    }
    assert!(
        StateEmbedder::fit(names(), &constant, 2).is_err(),
        "constant column"
    );
    let mut ragged = rows.clone();
    ragged[4].pop();
    assert!(
        StateEmbedder::fit(names(), &ragged, 2).is_err(),
        "ragged row"
    );
    let (embedder, ..) = fit();
    assert!(embedder.embed(&[1.0, 2.0]).is_err(), "wrong width");
}

#[test]
fn the_fitted_directions_are_unit_length_and_orthogonal() {
    let (embedder, ..) = fit();
    let c = embedder.components();
    assert_eq!(c.len(), 2, "premise: two directions were fitted");
    for v in c {
        let len: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((len - 1.0).abs() < 1e-9, "unit length, got {len}");
    }
    let dot: f64 = c[0].iter().zip(&c[1]).map(|(a, b)| a * b).sum();
    assert!(dot.abs() < 1e-6, "directions overlap: dot {dot}");
}
