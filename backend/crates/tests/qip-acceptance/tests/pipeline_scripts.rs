//! The scripts the pipeline runs, run for real.
//!
//! Three things, each about the pipeline doing less than its text said.
//!
//! **The secret scan reads the history, not only the tree (CICD-006).**
//! `scripts/check-secrets.sh` greps the tree as it stands. A credential
//! committed in one commit and deleted in the next passes that from the
//! second commit onwards, while every clone still carries the blob — and the
//! pipeline's own comment said the check "runs on the diff", which it never
//! did. `--history` reads every line any commit added instead.
//!
//! These tests run the real script against a scratch repository built here,
//! because the property is about what `git` holds and no amount of reading
//! the script's text shows that. The credential-shaped value is assembled at
//! run time so this file carries nothing a scanner would stop on.
//!
//! **The development tooling's tests run in the pipeline (CICD-030,
//! CICD-035).** The model gateway and the fleet's dispatcher and worker each
//! had a test file and no job ran any of them.
//!
//! **A gate is a step of a job, not a word in the file (CICD-015).** The
//! older check for "the pipeline runs X" is a substring search over ci.yml,
//! which a comment satisfies.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::{read, repository_root as repo_root};
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("qip-secret-history-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Run `git` in `dir` with an identity of its own, so the fixture commits do
/// not depend on, or sign with, whatever the machine running the suite has
/// configured.
fn git(dir: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(arguments)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The real script, run in `dir`: its exit code, stdout and stderr.
fn scan(dir: &Path, arguments: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new("bash")
        .arg(repo_root().join("scripts/check-secrets.sh"))
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("bash runs the scanner");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// A repository whose first commit adds a credential-shaped value and whose
/// second deletes it. Returns the directory, the first commit and the value.
fn repository_with_a_deleted_secret(name: &str) -> (PathBuf, String, String) {
    // The shape of an AWS access key id, which the scanner's second pattern
    // names. Built here rather than written out; see the module comment.
    let secret = format!("AKIA{}", "Q".repeat(16));
    let dir = scratch(name);
    git(&dir, &["init", "--quiet"]);
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::write(
        dir.join("config/settings.toml"),
        format!("region = \"us-east-1\"\nkey_id = {secret}\n"),
    )
    .unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "--quiet", "-m", "add settings"]);
    let introduced = git(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("config/settings.toml"), "region = \"us-east-1\"\n").unwrap();
    git(&dir, &["commit", "--quiet", "-am", "remove the key"]);
    (dir, introduced, secret)
}

/// The failure this prevents: a credential that was committed and then
/// deleted is invisible to a scan of the tree, so the pipeline went green on
/// a repository every clone of which still held the key.
///
/// Mutation: limit the history scan's `git log` to the newest commit (`-1`).
/// The finding disappears and the exit-1 assertion fails.
#[test]
fn a_secret_committed_and_then_deleted_passes_the_tree_scan_and_fails_the_history_scan() {
    let (dir, introduced, secret) = repository_with_a_deleted_secret("deleted");

    // Premise: the tree scan really does miss it. Without this the test
    // would pass on a fixture whose secret never left the tree, proving
    // nothing about history.
    let (code, stdout, stderr) = scan(&dir, &[]);
    assert_eq!(code, Some(0), "the tree scan found it after all: {stderr}");
    assert!(stdout.contains("secret scan: nothing found"), "{stdout}");

    let (code, stdout, stderr) = scan(&dir, &["--history"]);
    assert_eq!(
        code,
        Some(1),
        "the history scan did not fail on a secret one commit back: {stdout}{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "possible secret added by {introduced} in config/settings.toml"
        )),
        "the finding does not name the commit that added it and the path: {stderr}"
    );
    // A scanner that prints what it found has published it again, in a log
    // that outlives the rotation.
    assert!(
        !stderr.contains(&secret) && !stdout.contains(&secret),
        "the scan reprinted the secret it found"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The failure this prevents: a history scan with no way to record "this was
/// read and rotated" fails for ever on an old commit, which is the build
/// people learn to ignore — and one whose record needs no reason is a mute
/// button.
///
/// Mutation: drop the trailing space from the `grep -qF -- "$commit $path "`
/// match, so a line with no reason acknowledges the finding. The first
/// assertion below fails.
#[test]
fn a_reviewed_finding_is_skipped_only_when_the_record_says_what_it_was() {
    let (dir, introduced, _) = repository_with_a_deleted_secret("reviewed");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    let record = dir.join("scripts/secrets-reviewed.txt");

    // Premise: unrecorded, it fails.
    assert_eq!(scan(&dir, &["--history"]).0, Some(1));

    std::fs::write(&record, format!("{introduced} config/settings.toml\n")).unwrap();
    let (code, _, stderr) = scan(&dir, &["--history"]);
    assert_eq!(
        code,
        Some(1),
        "a record with no reason silenced the finding: {stderr}"
    );

    std::fs::write(
        &record,
        format!("{introduced} config/settings.toml rotated: fixture key, dead\n"),
    )
    .unwrap();
    let (code, stdout, stderr) = scan(&dir, &["--history"]);
    assert_eq!(code, Some(0), "a recorded finding still fails: {stderr}");
    assert!(
        stdout.contains("secret scan: nothing found in 2 commits of history"),
        "{stdout}"
    );

    // And the record is for one commit and one path, not for the secret: a
    // different commit adding the same value is a new finding.
    std::fs::write(
        &record,
        format!("{} config/settings.toml rotated: fixture\n", "0".repeat(40)),
    )
    .unwrap();
    assert_eq!(scan(&dir, &["--history"]).0, Some(1));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The failure this prevents: `actions/checkout` clones one commit unless
/// told otherwise. A history scan run on that reads one commit, finds
/// nothing, and says the history is clean.
///
/// Mutation: delete the `--is-shallow-repository` refusal. The scan of the
/// shallow clone exits 0 with "nothing found in 1 commits of history".
#[test]
fn a_shallow_clone_is_refused_rather_than_reported_clean() {
    let (dir, _, _) = repository_with_a_deleted_secret("shallow-origin");
    let clone = scratch("shallow-clone");
    git(
        &clone,
        &[
            "clone",
            "--quiet",
            "--depth",
            "1",
            &format!("file://{}", dir.display()),
            ".",
        ],
    );
    // Premise: the clone is shallow and the origin is not, so the two
    // verdicts below differ because of depth and nothing else.
    assert_eq!(
        git(&clone, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );
    assert_eq!(
        git(&dir, &["rev-parse", "--is-shallow-repository"]),
        "false"
    );
    assert_eq!(scan(&dir, &["--history"]).0, Some(1));

    let (code, stdout, stderr) = scan(&clone, &["--history"]);
    assert_eq!(
        code,
        Some(2),
        "a one-commit clone was scanned as if it were the history: {stdout}{stderr}"
    );
    assert!(stderr.contains("shallow clone"), "{stderr}");
    assert!(!stdout.contains("nothing found"), "{stdout}");

    // A mistyped flag must not fall through to the tree scan and print its
    // "nothing found" either.
    let (code, stdout, _) = scan(&dir, &["--histroy"]);
    assert_eq!(code, Some(2));
    assert!(!stdout.contains("nothing found"), "{stdout}");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&clone);
}

/// One job of ci.yml as trimmed lines: from its key at indent two to the next
/// key there. Empty when the job does not exist, which every caller asserts
/// against before reading anything out of it.
fn ci_job(name: &str) -> Vec<String> {
    let workflow = read(".github/workflows/ci.yml");
    let key = format!("  {name}:");
    workflow
        .lines()
        .skip_while(|line| line.trim_end() != key)
        .skip(1)
        .take_while(|line| {
            let is_next_job = line.starts_with("  ")
                && !line.starts_with("   ")
                && !line.trim_start().starts_with('#');
            !is_next_job
        })
        .map(|line| line.trim().to_string())
        .collect()
}

/// The failure this prevents: the mode existing and no pipeline running it,
/// or running it on the one-commit clone it refuses.
///
/// Mutation: delete the `--history` step from ci.yml's `secrets` job.
#[test]
fn the_pipeline_runs_the_history_scan_on_a_full_clone() {
    let job = ci_job("secrets");
    let job: Vec<&str> = job.iter().map(String::as_str).collect();
    // Premise: the job was found and still runs the tree scan, so the lines
    // being searched are the secrets job's and not an empty slice.
    assert!(
        job.contains(&"- run: ./scripts/check-secrets.sh"),
        "ci.yml's secrets job was not found, or no longer scans the tree: {job:?}"
    );

    // Whole lines, not `contains`: "check-secrets.sh --history" is a
    // substring of nothing else today, but "check-secrets.sh" is a substring
    // of it, and the tree-scan assertion above must not be satisfied by this
    // line alone.
    assert!(
        job.contains(&"- run: ./scripts/check-secrets.sh --history"),
        "ci.yml's secrets job does not run the history scan"
    );
    assert!(
        job.contains(&"fetch-depth: 0"),
        "ci.yml's secrets job clones one commit; the history scan refuses that"
    );
}

/// The failure this prevents (CICD-015): a gate dropped from the pipeline
/// while the pipeline still mentions it.
/// `infrastructure.rs::the_pipeline_gates_on_everything_it_claims_to` asks
/// whether ci.yml *contains* each gate's command, and a comment contains it.
/// So does a `run:` line somebody prefixed with `#`, and so does the same
/// command in a different job. Here each gate is a whole step line of the job
/// that owns it.
///
/// Mutation: comment out `- run: cargo audit --deny warnings` in ci.yml. The
/// substring check still passes, because the commented line still holds
/// "cargo audit"; this one fails naming `security-audit`.
#[test]
fn every_gate_the_pipeline_claims_is_a_step_of_its_own_job_and_not_a_mention() {
    const GATES: [(&str, &str); 9] = [
        ("format", "- run: cargo fmt --all --check"),
        (
            "lint",
            "- run: cargo clippy --workspace --all-targets --all-features",
        ),
        (
            "test",
            "- run: cargo test --workspace --all-features --no-fail-fast",
        ),
        (
            "release-build",
            "- run: cargo build --workspace --release --locked",
        ),
        (
            "dependency-policy",
            "- run: ./scripts/check-dependencies.sh",
        ),
        ("security-audit", "- run: cargo audit --deny warnings"),
        ("dependency-supply-chain", "- run: cargo deny check"),
        ("sbom", "- run: cargo cyclonedx --format json --all"),
        ("secrets", "- run: ./scripts/check-secrets.sh"),
    ];
    for (job, step) in GATES {
        let lines = ci_job(job);
        assert!(
            !lines.is_empty(),
            "ci.yml has no `{job}` job; the gate it ran is gone"
        );
        assert!(
            lines.iter().any(|line| line == step),
            "ci.yml's `{job}` job no longer has the step `{step}`"
        );
    }
}

/// Every `*.test.mjs` below `dir`, as repository-relative paths.
fn script_test_files(dir: &Path, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            script_test_files(&path, found);
        } else if path.to_string_lossy().ends_with(".test.mjs") {
            let relative = path.strip_prefix(repo_root()).unwrap();
            found.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// The failure this prevents: a test file beside a script that no job runs.
/// The gateway's tests were "run it yourself" for as long as they existed,
/// and two requirement rows recorded exactly that as their gap; a new test
/// file added beside them would have joined them silently.
///
/// Mutation: remove `scripts/fleet/worker.test.mjs` from the `scripts` job's
/// `node --test` line.
#[test]
fn every_test_file_under_scripts_is_run_by_the_pipelines_scripts_job() {
    let mut files = Vec::new();
    script_test_files(&repo_root().join("scripts"), &mut files);
    files.sort();
    // Premise: the walk found the tests this was written for, so "every file
    // is named" is not true of an empty list.
    assert!(
        files.contains(&"scripts/model-gateway.test.mjs".to_string()) && files.len() >= 3,
        "the walk of scripts/ found {files:?}"
    );

    // One file the job deliberately does not run, named with its reason so
    // the exemption is a record and not a blind spot. It drives a real
    // Chromium at /opt/pw-browsers/chromium (`venue-signup/browser.mjs`'s
    // DEFAULT_CHROMIUM); without one, eight of its eighteen tests fail on
    // `spawn ... ENOENT`, measured on 2026-10-04. Running it needs a browser
    // installed in the job, which is a dependency decision and not this
    // test's to make.
    const NEEDS_A_BROWSER: &str = "scripts/venue-signup/signup.test.mjs";
    assert!(
        files.iter().any(|file| file == NEEDS_A_BROWSER),
        "{NEEDS_A_BROWSER} is exempted here and no longer exists; remove the exemption"
    );
    files.retain(|file| file != NEEDS_A_BROWSER);

    let job = ci_job("scripts");
    let runs: Vec<&str> = job
        .iter()
        .filter_map(|line| line.strip_prefix("- run: node --test "))
        .flat_map(str::split_whitespace)
        .collect();
    assert!(
        !runs.is_empty(),
        "ci.yml's scripts job runs no `node --test` line: {job:?}"
    );
    // Whole arguments, both ways: a file on disk the job does not name is an
    // unrun test, and a name in the job with no file is a job that fails for
    // a reason nobody will guess.
    for file in &files {
        assert!(
            runs.contains(&file.as_str()),
            "{file} is not run by ci.yml's scripts job"
        );
    }
    for run in &runs {
        assert!(
            files.iter().any(|file| file == run),
            "ci.yml's scripts job names {run}, which is not a test file under scripts/"
        );
    }
}
