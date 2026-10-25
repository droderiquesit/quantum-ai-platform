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
