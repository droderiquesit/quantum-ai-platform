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
    // The test gate is two jobs since 2026-10-12: `test` runs every test
    // binary under nextest, in four partitions —
    // `every_workspace_crate_is_tested_by_exactly_one_partition_of_the_test_job`
    // below holds that one, since its run line is a matrix value — and
    // nextest does not run doctests, so `doctest` runs those and nothing
    // else. A `doctest` job deleted leaves the suite green and every doctest
    // unrun, which is the failure this entry exists for.
    const GATES: [(&str, &str); 9] = [
        ("format", "- run: cargo fmt --all --check"),
        (
            "lint",
            "- run: cargo clippy --workspace --all-targets --all-features",
        ),
        (
            "doctest",
            "- run: cargo test --workspace --all-features --doc --no-fail-fast",
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

/// The failure this prevents: a crate tested by no partition, or by two.
/// ci.yml's `test` job runs as four runners since 2026-10-12, three naming
/// their packages with `-p` and the fourth taking the rest of the workspace
/// with `--workspace --exclude …`. A crate named in a `-p` and missing from
/// the exclusions runs twice, which costs minutes; a crate in the exclusions
/// with no `-p` runs nowhere, and nothing else would notice — the suite
/// stays green with that crate's tests never run. So the two sets must be
/// the same set, and every name in them must be a crate in the tree, since
/// cargo refuses a `-p` it cannot find but silently accepts an `--exclude`
/// naming nothing.
///
/// Mutation: drop `-p qip-fastbrain` from the acceptance partition's
/// `packages`. The exclusions then name a crate no partition runs.
#[test]
fn every_workspace_crate_is_tested_by_exactly_one_partition_of_the_test_job() {
    let job = ci_job("test");
    let partitions: Vec<&str> = job
        .iter()
        .filter_map(|line| line.strip_prefix("packages: "))
        .map(|value| value.trim_matches('"'))
        .collect();
    // Premise: the matrix exists and the one run line is fed by it, so the
    // sets below describe what actually runs.
    assert!(
        partitions.len() >= 2,
        "ci.yml's test job no longer partitions the suite: {job:?}"
    );
    assert!(
        job.iter().any(|line| {
            line == "run: cargo nextest run ${{ matrix.packages }} --all-features --no-fail-fast"
        }),
        "ci.yml's test job no longer runs nextest over `matrix.packages`: {job:?}"
    );

    let mut named = std::collections::BTreeSet::new();
    let mut excluded = std::collections::BTreeSet::new();
    let mut rest_partitions = 0;
    for partition in &partitions {
        let words: Vec<&str> = partition.split_whitespace().collect();
        if words.first() == Some(&"--workspace") {
            rest_partitions += 1;
            for pair in words[1..].chunks(2) {
                assert_eq!(
                    pair.first().copied(),
                    Some("--exclude"),
                    "the rest-of-workspace partition holds something other than exclusions: {partition}"
                );
                let krate = pair.get(1).copied().unwrap_or_default();
                assert!(excluded.insert(krate), "`{krate}` is excluded twice");
            }
        } else {
            for pair in words.chunks(2) {
                assert_eq!(
                    pair.first().copied(),
                    Some("-p"),
                    "a named partition holds something other than `-p` pairs: {partition}"
                );
                let krate = pair.get(1).copied().unwrap_or_default();
                assert!(named.insert(krate), "`{krate}` is named by two partitions");
            }
        }
    }
    assert_eq!(
        rest_partitions, 1,
        "exactly one partition takes the rest of the workspace, so a new crate is tested once"
    );
    assert_eq!(
        named, excluded,
        "the crates the named partitions run and the crates the rest-of-workspace partition \
         excludes are not the same set: a crate in one and not the other runs twice or never"
    );
    for krate in &named {
        let exists = std::fs::read_dir(repo_root().join("backend/crates"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|group| group.path().join(krate).join("Cargo.toml").is_file());
        assert!(
            exists,
            "`{krate}` is named in ci.yml's test partitions and is not a crate under backend/crates/"
        );
    }
}

/// The failure this prevents: a timing suite renamed out from under the
/// nextest override that gives it the runner to itself. `.config/nextest.toml`
/// names, by binary, every suite that asserts on wall-clock time, and sets
/// `threads-required = "num-cpus"` on them, because ci run 956 failed
/// `m5_performance::feature_extraction_scales_linearly` on a 1.2 ms
/// measurement taken beside three tests over 120 s. A filter naming a binary
/// that no longer exists matches nothing and nextest says nothing; the suite
/// then runs beside everything else again, and fails the next time the
/// runner is busy — and reads as a flake rather than as this.
///
/// Mutation: rename `binary(fabric_bench)` to `binary(fabric_bench_x)` in the
/// filter; the walk finds no such test file. (Renaming `m5_performance`
/// fails the premise instead, which is also a failure, and a less specific
/// one.)
#[test]
fn every_timing_suite_the_nextest_override_names_is_a_test_binary_in_the_tree() {
    let config = read("backend/.config/nextest.toml");
    let filter = config
        .lines()
        .find_map(|line| line.trim().strip_prefix("filter = "))
        .expect("backend/.config/nextest.toml has an override filter");
    let names: Vec<&str> = filter
        .split("binary(")
        .skip(1)
        .map(|rest| rest.split(')').next().unwrap_or_default())
        .collect();
    // Premise: the filter names the suite the override was written for, so
    // the walk below is checking something.
    assert!(
        names.contains(&"m5_performance") && names.len() >= 4,
        "the override's filter no longer names the timing suites: {filter}"
    );
    assert!(
        config
            .lines()
            .any(|line| line.trim() == "threads-required = \"num-cpus\""),
        "the override no longer gives the timing suites the runner to themselves"
    );
    let groups = std::fs::read_dir(repo_root().join("backend/crates")).unwrap();
    let mut test_files = Vec::new();
    for group in groups.filter_map(Result::ok) {
        for krate in std::fs::read_dir(group.path())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            if let Ok(tests) = std::fs::read_dir(krate.path().join("tests")) {
                for test in tests.filter_map(Result::ok) {
                    if let Some(stem) = test.path().file_stem() {
                        test_files.push(stem.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    for name in names {
        assert!(
            test_files.iter().any(|stem| stem == name),
            "`binary({name})` in backend/.config/nextest.toml matches no tests/{name}.rs under \
             backend/crates/; a renamed suite runs beside everything else again"
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

/// The replay and benchmark output is kept as a run artifact.
///
/// R12-CICD-9085: the tick replay and performance suites print the numbers a
/// regression would show in, and a run that discards its log leaves nothing
/// to compare the next run with. The step must be a step of the `test` job
/// and not a mention in a comment, and it must run the two suites it names.
///
/// Mutation: delete the `name: replay-bench` line from ci.yml's `test` job.
#[test]
fn the_test_job_uploads_the_replay_and_benchmark_output_as_an_artifact() {
    let lines = ci_job("test");
    assert!(
        !lines.is_empty(),
        "premise: ci.yml has a `test` job to read"
    );
    assert!(
        lines.iter().any(|line| line == "- uses: actions/upload-artifact@v4"),
        "premise: the `test` job uploads an artifact at all"
    );
    assert!(
        lines.iter().any(|line| line == "name: replay-bench"),
        "ci.yml's `test` job no longer uploads an artifact named `replay-bench`"
    );
    assert!(
        lines.iter().any(|line| {
            line.starts_with("run: cargo test -p qip-acceptance --test tick_replay --test performance")
                && line.contains("tee replay-bench.txt")
        }),
        "ci.yml's `test` job no longer runs tick_replay and performance into replay-bench.txt"
    );
}
