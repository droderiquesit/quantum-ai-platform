//! The exploration budget's contract (blueprint §13.2).
//!
//! Three properties the module owes a caller, each of which a unit test
//! beside the code could state and none of which it could prove end to end:
//! the budget bounds the live probes in aggregate and not only one at a time;
//! what a kind is *measured* to resolve changes what is chosen next, which is
//! the whole of "the budget itself is optimised over time"; and two runs over
//! the same evidence choose the same probes, which is why the rule is an
//! upper-confidence bound rather than a Thompson draw.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::exploration::{
    ExplorationBook, MAXIMUM_PROBE_VALIDITY, ProbeCandidate, ProbeEvidence, ProbeKind, ProbeOutcome,
};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp, dec};

fn now() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn validity() -> Duration {
    Duration::from_hours(24)
}

fn candidate(
    kind: ProbeKind,
    subject: &str,
    uncertainty: f64,
    maximum_loss: Decimal,
) -> Result<ProbeCandidate> {
    ProbeCandidate::new(kind, subject, uncertainty, dec!("10000"), maximum_loss)
}

#[test]
fn the_live_probes_can_never_commit_more_than_the_budget() -> Result<()> {
    // One probe bounded at a quarter of the budget is the per-probe ceiling;
    // this is the other half of the same guarantee — four of them exhaust the
    // budget and the fifth is refused rather than funded out of capital
    // nobody set aside. A bound that holds one at a time and not in aggregate
    // is the shape of a control that reads as protection and is not.
    let mut book = ExplorationBook::new();
    let candidates: Vec<ProbeCandidate> = ["a", "b", "c", "d", "e", "f"]
        .into_iter()
        .map(|subject| candidate(ProbeKind::UncertainModel, subject, 0.5, dec!("250")))
        .collect::<Result<Vec<_>>>()?;
    // Premise: six candidates, each inside the per-probe ceiling of 250 on a
    // budget of 1000, so every refusal below is about the budget in aggregate
    // and not about any one probe being too large.
    assert_eq!(candidates.len(), 6);

    let plan = book.plan(dec!("1000"), &candidates, validity(), now())?;
    assert_eq!(
        plan.selected.len(),
        4,
        "the budget funded a number of probes other than the four it covers: {plan:?}"
    );
    assert_eq!(plan.committed_now, dec!("1000"));
    assert_eq!(plan.uncommitted(), Decimal::ZERO);
    assert_eq!(plan.declined.len(), 2);
    for declined in &plan.declined {
        assert!(
            declined.reason.contains("uncommitted"),
            "a candidate was declined for something other than the exhausted budget: {}",
            declined.reason
        );
    }
    book.open_plan(&plan)?;
    assert_eq!(
        book.committed(),
        dec!("1000"),
        "the live probes hold something other than the budget"
    );

    // And a second pass against the same budget opens nothing at all, because
    // every unit of it is already at risk.
    let more = book.plan(dec!("1000"), &candidates, validity(), now())?;
    assert!(
        more.selected.is_empty(),
        "a second pass funded probes out of a budget already fully committed: {more:?}"
    );
    Ok(())
}

#[test]
fn a_kind_that_has_resolved_nothing_loses_its_place_to_one_that_has_not_been_measured() -> Result<()>
{
    // "The reduction in uncertainty a probe produced is scored, so the budget
    // itself is optimised over time" — the row of §13.2 that makes this a
    // learning loop rather than a fixed heuristic. Three probes of one kind
    // that resolved nothing must cost that kind its place against an
    // unmeasured kind with the same uncertainty in front of it.
    let mut book = ExplorationBook::new();
    for subject in ["a1", "a2", "a3"] {
        let plan = book.plan(
            dec!("1000"),
            &[candidate(
                ProbeKind::CapacityAtSize,
                subject,
                0.5,
                dec!("100"),
            )?],
            validity(),
            now(),
        )?;
        book.open_plan(&plan)?;
        let id = plan.selected[0].id.clone();
        // Taken up, and it resolved nothing: the uncertainty after is the
        // uncertainty before.
        book.settle(
            &id,
            &ProbeOutcome::new(ProbeEvidence::Probed, dec!("10"), 0.5)?,
        )?;
    }
    let measured = book
        .record(ProbeKind::CapacityAtSize)
        .expect("the probed kind has a record");
    // Premise: the kind really was measured, three times, at nothing gained.
    assert_eq!(measured.probed, 3);
    assert_eq!(measured.measured_gain(), Some(0.0));

    // Two fresh subjects, equal uncertainty, neither ever probed: the only
    // difference between them is what their kind has been measured to
    // resolve.
    let plan = book.plan(
        dec!("1000"),
        &[
            candidate(ProbeKind::CapacityAtSize, "a4", 0.5, dec!("100"))?,
            candidate(ProbeKind::UncertainModel, "u1", 0.5, dec!("100"))?,
        ],
        validity(),
        now(),
    )?;
    assert_eq!(
        plan.selected.first().map(|probe| probe.kind),
        Some(ProbeKind::UncertainModel),
        "the kind measured to resolve nothing kept its place: {plan:?}"
    );
    Ok(())
}

#[test]
fn two_runs_over_the_same_evidence_select_the_same_probes_in_the_same_order() -> Result<()> {
    // Why the rule is an upper-confidence bound and not a Thompson draw: a
    // replay has to reach the same capital decisions. A sampler would make
    // the exploration budget the one allocation in this platform that could
    // not be reproduced from the log, and an allocation nobody can reproduce
    // is one nobody can audit.
    let mut book = ExplorationBook::new();
    let candidates = vec![
        candidate(ProbeKind::UncertainModel, "detector:gap", 0.7, dec!("100"))?,
        candidate(ProbeKind::StaleEstimate, "family:carry", 0.7, dec!("100"))?,
        candidate(
            ProbeKind::CapacityAtSize,
            "instrument:aaa",
            0.4,
            dec!("100"),
        )?,
    ];
    let first = book.plan(dec!("1000"), &candidates, validity(), now())?;
    let second = book.plan(dec!("1000"), &candidates, validity(), now())?;
    // Premise: there is something to be in an order about, and two of the
    // three candidates score identically — equal uncertainty, equal value,
    // equal history — so the order between them is decided by the tie-break
    // and by nothing else.
    assert!(
        first.selected.len() > 1,
        "fewer than two probes were selected, so an ordering proves nothing: {first:?}"
    );
    assert_eq!(first, second, "two plans over the same evidence differ");

    // The half that can actually fail: the same evidence presented in the
    // opposite order. A rule that let the caller's enumeration order decide a
    // tie would reproduce only for a caller who built the list the same way,
    // which is not a reproducible allocation — it is one that happens to
    // agree with itself.
    let reversed: Vec<_> = candidates.iter().rev().cloned().collect();
    let backwards = book.plan(dec!("1000"), &reversed, validity(), now())?;
    assert_eq!(
        first.selected, backwards.selected,
        "the selection depends on the order the candidates were handed in"
    );

    // And the same holds once the book has state: opening the first plan and
    // planning again twice must still agree with itself.
    book.open_plan(&first)?;
    let third = book.plan(dec!("1000"), &candidates, validity(), now())?;
    let fourth = book.plan(dec!("1000"), &candidates, validity(), now())?;
    assert_eq!(third, fourth);
    Ok(())
}

#[test]
fn a_probe_asking_to_outlive_the_ceiling_is_refused_rather_than_shortened() -> Result<()> {
    // The failure this prevents is the budget's own version of a control that
    // reads as protection and is not. `due` is the only thing that closes a
    // probe nobody settles or abandons and it decides on `expires_at`, so a
    // probe granted an unbounded validity never comes due, `committed()`
    // counts its maximum loss for the life of the process, and every later
    // candidate is declined *with a reason* while exploration has stopped.
    // Truncating to the ceiling instead would be worse than refusing: the
    // caller would hold an expiry the probe is not running under.
    let book = ExplorationBook::new();
    let candidates = vec![candidate(ProbeKind::UncertainModel, "a", 0.5, dec!("250"))?];

    // Premise, stated before anything is asserted about the refusal: the
    // ceiling is well inside the clock's range, so the over-ceiling value
    // below is representable and this test is about the ceiling rather than
    // about arithmetic. The sibling test owns the arithmetic.
    assert!(MAXIMUM_PROBE_VALIDITY < Duration::from_nanos(i64::MAX));
    let over = MAXIMUM_PROBE_VALIDITY + Duration::from_secs(1);

    let refused = book
        .plan(dec!("1000"), &candidates, over, now())
        .expect_err("a validity past the ceiling was accepted");
    assert_eq!(refused.code(), "denied");
    assert!(
        refused.message().contains("refused rather than truncated"),
        "the refusal did not say it refuses rather than shortens: {}",
        refused.message()
    );

    // And it admits a good one. A gate that refused everything would pass the
    // half of this test above and protect nothing, so the ceiling itself is
    // planned against and the probe it yields is checked for the expiry it
    // was actually given.
    let plan = book.plan(dec!("1000"), &candidates, MAXIMUM_PROBE_VALIDITY, now())?;
    assert_eq!(
        plan.selected.len(),
        1,
        "planning at exactly the ceiling funded nothing: {plan:?}"
    );
    let probe = &plan.selected[0];
    assert_eq!(
        probe.expires_at,
        now().saturating_add(MAXIMUM_PROBE_VALIDITY)
    );
    // The property the whole ceiling exists for: the probe really does lapse,
    // rather than carrying the `Timestamp::MAX` sentinel that means "no upper
    // bound" and can never be reached.
    assert_ne!(probe.expires_at, Timestamp::MAX);
    assert!(!probe.is_expired(now()));
    assert!(probe.is_expired(probe.expires_at));
    Ok(())
}

#[test]
fn a_probe_whose_expiry_runs_past_the_end_of_the_clock_is_refused_rather_than_saturated()
-> Result<()> {
    // `Timestamp::saturating_add` lands on `Timestamp::MAX`, which
    // `qip_core::Timestamp` documents as the sentinel meaning "no upper
    // bound" — so saturating here produced exactly the never-lapsing probe
    // the ceiling above exists to prevent, by a path the ceiling cannot see.
    // `Timestamp::MAX` is a real constructible value that point-in-time views
    // pass around, so this arm is reachable rather than decorative.
    let book = ExplorationBook::new();
    let candidates = vec![candidate(ProbeKind::UncertainModel, "a", 0.5, dec!("250"))?];

    // Premise: this validity is inside the ceiling, so the refusal below is
    // the arithmetic guard and not the ceiling guard firing again. Without
    // this the test would pass while proving the wrong control.
    assert!(validity() <= MAXIMUM_PROBE_VALIDITY);
    // Premise: the instant really is the end of the clock, which is what
    // makes the addition overflow.
    assert_eq!(Timestamp::MAX.as_nanos(), i64::MAX);

    let refused = book
        .plan(dec!("1000"), &candidates, validity(), Timestamp::MAX)
        .expect_err("an expiry past the end of the clock was accepted");
    assert_eq!(refused.code(), "numeric");
    assert!(
        refused.message().contains("past the end of"),
        "the refusal did not name the overflow: {}",
        refused.message()
    );
    // What the old arithmetic would have produced, asserted so the reader can
    // see the defect this guards and not merely the guard.
    assert_eq!(Timestamp::MAX.saturating_add(validity()), Timestamp::MAX);
    assert_eq!(book.open_count(), 0);
    Ok(())
}
