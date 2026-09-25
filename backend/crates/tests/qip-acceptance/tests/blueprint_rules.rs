//! Blueprint §2.1's language rule, enforced where it is held rather than
//! asserted where it is not.
//!
//! # Why this file exists
//!
//! `docs/DELIVERY-STATUS.md` scores §2.1 rule 1 — "every application is
//! written in Rust" — by running greps and reading the result. That is an
//! assertion about a tree, taken at the hour somebody ran it, and it has
//! already drifted. The row said "three Python tools and one Node one" and
//! cited `ls scripts/*.py scripts/*.mjs`; on 2026-09-21 that glob printed
//! four and two, and it never could see `scripts/venue-signup/`, which holds
//! three more `.mjs`, or `.claude/hooks/`, which holds three more `.py`.
//! Nobody was careless. A number written into prose is a measurement of
//! somebody else's checkout, which is the failure this repository has
//! recorded against itself in three separate rule files.
//!
//! The remedy those files reach for is "cite the command, not the number".
//! This file goes one step further for the half of the rule that actually
//! holds: it makes the command run in CI, so the claim fails loudly on the
//! commit that breaks it rather than quietly on the day somebody re-reads a
//! status table.
//!
//! # What is claimed here, and what is deliberately not
//!
//! §2.1 rule 1's scope names "hot path, warm services, batch jobs, solvers,
//! training, ingestion, tooling, frontend" and excludes "Python, TypeScript,
//! Java, Go, C++ as a primary language". The rule is **not held for the whole
//! of that scope** and this file does not pretend otherwise:
//!
//! * The browser layer is TypeScript by decision — ADR 0001 admits it, and
//!   ADR 0081 retires the Rust-frontend option as a deliberate non-goal. It
//!   is out of scope here because a decided deviation is not a regression.
//! * `vendor/templates/` is licensed reference material (ADR 0015), not an
//!   application this repository writes.
//! * The Python and Node tooling is a deviation **no ADR records**. That is
//!   the honest state and it is not this file's business to decide it. What
//!   this file does is stop the set from growing unnoticed, which is a
//!   different and smaller claim than the rule makes.
//!
//! So: the first test enforces the rule outright on the one surface where it
//! is held without qualification, and the second pins the accepted deviation
//! so that a fifth script has to be argued for rather than merely added.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use std::collections::BTreeSet;

/// The file extensions §2.1 rule 1 names as excluded primary languages,
/// plus the spellings each of them actually ships under.
///
/// The rule says "Python, TypeScript, Java, Go, C++". A test that looked only
/// for `.py`, `.ts`, `.java`, `.go` and `.cpp` would admit `.mjs`, `.pyi`,
/// `.tsx`, `.cc` and `.hpp`, every one of which is the same language wearing
/// a different suffix — and the one that would actually be used, because a
/// Node tool in this repository is written as `.mjs` today.
const EXCLUDED_LANGUAGE_EXTENSIONS: [&str; 16] = [
    "py", "pyi", "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "java", "go", "cpp", "cc",
    "cxx", "hpp",
];

/// Every file under `relative`, recursively, whose extension is one of
/// [`EXCLUDED_LANGUAGE_EXTENSIONS`], reported repository-relative.
fn excluded_language_files(relative: &str) -> BTreeSet<String> {
    let root = qip_acceptance::repository_root();
    let mut found = BTreeSet::new();
    for extension in EXCLUDED_LANGUAGE_EXTENSIONS {
        for path in qip_acceptance::files_with_extension(relative, extension) {
            let display = path.display().to_string();
            let relative_path = path
                .strip_prefix(&root)
                .map_or(display, |tail| tail.display().to_string());
            found.insert(relative_path);
        }
    }
    found
}

#[test]
fn every_source_file_in_the_backend_workspace_is_written_in_rust() {
    // §2.1 rule 1 on the surface where it holds without qualification. The
    // rule's own scope names the hot path, warm services, batch jobs,
    // solvers, training and ingestion, and every one of those is a crate
    // under `backend/crates`.
    //
    // The failure this prevents is not somebody rewriting a service in Go.
    // It is the build helper: one `generate.py` invoked from a `build.rs`,
    // or one `bench.mjs` beside a crate, and the workspace has a second
    // toolchain that the dependency policy, the clippy gate and
    // `check-dependencies.sh` are all blind to — none of them reads anything
    // but Rust and TOML.
    let offenders = excluded_language_files("backend/crates");

    // The premise, asserted first. `files_with_extension` returns an empty
    // vector for a directory it cannot read, so every assertion below is
    // satisfied by a walk that found nothing at all — and a path typo, a
    // move of the crate tree, or a `repository_root` that climbed the wrong
    // number of levels would each produce exactly that.
    let rust_files = qip_acceptance::files_with_extension("backend/crates", "rs");
    assert!(
        rust_files.len() > 500,
        "only {} Rust files were found under backend/crates; the walk is not \
         seeing the workspace, so the absence asserted below is trivially true",
        rust_files.len()
    );

    assert!(
        offenders.is_empty(),
        "these files under backend/crates are written in a language §2.1 rule 1 \
         excludes as a primary language: {offenders:?}. The rule's scope names \
         the hot path, warm services, batch jobs, solvers, training and \
         ingestion, and this is where all of them live. A second toolchain \
         here is invisible to check-dependencies.sh, to clippy and to the \
         dependency policy, all of which read only Rust and TOML"
    );
}

/// The non-Rust tooling this repository ships outside the browser layer and
/// outside `vendor/`, as accepted on 2026-09-21.
///
/// **This list is the assertion, not documentation of it.** The house
/// doctrine is that a list goes stale silently while a command goes stale
/// loudly — which is true of a list written into prose, where nothing runs
/// it. A list written into an assertion inverts that: it is the loudest
/// thing in the repository, because the build stops.
///
/// Sorted, repository-relative, and exactly what the walk reports, so that a
/// diff to this array is a diff a reviewer can read against the failure
/// message.
const ACCEPTED_NON_RUST_TOOLING: [&str; 12] = [
    ".claude/hooks/format-rust-after-edit.py",
    ".claude/hooks/guard-dangerous-command.py",
    ".claude/hooks/test_hooks.py",
    "scripts/audit-dead-code.py",
    "scripts/audit-register.py",
    "scripts/check-manifests.py",
    "scripts/model-gateway.mjs",
    "scripts/model-gateway.test.mjs",
    "scripts/terraform-undeletable.py",
    "scripts/venue-signup/browser.mjs",
    "scripts/venue-signup/signup.mjs",
    "scripts/venue-signup/signup.test.mjs",
];

#[test]
fn the_non_rust_tooling_outside_the_browser_layer_is_exactly_the_accepted_set() {
    // §2.1 rule 1's "tooling" clause, which the rule states and the tree does
    // not honour. The deviation is real, no ADR records it, and one of these
    // files — `terraform-undeletable.py` — is on `infra.yml`'s own path, so
    // a Python interpreter is load-bearing in the infrastructure pipeline.
    //
    // What fails here is *growth*. An equality rather than a subset check,
    // and in both directions on purpose: a thirteenth script has to be
    // argued for in the same commit that adds it, and a script that is
    // deleted has to be struck from the list, so the set cannot quietly
    // become a record of files that no longer exist.
    //
    // The row this belongs to, §2.1 in `docs/DELIVERY-STATUS.md`, said
    // "three Python tools and one Node one" while the tree held four and
    // five. Nothing caught that for as long as the only reader was a person
    // re-running a grep.
    let mut found = BTreeSet::new();
    for directory in [".claude/hooks", "scripts"] {
        found.extend(excluded_language_files(directory));
    }

    // The premise, asserted before the comparison. An empty walk would make
    // this test a claim that the repository ships no tooling at all, which
    // it would then happily fail — but a walk that found, say, only the two
    // hook files would compare a truncated set against the full one and
    // report the difference as deleted files, which is a confusing failure
    // rather than a silent one. Saying so up front costs one line.
    assert!(
        !found.is_empty(),
        "no non-Rust tooling was found under .claude/hooks or scripts at all; \
         the walk is not seeing the repository"
    );

    let accepted: BTreeSet<String> = ACCEPTED_NON_RUST_TOOLING
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        ACCEPTED_NON_RUST_TOOLING.len(),
        accepted.len(),
        "the accepted list repeats a path, so it constrains fewer files than it names"
    );

    let added: Vec<&String> = found.difference(&accepted).collect();
    let removed: Vec<&String> = accepted.difference(&found).collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "the non-Rust tooling outside the browser layer no longer matches the \
         set §2.1 records as this repository's accepted deviation from rule 1. \
         New: {added:?}. Gone: {removed:?}. If a file is new, say in the same \
         commit why the platform's own language could not do the job, update \
         the §2.1 row in docs/DELIVERY-STATUS.md, and raise an ADR if it sits \
         on a workflow path as scripts/terraform-undeletable.py does. If a \
         file is gone, strike it from this list so the set stops describing a \
         tree nobody has"
    );
}
