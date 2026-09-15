//! Blueprint §14.2's source register and §14.3's `Testing` gate.
//!
//! The property every test here is really about is one: a falsifier may only
//! be evaluated against records that became **knowable** after the hypothesis
//! was formed. A falsifier scored against data the hypothesis could already
//! see produces a verdict, and the verdict is worthless — which is worse than
//! producing none, because a number gets believed.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_world_model::falsification::{
    BLUEPRINT_SOURCES, Breach, FalsificationPass, Falsifier, HeldOut, HypothesisSource,
    Inadmissible, SOURCES, SourceCensus, SourceStanding, TrialLedger, Verdict,
};
use qip_world_model::features::FeatureValue;

// --- fixtures ---------------------------------------------------------------

fn formed() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn days(n: i64) -> Duration {
    Duration::from_days(n)
}

/// The claim under test in every fixture: `AAA`'s close will rise from 100,
/// contradicted by it reaching 99 or below.
fn falsifier() -> Result<Falsifier> {
    Falsifier::new(
        "AAA's close falls back to 99",
        "close",
        "AAA",
        Breach::FallsTo,
        99.0,
        3,
    )
}

/// A value true at `valid_at` and knowable at `available_at`.
fn value(v: f64, valid_at: Timestamp, available_at: Timestamp) -> FeatureValue {
    FeatureValue::new(v, valid_at, available_at)
}

// --- the leakage boundary ---------------------------------------------------

#[test]
fn a_record_knowable_before_the_hypothesis_was_formed_is_refused_as_in_sample() -> Result<()> {
    // The defect this prevents: evaluating a falsifier against the very data
    // the hypothesis was built on. The record below breaches the falsifier
    // outright, so if it were admitted the claim would be reported refuted —
    // on evidence it already had when it was written.
    let boundary = HeldOut::between(formed(), formed().saturating_add(days(10)))?;
    let leaked = value(
        80.0,
        formed().saturating_sub(days(2)),
        formed().saturating_sub(days(1)),
    );

    // Premise: this record would fire the falsifier if it were admissible.
    // Without this the refusal below could pass on a record that says nothing.
    assert!(
        leaked.value <= falsifier()?.level(),
        "the fixture record at {} does not breach the level {}, so admitting it would prove \
         nothing either way",
        leaked.value,
        falsifier()?.level()
    );

    assert_eq!(
        boundary.admit(&leaked).err(),
        Some(Inadmissible::InSample),
        "a record knowable before the hypothesis was formed was treated as held-out data"
    );

    // And the ledger refuses the whole sample rather than dropping the
    // offender: a sample that silently shrinks is a leak that already
    // happened and left no trace.
    let mut ledger = TrialLedger::new();
    let refusal = ledger.test("anomaly", &falsifier()?, &boundary, &[&leaked]);
    assert!(
        refusal.is_err(),
        "the ledger evaluated a falsifier against in-sample data instead of refusing it"
    );
    assert_eq!(
        ledger.spent("anomaly"),
        0,
        "a refused sample charged a trial against the family's budget"
    );
    Ok(())
}

#[test]
fn a_bar_that_opened_before_formation_and_closed_after_it_is_held_out_data() -> Result<()> {
    // The distinction only a bitemporal store can draw, and the reason this
    // module reads `available_at` rather than `valid_at`. The bar below
    // describes a bucket that opened before the claim was written; its close
    // was not knowable until the bucket shut, which was after. It is held-out
    // data, and a filter keyed on valid time would have thrown it away.
    let boundary = HeldOut::between(formed(), formed().saturating_add(days(10)))?;
    let straddling = value(
        101.0,
        formed().saturating_sub(days(1)),
        formed().saturating_add(days(1)),
    );

    // Premise: the record really does straddle the boundary in valid time.
    assert!(
        straddling.valid_at < boundary.formed_at(),
        "the fixture does not straddle formation, so it cannot show the distinction"
    );
    assert!(
        boundary.admit(&straddling).is_ok(),
        "a record that became knowable after formation was withheld, which is a valid-time filter wearing a bitemporal one's clothes"
    );

    // And the mirror case: valid recently, knowable long ago is in-sample.
    let restated = value(
        101.0,
        formed().saturating_add(days(1)),
        formed().saturating_sub(days(1)),
    );
    assert_eq!(
        boundary.admit(&restated).err(),
        Some(Inadmissible::KnowableBeforeTrue),
        "a record stamped knowable before it was true was admitted"
    );
    Ok(())
}

#[test]
fn a_record_not_knowable_by_the_evaluation_instant_is_refused() -> Result<()> {
    // Leakage running the other way: an evaluation reading its own future.
    let evaluated_at = formed().saturating_add(days(10));
    let boundary = HeldOut::between(formed(), evaluated_at)?;
    let future = value(
        80.0,
        formed().saturating_add(days(5)),
        evaluated_at.saturating_add(days(1)),
    );

    // Premise: it is inside the held-out window in valid time, so only the
    // knowable instant can exclude it.
    assert!(
        future.valid_at > boundary.formed_at() && future.valid_at < evaluated_at,
        "the fixture is outside the window in valid time, so the valid-time filter would \
         already have caught it"
    );
    assert_eq!(
        boundary.admit(&future).err(),
        Some(Inadmissible::NotYetKnowable),
        "an evaluation was allowed to read a record that had not become knowable yet"
    );
    Ok(())
}

#[test]
fn a_non_finite_observation_is_refused_rather_than_reported_as_survival() -> Result<()> {
    // A NaN compares false against every threshold, so admitting one would
    // report the falsifier survived on evidence that says nothing at all.
    let boundary = HeldOut::between(formed(), formed().saturating_add(days(10)))?;
    let nan = value(
        f64::NAN,
        formed().saturating_add(days(1)),
        formed().saturating_add(days(2)),
    );
    assert!(
        !nan.value.is_finite(),
        "the fixture is not the case under test"
    );
    assert_eq!(
        boundary.admit(&nan).err(),
        Some(Inadmissible::NotFinite),
        "a non-finite observation was admitted to a held-out sample"
    );
    Ok(())
}

#[test]
fn a_held_out_window_that_opens_at_or_before_formation_is_refused() -> Result<()> {
    // Refused rather than swapped: a window that ran backwards would admit
    // exactly the records the hypothesis was built on, and correcting the
    // caller's arithmetic would keep its bug in production.
    assert!(
        HeldOut::between(formed(), formed()).is_err(),
        "a window with no elapsed time was accepted, so every in-sample record is held out"
    );
    assert!(
        HeldOut::between(formed(), formed().saturating_sub(days(1))).is_err(),
        "a window running backwards was accepted"
    );
    assert!(
        HeldOut::between(formed(), formed().saturating_add(Duration::from_secs(1))).is_ok(),
        "a window of one second was refused, so the gate refuses everything"
    );
    Ok(())
}

#[test]
fn a_partition_counts_what_it_withheld_rather_than_dropping_it_silently() -> Result<()> {
    // The difference between "tested on four days of new data" and "tested on
    // four days, three of which it had already seen".
    let evaluated_at = formed().saturating_add(days(10));
    let boundary = HeldOut::between(formed(), evaluated_at)?;
    let in_sample = value(
        100.0,
        formed().saturating_sub(days(2)),
        formed().saturating_sub(days(1)),
    );
    let held_out = value(
        101.0,
        formed().saturating_add(days(1)),
        formed().saturating_add(days(1)),
    );
    let future = value(
        102.0,
        formed().saturating_add(days(5)),
        evaluated_at.saturating_add(days(1)),
    );
    let history = [&in_sample, &held_out, &future];

    // Premise: the history really does hold all three kinds.
    assert_eq!(
        history.len(),
        3,
        "the fixture lost a record before the test ran"
    );

    let (sample, tally) = boundary.partition(&history);
    assert_eq!(
        sample.len(),
        1,
        "the partition admitted something it should have withheld"
    );
    assert_eq!(tally.in_sample, 1, "the in-sample record was not counted");
    assert_eq!(
        tally.not_yet_knowable, 1,
        "the not-yet-knowable record was not counted"
    );
    assert_eq!(
        tally.total(),
        2,
        "the tally does not account for everything withheld"
    );
    assert!(
        !tally.is_empty(),
        "a tally holding two records reports itself empty"
    );
    Ok(())
}

// --- derived statistics -----------------------------------------------------

#[test]
fn a_rolling_statistic_is_knowable_only_when_its_latest_input_is() -> Result<()> {
    // The subtlest form of the leak, and the one a per-record filter cannot
    // catch. A statistic over a window is knowable when the *last* record in
    // that window arrives, and "last" means last to become knowable, not last
    // in valid time. A restatement published late, sitting in the middle of a
    // window, makes the whole window knowable later than its final element —
    // and a derived record stamped with its final element's availability
    // would be readable before it could have been computed.
    let late = value(
        3.0,
        formed().saturating_add(days(2)),
        formed().saturating_add(days(9)),
    );
    let first = value(
        1.0,
        formed().saturating_add(days(1)),
        formed().saturating_add(days(1)),
    );
    let last = value(
        5.0,
        formed().saturating_add(days(3)),
        formed().saturating_add(days(3)),
    );

    // Premise: the late record is in the middle in valid time and last in
    // knowable time, which is the whole point of the fixture.
    assert!(
        late.valid_at < last.valid_at,
        "the fixture's late record is not in the middle"
    );
    assert!(
        late.available_at > last.available_at,
        "the fixture's late record is not the last to arrive"
    );

    let derived =
        qip_world_model::falsification::rolling_statistic(&[&first, &late, &last], 3, |window| {
            window.iter().sum::<f64>() / window.len() as f64
        })?;
    assert_eq!(
        derived.len(),
        1,
        "one window over three records produced {} statistic(s)",
        derived.len()
    );
    assert_eq!(
        derived[0].valid_at, last.valid_at,
        "the statistic describes an instant other than its window's last"
    );
    assert_eq!(
        derived[0].available_at, late.available_at,
        "the statistic was stamped knowable before the record that completed it had arrived, \
         which makes it readable before it could have been computed"
    );
    Ok(())
}

#[test]
fn a_rolling_statistic_over_held_out_records_is_itself_held_out() -> Result<()> {
    // Why the kernel derives realised volatility from an already-partitioned
    // close series rather than from the store: a twenty-day volatility
    // computed over whatever window is to hand is mostly made of closes the
    // hypothesis could already see, and a filter applied to the *statistic*
    // would never notice.
    let boundary = HeldOut::between(formed(), formed().saturating_add(days(10)))?;
    let held_out: Vec<FeatureValue> = (1..=4)
        .map(|i| {
            value(
                100.0 + f64::from(i),
                formed().saturating_add(days(i64::from(i))),
                formed().saturating_add(days(i64::from(i))),
            )
        })
        .collect();
    let refs: Vec<&FeatureValue> = held_out.iter().collect();

    // Premise: every input is held out.
    assert!(
        refs.iter().all(|value| boundary.admit(value).is_ok()),
        "the fixture's inputs are not all held out, so the property under test is untestable"
    );

    let derived = qip_world_model::falsification::rolling_statistic(&refs, 2, |window| {
        window.iter().sum::<f64>() / window.len() as f64
    })?;
    assert_eq!(
        derived.len(),
        3,
        "three windows of two over four records were not produced"
    );
    for statistic in &derived {
        assert!(
            boundary.admit(statistic).is_ok(),
            "a statistic derived only from held-out records was itself refused as {:?}",
            boundary.admit(statistic).err()
        );
    }
    Ok(())
}

#[test]
fn a_rolling_statistic_refuses_an_unordered_series_and_a_zero_window() -> Result<()> {
    let later = value(
        2.0,
        formed().saturating_add(days(2)),
        formed().saturating_add(days(2)),
    );
    let earlier = value(
        1.0,
        formed().saturating_add(days(1)),
        formed().saturating_add(days(1)),
    );

    // Premise: the pair really is out of valid-time order.
    assert!(
        later.valid_at > earlier.valid_at,
        "the fixture is already in order"
    );
    assert!(
        qip_world_model::falsification::rolling_statistic(&[&later, &earlier], 2, |w| w[0])
            .is_err(),
        "an out-of-order series was sorted rather than refused, which silently changes what the \
         statistic is a statistic of"
    );
    assert!(
        qip_world_model::falsification::rolling_statistic(&[&earlier, &later], 0, |w| w[0])
            .is_err(),
        "a zero window was accepted, deriving a statistic from no data at all"
    );
    // A window longer than the series is not an error: it is the ordinary
    // state of a claim that has not gathered enough held-out data yet, and
    // the empty result becomes `Undetermined` rather than a stage problem.
    assert!(
        qip_world_model::falsification::rolling_statistic(&[&earlier, &later], 5, |w| w[0])?
            .is_empty(),
        "a window longer than the series produced a statistic from records that do not exist"
    );
    assert_eq!(
        qip_world_model::falsification::rolling_statistic(&[&earlier, &later], 2, |w| w[0])?.len(),
        1,
        "a window equal to the series produced no statistic, so the function refuses everything"
    );
    Ok(())
}

// --- the verdicts -----------------------------------------------------------

#[test]
fn a_held_out_observation_that_contradicts_the_claim_refutes_it_and_records_the_refutation()
-> Result<()> {
    let boundary = HeldOut::between(formed(), formed().saturating_add(days(10)))?;
    let falsifier = falsifier()?;
    let breaching = value(
        98.0,
        formed().saturating_add(days(1)),
        formed().saturating_add(days(2)),
    );

    // Premise: nothing has been refuted, and the observation is genuinely
    // held out rather than in-sample data the test smuggled in.
    let mut ledger = TrialLedger::new();
    assert_eq!(
        ledger.refutations(),
        0,
        "the ledger began with a refutation in it"
    );
    assert!(
        boundary.admit(&breaching).is_ok(),
        "the fixture record is not held-out data"
    );

    let verdict = ledger.test("anomaly", &falsifier, &boundary, &[&breaching])?;
    match verdict {
        Verdict::Refuted {
            observed,
            knowable_at,
            ..
        } => {
            assert!(
                observed <= falsifier.level(),
                "a refutation on an observation that did not breach the level"
            );
            assert!(
                knowable_at > boundary.formed_at(),
                "the refuting observation was knowable before the claim was formed"
            );
        }
        other => panic!(
            "a breaching held-out observation produced {}",
            other.as_str()
        ),
    }

    // §14.3's last row: a refuted hypothesis is recorded so the same idea
    // proposed again is recognisable as one that already failed.
    assert!(
        ledger.already_refuted("anomaly", falsifier.statement()),
        "the refutation was not recorded, so the same idea can be re-proposed unchallenged"
    );
    assert!(
        !ledger.already_refuted("other_family", falsifier.statement()),
        "a refutation under one family was charged to another"
    );
    assert_eq!(
        ledger.spent("anomaly"),
        1,
        "the evaluation was not charged a trial"
    );
    Ok(())
}

#[test]
fn a_falsifier_that_survives_too_little_held_out_data_is_undetermined_rather_than_supported()
-> Result<()> {
    // A hypothesis nothing has contradicted yet and a hypothesis that
    // withstood evidence are not the same claim, and reporting the first as
    // the second is how the Testing gate clears everything it sees.
    let boundary = HeldOut::between(formed(), formed().saturating_add(days(10)))?;
    let falsifier = falsifier()?;
    let one = value(
        105.0,
        formed().saturating_add(days(1)),
        formed().saturating_add(days(1)),
    );
    let two = value(
        106.0,
        formed().saturating_add(days(2)),
        formed().saturating_add(days(2)),
    );

    // Premise: neither observation breaches, so the only question is how many.
    assert!(
        one.value > falsifier.level() && two.value > falsifier.level(),
        "the fixture breaches the falsifier, so this tests refutation and not sufficiency"
    );
    assert_eq!(
        falsifier.minimum_observations(),
        3,
        "the fixture's bar moved"
    );

    let mut ledger = TrialLedger::new();
    let thin = ledger.test("anomaly", &falsifier, &boundary, &[&one, &two])?;
    assert_eq!(
        thin,
        Verdict::Undetermined {
            observations: 2,
            required: 3
        },
        "two observations cleared a gate that asks for three"
    );

    let three = value(
        107.0,
        formed().saturating_add(days(3)),
        formed().saturating_add(days(3)),
    );
    let enough = ledger.test("anomaly", &falsifier, &boundary, &[&one, &two, &three])?;
    assert_eq!(
        enough,
        Verdict::Survived { observations: 3 },
        "three observations did not clear a gate that asks for three, so it refuses everything"
    );
    assert_eq!(
        ledger.spent("anomaly"),
        2,
        "two evaluations charged something other than two trials"
    );
    Ok(())
}

#[test]
fn a_falsifier_that_could_report_survival_on_no_evidence_at_all_is_refused() -> Result<()> {
    assert!(
        Falsifier::new("x", "close", "AAA", Breach::FallsTo, 99.0, 0).is_err(),
        "a falsifier needing zero observations was accepted, so it clears the Testing gate the \
         instant a hypothesis is formed"
    );
    assert!(
        Falsifier::new("x", "close", "AAA", Breach::FallsTo, f64::NAN, 3).is_err(),
        "a non-finite level was accepted; it compares false against every observation and so \
         reports survival forever"
    );
    assert!(
        Falsifier::new("  ", "close", "AAA", Breach::FallsTo, 99.0, 3).is_err(),
        "a falsifier with no statement was accepted"
    );
    assert!(
        Falsifier::new("x", "close", "   ", Breach::FallsTo, 99.0, 3).is_err(),
        "a falsifier naming no subject was accepted, so a claim about one instrument can be \
         settled by an observation of another"
    );
    assert!(
        Falsifier::new("x", "close", "AAA", Breach::FallsTo, 99.0, 3).is_ok(),
        "a well-formed falsifier was refused, so the constructor refuses everything"
    );
    Ok(())
}

// --- the trial budget -------------------------------------------------------

#[test]
fn a_family_that_has_spent_its_trial_budget_is_refused_a_further_test() -> Result<()> {
    // §14.3's Testing row: a falsifier evaluation counts against the family's
    // cumulative trial budget. A budget that never stops anything is the
    // multiple-comparisons problem with a ledger beside it.
    let boundary = HeldOut::between(formed(), formed().saturating_add(days(10)))?;
    let falsifier = falsifier()?;
    let surviving = value(
        105.0,
        formed().saturating_add(days(1)),
        formed().saturating_add(days(1)),
    );
    let mut ledger = TrialLedger::with_bounds(2, 8)?;

    // Premise: the first two trials run.
    assert_eq!(
        ledger.budget(),
        2,
        "the fixture's budget is not what the test assumes"
    );
    for trial in 1..=2 {
        let verdict = ledger.test("anomaly", &falsifier, &boundary, &[&surviving])?;
        assert!(
            !matches!(verdict, Verdict::BudgetExhausted { .. }),
            "trial {trial} was refused while the budget still had room"
        );
    }
    assert_eq!(ledger.spent("anomaly"), 2, "two trials did not charge two");

    assert_eq!(
        ledger.test("anomaly", &falsifier, &boundary, &[&surviving])?,
        Verdict::BudgetExhausted {
            spent: 2,
            budget: 2
        },
        "a third trial ran against a budget of two"
    );
    assert_eq!(
        ledger.spent("anomaly"),
        2,
        "a refused evaluation charged a trial anyway, so the budget shrinks whenever it fires"
    );
    // A different family is untouched: the budget is per family, not global.
    assert!(
        !matches!(
            ledger.test("other_family", &falsifier, &boundary, &[&surviving])?,
            Verdict::BudgetExhausted { .. }
        ),
        "one family's exhausted budget stopped another family being tested"
    );
    Ok(())
}

#[test]
fn a_ledger_that_cannot_track_another_family_refuses_rather_than_forgetting_one() -> Result<()> {
    // Evicting a family resets its spend to zero and hands it a fresh budget,
    // which is a cap that makes the thing it caps unlimited. The ledger fails
    // closed instead.
    let boundary = HeldOut::between(formed(), formed().saturating_add(days(10)))?;
    let falsifier = falsifier()?;
    let surviving = value(
        105.0,
        formed().saturating_add(days(1)),
        formed().saturating_add(days(1)),
    );
    let mut ledger = TrialLedger::with_bounds(8, 2)?;

    ledger.test("family_one", &falsifier, &boundary, &[&surviving])?;
    ledger.test("family_two", &falsifier, &boundary, &[&surviving])?;
    // Premise: the table is full and both families are genuinely tracked.
    assert_eq!(
        ledger.families(),
        2,
        "the fixture did not fill the family table"
    );

    assert!(
        ledger
            .test("family_three", &falsifier, &boundary, &[&surviving])
            .is_err(),
        "a third family was admitted to a ledger bounded at two, so it is tracked by nothing \
         and has no budget"
    );
    assert_eq!(
        ledger.families(),
        2,
        "the refused family was recorded anyway"
    );
    assert!(
        ledger
            .test("family_one", &falsifier, &boundary, &[&surviving])
            .is_ok(),
        "a family already tracked was refused once the table filled, so a full ledger stops \
         testing everything"
    );

    assert!(
        ledger
            .test("   ", &falsifier, &boundary, &[&surviving])
            .is_err(),
        "an unnamed family was accepted, and every unnamed family shares one budget"
    );
    assert!(
        TrialLedger::with_bounds(0, 8).is_err(),
        "a zero budget was accepted; it refuses every evaluation while reporting a working gate"
    );
    assert!(
        TrialLedger::with_bounds(8, 0).is_err(),
        "a zero family bound was accepted; it refuses every hypothesis class there is"
    );
    Ok(())
}

// --- §14.2: the source register ---------------------------------------------

#[test]
fn a_source_census_enumerates_every_blueprint_source_and_says_which_are_unwired() {
    // The §14.2 gap is that a hypothesis had no recorded provenance, so a
    // platform implementing one source off the blueprint's list read exactly
    // like one implementing all six.
    let mut census = SourceCensus::new();

    // Premise: nothing is wired until something declares it. A census that
    // began with a source wired would be asserting a constant, not a fact.
    assert_eq!(census.wired(), 0, "a fresh census claims a source is wired");
    assert_eq!(
        SOURCES.len(),
        BLUEPRINT_SOURCES + 1,
        "the source table lost an entry"
    );
    for source in SOURCES {
        match census.standing(source) {
            Some(SourceStanding::Unwired { reason }) => {
                assert!(
                    !reason.trim().is_empty(),
                    "{source} is unwired for no stated reason"
                );
            }
            other => panic!("{source} began as {other:?} rather than unwired"),
        }
    }

    census.record(HypothesisSource::DetectedAnomaly);
    census.record(HypothesisSource::DetectedAnomaly);
    assert_eq!(census.proposed(), 2, "two proposals were not counted");
    assert_eq!(
        census.wired(),
        1,
        "recording a proposal did not wire its source"
    );
    assert_eq!(
        census.blueprint_sources_wired(),
        0,
        "the platform's own off-list source was counted against the §14.2 table, which reports \
         progress that has not happened"
    );

    // The silent-when-idle case: a wired source with nothing to say this
    // cycle reads as `0 proposed`, not as absence.
    census.declare(HypothesisSource::CrossAssetTransfer);
    assert_eq!(
        census.standing(HypothesisSource::CrossAssetTransfer),
        Some(&SourceStanding::Wired { proposed: 0 }),
        "a wired source that proposed nothing fell back to unwired, so an idle source is \
         indistinguishable from an unbuilt one"
    );
    assert_eq!(
        census.blueprint_sources_wired(),
        1,
        "a wired §14.2 source was not counted"
    );
}

#[test]
fn a_pass_that_tested_nothing_still_describes_itself() {
    // The failure mode this guards: a module that returns nothing when it has
    // no subject reaches no surface at all in the only state a fresh
    // deployment is ever in.
    let pass = FalsificationPass::default();
    assert_eq!(pass.tested, 0, "the fixture is not the idle case");
    let idle = pass.describe();
    assert!(
        idle.contains("0 tested against held-out data"),
        "an idle pass described itself as {idle:?}, which does not say it tested nothing"
    );
    assert!(
        SourceCensus::new().describe().contains("none proposing"),
        "a census with nothing wired does not say so"
    );

    // And a pass that did something says what: the two states are
    // distinguishable, which is the whole point of describing the idle one.
    let mut busy = FalsificationPass::default();
    busy.observe(&Verdict::Survived { observations: 5 });
    busy.observe(&Verdict::BudgetExhausted {
        spent: 64,
        budget: 64,
    });
    assert_eq!(busy.tested, 1, "a refused evaluation was counted as a test");
    assert_eq!(busy.budget_exhausted, 1, "the refusal was not counted");
    assert!(
        !busy.describe().contains("0 tested against held-out data"),
        "a pass that tested one claim describes itself as idle"
    );
}
