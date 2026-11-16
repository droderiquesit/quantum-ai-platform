//! R12-TICK-2084, ADR 0609: the proposal book is rebuilt from the log at
//! start-up. A file-backed log needs a temp path, which the kernel's `src/`
//! may not name (`architecture`'s environment-read guard), so this lives here.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::expect_used)]

use qip_core::Context;
use qip_core::time::Timestamp;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::mismatch::{
    HypothesisStatus, PROPOSAL_FAMILY, PROPOSED_CAPACITY, ProposedHypothesis, from_log,
};
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn proposal(n: usize) -> ProposedHypothesis {
    ProposedHypothesis {
        event: format!("evt-{n}"),
        instrument: format!("INST{n}"),
        expected_direction: 1.0,
        measured_bps: -10.0,
        status: HypothesisStatus::Proposed,
    }
}

/// A restarted platform over the same log has the book the first one
/// had: the working set, the trials spent (the budget is not handed back
/// by a restart) and a journal sequence that continues, so no key repeats.
///
/// Mutations: replace `resume`'s `restored` call with `Book::new()` — the
/// working-set assertion fails; make `resume` return sequence 0 — the
/// "no key repeated" assertion fails; skip `restore_spent` — the budget
/// assertion fails.
#[test]
fn a_restart_rebuilds_the_book_from_the_log_keeps_the_spend_and_continues_the_sequence() {
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-mismatch-resume-{}-{}",
        std::process::id(),
        at().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("events.jsonl");
    let make = || -> Platform {
        let config = PlatformConfig::default().with_event_log_file(&path);
        let (context, _clock) = Context::deterministic(at(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    };
    let budget = {
        let mut first = make();
        let budget = first.falsification().ledger().budget();
        let admission = first
            .admit_proposed_hypotheses((0..budget).map(proposal).collect(), at())
            .expect("admits");
        assert_eq!(admission.admitted, budget, "premise: the budget was spent");
        budget
    };
    let mut second = make();
    let book = second.falsification();
    assert_eq!(
        book.ledger().spent(PROPOSAL_FAMILY),
        budget,
        "a restart handed the family a fresh budget"
    );
    assert_eq!(
        book.proposed().len(),
        PROPOSED_CAPACITY,
        "the working set was not rebuilt from the log"
    );
    assert_eq!(
        book.proposals_evicted() as usize,
        budget - PROPOSED_CAPACITY
    );
    let again = second
        .admit_proposed_hypotheses(vec![proposal(budget + 1)], at())
        .expect("admits");
    assert_eq!(again.refused_on_budget, 1, "the spent budget was forgotten");
    let log = from_log(second.event_log()).expect("decodes");
    let mut sequences: Vec<u64> = log.iter().map(|e| e.sequence).collect();
    let before = sequences.len();
    sequences.sort_unstable();
    sequences.dedup();
    assert_eq!(sequences.len(), before, "a journal key was repeated");
    let _ = std::fs::remove_dir_all(&directory);
}
