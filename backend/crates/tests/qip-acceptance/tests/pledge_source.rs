//! ADR 0387: what is pledged has one source, the ledger's collateral
//! balance, read through `qip_contracts::ledger::LedgerObligations` into the
//! survival waterfall. `qip_capital::bank::PostingBook` is retired from
//! composition: it stays a tested model of one rule (collateral is
//! encumbered from the instant of posting until the venue confirms release)
//! and nothing that ships may construct one.
//!
//! The failure this prevents is two books about the same pledge. A
//! `PostingBook` opened in the kernel would be fed by whoever opened it, the
//! ledger by its own operations, and the survival waterfall would withhold
//! one figure while the collateral graph was built from the other. Lane L07
//! declined to build that producer on 2026-10-18 for this reason; this suite
//! is what stops the next lane building it by accident.
//!
//! It cannot be derived from the ledger either, yet. The ledger's
//! `CollateralPledge` record is the only posting record there is, and ADR
//! 0153 decision 3 declines every producer of it until a venue statement
//! states a pledge (`ledger_declined_kinds.rs` holds that). When one does,
//! the producer and the derivation are one piece of work under a new ADR.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

const BANK: &str = "backend/crates/services/qip-capital/src/bank.rs";

/// Whether `code` names `name` as a whole identifier.
fn names(code: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    code.match_indices(name).any(|(at, _)| {
        let before = code[..at].chars().next_back();
        let after = code[at + name.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Mutation, fails this test: add
/// `const _RETIRED: fn() = || drop(qip_capital::bank::PostingBook::default());`
/// to `qip-kernel/src/cross_margin.rs`.
#[test]
fn no_production_source_outside_the_bank_module_names_the_posting_book() {
    let root = qip_acceptance::repository_root();
    let files = qip_acceptance::files_with_extension("backend/crates", "rs");
    let mut scanned = 0usize;
    let mut saw_kernel = false;
    let mut offenders = Vec::new();
    for path in &files {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        // A crate's `tests/` directory and the acceptance crate are tests.
        if relative.contains("/tests/") || relative == BANK {
            continue;
        }
        scanned += 1;
        saw_kernel |= relative.ends_with("qip-kernel/src/cross_margin.rs");
        let text = std::fs::read_to_string(path).expect("a source file is readable");
        let code = qip_acceptance::production_text(&text);
        if names(&code, "PostingBook") {
            offenders.push(relative);
        }
    }
    // Premises: the walk covered the workspace and the kernel module that
    // builds the collateral graph, and the matcher fires on the one file
    // that does declare the type.
    assert!(scanned > 500, "only {scanned} production sources scanned");
    assert!(
        saw_kernel,
        "the scan must cover the kernel's cross_margin.rs"
    );
    let bank = qip_acceptance::production_text(&qip_acceptance::read(BANK));
    assert!(
        names(&bank, "PostingBook") && !names(&bank, "PostingBoo"),
        "premise: the matcher finds PostingBook in bank.rs as a whole identifier"
    );
    assert!(
        offenders.is_empty(),
        "production code names PostingBook: {offenders:?}. ADR 0387 retires it from \
         composition: what is pledged is read from the ledger through LedgerObligations, and a \
         second book would be a second claim about the same collateral"
    );
}

/// The one source is still wired: the kernel still takes the ledger's
/// obligations and the survival figures still read them. Without this the
/// test above could pass on a tree where nothing reads a pledge at all.
///
/// Mutation, fails this test: in `qip-api/src/routes.rs`, replace the
/// `platform.observe_ledger_obligations(obligations)` call with a `drop`.
#[test]
fn the_ledgers_obligations_are_still_the_reading_the_kernel_and_the_api_cycle_take() {
    let platform = qip_acceptance::production_text(&qip_acceptance::read(
        "backend/crates/runtime/qip-kernel/src/platform.rs",
    ));
    assert!(
        names(&platform, "observe_ledger_obligations") && names(&platform, "LedgerObligations"),
        "Platform::observe_ledger_obligations is gone; the ledger's pledge no longer reaches \
         the survival waterfall"
    );
    let routes = qip_acceptance::production_text(&qip_acceptance::read(
        "backend/crates/apps/qip-api/src/routes.rs",
    ));
    assert!(
        names(&routes, "observe_ledger_obligations"),
        "qip-api's cycle no longer hands the ledger's obligations to the platform"
    );
}
