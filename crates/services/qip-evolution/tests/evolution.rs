//! Tests for the evolution brain.
//!
//! Most of these try to get a strategy promoted on the strength of having been
//! searched for. The headline pair is
//! [`the_same_challenger_wins_a_small_search_and_loses_a_large_one`] and
//! [`a_larger_search_never_lowers_the_bar`]: identical returns, identical
//! costs, identical champion, and the verdict turns on nothing but how many
//! siblings the challenger was picked from. The rest check the four structural
//! defences the crate root claims — a candidate is a proof, a trial count is
//! counted, a score is net of costs, and there is one path to capital — and the
//! determinism the whole search depends on.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::edge::{Deduction, DeductionKind, NetEdge};
use qip_contracts::{Conviction, FeatureKey, SignalKind, StrategyId};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, ObjectId, Timestamp, dec};
use qip_evolution::attribution::{Attribution, AttributionLedger, RealisedTrade};
use qip_evolution::challenger::{ChallengerEntry, ChallengerTest, TrialLedger};
use qip_evolution::cost_model::{CostModelCalibration, CostObservation, NetReturns};
use qip_evolution::discovery::{DiscoveryPolicy, FeatureCandidate, FeatureProposal, FeatureScreen};
use qip_evolution::generate::{Candidate, GenerationRun, StrategyGenerator};
use qip_evolution::mutate::{Challenger, Mutator};
use qip_evolution::promotion::{ChampionBook, advance_challenger};
use qip_evolution::scoring::{Outcome, ScoreBand, Scoreboard, evidence_weight};
use qip_evolution::{FeaturePalette, Grammar};
use qip_lifecycle::evidence::StrategyEvidence;
use qip_lifecycle::ledger::LifecycleLedger;
use qip_numerics::stats;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::Type;

const PERIODS_PER_YEAR: f64 = 252.0;
/// A year of daily observations: the smallest window the challenger policy
/// will compare two Sharpe ratios over.
const YEAR: usize = 252;

fn subject() -> ObjectId {
    ObjectId::from_string("AAPL")
}

fn other_subject() -> ObjectId {
    ObjectId::from_string("MSFT")
}

fn now() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

/// A vocabulary with at least two features of every type, so that a
/// type-preserving swap always has somewhere to go.
fn catalogue(on: &ObjectId) -> Result<FeatureCatalogue> {
    let mut catalogue = FeatureCatalogue::new();
    for (name, value_type) in [
        ("microprice", Type::Exact),
        ("mid", Type::Exact),
        ("spread", Type::Exact),
        ("imbalance", Type::Statistic),
        ("volatility", Type::Statistic),
        ("momentum", Type::Statistic),
        ("trades", Type::Count),
        ("cancels", Type::Count),
        ("halted", Type::Flag),
        ("auction", Type::Flag),
    ] {
        catalogue.declare(FeatureKey::new(name, on.clone()), value_type)?;
    }
    Ok(catalogue)
}

fn grammar_over(on: &ObjectId) -> Result<Grammar> {
    Ok(Grammar::over(FeaturePalette::from_catalogue(
        &catalogue(on)?,
        on,
    )?))
}

/// One search: `count` candidates proposed, compiled and counted.
fn search(on: &ObjectId, lineage: &str, seed: u64, count: usize) -> Result<GenerationRun> {
    let mut generator = StrategyGenerator::new(grammar_over(on)?, lineage, seed);
    let mut compiler = StrategyCompiler::new(catalogue(on)?);
    Ok(generator.generate(count, &mut compiler))
}

/// A return series with a chosen annualised Sharpe, drawn from a seeded stream
/// so every run of this suite sees the same numbers.
///
/// The draw is standardised afterwards, so the realised Sharpe is the one asked
/// for rather than one near it — a test of a threshold should not also be a
/// test of sampling luck.
fn series(seed: u64, n: usize, annualised_sharpe: f64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    // Sum of two uniforms, centred: a deterministic bell-ish shape with no
    // dependence on a distribution implementation that might change.
    let raw: Vec<f64> = (0..n)
        .map(|_| rng.next_f64() + rng.next_f64() - 1.0)
        .collect();
    let mean = stats::mean(&raw);
    let sigma = stats::stddev(&raw);
    let target_sigma = 0.01;
    let target_mean = annualised_sharpe / PERIODS_PER_YEAR.sqrt() * target_sigma;
    raw.iter()
        .map(|value| (value - mean) / sigma * target_sigma + target_mean)
        .collect()
}

/// A net-of-costs series with a chosen annualised Sharpe *after* costs.
///
/// Built backwards: the charge is added back on to make the gross series, so
/// the number under test is the one a trader would actually have kept.
fn net_series(seed: u64, n: usize, annualised_sharpe: f64, cost_bps: f64) -> Result<NetReturns> {
    let net = series(seed, n, annualised_sharpe);
    let charge = cost_bps / 10_000.0;
    let gross: Vec<f64> = net.iter().map(|value| value + charge).collect();
    NetReturns::flat_bps(&gross, cost_bps)
}

fn annualised_sharpe_of(returns: &[f64]) -> f64 {
    stats::mean(returns) / stats::stddev(returns) * PERIODS_PER_YEAR.sqrt()
}

/// A complete expectation: every deduction kind considered, as
/// [`NetEdge::require_complete`] insists.
fn complete_expectation(gross: Decimal, each_deduction: Decimal) -> Result<NetEdge> {
    let mut edge = NetEdge::gross(gross, dec!("100"))?;
    for kind in DeductionKind::all() {
        edge = edge.deduct(Deduction::new(kind, each_deduction, "modelled")?);
    }
    Ok(edge)
}

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

#[test]
fn a_palette_refuses_a_subject_it_has_no_words_for() -> Result<()> {
    let refusal = FeaturePalette::from_catalogue(&catalogue(&subject())?, &other_subject());
    let error = refusal.expect_err("a palette with no features should not be built");
    assert_eq!(error.code(), "not_found");
    assert!(error.message().contains("MSFT"));
    Ok(())
}

#[test]
fn a_palette_offers_every_declared_feature_at_its_declared_type() -> Result<()> {
    let subject = subject();
    let palette = FeaturePalette::from_catalogue(&catalogue(&subject)?, &subject)?;
    assert_eq!(palette.len(), 10);
    assert_eq!(palette.of_type(Type::Exact).len(), 3);
    assert_eq!(palette.of_type(Type::Statistic).len(), 3);
    assert_eq!(palette.of_type(Type::Count).len(), 2);
    assert_eq!(palette.of_type(Type::Flag).len(), 2);
    // Flags are not arithmetic, so they are not offered where arithmetic is.
    assert!(!palette.numeric_types().contains(&Type::Flag));
    Ok(())
}

#[test]
fn a_generated_strategy_only_names_features_the_palette_declares() -> Result<()> {
    let subject = subject();
    let palette = FeaturePalette::from_catalogue(&catalogue(&subject)?, &subject)?;
    let run = search(&subject, "vocab", 3, 60)?;
    assert!(!run.accepted().is_empty());
    for candidate in run.accepted() {
        for input in candidate.compiled().inputs() {
            assert!(
                palette.type_of(input).is_some(),
                "{} reads {}, which the palette never offered",
                candidate.id(),
                input.canonical()
            );
        }
    }
    Ok(())
}

#[test]
fn the_first_rule_of_a_generated_strategy_protects() -> Result<()> {
    // Rules are tried in order and the first match wins, so an exit written
    // behind an entry that fires can never be reached.
    let run = search(&subject(), "order", 11, 60)?;
    for candidate in run.accepted() {
        let first = candidate
            .spec()
            .rules
            .first()
            .expect("a compiled strategy has at least one rule");
        assert!(
            matches!(first.kind, SignalKind::Exit | SignalKind::Stand),
            "{} opens with {:?}",
            candidate.id(),
            first.kind
        );
    }
    Ok(())
}

#[test]
fn a_generated_rule_claims_no_evidence_it_does_not_have() -> Result<()> {
    let run = search(&subject(), "fresh", 17, 40)?;
    for candidate in run.accepted() {
        for rule in &candidate.spec().rules {
            assert_eq!(rule.observations, 0);
        }
    }
    // Which is to say: a freshly generated conviction reads as a coin flip.
    let fresh = Conviction::new(0.8, 0);
    assert!((fresh.shrunk() - 0.5).abs() < 1e-12);
    Ok(())
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn the_same_seed_generates_the_same_population() -> Result<()> {
    let subject = subject();
    let first = search(&subject, "repeat", 2024, 50)?;
    let second = search(&subject, "repeat", 2024, 50)?;
    assert_eq!(first.accepted_specs(), second.accepted_specs());
    assert_eq!(first.refusals(), second.refusals());
    Ok(())
}

#[test]
fn a_different_seed_generates_a_different_population() -> Result<()> {
    let subject = subject();
    let first = search(&subject, "repeat", 2024, 50)?;
    let second = search(&subject, "repeat", 2025, 50)?;
    assert_ne!(first.accepted_specs(), second.accepted_specs());
    Ok(())
}

#[test]
fn the_same_seed_mutates_a_champion_the_same_way() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "parent", 5, 4)?;
    let champion = &run.accepted()[0];

    let mut first_compiler = StrategyCompiler::new(catalogue(&subject)?);
    let mut second_compiler = StrategyCompiler::new(catalogue(&subject)?);
    let first = Mutator::new(grammar_over(&subject)?, 77).mutate(champion, 40, &mut first_compiler);
    let second =
        Mutator::new(grammar_over(&subject)?, 77).mutate(champion, 40, &mut second_compiler);

    assert_eq!(first.accepted_specs(), second.accepted_specs());
    let describe = |run: &qip_evolution::MutationRun| -> Vec<String> {
        run.accepted()
            .iter()
            .map(|challenger| challenger.mutation().describe())
            .collect()
    };
    assert_eq!(describe(&first), describe(&second));
    Ok(())
}

// ---------------------------------------------------------------------------
// Nothing is silently lost
// ---------------------------------------------------------------------------

#[test]
fn every_proposed_candidate_is_accepted_or_discarded() -> Result<()> {
    let run = search(&subject(), "accounted", 31, 75)?;
    assert_eq!(run.requested(), 75);
    assert!(run.accounted_for());
    assert_eq!(run.evaluable(), run.accepted().len());
    Ok(())
}

#[test]
fn every_attempted_mutation_is_accepted_or_rejected() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "parent", 5, 4)?;
    let mut compiler = StrategyCompiler::new(catalogue(&subject)?);
    let mutated =
        Mutator::new(grammar_over(&subject)?, 13).mutate(&run.accepted()[0], 50, &mut compiler);
    assert_eq!(mutated.requested(), 50);
    assert!(mutated.accounted_for());
    Ok(())
}

#[test]
fn a_candidate_the_compiler_refuses_is_kept_with_the_compilers_own_reason() -> Result<()> {
    // A palette proposing features nobody has declared: exactly what a
    // discovery run offers before somebody adds them to the graph.
    let subject = subject();
    let undeclared = FeaturePalette::from_keys(
        subject.clone(),
        [
            (
                FeatureKey::new("not-yet-a-feature", subject.clone()),
                Type::Statistic,
            ),
            (
                FeatureKey::new("also-not-one", subject.clone()),
                Type::Statistic,
            ),
        ],
    )?;
    let mut generator = StrategyGenerator::new(Grammar::over(undeclared), "proposed", 8);
    let mut compiler = StrategyCompiler::new(catalogue(&subject)?);
    let run = generator.generate(20, &mut compiler);

    assert!(run.accepted().is_empty());
    assert_eq!(run.discarded().len(), 20);
    assert!(run.accounted_for());
    for discarded in run.discarded() {
        assert_eq!(discarded.reason().code(), "not_found");
        assert!(
            discarded.explain().contains("not-yet-a-feature")
                || discarded.explain().contains("also-not-one")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The trial count is counted, not asserted
// ---------------------------------------------------------------------------

#[test]
fn a_ledger_that_has_counted_nothing_declares_nothing() {
    let ledger = TrialLedger::new();
    assert_eq!(ledger.trials(), 0);
    assert!(ledger.declared().is_none());
}

#[test]
fn a_ledger_counts_what_was_scored_and_not_what_was_refused() -> Result<()> {
    // Twenty candidates proposed, none of which reached an evaluation because
    // none of them compiled. A search that scored nothing declares nothing:
    // refusals must not inflate the number that deflates a Sharpe ratio.
    let subject = subject();
    let undeclared = FeaturePalette::from_keys(
        subject.clone(),
        [(
            FeatureKey::new("undeclared", subject.clone()),
            Type::Statistic,
        )],
    )?;
    let mut generator = StrategyGenerator::new(Grammar::over(undeclared), "refused", 4);
    let mut compiler = StrategyCompiler::new(catalogue(&subject)?);
    let refused = generator.generate(20, &mut compiler);

    let mut ledger = TrialLedger::new();
    ledger.record_generation(&refused);
    assert_eq!(ledger.refused(), 20);
    assert_eq!(ledger.trials(), 0);
    assert!(ledger.declared().is_none());

    // And a run that did score things moves the count by exactly that many.
    let scored = search(&subject, "scored", 6, 12)?;
    ledger.record_generation(&scored);
    assert_eq!(ledger.trials(), 12);
    assert_eq!(
        ledger.declared().map(qip_evolution::TrialCount::get),
        Some(12)
    );
    Ok(())
}

#[test]
fn a_challenger_whose_search_nobody_counted_is_refused() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "undeclared", 21, 2)?;
    let entry = ChallengerEntry::undeclared(
        StrategyId::new("champion"),
        &run.accepted()[0],
        net_series(1, YEAR, 1.0, 2.0)?,
        net_series(2, YEAR, 3.2, 2.0)?,
        PERIODS_PER_YEAR,
    );
    let error = ChallengerTest::default()
        .evaluate(&entry)
        .expect_err("an undeclared search is not a result");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("how many candidates were tried"));
    Ok(())
}

// ---------------------------------------------------------------------------
// The multiple-testing correction itself
// ---------------------------------------------------------------------------

/// Build an entry for `challenger` whose trial count is whatever `ledger`
/// counted, over a fixed window and a fixed cost.
fn entry_for(
    challenger: &Candidate,
    champion_sharpe: f64,
    challenger_sharpe: f64,
    ledger: &TrialLedger,
) -> Result<ChallengerEntry> {
    Ok(ChallengerEntry::from_ledger(
        StrategyId::new("champion"),
        challenger,
        net_series(101, YEAR, champion_sharpe, 2.0)?,
        net_series(202, YEAR, challenger_sharpe, 2.0)?,
        PERIODS_PER_YEAR,
        ledger,
    ))
}

#[test]
fn the_same_challenger_wins_a_small_search_and_loses_a_large_one() -> Result<()> {
    let subject = subject();
    let small = search(&subject, "small", 1, 5)?;
    let large = search(&subject, "large", 2, 5_000)?;
    let challenger = &small.accepted()[0];

    let mut counted_five = TrialLedger::new();
    counted_five.record_generation(&small);
    let mut counted_five_thousand = TrialLedger::new();
    counted_five_thousand.record_generation(&large);
    assert_eq!(counted_five.trials(), 5);
    assert_eq!(counted_five_thousand.trials(), 5_000);

    let test = ChallengerTest::default();
    let after_a_small_search = test.evaluate(&entry_for(challenger, 1.0, 3.2, &counted_five)?)?;
    let after_a_large_search =
        test.evaluate(&entry_for(challenger, 1.0, 3.2, &counted_five_thousand)?)?;

    // Identical returns, identical costs, identical champion.
    assert!(
        (after_a_small_search.challenger_sharpe - after_a_large_search.challenger_sharpe).abs()
            < 1e-12
    );
    assert!((after_a_small_search.margin - after_a_large_search.margin).abs() < 1e-12);

    assert!(after_a_small_search.challenger_wins());
    assert!(!after_a_large_search.challenger_wins());

    // And the large search fails for the right reason: not the margin, which
    // is unchanged, but what the search alone would have produced.
    assert!(after_a_large_search.clears_naive_bar);
    assert!(!after_a_large_search.clears_selection_threshold);
    assert!(after_a_large_search.selection_explains_the_margin());
    assert!(!after_a_small_search.selection_explains_the_margin());
    assert!(
        after_a_large_search.deflated.expected_maximum
            > after_a_small_search.deflated.expected_maximum
    );
    Ok(())
}

#[test]
fn a_larger_search_never_lowers_the_bar() -> Result<()> {
    // Four real searches of growing size, folded cumulatively into one ledger.
    // The bar a challenger must clear is monotone in the count, and so, given
    // fixed returns, is the verdict: a challenger that loses at some search
    // size never starts winning again at a larger one.
    let subject = subject();
    let mut ledger = TrialLedger::new();
    let mut snapshots = Vec::new();
    let first = search(&subject, "grow-a", 41, 5)?;
    let challenger = first.accepted()[0].clone();
    ledger.record_generation(&first);
    snapshots.push(ledger);
    for (index, extra) in [45usize, 450, 4_500].into_iter().enumerate() {
        let run = search(&subject, "grow", 42 + index as u64, extra)?;
        ledger.record_generation(&run);
        snapshots.push(ledger);
    }
    assert_eq!(
        snapshots
            .iter()
            .map(TrialLedger::trials)
            .collect::<Vec<_>>(),
        vec![5, 50, 500, 5_000]
    );

    let test = ChallengerTest::default();
    let mut previous_bar = f64::NEG_INFINITY;
    let mut still_winning = true;
    for snapshot in &snapshots {
        let verdict = test.evaluate(&entry_for(&challenger, 1.0, 3.2, snapshot)?)?;
        assert!(
            verdict.deflated.expected_maximum >= previous_bar,
            "the bar fell as the search grew"
        );
        previous_bar = verdict.deflated.expected_maximum;
        if !verdict.challenger_wins() {
            still_winning = false;
        }
        assert!(
            still_winning || !verdict.challenger_wins(),
            "a challenger started winning again at a larger search size"
        );
    }
    // The sequence really did cross the line rather than passing or failing
    // throughout, which is what makes the monotonicity worth asserting.
    assert!(!still_winning);
    Ok(())
}

#[test]
fn clearing_the_naive_bar_is_reported_and_not_obeyed() -> Result<()> {
    // A search of five hundred: the challenger's Sharpe still exceeds what the
    // search alone explains, but not by enough to be credible. A spreadsheet
    // would have promoted it on the margin alone.
    let subject = subject();
    let run = search(&subject, "mid", 9, 500)?;
    let mut ledger = TrialLedger::new();
    ledger.record_generation(&run);

    let verdict =
        ChallengerTest::default().evaluate(&entry_for(&run.accepted()[0], 1.0, 3.2, &ledger)?)?;
    assert!(verdict.clears_naive_bar);
    assert!(verdict.clears_selection_threshold);
    assert!(!verdict.deflated_is_credible);
    assert!(!verdict.challenger_wins());
    assert_eq!(verdict.failures().len(), 1);
    assert!(verdict.failures()[0].starts_with("deflated_sharpe_credible"));
    Ok(())
}

#[test]
fn a_challenger_measured_over_another_window_is_not_compared() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "window", 12, 3)?;
    let mut ledger = TrialLedger::new();
    ledger.record_generation(&run);
    let entry = ChallengerEntry::from_ledger(
        StrategyId::new("champion"),
        &run.accepted()[0],
        net_series(1, YEAR, 1.0, 2.0)?,
        net_series(2, YEAR + 40, 3.2, 2.0)?,
        PERIODS_PER_YEAR,
        &ledger,
    );
    let error = ChallengerTest::default()
        .evaluate(&entry)
        .expect_err("two different windows are not a comparison");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("different window"));
    Ok(())
}

#[test]
fn too_short_a_window_is_refused_rather_than_scored() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "short", 12, 3)?;
    let mut ledger = TrialLedger::new();
    ledger.record_generation(&run);
    let entry = ChallengerEntry::from_ledger(
        StrategyId::new("champion"),
        &run.accepted()[0],
        net_series(1, 60, 1.0, 2.0)?,
        net_series(2, 60, 3.2, 2.0)?,
        PERIODS_PER_YEAR,
        &ledger,
    );
    let error = ChallengerTest::default()
        .evaluate(&entry)
        .expect_err("sixty observations is not a year");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("below the 250"));
    Ok(())
}

// ---------------------------------------------------------------------------
// A score is net of costs
// ---------------------------------------------------------------------------

#[test]
fn a_cost_series_that_does_not_line_up_is_refused() {
    let error = NetReturns::of(&[0.01, 0.02, 0.03], &[0.001, 0.001])
        .expect_err("a cost series of the wrong length charges the wrong periods");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("wrong periods"));
}

#[test]
fn a_charge_that_adds_to_a_return_is_refused() {
    let error = NetReturns::of(&[0.01, 0.02], &[0.001, -0.005])
        .expect_err("a negative cost is a gross series wearing a net label");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("rebate"));
}

#[test]
fn net_returns_are_gross_less_the_charge() -> Result<()> {
    let gross = vec![0.010, 0.020, -0.005];
    let charge = vec![0.001, 0.002, 0.001];
    let net = NetReturns::of(&gross, &charge)?;
    for (index, value) in net.as_slice().iter().enumerate() {
        assert!((value - (gross[index] - charge[index])).abs() < 1e-15);
    }
    assert!((net.total_cost() - 0.004).abs() < 1e-15);
    assert!(!net.is_costless());
    // And the gross series is recoverable, so the size of the deduction is
    // visible next to the result it changed.
    for (recovered, original) in net.gross().iter().zip(&gross) {
        assert!((recovered - original).abs() < 1e-15);
    }
    Ok(())
}

#[test]
fn a_window_that_was_charged_nothing_is_refused() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "free", 15, 3)?;
    let mut ledger = TrialLedger::new();
    ledger.record_generation(&run);
    let entry = ChallengerEntry::from_ledger(
        StrategyId::new("champion"),
        &run.accepted()[0],
        net_series(1, YEAR, 1.0, 2.0)?,
        net_series(2, YEAR, 3.2, 0.0)?,
        PERIODS_PER_YEAR,
        &ledger,
    );
    let error = ChallengerTest::default()
        .evaluate(&entry)
        .expect_err("a year of free trading is a backtest, not a result");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("charged nothing"));
    Ok(())
}

#[test]
fn charging_more_never_raises_the_score() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "costed", 16, 4)?;
    let mut ledger = TrialLedger::new();
    ledger.record_generation(&run);
    let test = ChallengerTest::default();

    let gross = series(303, YEAR, 3.2);
    let mut previous = f64::INFINITY;
    for cost_bps in [0.5_f64, 2.0, 8.0, 20.0] {
        let entry = ChallengerEntry::from_ledger(
            StrategyId::new("champion"),
            &run.accepted()[0],
            net_series(101, YEAR, 1.0, 2.0)?,
            NetReturns::flat_bps(&gross, cost_bps)?,
            PERIODS_PER_YEAR,
            &ledger,
        );
        let verdict = test.evaluate(&entry)?;
        assert!(
            verdict.challenger_sharpe < previous,
            "charging {cost_bps}bp did not lower the score"
        );
        previous = verdict.challenger_sharpe;
    }
    Ok(())
}

#[test]
fn a_challenger_that_wins_gross_and_loses_net_does_not_win() -> Result<()> {
    // The champion trades rarely and the challenger trades constantly. On
    // gross alpha the challenger is the better strategy and it is not close;
    // after what it costs to trade it that often, it is worse.
    let subject = subject();
    let run = search(&subject, "gross", 19, 4)?;
    let mut ledger = TrialLedger::new();
    ledger.record_generation(&run);

    let champion_gross = series(404, YEAR, 1.6);
    let challenger_gross = series(505, YEAR, 2.6);
    assert!(annualised_sharpe_of(&challenger_gross) > annualised_sharpe_of(&champion_gross));

    let entry = ChallengerEntry::from_ledger(
        StrategyId::new("champion"),
        &run.accepted()[0],
        NetReturns::flat_bps(&champion_gross, 1.0)?,
        NetReturns::flat_bps(&challenger_gross, 12.0)?,
        PERIODS_PER_YEAR,
        &ledger,
    );
    let verdict = ChallengerTest::default().evaluate(&entry)?;
    assert!(verdict.margin < 0.0);
    assert!(!verdict.clears_naive_bar);
    assert!(!verdict.challenger_wins());
    assert!(verdict.challenger_cost_charged > verdict.champion_cost_charged);
    Ok(())
}

// ---------------------------------------------------------------------------
// Correcting the cost model
// ---------------------------------------------------------------------------

/// `count` fills whose realised cost overran the model by `error_bps`.
fn calibration(count: usize, error_bps: f64) -> CostModelCalibration {
    let mut calibration = CostModelCalibration::new();
    for index in 0..count {
        // The modelled cost has to vary or the regression has no slope to fit.
        let modelled = 5.0 + (index % 7) as f64 * 0.5;
        calibration.observe(&CostObservation::new(
            "nyse/small",
            modelled,
            modelled + error_bps,
        ));
    }
    calibration
}

#[test]
fn a_small_sample_moves_the_cost_model_less_than_a_large_one() -> Result<()> {
    let few = calibration(5, 3.0).propose("nyse/small")?;
    let many = calibration(500, 3.0).propose("nyse/small")?;
    // The same observed error, from very different amounts of evidence.
    assert!((few.mean_error_bps - many.mean_error_bps).abs() < 1e-9);
    assert!(few.applied_bias_bps.abs() < many.applied_bias_bps.abs());
    assert!(few.applied_bias_bps.abs() < 1.0);
    assert!(many.applied_bias_bps > 2.5);
    // The shrinkage is the same weight a score uses, not a second one.
    assert!((few.applied_bias_bps - 3.0 * evidence_weight(5)).abs() < 1e-9);
    Ok(())
}

#[test]
fn a_correction_nobody_could_check_forward_is_not_applied() -> Result<()> {
    let few = calibration(5, 3.0).propose("nyse/small")?;
    assert!(few.out_of_sample_improvement_bps.is_none());
    assert!(!few.is_worth_applying(1));
    assert!(few.summarise().contains("too little history"));
    Ok(())
}

#[test]
fn a_systematic_underestimate_earns_a_correction() -> Result<()> {
    let update = calibration(500, 3.0).propose("nyse/small")?;
    assert_eq!(update.observations, 500);
    assert!(update.mean_absolute_error_bps > 2.9);
    assert!(
        update
            .out_of_sample_improvement_bps
            .is_some_and(|improvement| improvement > 0.0)
    );
    assert!(update.is_worth_applying(100));
    // A model that is offset but scales correctly: slope one, intercept the
    // offset.
    assert!((update.slope - 1.0).abs() < 1e-6);
    assert!((update.intercept_bps - 3.0).abs() < 1e-6);
    assert!((update.corrected(5.0) - (5.0 + update.applied_bias_bps)).abs() < 1e-12);
    Ok(())
}

#[test]
fn a_context_with_no_fills_has_nothing_to_propose() {
    let calibration = CostModelCalibration::new();
    let error = calibration
        .propose("nyse/small")
        .expect_err("no fills, no proposal");
    assert_eq!(error.code(), "not_found");
    assert!(calibration.propose_all().is_empty());
}

// ---------------------------------------------------------------------------
// Feature discovery: the same correction in a different currency
// ---------------------------------------------------------------------------

fn returns_and_features(n: usize) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mut rng = Xoshiro256::seeded(5);
    let returns: Vec<f64> = (0..n)
        .map(|_| rng.next_f64() + rng.next_f64() - 1.0)
        .collect();
    let mut signal_noise = Xoshiro256::seeded(11);
    let predictive: Vec<f64> = returns
        .iter()
        .map(|value| value + (signal_noise.next_f64() - 0.5) * 2.0)
        .collect();
    let mut pure = Xoshiro256::seeded(13);
    let noise: Vec<f64> = (0..n).map(|_| pure.next_f64() - 0.5).collect();
    (returns, predictive, noise)
}

#[test]
fn an_undeclared_screen_is_refused() -> Result<()> {
    let (returns, predictive, _) = returns_and_features(500);
    let candidates = vec![FeatureCandidate::new(
        FeatureKey::new("predictive", subject()),
        Type::Statistic,
        predictive,
    )];
    let screen = FeatureScreen::new(DiscoveryPolicy::default());
    let error = screen
        .screen(&candidates, &returns, None)
        .expect_err("an undeclared screen is not a discovery");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("how many features were screened"));

    // And a screen cannot claim to have looked at fewer than it offers.
    let understated = screen
        .screen(&candidates, &returns, Some(0))
        .expect_err("zero screened but one offered");
    assert_eq!(understated.code(), "invalid");
    Ok(())
}

#[test]
fn the_bar_rises_with_the_search_and_falls_with_the_evidence() {
    let screen = FeatureScreen::new(DiscoveryPolicy::default());
    let mut previous = 0.0;
    for screened in [1usize, 10, 100, 1_000, 10_000] {
        let bar = screen.selection_bar(screened, 500);
        assert!(bar >= previous, "the bar fell as the search grew");
        previous = bar;
    }
    let mut previous = f64::INFINITY;
    for observations in [250usize, 500, 1_000, 4_000] {
        let bar = screen.selection_bar(1_000, observations);
        assert!(bar <= previous, "the bar rose as the evidence grew");
        previous = bar;
    }
    // A floor, so that a tiny correlation is never a feature however few were
    // screened.
    assert!(screen.selection_bar(1, 1_000_000_000) >= 0.03);
}

#[test]
fn the_same_correlation_is_a_discovery_in_a_small_search_and_noise_in_a_large_one() {
    let policy = DiscoveryPolicy::default();
    let screen = FeatureScreen::new(policy);
    let observations = 500;
    let correlation = 0.13;

    let proposal = |screened: usize| FeatureProposal {
        key: FeatureKey::new("candidate", subject()),
        value_type: Type::Statistic,
        rank_correlation: correlation,
        fold_agreement: 1.0,
        folds: 5,
        observations,
        screened,
        bar: screen.selection_bar(screened, observations),
    };

    let after_ten = proposal(10);
    let after_a_thousand = proposal(1_000);
    assert!(after_ten.bar < correlation);
    assert!(after_a_thousand.bar > correlation);
    assert!(after_ten.is_discovery(&policy));
    assert!(!after_a_thousand.is_discovery(&policy));
    // Nothing about the feature changed; only the number of siblings it was
    // picked from.
    assert!(after_ten.is_stable(&policy) && after_a_thousand.is_stable(&policy));
}

#[test]
fn a_predictive_feature_survives_the_folds_and_noise_does_not() -> Result<()> {
    let policy = DiscoveryPolicy::default();
    let screen = FeatureScreen::new(policy);
    let (returns, predictive, noise) = returns_and_features(500);
    let candidates = vec![
        FeatureCandidate::new(
            FeatureKey::new("predictive", subject()),
            Type::Statistic,
            predictive,
        ),
        FeatureCandidate::new(FeatureKey::new("noise", subject()), Type::Statistic, noise),
    ];
    let proposals = screen.screen(&candidates, &returns, Some(50))?;

    // Results come back in the order they were offered, so a run is diffable.
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[0].key.name, "predictive");
    assert_eq!(proposals[1].key.name, "noise");

    assert!(proposals[0].rank_correlation > proposals[0].bar);
    assert!((proposals[0].fold_agreement - 1.0).abs() < 1e-12);
    assert!(proposals[0].is_discovery(&policy));
    // Confidence is shrunk by the number of folds behind it, so five agreeing
    // folds is not the same claim as twenty.
    assert!(proposals[0].confidence().shrunk() < 0.65);

    assert!(proposals[1].rank_correlation.abs() < proposals[1].bar);
    assert!(!proposals[1].clears_the_search());
    assert!(!proposals[1].is_discovery(&policy));

    assert_eq!(screen.discoveries(&proposals).len(), 1);
    Ok(())
}

#[test]
fn a_feature_measured_over_another_window_is_refused() {
    let screen = FeatureScreen::new(DiscoveryPolicy::default());
    let (returns, predictive, _) = returns_and_features(500);
    let candidates = vec![FeatureCandidate::new(
        FeatureKey::new("short", subject()),
        Type::Statistic,
        predictive[..400].to_vec(),
    )];
    let error = screen
        .screen(&candidates, &returns, Some(10))
        .expect_err("400 values against 500 returns");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("different window"));
}

#[test]
fn a_discovered_feature_reaches_a_generator_only_by_being_declared() -> Result<()> {
    let subject = subject();
    let mut catalogue = catalogue(&subject)?;
    let key = FeatureKey::new("queue-ahead", subject.clone());
    assert!(!catalogue.contains(&key));

    let proposal = FeatureProposal {
        key: key.clone(),
        value_type: Type::Count,
        rank_correlation: 0.2,
        fold_agreement: 1.0,
        folds: 5,
        observations: 500,
        screened: 10,
        bar: 0.1,
    };
    proposal.declare_into(&mut catalogue)?;
    assert!(catalogue.contains(&key));
    // Only now can a palette built from the catalogue name it.
    let palette = FeaturePalette::from_catalogue(&catalogue, &subject)?;
    assert_eq!(palette.type_of(&key), Some(Type::Count));
    Ok(())
}

// ---------------------------------------------------------------------------
// Attribution: exact, and split so the two findings lead to different actions
// ---------------------------------------------------------------------------

fn trade_with_slippage_overrun() -> Result<RealisedTrade> {
    let realised_deductions: Vec<Deduction> = DeductionKind::all()
        .into_iter()
        .map(|kind| {
            let amount = if kind == DeductionKind::Slippage {
                dec!("5")
            } else {
                dec!("1")
            };
            Deduction::new(kind, amount, "realised")
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RealisedTrade {
        strategy: StrategyId::new("momentum-v3"),
        context: "volatile".to_string(),
        expected: complete_expectation(dec!("100"), dec!("1"))?,
        realised_gross: dec!("120"),
        realised_deductions,
    })
}

#[test]
fn an_incomplete_expectation_cannot_be_attributed() -> Result<()> {
    let partial = NetEdge::gross(dec!("100"), dec!("100"))?.deduct(Deduction::new(
        DeductionKind::Spread,
        dec!("1"),
        "modelled",
    )?);
    let trade = RealisedTrade {
        strategy: StrategyId::new("momentum-v3"),
        context: "calm".to_string(),
        expected: partial,
        realised_gross: dec!("100"),
        realised_deductions: Vec::new(),
    };
    let error = Attribution::of(&trade)
        .expect_err("an incomplete expectation books its own gaps as lost alpha");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("did not consider"));
    Ok(())
}

#[test]
fn the_parts_of_an_attribution_sum_to_the_whole_exactly() -> Result<()> {
    let trade = trade_with_slippage_overrun()?;
    let attribution = Attribution::of(&trade)?;
    assert!(attribution.identity_holds());
    // Not approximately: these are exact decimals, so the equality is real.
    assert_eq!(attribution.expected_net(), dec!("91"));
    assert_eq!(attribution.realised_net(), dec!("107"));
    assert_eq!(attribution.gross_surprise(), dec!("20"));
    assert_eq!(attribution.total_cost_surprise(), dec!("4"));
    assert_eq!(
        attribution.expected_net() + attribution.gross_surprise()
            - attribution.total_cost_surprise(),
        attribution.realised_net()
    );
    Ok(())
}

#[test]
fn a_good_idea_with_a_wrong_cost_model_is_not_a_bad_idea() -> Result<()> {
    let attribution = Attribution::of(&trade_with_slippage_overrun()?)?;
    // The idea arrived in full and then some.
    assert!(attribution.gross_surprise().is_positive());
    // And the costs overran, which is a different finding leading to a
    // different action.
    assert!(attribution.total_cost_surprise().is_positive());
    assert_eq!(
        attribution.worst_overrun(),
        Some((DeductionKind::Slippage, dec!("4")))
    );
    assert_eq!(
        attribution.surprise_of(DeductionKind::Spread),
        Decimal::ZERO
    );
    // Columns line up with `DeductionKind::all`, so two attributions can be
    // read side by side.
    let kinds: Vec<DeductionKind> = attribution
        .cost_surprise()
        .iter()
        .map(|(kind, _)| *kind)
        .collect();
    assert_eq!(kinds, DeductionKind::all().to_vec());
    Ok(())
}

#[test]
fn a_ledger_totals_the_overrun_by_kind() -> Result<()> {
    let mut ledger = AttributionLedger::new();
    for _ in 0..4 {
        ledger.record(Attribution::of(&trade_with_slippage_overrun()?)?);
    }
    assert_eq!(ledger.len(), 4);
    let by_kind = ledger.overrun_by_kind();
    let slippage = by_kind
        .iter()
        .find(|(kind, _)| *kind == DeductionKind::Slippage)
        .map(|(_, amount)| *amount);
    assert_eq!(slippage, Some(dec!("16")));
    assert_eq!(ledger.outcomes().len(), 4);
    Ok(())
}

// ---------------------------------------------------------------------------
// Scoring: never one number, and never unshrunk
// ---------------------------------------------------------------------------

fn board_with(observations: u32, value: f64, context: &str) -> Scoreboard {
    let mut board = Scoreboard::strategies();
    for _ in 0..observations {
        board.observe(Outcome::new("momentum-v3", context, value));
    }
    board
}

#[test]
fn a_score_shrinks_toward_its_prior_when_the_sample_is_small() -> Result<()> {
    let thin = board_with(3, 1.0, "calm");
    let thick = board_with(400, 1.0, "calm");
    let thin_score = thin.score("momentum-v3", "calm").expect("observed");
    let thick_score = thick.score("momentum-v3", "calm").expect("observed");

    // The same perfect record, believed very differently.
    assert!((thin_score.observed() - thick_score.observed()).abs() < 1e-12);
    assert!(thin_score.score() < thick_score.score());
    assert!(thin_score.score() < 0.6);
    assert!(thick_score.score() > 0.9);
    assert_eq!(thin_score.band(), ScoreBand::Unproven);
    assert_eq!(thick_score.band(), ScoreBand::Established);
    assert!(!thin_score.is_confident());
    assert!(thick_score.is_confident());
    Ok(())
}

#[test]
fn the_shrinkage_weight_is_convictions_own() {
    // Recovered from `Conviction`, not restated, so the two cannot drift.
    for observations in [0u32, 1, 5, 30, 100, 1_000] {
        let expected = f64::from(observations) / (f64::from(observations) + 30.0);
        assert!((evidence_weight(observations) - expected).abs() < 1e-12);
    }
    // And a score with the default prior says the same thing as the
    // conviction it can be turned into.
    let board = board_with(40, 0.75, "calm");
    let score = board.score("momentum-v3", "calm").expect("observed");
    assert!((score.score() - score.conviction().shrunk()).abs() < 1e-12);
}

#[test]
fn a_band_only_rises_with_evidence() {
    let mut previous = ScoreBand::Unproven;
    for observations in [1u32, 5, 10, 20, 45, 100, 500] {
        let board = board_with(observations, 1.0, "calm");
        let band = board.score("momentum-v3", "calm").expect("observed").band();
        assert!(band >= previous);
        previous = band;
    }
    assert_eq!(previous, ScoreBand::Established);
}

#[test]
fn pooling_hides_a_spread_the_board_will_still_report() {
    let mut board = Scoreboard::strategies();
    for _ in 0..200 {
        board.observe(Outcome::binary("momentum-v3", "calm", true));
    }
    for _ in 0..200 {
        board.observe(Outcome::binary("momentum-v3", "volatile", false));
    }
    let pooled = board.pooled("momentum-v3").expect("seen");
    assert!(
        (pooled.score() - 0.5).abs() < 0.05,
        "pooled reads as average"
    );

    // Which describes neither context.
    let spread = board.spread("momentum-v3").expect("seen");
    assert!(spread > 0.7, "the pool is hiding a spread of {spread}");
    let best = board.best_context("momentum-v3").expect("seen");
    assert_eq!(best.context(), "calm");
    assert_eq!(board.scores_of("momentum-v3").len(), 2);
}

#[test]
fn an_outcome_outside_the_unit_interval_is_clamped_rather_than_dropped() {
    let mut board = Scoreboard::execution();
    board.observe(Outcome::new("nyse", "large", 1.4));
    board.observe(Outcome::new("nyse", "large", f64::NAN));
    let score = board.score("nyse", "large").expect("observed");
    assert_eq!(score.observations(), 2);
    assert!((score.observed() - 0.5).abs() < 1e-12);
}

// ---------------------------------------------------------------------------
// One path to capital, and it is not in this crate
// ---------------------------------------------------------------------------

/// A champion, a challenger derived from it, and a verdict about that
/// challenger from a search of `trials` candidates.
fn duel(
    lineage: &str,
    trials: usize,
    seed: u64,
) -> Result<(Challenger, qip_evolution::ChallengerVerdict)> {
    let subject = subject();
    let run = search(&subject, lineage, seed, trials)?;
    let mut compiler = StrategyCompiler::new(catalogue(&subject)?);
    let mutated = Mutator::new(grammar_over(&subject)?, seed + 1).mutate(
        &run.accepted()[0],
        4,
        &mut compiler,
    );
    let challenger = mutated.accepted()[0].clone();

    let mut ledger = TrialLedger::new();
    ledger.record_generation(&run);
    ledger.record_mutation(&mutated);

    let entry = ChallengerEntry::from_ledger(
        StrategyId::new("champion"),
        challenger.candidate(),
        net_series(101, YEAR, 1.0, 2.0)?,
        net_series(202, YEAR, 3.2, 2.0)?,
        PERIODS_PER_YEAR,
        &ledger,
    );
    let verdict = ChallengerTest::default().evaluate(&entry)?;
    Ok((challenger, verdict))
}

#[test]
fn a_challenger_never_inherits_the_champions_identity() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "identity", 23, 3)?;
    let champion = &run.accepted()[0];
    let mut compiler = StrategyCompiler::new(catalogue(&subject)?);
    let mutated = Mutator::new(grammar_over(&subject)?, 24).mutate(champion, 30, &mut compiler);
    assert!(!mutated.accepted().is_empty());
    for challenger in mutated.accepted() {
        assert_ne!(challenger.id(), champion.id());
        assert_eq!(challenger.champion(), champion.id());
        // A mutation is about the same instrument as its parent.
        assert_eq!(challenger.candidate().spec().subject, subject);
        // And it says what it did, at a named site.
        assert!(!challenger.mutation().site.is_empty());
        assert!(
            challenger
                .mutation()
                .describe()
                .contains(challenger.mutation().kind.as_str())
        );
    }
    Ok(())
}

#[test]
fn a_verdict_about_another_strategy_does_not_promote_this_one() -> Result<()> {
    let (challenger, winning) = duel("mine", 2, 51)?;
    let (other, _) = duel("theirs", 2, 61)?;
    let mut ledger = LifecycleLedger::new();
    let evidence = StrategyEvidence::new();

    let error = advance_challenger(
        &mut ledger,
        &other,
        &winning,
        &evidence,
        None,
        "borrowed evidence",
        now(),
    )
    .expect_err("a verdict about another strategy is not evidence about this one");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("the verdict is about"));

    let mut book = ChampionBook::new();
    let refusal = book
        .crown(&ledger, &subject(), &other, &winning, now())
        .expect_err("the same substitution, at the crown");
    assert_eq!(refusal.code(), "invalid");
    assert!(challenger.id() != other.id());
    Ok(())
}

#[test]
fn a_challenger_the_search_explains_cannot_be_promoted_or_crowned() -> Result<()> {
    let (challenger, losing) = duel("explained", 5_000, 71)?;
    assert!(!losing.challenger_wins());
    assert!(losing.selection_explains_the_margin());

    let mut ledger = LifecycleLedger::new();
    let error = advance_challenger(
        &mut ledger,
        &challenger,
        &losing,
        &StrategyEvidence::new(),
        None,
        "best of five thousand",
        now(),
    )
    .expect_err("the best of an explained search is not a promotion");
    assert_eq!(error.code(), "guard");
    assert!(error.message().contains("did not beat"));

    let mut book = ChampionBook::new();
    let refusal = book
        .crown(&ledger, &subject(), &challenger, &losing, now())
        .expect_err("nor a coronation");
    assert_eq!(refusal.code(), "guard");
    assert!(book.is_empty());
    Ok(())
}

#[test]
fn a_challenger_about_another_instrument_cannot_be_crowned() -> Result<()> {
    let (challenger, winning) = duel("elsewhere", 2, 81)?;
    let mut book = ChampionBook::new();
    let error = book
        .crown(
            &LifecycleLedger::new(),
            &other_subject(),
            &challenger,
            &winning,
            now(),
        )
        .expect_err("a strategy about AAPL cannot speak for MSFT");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("cannot be champion of"));
    Ok(())
}

#[test]
fn a_book_that_already_has_a_champion_is_not_bootstrapping() -> Result<()> {
    let subject = subject();
    let run = search(&subject, "first", 91, 2)?;
    let ledger = LifecycleLedger::new();
    let mut book = ChampionBook::new();

    let succession = book.install_first(&ledger, &subject, &run.accepted()[0], now())?;
    assert!(succession.deposed.is_none());
    assert!(succession.describe().contains("becomes the first champion"));
    assert_eq!(book.champion(&subject), Some(run.accepted()[0].id()));

    let error = book
        .install_first(&ledger, &subject, &run.accepted()[1], now())
        .expect_err("a replacement is a succession, not an installation");
    assert_eq!(error.code(), "denied");
    assert_eq!(book.len(), 1);
    Ok(())
}

#[test]
fn a_succession_names_who_was_deposed() -> Result<()> {
    let subject = subject();
    let (challenger, winning) = duel("usurper", 2, 101)?;
    let incumbent = search(&subject, "incumbent", 111, 1)?;
    let ledger = LifecycleLedger::new();
    let mut book = ChampionBook::new();
    book.install_first(&ledger, &subject, &incumbent.accepted()[0], now())?;

    let succession = book.crown(&ledger, &subject, &challenger, &winning, now())?;
    assert_eq!(
        succession.deposed.as_ref(),
        Some(incumbent.accepted()[0].id())
    );
    assert_eq!(&succession.champion, challenger.id());
    assert!(succession.describe().contains("replaces"));
    assert_eq!(book.champion(&subject), Some(challenger.id()));
    Ok(())
}

#[test]
fn a_champion_holding_no_capital_is_reported_stale() -> Result<()> {
    // Winning a comparison makes a strategy the champion of the register. It
    // does not make it a strategy that may hold capital: that is the ledger's
    // to say, and here the ledger has never promoted it past candidate.
    let subject = subject();
    let (challenger, winning) = duel("uncapitalised", 2, 121)?;
    let ledger = LifecycleLedger::new();
    let mut book = ChampionBook::new();
    book.crown(&ledger, &subject, &challenger, &winning, now())?;

    let stale = book.stale(&ledger);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].1, challenger.id());
    assert_eq!(book.dethrone(&subject).as_ref(), Some(challenger.id()));
    assert!(book.is_empty());
    Ok(())
}
