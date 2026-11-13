//! ADR 0399 decision 1: a currency conversion on the paper desk is an FX spot
//! fill, booked by the platform as a position in the pair, and that is the
//! one record of it. `qip_capital::bank::CurrencyBook` stays a tested model
//! of its rule — both legs together or not at all — and nothing that ships
//! constructs one.
//!
//! The failure this prevents is two books about the same cash. A
//! `CurrencyBook` opened in the kernel would refuse to sell a balance it did
//! not hold while the portfolio held a short in the pair, and the two would
//! disagree the first time either was right. R12-COVERAGE-5098 named the
//! missing caller as a gap; this suite is what stops a lane closing it by
//! opening a second writer.

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
/// `const _SECOND_BOOK: fn() = || drop(qip_capital::bank::CurrencyBook::new());`
/// to `qip-kernel/src/financing.rs`.
#[test]
fn no_production_source_outside_the_bank_module_names_the_currency_book() {
    let root = qip_acceptance::repository_root();
    let files = qip_acceptance::files_with_extension("backend/crates", "rs");
    let mut scanned = 0usize;
    let mut saw_financing = false;
    let mut offenders = Vec::new();
    for path in &files {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative.contains("/tests/") || relative == BANK {
            continue;
        }
        scanned += 1;
        saw_financing |= relative.ends_with("qip-kernel/src/financing.rs");
        let text = std::fs::read_to_string(path).expect("a source file is readable");
        let code = qip_acceptance::production_text(&text);
        if names(&code, "CurrencyBook") {
            offenders.push(relative);
        }
    }
    // Premises: the walk covered the workspace and the kernel module that
    // prices conversions, and the matcher fires on the file that declares
    // the type.
    assert!(scanned > 500, "only {scanned} production sources scanned");
    assert!(
        saw_financing,
        "the scan must cover the kernel's financing.rs"
    );
    let bank = qip_acceptance::production_text(&qip_acceptance::read(BANK));
    assert!(
        names(&bank, "CurrencyBook") && !names(&bank, "CurrencyBoo"),
        "premise: the matcher finds CurrencyBook in bank.rs as a whole identifier"
    );
    assert!(
        offenders.is_empty(),
        "production code names CurrencyBook: {offenders:?}. ADR 0399 decision 1: a conversion \
         on the paper desk is an FX spot fill and the platform's position in the pair is its one \
         record; a second book would be a second claim about the same cash"
    );
}
