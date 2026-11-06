//! What `deploy.yml` will sign, proven by running its own gate steps.
//!
//! R12-CICD-9066: agent-produced code goes through Git, tests, attestation
//! and deployment policy like any other change, and a fork or pull-request
//! commit cannot be signed. The workflow's attestor holds the one key Binary
//! Authorization admits an image on, so the property that matters is which
//! commits reach the step that uses it.
//!
//! The automatic path's refusal is a shell step; it is extracted from the
//! committed workflow and executed under each event shape, rather than read,
//! because a script that reads correctly and exits 0 on every input is the
//! failure a text check cannot see.
#![allow(clippy::panic_in_result_fn)]

use std::process::Command;

use qip_acceptance::read;

const DEPLOY: &str = ".github/workflows/deploy.yml";

/// The `run: |` body of the step named `name`, dedented.
fn step_script(workflow: &str, name: &str) -> String {
    let marker = format!("- name: {name}\n");
    let at = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("{DEPLOY} has no step named {name:?}"));
    let rest = &workflow[at + marker.len()..];
    let step_indent = workflow[..at]
        .rsplit('\n')
        .next()
        .map_or(0, |line| line.len());
    let mut lines = rest.lines();
    // Up to the `run: |` key of this step and no further than the next step.
    let mut found = false;
    for line in lines.by_ref() {
        let indent = line.len() - line.trim_start().len();
        if line.trim_start().starts_with("- name:") && indent <= step_indent {
            break;
        }
        if line.trim() == "run: |" {
            found = true;
            break;
        }
    }
    assert!(found, "{DEPLOY}: step {name:?} has no `run: |` block");
    let body: Vec<&str> = lines
        .take_while(|line| {
            line.trim().is_empty() || line.len() - line.trim_start().len() > step_indent + 2
        })
        .collect();
    let indent = body
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    body.iter()
        .map(|line| line.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run `script` under bash with exactly `env` and report its exit code.
fn run(script: &str, env: &[(&str, &str)]) -> i32 {
    let mut command = Command::new("bash");
    command.arg("-c").arg(script).env_clear();
    command.env("PATH", "/usr/bin:/bin");
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("cannot run bash: {error}"));
    output.status.code().unwrap_or(-1)
}

#[test]
fn a_ci_run_started_by_a_pull_request_or_from_a_fork_cannot_start_a_deployment() {
    let workflow = read(DEPLOY);
    let script = step_script(&workflow, "the triggering ci run is this repository's own");
    let this = "droderiquesit/quantum-ai-platform";

    // The admitting half first: a push to this repository proceeds. Without
    // it, a script that refused everything would pass every case below.
    assert_eq!(
        run(
            &script,
            &[
                ("TRIGGER_EVENT", "push"),
                ("TRIGGER_REPOSITORY", this),
                ("THIS_REPOSITORY", this)
            ]
        ),
        0,
        "a push to this repository was refused, so the gate refuses everything"
    );

    for (case, event, repository) in [
        ("a pull request from this repository", "pull_request", this),
        (
            "a pull request from a fork",
            "pull_request",
            "someone/quantum-ai-platform",
        ),
        ("a pull_request_target run", "pull_request_target", this),
        (
            "a push to a fork whose branch is called main",
            "push",
            "someone/quantum-ai-platform",
        ),
        ("a run naming no event", "", this),
        ("a run naming no repository", "push", ""),
    ] {
        assert_ne!(
            run(
                &script,
                &[
                    ("TRIGGER_EVENT", event),
                    ("TRIGGER_REPOSITORY", repository),
                    ("THIS_REPOSITORY", this),
                ]
            ),
            0,
            "{case} reached the images job, which builds that commit and has the \
             environment's attestor sign it"
        );
    }

    // And the step runs on the automatic path at all.
    let marker = "- name: the triggering ci run is this repository's own\n";
    let header = &workflow[workflow.find(marker).unwrap_or(0)..];
    assert!(
        header
            .lines()
            .nth(1)
            .is_some_and(|line| line.trim() == "if: github.event_name == 'workflow_run'"),
        "the provenance step is not bound to the workflow_run path it guards"
    );
}

#[test]
fn a_dispatched_deployment_trusts_only_a_ci_run_started_by_a_push() {
    // On the manual path the commit's ci history is asked for. A pull
    // request's ci run carries the pull request's head as its `head_sha` and
    // succeeds like any other, so a query without `event=push` finds it and
    // a dispatch naming a fork's commit signs the fork's code.
    let workflow = read(DEPLOY);
    for step in [
        "ci succeeded for this commit",
        "static analysis passed in that ci run",
    ] {
        let script = step_script(&workflow, step);
        let queries: Vec<&str> = script
            .lines()
            .filter(|line| line.contains("actions/workflows/ci.yml/runs?"))
            .collect();
        assert!(
            !queries.is_empty(),
            "premise: step {step:?} no longer asks for the commit's ci runs"
        );
        for query in queries {
            assert!(
                query.contains("head_sha=${DEPLOY_SHA}")
                    && query.contains("&status=success")
                    && query.contains("&event=push"),
                "step {step:?} trusts a ci run not started by a push: {query}"
            );
        }
    }
}

#[test]
fn only_a_branch_of_this_repository_can_obtain_the_signing_identity() {
    // The workflow's checks are the first line; the identity federation is
    // the second, and holds even against an edited workflow. A pull request
    // authenticates as `refs/pull/<n>/merge` and a tag as `refs/tags/<t>`;
    // the provider admits neither.
    let cicd: String = read("infrastructure/terraform/modules/cicd/main.tf")
        .split_whitespace()
        .collect();
    assert!(
        cicd.contains(
            "attribute_condition=\"attribute.repository=='${var.github_repository}'&&attribute.ref.startsWith('refs/heads/')\""
        ),
        "the GitHub provider admits a token from a pull-request or tag ref, so a run that is \
         not a branch of this repository can impersonate the account that signs"
    );
}

/// The `- name: <name>` step's own text, up to the next step of its job.
fn step_text<'a>(workflow: &'a str, name: &str) -> &'a str {
    let marker = format!("      - name: {name}\n");
    let at = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("{DEPLOY} has no step named {name:?}"));
    let rest = &workflow[at + marker.len()..];
    let end = rest
        .find("\n      - name:")
        .or_else(|| rest.find("\n  # "))
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn a_high_or_critical_finding_stops_an_image_before_it_is_signed_on_every_path() {
    // R12-SEC-9070: security findings are triaged by a severity policy that
    // controls promotion. The policy is two committed bars that agree — the
    // image scan's `--severity CRITICAL,HIGH` and CodeQL's
    // `min_security_severity` 7.0, the bottom of its "high" band — and what
    // makes either control promotion is where it runs: before the attestor
    // signs, with nothing able to skip it. A scan that warns, or that runs
    // only when the job built (and not when it promotes a digest built
    // earlier), would leave the policy written and the promotion unchanged.
    let workflow = read(DEPLOY);
    let scan = step_text(&workflow, "scan image");
    for flag in ["--severity CRITICAL,HIGH", "--exit-code 1", "trivy image"] {
        assert!(
            scan.contains(flag),
            "the image scan does not carry `{flag}`, so a high or critical finding does not \
             stop the image"
        );
    }
    let header: Vec<&str> = scan
        .lines()
        .take_while(|line| line.trim() != "run: |")
        .collect();
    assert!(
        !header.iter().any(|line| {
            let line = line.trim();
            line.starts_with("if:") || line.starts_with("continue-on-error:")
        }),
        "the image scan is conditional or allowed to fail ({header:?}); a promotion that \
         skips it signs an image nobody scanned"
    );

    let scanned = workflow.find("      - name: scan image\n");
    let signed = workflow.find("      - name: sign the pushed image\n");
    let pushed = workflow.find("      - name: push\n");
    assert!(
        matches!((scanned, pushed, signed), (Some(s), Some(p), Some(g)) if s < p && p < g),
        "the image is not scanned before it is pushed and signed; the attestation would \
         admit bytes the severity bar never saw"
    );

    // The images job between the scan and the signature holds no step that
    // may fail without failing the job.
    let images_job = &workflow[scanned.unwrap_or(0)..signed.unwrap_or(0)];
    assert!(
        !images_job.contains("continue-on-error: true"),
        "a step between the scan and the signature may fail without failing the job"
    );

    // And the two bars are one policy, not two that drifted.
    let policy: serde_json::Value = serde_json::from_str(&read("scripts/sast-policy.json"))
        .unwrap_or_else(|error| panic!("scripts/sast-policy.json does not parse: {error}"));
    assert_eq!(
        policy["min_security_severity"].as_f64(),
        Some(7.0),
        "CodeQL's blocking bar is no longer the bottom of the high band the image scan stops on"
    );
    assert_eq!(policy["band"].as_str(), Some("high"));
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("--severity CRITICAL,HIGH \\\n            --exit-code 1"),
        "ci's filesystem scan no longer stops on the same high and critical bar"
    );
}

/// The text of the `gate` job: from its key to the next job's key.
fn gate_job(workflow: &str) -> &str {
    let at = workflow
        .find("\n  gate:\n")
        .unwrap_or_else(|| panic!("{DEPLOY} has no `gate` job"));
    let rest = &workflow[at + 1..];
    let end = rest[1..]
        .lines()
        .scan(1usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| {
            line.starts_with("  ")
                && !line.starts_with("   ")
                && !line.trim_start().starts_with('#')
                && line.trim_end().ends_with(':')
        })
        .map_or(rest.len(), |(start, _)| start);
    &rest[..end]
}

#[test]
fn a_deployment_whose_ci_run_did_not_succeed_is_refused_on_both_paths() {
    // R12-CICD-9106: the deploy gates on ci. `deploy.yml` holds that in two
    // steps of its `gate` job, one per trigger. Until 2026-10-24 either
    // could be deleted and no acceptance suite failed. AUDIT-3 found it:
    // the row was COMPLETE on a test that reads neither workflow. Each
    // script is extracted from the committed workflow and run under bash,
    // admitting case first, because a script that exits 1 on every input
    // passes every refusal below.
    let workflow = read(DEPLOY);
    let gate = gate_job(&workflow);

    // The automatic path: the conclusion of the ci run that triggered this.
    let step = "the triggering ci run succeeded";
    assert!(
        gate.contains(&format!(
            "      - name: {step}\n        if: github.event_name == 'workflow_run'\n"
        )),
        "step {step:?} is not in the gate job bound to the workflow_run path it guards"
    );
    assert!(
        step_text(&workflow, step)
            .contains("CI_CONCLUSION: ${{ github.event.workflow_run.conclusion }}"),
        "step {step:?} no longer reads the triggering run's conclusion"
    );
    let script = step_script(&workflow, step);
    assert_eq!(
        run(&script, &[("CI_CONCLUSION", "success")]),
        0,
        "a successful ci run was refused, so the automatic path refuses everything"
    );
    for conclusion in [
        "failure",
        "cancelled",
        "timed_out",
        "skipped",
        "neutral",
        "action_required",
        "stale",
        "startup_failure",
        "",
    ] {
        assert_ne!(
            run(&script, &[("CI_CONCLUSION", conclusion)]),
            0,
            "a ci run that concluded {conclusion:?} started a deployment"
        );
    }

    // The manual path: the commit's own push-event ci history, through `gh`.
    // A stand-in `gh` on PATH answers the `--jq '.total_count'` query with a
    // fixed count, or fails the way an unreachable API does.
    let step = "ci succeeded for this commit";
    assert!(
        gate.contains(&format!(
            "      - name: {step}\n        if: github.event_name == 'workflow_dispatch'\n"
        )),
        "step {step:?} is not in the gate job bound to the workflow_dispatch path it guards"
    );
    let script = step_script(&workflow, step);
    let bin = std::env::temp_dir().join(format!("qip-deploy-gate-gh-{}", std::process::id()));
    std::fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("{bin:?}: {error}"));
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        "#!/bin/bash\n[ \"$FAKE_GH_FAIL\" = 1 ] && exit 1\necho \"$FAKE_GH_COUNT\"\n",
    )
    .unwrap_or_else(|error| panic!("{gh:?}: {error}"));
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("{gh:?}: {error}"));
    }
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let manual = |count: &str, fail: &str| {
        run(
            &script,
            &[
                ("PATH", path.as_str()),
                ("FAKE_GH_COUNT", count),
                ("FAKE_GH_FAIL", fail),
                ("GH_TOKEN", "unused"),
                ("THIS_REPOSITORY", "droderiquesit/quantum-ai-platform"),
                ("DEPLOY_SHA", "0123456789abcdef0123456789abcdef01234567"),
            ],
        )
    };
    assert_eq!(
        manual("1", "0"),
        0,
        "a commit with a successful push-event ci run was refused, so the manual path \
         refuses everything"
    );
    // An empty or `null` count is a bash error in `[ "$runs" -lt 1 ]`, and
    // an `if` condition is exempt from `set -e`: before the step checked the
    // count was a number, either fell through to "ci succeeded".
    for (case, count, fail) in [
        ("no successful ci run", "0", "0"),
        ("an unreachable API", "", "1"),
        ("an empty answer", "", "0"),
        ("a null answer", "null", "0"),
        ("a non-numeric answer", "1; true", "0"),
    ] {
        assert_ne!(
            manual(count, fail),
            0,
            "a dispatch with {case} for its commit started a deployment"
        );
    }
    let _ = std::fs::remove_dir_all(&bin);
}

/// R12-CICD-9066: infrastructure goes through the same tests as code. The
/// apply actions of `infra.yml` run the same refusal deploy.yml does: a
/// commit with no successful push-started ci run is not applied. The step is
/// executed against a stub `gh` that reports each count, because a step that
/// reads correctly and exits 0 on every input is what a text check misses.
#[test]
fn an_infrastructure_apply_of_a_commit_ci_has_not_passed_is_refused() {
    let workflow = read(".github/workflows/infra.yml");
    let marker = "- name: ci succeeded for this commit\n";
    let at = workflow
        .find(marker)
        .unwrap_or_else(|| panic!("infra.yml has no ci gate step"));
    let condition = workflow[at + marker.len()..]
        .lines()
        .next()
        .unwrap_or_default()
        .trim();
    assert_eq!(
        condition, "if: inputs.action == 'up' || inputs.action == 'apps'",
        "the ci gate must cover both apply actions"
    );
    let apply = workflow
        .find("- name: up\n")
        .unwrap_or_else(|| panic!("premise: infra.yml has an up step"));
    assert!(at < apply, "the ci gate must run before the apply");

    let script = step_script(&workflow, "ci succeeded for this commit");
    assert!(
        script.contains("head_sha=${THIS_SHA}&status=success&event=push"),
        "the gate trusts a ci run not started by a push: {script}"
    );
    let dir = std::env::temp_dir().join(format!("qip-infra-gate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("scratch dir: {e}"));
    let gh = dir.join("gh");
    let path = format!("{}:/usr/bin:/bin", dir.display());
    for (count, expected) in [("0", 1), ("", 1), ("null", 1), ("1", 0), ("3", 0)] {
        std::fs::write(&gh, format!("#!/bin/bash\necho '{count}'\n"))
            .unwrap_or_else(|e| panic!("stub gh: {e}"));
        let status = Command::new("chmod")
            .arg("+x")
            .arg(&gh)
            .status()
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        assert!(status.success());
        let code = {
            let mut command = Command::new("bash");
            command.arg("-c").arg(&script).env_clear();
            command.env("PATH", &path);
            command.env("THIS_REPOSITORY", "owner/repo");
            command.env("THIS_SHA", "abc123");
            command
                .output()
                .map_or(-1, |o| o.status.code().unwrap_or(-1))
        };
        assert_eq!(code, expected, "ci run count {count:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// R12-CICD-7116 and R12-CICD-7119: a deploy is not promoted unless the
/// simulation gate and the performance gate succeeded in the ci run it
/// trusts. ci.yml must define both jobs under the names the deploy step reads,
/// and the step is executed against a stub `gh` answering each job's
/// conclusion, because a step that exits 0 on every answer is what a text
/// check misses.
#[test]
fn a_deployment_whose_simulation_or_performance_gate_did_not_succeed_is_refused() {
    let ci = read(".github/workflows/ci.yml");
    for (job, name) in [
        ("simulation-gate", "simulation gate"),
        ("performance-gate", "performance gate"),
    ] {
        assert!(
            ci.contains(&format!("\n  {job}:\n    name: {name}\n")),
            "ci.yml does not define the {name} job the deploy gate requires"
        );
    }
    assert!(
        ci.contains("--test performance") && ci.contains("--test arch_056_simulation_engine"),
        "a gate job no longer runs its suite"
    );

    let workflow = read(DEPLOY);
    let script = step_script(
        &workflow,
        "the simulation and performance gates passed in that ci run",
    );
    let dir = std::env::temp_dir().join(format!("qip-gates-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("scratch dir: {e}"));
    let gh = dir.join("gh");
    std::fs::write(
        &gh,
        r#"#!/bin/bash
case "$*" in
  *'simulation gate'*) echo "$SIM" ;;
  *'performance gate'*) echo "$PERF" ;;
  *) echo "unexpected gh call: $*" >&2; exit 2 ;;
esac
"#,
    )
    .unwrap_or_else(|e| panic!("stub gh: {e}"));
    let status = Command::new("chmod")
        .arg("+x")
        .arg(&gh)
        .status()
        .unwrap_or_else(|e| panic!("chmod: {e}"));
    assert!(status.success());
    let path = format!("{}:/usr/bin:/bin", dir.display());
    let code = |sim: &str, perf: &str| {
        let mut command = Command::new("bash");
        command.arg("-c").arg(&script).env_clear();
        command.env("PATH", &path);
        command.env("THIS_REPOSITORY", "owner/repo");
        command.env("TRIGGER_RUN_ID", "42");
        command.env("DEPLOY_SHA", "abc123");
        command.env("SIM", sim);
        command.env("PERF", perf);
        command
            .output()
            .map_or(-1, |o| o.status.code().unwrap_or(-1))
    };
    assert_eq!(
        code("success", "success"),
        0,
        "both gates passed and the deploy was refused"
    );
    assert_eq!(
        code("", "success"),
        1,
        "a ci run with no simulation gate was admitted"
    );
    assert_eq!(
        code("success", ""),
        1,
        "a ci run with no performance gate was admitted"
    );
    assert_eq!(
        code("failure", "success"),
        1,
        "a failed simulation gate was admitted"
    );
    assert_eq!(
        code("success", "cancelled"),
        1,
        "a cancelled performance gate was admitted"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scratch directory holding executable stubs, unique to one test.
fn stub_dir(test: &str, stubs: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("qip-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("scratch dir: {e}"));
    for (name, body) in stubs {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap_or_else(|e| panic!("stub {name}: {e}"));
        let status = Command::new("chmod")
            .arg("+x")
            .arg(&path)
            .status()
            .unwrap_or_else(|e| panic!("chmod: {e}"));
        assert!(status.success(), "chmod +x {name} failed");
    }
    dir
}

const COPY_STEP: &str = "copy the signed digest to the secondary registry";

/// R12-RES-9177: the signed digest reaches the secondary registry, or the run
/// fails.
///
/// The step is executed, not read: a copy that "succeeded" while the replica
/// held nothing, or held other bytes, is precisely what a region loss would
/// discover too late. A stub `gcloud` answers each `describe` from the
/// environment the case sets, and a stub `docker` records the copy it was
/// asked to make, so each case says what the registry reported and what the
/// step concluded.
///
/// Mutation: drop the read-back comparison (`if [ "$copied" != "$digest" ]`
/// made `if false`). The different-digest and nothing-resolved cases are
/// admitted and this fails naming them.
#[test]
fn the_signed_digest_is_copied_to_the_secondary_registry_and_refused_unless_it_reads_back_identical()
 {
    let workflow = read(DEPLOY);
    let script = step_script(&workflow, COPY_STEP);
    let dir = stub_dir(
        "secondary-copy",
        &[
            (
                "gcloud",
                r#"#!/bin/bash
case "$1 $2" in
  'auth configure-docker') echo "$3" >> "$LOG.auth"; exit 0 ;;
  'artifacts docker') ;;
  *) echo "unexpected gcloud call: $*" >&2; exit 2 ;;
esac
ref="$5"
case "$ref" in
  "$PRIMARY_PREFIX"/*) value="$PRIMARY" ;;
  "$SECONDARY_PREFIX"/*@*) value="$SECONDARY_BY_DIGEST" ;;
  "$SECONDARY_PREFIX"/*) value="$SECONDARY_BY_TAG" ;;
  *) echo "unexpected image: $ref" >&2; exit 2 ;;
esac
[ -n "$value" ] || { echo "ERROR: image not found: $ref" >&2; exit 1; }
echo "$value"
"#,
            ),
            ("docker", "#!/bin/bash\necho \"$*\" >> \"$LOG\"\n"),
        ],
    );
    let path = format!("{}:/usr/bin:/bin", dir.display());
    let log = dir.join("calls");
    let primary = "us-east4-docker.pkg.dev/build-project/qip-dev";
    let secondary = "us-west1-docker.pkg.dev/build-project/qip-dev-secondary";
    let signed = "sha256:aaaa";
    // (by tag, by digest) -> (exit code, docker calls)
    let case = |by_tag: &str, by_digest: &str| -> (i32, String) {
        let _ = std::fs::remove_file(&log);
        let mut command = Command::new("bash");
        command.arg("-c").arg(&script).env_clear();
        command.env("PATH", &path);
        command.env("LOG", &log);
        command.env("DEPLOY_SHA", "abc123");
        command.env("BINARY", "qip-api");
        command.env("BUILD_REGISTRY", primary);
        command.env("BUILD_REGISTRY_HOST", "us-east4-docker.pkg.dev");
        command.env("SECONDARY_REGISTRY", secondary);
        command.env("PRIMARY_PREFIX", primary);
        command.env("SECONDARY_PREFIX", secondary);
        command.env("PRIMARY", signed);
        command.env("SECONDARY_BY_TAG", by_tag);
        command.env("SECONDARY_BY_DIGEST", by_digest);
        let code = command
            .output()
            .map_or(-1, |o| o.status.code().unwrap_or(-1));
        (code, std::fs::read_to_string(&log).unwrap_or_default())
    };

    // The copy lands with the signed digest: success, and the copy was made
    // from the primary's digest to the replica's commit tag.
    let (code, calls) = case("", signed);
    assert_eq!(
        code, 0,
        "a copy that read back the signed digest was refused"
    );
    assert_eq!(
        calls.trim(),
        format!(
            "buildx imagetools create --tag {secondary}/qip-api:abc123 {primary}/qip-api@{signed}"
        ),
        "the step did not copy the signed digest by digest to the replica's commit tag"
    );
    let auth = std::fs::read_to_string(format!("{}.auth", log.display())).unwrap_or_default();
    assert!(
        auth.trim()
            .split(',')
            .any(|host| host == "us-west1-docker.pkg.dev"),
        "docker was not given credentials for the secondary registry's host: {auth:?}"
    );

    // The replica answers with other bytes for the digest: refused.
    let (code, _) = case("", "sha256:bbbb");
    assert_ne!(
        code, 0,
        "the secondary resolved a different digest and the run was admitted"
    );

    // The replica answers nothing for the digest: refused.
    let (code, _) = case("", "");
    assert_ne!(
        code, 0,
        "the secondary resolved nothing and the run was admitted, so a copy that never \
         landed would read as a release kept in two regions"
    );

    // A re-run finds the tag already holding the signed digest: no second
    // copy, and success.
    let (code, calls) = case(signed, signed);
    assert_eq!(code, 0, "a re-run for a commit already copied was refused");
    assert!(
        calls.is_empty(),
        "a re-run copied again onto an immutable tag: {calls:?}"
    );

    // The replica's tag already holds other bytes: refused, not overwritten.
    let (code, calls) = case("sha256:cccc", signed);
    assert_ne!(
        code, 0,
        "the replica's commit tag held other bytes and the run was admitted"
    );
    assert!(
        calls.is_empty(),
        "the step tried to copy over a tag holding other bytes: {calls:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The copy runs only when a secondary registry is declared, and directly
/// after the signature, so what is copied is what was signed and an
/// environment with none declared — every one today — never reaches it.
///
/// Mutation: delete the step's `if:` line. This fails.
#[test]
fn the_copy_to_the_secondary_registry_runs_only_when_one_is_declared_and_only_after_signing() {
    let workflow = read(DEPLOY);
    let step = step_text(&workflow, COPY_STEP);
    let header: Vec<&str> = step
        .lines()
        .take_while(|line| line.trim() != "run: |")
        .map(str::trim)
        .collect();
    assert!(
        header.contains(&"if: steps.identity.outputs.secondary_registry != ''"),
        "the copy is not gated on the build tfvars declaring a secondary registry ({header:?}); \
         it would run against a repository no environment has"
    );
    assert!(
        !header
            .iter()
            .any(|line| line.starts_with("continue-on-error:")),
        "the copy may fail without failing the job"
    );
    let signed = workflow.find("      - name: sign the pushed image\n");
    let copied = workflow.find(&format!("      - name: {COPY_STEP}\n"));
    let next = signed.and_then(|at| {
        workflow[at + 1..]
            .find("      - name: ")
            .map(|offset| at + 1 + offset)
    });
    assert!(
        matches!((signed, copied), (Some(s), Some(c)) if s < c) && next == copied,
        "the copy is not the step directly after the signature"
    );
}

/// The identity step reads the secondary region from the build
/// environment's tfvars under the region's own refusals.
///
/// Executed in a scratch tree holding one tfvars, because the parse is shell
/// and a `sed` that reads correctly and never matches is the failure a text
/// check cannot see.
///
/// Mutation: make the character-class arm admit anything (`''|*[!a-z0-9-]*`
/// to `''`). The upper-case and command-substitution cases are admitted and
/// this fails naming them.
#[test]
fn the_identity_step_derives_the_secondary_registry_from_the_build_tfvars_and_refuses_a_malformed_one()
 {
    let workflow = read(DEPLOY);
    let script = step_script(&workflow, "derive the identity from the tfvars");
    assert!(
        !script.contains("${{"),
        "premise: the identity step substitutes no expression, so it can be run as written"
    );
    let root = std::env::temp_dir().join(format!("qip-secondary-identity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let environment = root.join("infrastructure/environments/dev");
    std::fs::create_dir_all(&environment).unwrap_or_else(|e| panic!("scratch tree: {e}"));
    let output = root.join("github-output");
    let derive = |line: &str| -> (i32, String) {
        std::fs::write(
            environment.join("terraform.tfvars"),
            format!(
                "project_id     = \"build-project\"\nproject_number = 123456789012\nregion         = \"us-east4\"\n{line}\n"
            ),
        )
        .unwrap_or_else(|e| panic!("tfvars: {e}"));
        let _ = std::fs::remove_file(&output);
        let mut command = Command::new("bash");
        command
            .arg("-c")
            .arg(&script)
            .env_clear()
            .current_dir(&root);
        command.env("PATH", "/usr/bin:/bin");
        command.env("TARGET_ENVIRONMENT", "dev");
        command.env("BUILD_ENVIRONMENT", "dev");
        command.env("GITHUB_OUTPUT", &output);
        let code = command
            .output()
            .map_or(-1, |o| o.status.code().unwrap_or(-1));
        let written = std::fs::read_to_string(&output).unwrap_or_default();
        let secondary = written
            .lines()
            .find_map(|l| l.strip_prefix("secondary_registry="))
            .map(str::to_string)
            .unwrap_or_else(|| format!("<no secondary_registry line in {written:?}>"));
        (code, secondary)
    };

    // Premise: the step runs to the end on this tree, or every refusal below
    // would be a refusal of something else.
    let (code, secondary) = derive("");
    assert_eq!(code, 0, "the identity step failed on a well-formed tfvars");
    assert_eq!(secondary, "", "no secondary_region line declared a replica");

    for line in ["secondary_region = null", "secondary_region = null # off"] {
        assert_eq!(
            derive(line),
            (0, String::new()),
            "`{line}` declared a replica"
        );
    }
    assert_eq!(
        derive("secondary_region = \"us-west1\""),
        (
            0,
            "us-west1-docker.pkg.dev/build-project/qip-dev-secondary".to_string()
        ),
        "a named secondary region did not produce the repository modules/registry declares"
    );

    for line in [
        "secondary_region = \"US-WEST1\"",
        "secondary_region = \"$(id)\"",
        "secondary_region = \"us-west1;true\"",
        "secondary_region = \"\"",
        "secondary_region = us-west1",
        "secondary_region = nul",
        "secondary_region = \"us-east4\"",
    ] {
        let (code, _) = derive(line);
        assert_ne!(
            code, 0,
            "`{line}` was admitted; a malformed or same-region secondary must stop the run \
             rather than be read as none or pasted into a registry host"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_portal_build_id_follows_the_commit_epoch_deploy_passes_and_is_never_a_constant() {
    // R12-CICD-10117. Next names `/_next/static/<id>/` and writes `BUILD_ID`
    // from a random id unless `generateBuildId` is pinned, so two builds of
    // one commit differed in 453 of 2,921 output files (measured
    // 2026-10-25). deploy.yml already passes the commit's epoch as
    // `SOURCE_DATE_EPOCH`. The id is useful only if the config reads it and
    // the Dockerfile's build stage declares it, because an undeclared build
    // arg never reaches `npm run build`. A constant id would also be
    // reproducible, and wrong: every path is served `immutable` for a year,
    // so a constant pins browsers to the first deploy's manifests.
    let config = read("frontend/portal/next.config.ts");
    let code: String = config
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !line.starts_with("//") && !line.starts_with('*') && !line.starts_with("/*")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let pinned = code
        .split("generateBuildId:")
        .nth(1)
        .unwrap_or_else(|| panic!("frontend/portal/next.config.ts does not set generateBuildId"));
    let body: String = pinned.chars().take(200).collect();
    assert!(
        body.contains("process.env.SOURCE_DATE_EPOCH"),
        "the portal's generateBuildId does not read SOURCE_DATE_EPOCH, so its build id does not \
         follow the commit: {body}"
    );
    assert!(
        body.contains("${process.env.SOURCE_DATE_EPOCH}"),
        "the portal's build id is not derived from SOURCE_DATE_EPOCH's value, so it is the same \
         for every commit: {body}"
    );

    let dockerfile = read("infrastructure/docker/portal.Dockerfile");
    let build_stage: Vec<&str> = dockerfile
        .lines()
        .skip_while(|line| !(line.starts_with("FROM ") && line.ends_with(" AS build")))
        .skip(1)
        .take_while(|line| !line.starts_with("FROM "))
        .collect();
    // Premise: the build stage was found and runs the build.
    assert!(
        build_stage
            .iter()
            .any(|line| line.trim() == "RUN npm run build"),
        "premise: portal.Dockerfile's build stage was not read"
    );
    let declared = build_stage
        .iter()
        .position(|line| line.trim() == "ARG SOURCE_DATE_EPOCH");
    let builds = build_stage
        .iter()
        .position(|line| line.trim() == "RUN npm run build");
    assert!(
        matches!((declared, builds), (Some(d), Some(b)) if d < b),
        "portal.Dockerfile's build stage does not declare ARG SOURCE_DATE_EPOCH before \
         `npm run build`, so deploy.yml's --build-arg never reaches next.config.ts"
    );

    // And deploy.yml does pass it on the portal's path, the custom-Dockerfile
    // branch of the build step.
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("recipe=(--build-arg \"SOURCE_DATE_EPOCH=${epoch}\")"),
        "deploy.yml's build recipe no longer starts from SOURCE_DATE_EPOCH, so the portal's build \
         id has nothing to follow"
    );
}

// --- R12-CICD-10117: the portal's second build, compared file by file ------
//
// Two `next build`s of one tree with one epoch differ in four files, each a
// per-build secret Next generates afresh. Owner decision 13: exclude them,
// do not inject a shared secret. deploy.yml rebuilds the portal uncached,
// exports both filesystems and runs `compare-image-trees.sh` over them with
// the committed exceptions list. The tests below execute that same script on
// trees they build, because a comparison that reads correctly and exits 0 on
// every input is the failure only running it can show.

const COMPARE_SCRIPT: &str = ".github/reproducibility/compare-image-trees.sh";
const PORTAL_EXCEPTIONS: &str = ".github/reproducibility/portal-exceptions.txt";
const PORTAL_DOCKERFILE: &str = "infrastructure/docker/portal.Dockerfile";

/// The four image paths decision 13 excuses, and nothing else.
const PER_BUILD_SECRETS: [&str; 4] = [
    "/app/portal/.next/prerender-manifest.json",
    "/app/portal/.next/server/preview-props.json",
    "/app/portal/.next/server/server-reference-manifest.js",
    "/app/portal/.next/server/server-reference-manifest.json",
];

/// The exceptions file's paths: every line that is neither blank nor a
/// comment, trimmed, in file order.
fn listed_exceptions() -> Vec<String> {
    read(PORTAL_EXCEPTIONS)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// A fresh, empty scratch directory for one test.
fn scratch_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "qip-portal-reproducibility-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", path.display()));
    path
}

/// Write `contents` at the image path `path` under `root`.
fn put(root: &std::path::Path, path: &str, contents: &str) {
    let file = root.join(path.trim_start_matches('/'));
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    std::fs::write(&file, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", file.display()));
}

/// One build's filesystem: an ordinary part every build shares, a symlink,
/// and the four secret files carrying a value unique to `build`.
fn portal_tree(root: &std::path::Path, build: &str) {
    put(
        root,
        "/app/portal/server.js",
        "require('./.next/server');\n",
    );
    put(root, "/app/portal/.next/BUILD_ID", "qip-1761868800");
    put(
        root,
        "/app/portal/.next/static/qip-1761868800/_buildManifest.js",
        "self.__BUILD_MANIFEST={};\n",
    );
    put(root, "/etc/alpine-release", "3.22.2\n");
    for secret in PER_BUILD_SECRETS {
        put(root, secret, &format!("{{\"key\":\"{build}-{secret}\"}}"));
    }
    let link = root.join("usr/bin/node-link");
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    std::os::unix::fs::symlink("/usr/local/bin/node", &link)
        .unwrap_or_else(|error| panic!("cannot link {}: {error}", link.display()));
}

/// Two builds of one commit as the measurement found them: identical except
/// for the four per-build secrets. The premise every case below starts from,
/// asserted rather than assumed, so a fixture that wrote the same secret
/// twice cannot make the admitting case pass for the wrong reason.
fn two_builds(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let scratch = scratch_dir(name);
    let first = scratch.join("first");
    let second = scratch.join("second");
    portal_tree(&first, "first");
    portal_tree(&second, "second");
    for secret in PER_BUILD_SECRETS {
        let relative = secret.trim_start_matches('/');
        let a = std::fs::read(first.join(relative)).unwrap_or_default();
        let b = std::fs::read(second.join(relative)).unwrap_or_default();
        assert!(
            !a.is_empty() && a != b,
            "premise: the fixture's two builds must differ in {secret}, or the admitting case \
             proves nothing about the exceptions"
        );
    }
    (scratch, first, second)
}

/// Run the committed comparison over `first` and `second` with the
/// committed exceptions list; its exit code and its stderr.
fn compare(first: &std::path::Path, second: &std::path::Path) -> (i32, String) {
    let root = qip_acceptance::repository_root();
    let script = root.join(COMPARE_SCRIPT);
    assert!(
        script.is_file(),
        "{COMPARE_SCRIPT} does not exist; deploy.yml's portal comparison has nothing to run"
    );
    let output = Command::new("bash")
        .arg(&script)
        .arg(root.join(PORTAL_EXCEPTIONS))
        .arg(first)
        .arg(second)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap_or_else(|error| panic!("cannot run bash: {error}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn the_portal_exceptions_list_names_exactly_the_four_per_build_secret_files() {
    // The list is what makes the check a check. A fifth line excuses a
    // difference nobody has explained; a list that lost one of the four
    // fails every portal build for a known reason and invites somebody to
    // switch the step off.
    let listed = listed_exceptions();
    assert_eq!(
        listed.len(),
        4,
        "{PORTAL_EXCEPTIONS} lists {} paths, {listed:?}; decision 13 excuses exactly four",
        listed.len()
    );
    let mut sorted = listed.clone();
    sorted.sort();
    let mut expected: Vec<String> = PER_BUILD_SECRETS.iter().map(|p| (*p).to_owned()).collect();
    expected.sort();
    assert_eq!(
        sorted, expected,
        "{PORTAL_EXCEPTIONS} does not name exactly the four per-build secret files"
    );
}

#[test]
fn deploy_rebuilds_the_portal_uncached_and_compares_it_with_the_committed_script_and_list() {
    // The script's tests prove nothing about a step that does not run it,
    // or runs it on a build that is not the portal's recipe.
    let workflow = read(DEPLOY);
    let step = step_text(
        &workflow,
        "a second build of the portal differs only in its per-build secrets",
    );
    for needle in [
        "if: steps.build.outcome == 'success' && matrix.binary == 'qip-portal' && steps.build.outputs.where == 'runner'",
        "--no-cache",
        "--build-arg \"SOURCE_DATE_EPOCH=${{ steps.build.outputs.epoch }}\"",
        "--file \"${{ matrix.dockerfile }}\"",
        "\"${{ matrix.context }}\"",
        "docker export \"$container\"",
        "sudo bash .github/reproducibility/compare-image-trees.sh",
        ".github/reproducibility/portal-exceptions.txt",
        "\"$trees/first\" \"$trees/second\"",
    ] {
        assert!(
            step.contains(needle),
            "deploy.yml's portal comparison step no longer carries `{needle}`"
        );
    }
    // The existing step no longer claims the portal is unmeasured.
    assert!(
        !workflow.contains("its reproducibility is recorded as open work"),
        "deploy.yml still says the portal's reproducibility is unmeasured"
    );
    // And the epoch reaches `next build` inside the image, or the build id
    // is random and the comparison fails on every page that names it.
    let dockerfile = read(PORTAL_DOCKERFILE);
    let declared = dockerfile
        .find("\nARG SOURCE_DATE_EPOCH\n")
        .unwrap_or_else(|| panic!("{PORTAL_DOCKERFILE} does not declare SOURCE_DATE_EPOCH"));
    let builds = dockerfile
        .find("\nRUN npm run build\n")
        .unwrap_or_else(|| panic!("{PORTAL_DOCKERFILE} no longer runs `npm run build`"));
    assert!(
        declared < builds,
        "{PORTAL_DOCKERFILE} declares SOURCE_DATE_EPOCH after `next build` has run"
    );
}

#[test]
fn two_portal_builds_differing_only_in_the_four_listed_secrets_are_admitted() {
    let (scratch, first, second) = two_builds("admitted");
    let (code, stderr) = compare(&first, &second);
    assert_eq!(
        code, 0,
        "two builds differing only in the listed secrets were refused, so the check refuses \
         every portal build: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn a_portal_build_differing_in_an_unlisted_file_is_refused_and_the_file_is_named() {
    let (scratch, first, second) = two_builds("unlisted");
    put(
        &second,
        "/app/portal/server.js",
        "require('./.next/elsewhere');\n",
    );
    let (code, stderr) = compare(&first, &second);
    assert_eq!(
        code, 1,
        "a build differing in server.js was not refused: {stderr}"
    );
    assert!(
        stderr.contains("/app/portal/server.js differs between the two builds"),
        "the refusal does not name the file that differed: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn a_listed_secret_file_missing_from_either_build_is_refused() {
    for (which, missing) in [
        ("first", "/app/portal/.next/server/preview-props.json"),
        (
            "second",
            "/app/portal/.next/server/server-reference-manifest.js",
        ),
    ] {
        let (scratch, first, second) = two_builds(&format!("missing-{which}"));
        let tree = if which == "first" { &first } else { &second };
        std::fs::remove_file(tree.join(missing.trim_start_matches('/')))
            .unwrap_or_else(|error| panic!("cannot remove {missing}: {error}"));
        let (code, stderr) = compare(&first, &second);
        assert_eq!(
            code, 1,
            "{missing} gone from the {which} build was not refused: {stderr}"
        );
        assert!(
            stderr.contains(&format!(
                "listed exception {missing} is not a file in the {which} build"
            )),
            "the refusal does not name the listed file gone from the {which} build: {stderr}"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

#[test]
fn a_file_present_in_only_one_portal_build_is_refused_and_named() {
    let (scratch, first, second) = two_builds("extra");
    put(&second, "/app/portal/.next/server/extra-chunk.js", "0\n");
    let (code, stderr) = compare(&first, &second);
    assert_eq!(
        code, 1,
        "a file only the second build holds was not refused: {stderr}"
    );
    assert!(
        stderr.contains(
            "/app/portal/.next/server/extra-chunk.js is in the second build and missing from the first"
        ),
        "the refusal does not name the extra file: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}
