//! Evolution reaching the promotion ladder, with its trial count intact.
//!
//! Both `qip-evolution` and `qip-training` were dependencies of `qip-kernel`
//! that no line of the kernel named: six and a half thousand lines that
//! compiled into a binary and ran in none of it. These tests exercise the seam
//! that joins them, and most of them are about one number.
//!
//! Generate ten thousand strategies, keep the best backtest, and you have
//! promoted noise. The count of how many were tried is what tells those apart,
//! and it has to survive a handoff between two crates that each track it
//! carefully on their own side.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::FeatureKey;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp};
use qip_evolution::generate::Candidate;
use qip_evolution::grammar::Grammar;
use qip_evolution::palette::FeaturePalette;
use qip_kernel::central::factory::StrategyFactory;
use qip_kernel::central::foundry::{HoldoutInputs, StrategyFoundry};
use qip_lifecycle::evidence::{CrossValidationRun, LeakageAudit};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::ir::Type;

fn subject() -> ObjectId {
    ObjectId::from_string("AAPL")
}

fn now() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn venue() -> VenueId {
    VenueId::new("XNAS")
}

/// At least two features of every type, so a type-preserving swap always has
/// somewhere to go and the generator is not starved.
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

fn foundry(seed: u64) -> Result<StrategyFoundry> {
    let on = subject();
    let grammar = Grammar::over(FeaturePalette::from_catalogue(&catalogue(&on)?, &on)?);
    StrategyFoundry::new(
        catalogue(&on)?,
        grammar,
        "cell-london",
        venue(),
        "foundry-tests",
        seed,
    )
}

/// Out-of-sample evidence for a candidate. Deliberately plausible and
/// deliberately not generated here: the foundry does not score its own work.
fn holdout() -> HoldoutInputs {
    HoldoutInputs {
        returns: (0..252)
            .map(|i| 0.0004 + 0.001 * ((i % 7) as f64 - 3.0))
            .collect(),
        in_sample_folds: vec![vec![0.001; 40], vec![0.0008; 40]],
        out_of_sample_folds: vec![vec![0.0006; 20], vec![0.0004; 20]],
        periods_per_year: 252.0,
        cross_validation: CrossValidationRun {
            folds: 2,
            label_horizon: 5,
            embargo: 5,
            observations: 252,
            purged: 10,
            embargoed: 10,
        },
        leakage: LeakageAudit {
            timings: Vec::new(),
            restated_without_snapshots: Vec::new(),
        },
    }
}

fn first_pending(foundry: &StrategyFoundry) -> Result<StrategyId> {
    foundry
        .pending()
        .first()
        .map(|candidate: &Candidate| candidate.id().clone())
        .ok_or_else(|| qip_core::error::Error::not_found("a pending candidate"))
}

// --- the number the whole seam exists to carry ------------------------------

#[test]
fn the_trial_count_that_reaches_the_gate_is_the_whole_search_not_the_last_round() -> Result<()> {
    // The failure this is built against: a handoff that passes the round's
    // count instead of the search's understates trials by however many rounds
    // have run, and does it silently — a candidate carrying `trials: 40` is
    // indistinguishable from an honest one except in being wrong.
    let mut foundry = foundry(11)?;
    let mut factory = StrategyFactory::new();

    let first = foundry.search(40)?;
    let second = foundry.search(40)?;
    let third = foundry.search(40)?;

    assert!(
        third.trials > second.trials && second.trials > first.trials,
        "the cumulative count did not grow across rounds: {first:?} {second:?} {third:?}"
    );
    assert_eq!(
        foundry.trials(),
        third.trials,
        "the foundry disagrees with the round it just reported"
    );

    let strategy = first_pending(&foundry)?;
    foundry.register(&mut factory, &strategy, holdout(), now())?;

    let registered = factory
        .candidate(&strategy)
        .ok_or_else(|| qip_core::error::Error::not_found("the registered candidate"))?;
    let evidence = registered
        .evidence()
        .holdout
        .as_ref()
        .ok_or_else(|| qip_core::error::Error::not_found("the holdout evidence"))?;

    assert_eq!(
        evidence.trials,
        foundry.trials(),
        "the gate will deflate by a different number than the search actually ran"
    );
    assert!(
        evidence.trials >= third.accepted,
        "the trial count is smaller than the last round alone, so rounds were dropped"
    );
    Ok(())
}

#[test]
fn refused_candidates_are_counted_separately_and_never_folded_into_trials() -> Result<()> {
    // A candidate the compiler refused was never scored, so it did not
    // contribute a draw to the maximum the best was picked from. Folding it in
    // would overstate the search and bury a real result; ignoring it entirely
    // would hide that the generator is producing rubbish.
    let mut foundry = foundry(5)?;
    let round = foundry.search(60)?;

    assert_eq!(
        round.requested, 60,
        "the round did not report what was asked for"
    );
    assert_eq!(
        round.accepted + round.refused,
        round.requested,
        "candidates went missing between proposal and accounting"
    );
    assert_eq!(
        round.trials, round.accepted,
        "refused candidates were folded into the trial count"
    );
    assert_eq!(foundry.refused(), round.refused);
    Ok(())
}

#[test]
fn discarding_a_candidate_does_not_lower_the_trial_count() -> Result<()> {
    // It was generated, it was looked at, and it contributed a draw to the
    // distribution the survivors were picked from. A count that fell when a
    // candidate was thrown away would let a search launder its own size.
    let mut foundry = foundry(3)?;
    foundry.search(40)?;
    let before = foundry.trials();

    let strategy = first_pending(&foundry)?;
    assert!(foundry.discard(&strategy));
    assert_eq!(
        foundry.trials(),
        before,
        "discarding a candidate lowered the trial count"
    );
    assert!(
        !foundry.pending().iter().any(|c| *c.id() == strategy),
        "the discarded candidate is still pending"
    );
    assert!(
        !foundry.discard(&strategy),
        "discarding twice reported success"
    );
    Ok(())
}

#[test]
fn a_candidate_from_outside_this_search_cannot_borrow_its_trial_count() -> Result<()> {
    // Two foundries, two searches. Registering one's candidate through the
    // other would attach a trial count describing a search that candidate was
    // never part of — which is the same lie as understating it, told sideways.
    let mut mine = foundry(7)?;
    let mut theirs = foundry(9)?;
    let mut factory = StrategyFactory::new();

    mine.search(40)?;
    theirs.search(40)?;
    let their_strategy = first_pending(&theirs)?;

    let error = mine
        .register(&mut factory, &their_strategy, holdout(), now())
        .expect_err("a foreign candidate was registered");
    assert!(
        error.message().contains("not pending"),
        "the refusal does not say why: {}",
        error.message()
    );
    Ok(())
}

// --- the seam itself --------------------------------------------------------

#[test]
fn a_generated_candidate_reaches_the_ladder_and_starts_at_the_bottom() -> Result<()> {
    // The whole point: a strategy nobody wrote by hand, on the promotion
    // ladder, with the gates unchanged. Registration is not promotion — it
    // puts the candidate on the bottom rung and every move up stays the
    // factory's decision.
    let mut foundry = foundry(21)?;
    let mut factory = StrategyFactory::new();
    foundry.search(40)?;

    let strategy = first_pending(&foundry)?;
    foundry.register(&mut factory, &strategy, holdout(), now())?;

    assert_eq!(
        factory.candidates().count(),
        1,
        "the candidate did not reach the factory"
    );
    assert!(
        !factory.holds_capital(&strategy),
        "a freshly generated strategy was registered holding capital"
    );

    // And it is off the pending list: registering the same candidate twice
    // would put one search's work on the ladder as two independent results.
    assert!(
        !foundry.pending().iter().any(|c| *c.id() == strategy),
        "the registered candidate is still pending"
    );
    assert!(
        foundry
            .register(&mut factory, &strategy, holdout(), now())
            .is_err(),
        "the same candidate was registered twice"
    );
    Ok(())
}

#[test]
fn the_compiled_form_and_the_arena_it_indexes_come_from_one_compilation() -> Result<()> {
    // `StrategyCandidate::new` refuses a compiled strategy whose plan names a
    // node its program does not contain. That check is only meaningful if the
    // foundry hands over the compiler's own arena, so this is the test that
    // the seam is wired to the right program rather than a fresh one.
    let mut foundry = foundry(13)?;
    let mut factory = StrategyFactory::new();
    foundry.search(40)?;

    let strategy = first_pending(&foundry)?;
    foundry.register(&mut factory, &strategy, holdout(), now())?;

    let registered = factory
        .candidate(&strategy)
        .ok_or_else(|| qip_core::error::Error::not_found("the registered candidate"))?;
    for node in registered.compiled().plan() {
        assert!(
            registered.program().node(*node).is_some(),
            "the compiled form indexes a node the registered arena does not hold"
        );
    }
    assert_eq!(registered.cell(), "cell-london");
    assert_eq!(*registered.venue(), venue());
    Ok(())
}

#[test]
fn a_search_is_reproducible_from_its_seed() -> Result<()> {
    // Two foundries, same seed, same vocabulary: the same strategies in the
    // same order. A search nobody can replay is a search whose result nobody
    // can check.
    let mut first = foundry(101)?;
    let mut second = foundry(101)?;
    let first_round = first.search(30)?;
    let second_round = second.search(30)?;

    assert_eq!(
        first_round, second_round,
        "the same seed produced a different round"
    );

    // Compare what the strategies *do*, not what they are called. Ids are a
    // lineage and a counter, so comparing them would pass even if the two
    // searches had written completely different rules.
    let rules_of = |foundry: &StrategyFoundry| -> Vec<String> {
        foundry
            .pending()
            .iter()
            .map(|candidate| format!("{:?}", candidate.spec().rules))
            .collect()
    };
    assert_eq!(
        rules_of(&first),
        rules_of(&second),
        "the same seed wrote different strategies"
    );
    let first_ids: Vec<_> = first.pending().iter().map(|c| c.id().clone()).collect();
    let second_ids: Vec<_> = second.pending().iter().map(|c| c.id().clone()).collect();
    assert_eq!(
        first_ids, second_ids,
        "the same seed produced different ids"
    );

    // A different seed writes different strategies *and* names them
    // differently, so two searches cannot collide on the ladder.
    let mut other = foundry(102)?;
    other.search(30)?;
    assert_ne!(
        rules_of(&first),
        rules_of(&other),
        "two seeds wrote identical strategies"
    );
    let other_ids: Vec<_> = other.pending().iter().map(|c| c.id().clone()).collect();
    assert_ne!(
        first_ids, other_ids,
        "two searches minted colliding ids; the factory keys by id and would confuse them"
    );
    Ok(())
}

#[test]
fn a_foundry_refuses_a_search_of_nothing_and_an_unnamed_cell() -> Result<()> {
    let mut foundry = foundry(1)?;
    assert!(
        foundry.search(0).is_err(),
        "a search of no candidates was recorded as a round"
    );
    assert_eq!(foundry.rounds(), 0);

    let on = subject();
    let grammar = Grammar::over(FeaturePalette::from_catalogue(&catalogue(&on)?, &on)?);
    assert!(
        StrategyFoundry::new(catalogue(&on)?, grammar, "  ", venue(), "unnamed", 1).is_err(),
        "a foundry was built without naming the cell its capital would be granted to"
    );
    Ok(())
}

#[test]
fn every_generated_strategy_carries_an_expiry() -> Result<()> {
    // A signal without an expiry gets acted on at the worst possible moment.
    // The grammar is what guarantees it here, and a generated strategy is
    // exactly where nobody is watching.
    let mut foundry = foundry(31)?;
    foundry.search(40)?;
    for candidate in foundry.pending() {
        assert!(
            candidate.compiled().validity() > Duration::ZERO,
            "{} was generated with no signal expiry",
            candidate.id()
        );
    }
    Ok(())
}
