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
const ACCEPTED_NON_RUST_TOOLING: [&str; 45] = [
    ".claude/hooks/format-rust-after-edit.py",
    ".claude/hooks/guard-dangerous-command.py",
    // Tests the guard above, the hook the harness runs before every Bash
    // call. The hook is Python and so is its existing regression suite,
    // `test_hooks.py`, so the test is written in the language of the code it
    // loads. It is on no workflow path, and CI does not run it: the `hooks`
    // job runs `test_hooks.py` alone, and that file never loads this one. It
    // is exercised only when somebody runs it by hand.
    ".claude/hooks/test_guard_dangerous_command.py",
    ".claude/hooks/test_hooks.py",
    // The development-agent boundary (round 2's CI/CD lane, R12 GOV-9067,
    // SEC-9068, CICD-9076/9082/9083). `agent-guard.mjs` IS on workflow
    // paths: every deploying workflow sparse-checks `scripts/` out of the
    // default branch and runs it as its first step, before anything
    // authenticates and before any toolchain is installed, so a branch
    // cannot loosen the rule it is judged by. Rust could not do that job
    // there — a guard that first had to compile the workspace would run
    // after the steps it exists to precede — and Node is on every hosted
    // runner, built-ins only, nothing installed. `agent-lanes.mjs` reads the
    // same `agent-policy.json` and is written in the guard's language, and
    // `agent-egress.mjs` is the agent sandbox's one way out (SEC-9077), run
    // on `agent-sandbox.yml`'s path from a root-owned copy of the default
    // branch's scripts. ADR 0117 records all three: the policy file, what
    // the guard refuses, its blind spots and what would make it wrong.
    "scripts/agent-egress.mjs",
    "scripts/agent-egress.test.mjs",
    "scripts/agent-guard.mjs",
    "scripts/agent-guard.test.mjs",
    "scripts/agent-lanes.mjs",
    "scripts/agent-lanes.test.mjs",
    "scripts/audit-dead-code.py",
    "scripts/check-manifests.py",
    // The ADR 0102 fleet: the worker runs as a Cloud Run Job task in a
    // container holding a Node runtime and no Rust toolchain, and it calls
    // the Node gateway below, so it is written in the gateway's language.
    // The dispatcher stays on the desktop and reuses the worker's validator.
    // Not on a workflow path.
    "scripts/fleet/dispatch.mjs",
    "scripts/fleet/dispatch.test.mjs",
    "scripts/fleet/worker.mjs",
    "scripts/fleet/worker.test.mjs",
    // The game-day harness (R12 RES-10186): drives the real Rust binaries
    // as child processes, injects SIGKILL/SIGSTOP and times recovery over
    // their HTTP probes. Written in the scripts job's one runtime rather
    // than as a crate because it is a driver *of* the binaries, not part of
    // them, and a harness linked into the workspace would be the thing it
    // measures. On a workflow path only as `node --test` in ci.yml's
    // `scripts` job. The fixture is its test double and models no binary.
    // The agent-authority drill (R12 SEC-10157): it calls the Node judges
    // `scripts/agent-guard.mjs` and `scripts/agent-lanes.mjs` directly, so
    // it is Node for the reason they are; Rust would have to shell out to
    // them and could not import the functions it is testing.
    "scripts/game-day/agent-escalation.mjs",
    "scripts/game-day/agent-escalation.test.mjs",
    "scripts/game-day/fixtures/fake-service.mjs",
    "scripts/game-day/harness.mjs",
    "scripts/game-day/harness.test.mjs",
    "scripts/model-gateway.mjs",
    "scripts/model-gateway.test.mjs",
    // The rule-citation gate (R12 GOV-9080), on ci.yml's `rule-citation`
    // job: it reads the pull request's body from the event file and refuses
    // one that changes a rule file without naming it. It must answer on a
    // pull request before anything compiles, and the job installs Node and
    // no Rust toolchain — a `qip` subcommand would mean building the
    // workspace to read a JSON field — so it is in Node, built-ins only,
    // beside the agent guard and the SAST gate. On a workflow path, so it
    // owes an ADR on the same terms as those two; none is written yet.
    "scripts/rule-citation.mjs",
    "scripts/rule-citation.test.mjs",
    // The CodeQL blocking gate (R12 CICD-9105), on `codeql-analysis.yml`'s
    // path: it reads the SARIF the analysis wrote and fails the `sast` job
    // on a finding at or above `sast-policy.json`'s threshold. That job
    // installs Node and no Rust toolchain, and the gate must judge the
    // analysis in the job that ran it, so it is in Node, built-ins only.
    // On a workflow path, so it owes an ADR on the same terms as the agent
    // guard above: ADR 0117, which records its policy file and threshold.
    "scripts/sast-gate.mjs",
    "scripts/sast-gate.test.mjs",
    // The ScrapeGraphAI extraction tool, built on the owner's direction of
    // 2026-10-13 with the ADR waived for process only. Python because the
    // library it wraps is Python, and the platform's language cannot host
    // it: an operator-run proposal tool, linked into no crate, on no
    // workflow path, and nothing in the platform reads what it writes
    // (scripts/scraper/README.md). The tests are pytest, in the language of
    // the code they load; CI does not run them, because ci.yml's scripts job
    // runs `node --test` only.
    "scripts/scraper/qip_scraper/__init__.py",
    "scripts/scraper/qip_scraper/__main__.py",
    "scripts/scraper/qip_scraper/cli.py",
    "scripts/scraper/qip_scraper/extract.py",
    "scripts/scraper/qip_scraper/fetch.py",
    "scripts/scraper/qip_scraper/guard.py",
    "scripts/scraper/qip_scraper/verify.py",
    "scripts/scraper/tests/conftest.py",
    "scripts/scraper/tests/test_cli.py",
    "scripts/scraper/tests/test_extract.py",
    "scripts/scraper/tests/test_fetch.py",
    "scripts/scraper/tests/test_guard.py",
    "scripts/scraper/tests/test_verify.py",
    // Sends the owner a text through a carrier email gateway over SMTP with
    // STARTTLS. Rust cannot do the job here: TLS in-tree is what ADR 0009
    // forbids, and a dependency needs an ADR. Not on a workflow path.
    "scripts/send-sms.py",
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
         commit why the platform's own language could not do the job, say it \
         again beside the file's entry in this list, and raise an ADR if it sits \
         on a workflow path as scripts/terraform-undeletable.py does. If a \
         file is gone, strike it from this list so the set stops describing a \
         tree nobody has"
    );
}

/// The Node tooling that sits on a workflow path, and the ADR that records
/// each. A list entry saying "ADR 0117 records it" is a citation; this makes
/// it one a build checks.
const WORKFLOW_PATH_TOOLING_ADR: (&str, [&str; 4]) = (
    "0117-",
    [
        "scripts/agent-guard.mjs",
        "scripts/agent-lanes.mjs",
        "scripts/agent-egress.mjs",
        "scripts/sast-gate.mjs",
    ],
);

#[test]
fn the_node_tooling_on_workflow_paths_is_recorded_by_an_accepted_adr() {
    // The failure this prevents is the one the list above recorded against
    // itself: two scripts on workflow paths whose entries said an ADR was
    // owed and none was written. Now the ADR exists, and this fails if it is
    // renumbered, unaccepted, or stops naming a script it is cited for.
    let (prefix, scripts) = WORKFLOW_PATH_TOOLING_ADR;
    let adr_dir = qip_acceptance::repository_root().join("docs/adr");
    let mut matches: Vec<String> = std::fs::read_dir(&adr_dir)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", adr_dir.display()))
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(prefix) && name.ends_with(".md"))
        .collect();
    matches.sort();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one ADR numbered {prefix}*, found {matches:?}"
    );
    let text = qip_acceptance::read(&format!("docs/adr/{}", matches[0]));
    assert!(
        text.contains("- **Status**: **Accepted**"),
        "{} is not accepted, so the tooling it is cited for is still unrecorded",
        matches[0]
    );
    for script in scripts {
        assert!(
            ACCEPTED_NON_RUST_TOOLING.contains(&script),
            "{script} is cited as workflow-path tooling but is not in the accepted set"
        );
        let name = script.rsplit('/').next().unwrap_or(script);
        assert!(
            text.contains(name),
            "{} does not name {name}, which the accepted tooling list cites it for",
            matches[0]
        );
    }
}
