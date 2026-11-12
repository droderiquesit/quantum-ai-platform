//! R12-CICD-9116, ADR 0452: a Cloud Run request service is released by a
//! traffic-split canary that Kargo's promotion writes, held by an SLO gate
//! that fails closed, and rolled back to the previous promoted digest in one
//! promotion, because that digest's revision is still in the service at 0%.
//!
//! Read from the committed text: the control plane is suspended (ADR 0093),
//! so nothing here has been evaluated by Kargo, Argo CD or Cloud Run. What
//! this suite holds is that the text says what the record decides, and that
//! a change which quietly breaks one half — a gate that passes on no data, a
//! split on a singleton, a rollback that needs a new revision — fails here.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::fs;
use std::path::PathBuf;

use qip_observability::critical_paths::{SERVICE_ERROR_OBJECTIVE, service_objectives};
use qip_observability::metrics::names;

const ENVIRONMENTS: [&str; 4] = ["dev", "test", "stage", "prod"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn read(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The YAML text of `key:`'s value at exactly `indent` spaces, for flat
/// scalars, with surrounding quotes removed.
fn scalar_after(text: &str, prefix: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(|value| {
            value
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string()
        })
}

/// The `traffic:` block at the end of a RunService manifest, as
/// `(type, revision, percent, tag)` per target, in order.
fn traffic(manifest: &str) -> Vec<(String, Option<String>, u32, Option<String>)> {
    let block = manifest
        .split("\n  traffic:\n")
        .nth(1)
        .expect("the manifest has a top-level spec.traffic block");
    let mut targets = Vec::new();
    for chunk in block.split("\n  - ").map(|c| c.trim_start_matches("  - ")) {
        let mut kind = None;
        let mut revision = None;
        let mut percent = None;
        let mut tag = None;
        for line in chunk.lines().map(str::trim) {
            if line.starts_with('#') {
                continue;
            }
            if let Some(v) = line.strip_prefix("type: ") {
                kind = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("revision: ") {
                revision = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("percent: ") {
                percent = Some(v.parse::<u32>().expect("an integer percent"));
            } else if let Some(v) = line.strip_prefix("tag: ") {
                tag = Some(v.to_string());
            }
        }
        if let (Some(kind), Some(percent)) = (kind, percent) {
            targets.push((kind, revision, percent, tag));
        }
    }
    targets
}

fn api_digest(environment: &str) -> String {
    let kustomization = read(&format!(
        "infrastructure/gitops/envs/{environment}/kustomization.yaml"
    ));
    let block = kustomization
        .split("- name: qip-api\n")
        .nth(1)
        .expect("the kustomization maps qip-api");
    scalar_after(block, "  digest: ").expect("qip-api has a digest line")
}

/// The revision name ADR 0452 derives from a digest:
/// `qip-<env>-api-<first twelve hex>`, which is what the promotion's
/// `Digest[7:19]` slices out of `sha256:<hex>`.
fn revision_for(environment: &str, digest: &str) -> String {
    let hex = digest
        .strip_prefix("sha256:")
        .unwrap_or_else(|| panic!("{digest} is not a sha256 digest"));
    format!("qip-{environment}-api-{}", &hex[..12])
}

// The verification of R12-CICD-9116: rollback to the previous digest is one
// action. It is one action only while the previous digest's revision is
// still in the service under a known name — then a promotion of the
// previous freight is a traffic change, not a new revision. A manifest that
// dropped the standby target, or named a revision by anything but its
// digest, would make the rollback a rebuild again.
#[test]
fn every_api_manifest_keeps_a_stable_and_a_standby_target_so_rollback_is_one_promotion() {
    for environment in ENVIRONMENTS {
        let manifest = read(&format!(
            "infrastructure/gitops/envs/{environment}/api.yaml"
        ));
        let targets = traffic(&manifest);
        assert_eq!(
            targets.len(),
            2,
            "{environment}/api.yaml must carry exactly two traffic targets, stable then standby: \
             {targets:?}"
        );
        assert_eq!(
            targets[0].3.as_deref(),
            Some("stable"),
            "{environment}: index 0 is stable"
        );
        assert_eq!(
            targets[1].3.as_deref(),
            Some("standby"),
            "{environment}: index 1 is standby"
        );
        assert_eq!(
            targets[0].2 + targets[1].2,
            100,
            "{environment}: the two shares sum to 100"
        );
        let digest = api_digest(environment);
        if digest.starts_with("sha256:") {
            let named = revision_for(environment, &digest);
            assert_eq!(
                scalar_after(&manifest, "    revision: ").as_deref(),
                Some(named.as_str()),
                "{environment}: the template's revision must be named from the promoted digest"
            );
            for (kind, revision, _, tag) in &targets {
                assert_eq!(
                    kind, "TRAFFIC_TARGET_ALLOCATION_TYPE_REVISION",
                    "{environment} {tag:?}"
                );
                assert_eq!(
                    revision.as_deref(),
                    Some(named.as_str()),
                    "{environment} {tag:?}"
                );
            }
            assert_eq!(
                targets[0].2, 100,
                "{environment}: at rest the stable share is all of it"
            );
        } else {
            assert_eq!(
                digest, "TO-PIN",
                "{environment}: an unpinned environment says so"
            );
        }
    }
}

// ADR 0452 decision 1. A split gives two revisions a share of requests; on a
// min = max = 1 singleton it would run two decision loops over one book.
#[test]
fn a_singleton_brain_is_never_canaried_by_traffic() {
    let task = read("infrastructure/gitops/kargo/base/promotiontask.yaml");
    for line in task.lines().filter(|l| l.trim_start().starts_with("path:")) {
        assert!(
            !(line.contains("fastbrain.yaml") || line.contains("deepbrain.yaml")),
            "the promotion edits a singleton's manifest: {line}"
        );
    }
    assert!(task.contains("path: ./repo/infrastructure/gitops/envs/${{ ctx.stage }}/api.yaml"));
    for environment in ENVIRONMENTS {
        for brain in ["fastbrain", "deepbrain"] {
            let manifest = read(&format!(
                "infrastructure/gitops/envs/{environment}/{brain}.yaml"
            ));
            assert!(manifest.contains("      minInstanceCount: 1\n      maxInstanceCount: 1\n"));
            let targets = traffic(&manifest);
            assert_eq!(
                targets,
                vec![(
                    "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST".to_string(),
                    None,
                    100,
                    None
                )],
                "{environment}/{brain}.yaml routes anything but 100% to latest"
            );
        }
    }
}

/// One `- uses:` step of the promotion task, by its `as:` name.
fn step<'a>(task: &'a str, name: &str) -> &'a str {
    let start = task
        .find(&format!("      as: {name}\n"))
        .unwrap_or_else(|| panic!("the promotion has no step `{name}`"));
    let begin = task[..start].rfind("    - uses: ").expect("a step header");
    let end = task[start..]
        .find("\n    - uses: ")
        .map_or(task.len(), |offset| start + offset);
    &task[begin..end]
}

const NEXT: &str = "\"qip-\" + ctx.stage + \"-api-\" + imageFrom(vars.warehouseRegistry + \"/qip-api\").Digest[7:19]";

// ADR 0452 decision 2: the three cases, each keyed on the incoming digest's
// revision name, all before the commit. A canary step that wrote 100% to the
// new revision would make every release a cut-over again; a completion that
// kept the canary's share would never finish; a rollback that wrote nothing
// to the standby would delete the previous revision's route.
#[test]
fn the_promotion_writes_canary_completion_and_rollback_from_the_digest_named_revision() {
    let task = read("infrastructure/gitops/kargo/base/promotiontask.yaml");
    let commit = task.find("      as: commit\n").expect("a commit step");

    let parse = step(&task, "api_traffic");
    assert!(parse.contains("uses: yaml-parse"));
    assert!(parse.contains("fromExpression: spec.traffic[0].revision"));
    assert!(parse.contains("fromExpression: spec.traffic[1].revision"));

    let canary = step(&task, "api_canary");
    assert!(canary.contains(&format!("task.outputs.api_traffic.stable != {NEXT}")));
    assert!(canary.contains(&format!("task.outputs.api_traffic.standby != {NEXT}")));
    assert!(canary.contains(
        "key: spec.traffic.0.percent\n            value: ${{ 100 - int(vars.canaryPercent) }}"
    ));
    assert!(canary.contains(&format!(
        "key: spec.traffic.1.revision\n            value: ${{{{ {NEXT} }}}}"
    )));
    assert!(canary.contains(
        "key: spec.traffic.1.percent\n            value: ${{ int(vars.canaryPercent) }}"
    ));

    let complete = step(&task, "api_complete");
    assert!(complete.contains(&format!("task.outputs.api_traffic.standby == {NEXT}")));
    assert!(complete.contains(
        "key: spec.traffic.0.revision\n            value: ${{ task.outputs.api_traffic.standby }}"
    ));
    assert!(complete.contains("key: spec.traffic.0.percent\n            value: ${{ 100 }}"));
    assert!(complete.contains(
        "key: spec.traffic.1.revision\n            value: ${{ task.outputs.api_traffic.stable }}"
    ));
    assert!(complete.contains("key: spec.traffic.1.percent\n            value: ${{ 0 }}"));

    let hold = step(&task, "api_hold");
    assert!(hold.contains(&format!("task.outputs.api_traffic.stable == {NEXT}")));
    assert!(hold.contains("key: spec.traffic.0.percent\n            value: ${{ 100 }}"));
    assert!(hold.contains("key: spec.traffic.1.percent\n            value: ${{ 0 }}"));

    for name in ["api_traffic", "api_complete", "api_hold", "api_canary"] {
        let at = task
            .find(&format!("      as: {name}\n"))
            .expect("step present");
        assert!(
            at < commit,
            "`{name}` must run before the promotion commits"
        );
    }
    assert!(
        task.contains("if: ${{ int(vars.canaryPercent) < 1 || int(vars.canaryPercent) > 50 }}"),
        "an out-of-range canary share is refused, not clamped"
    );
}

// ADR 0452 decision 3. A gate that passed on silence would pass every canary
// in an environment nothing pushes from — and dev pushes nothing until the
// exporter is configured — so the three silent shapes are each a refusal.
#[test]
fn the_slo_gate_reads_the_canary_revisions_own_series_and_fails_closed() {
    let declared = service_objectives()
        .into_iter()
        .find(|path| path.slo.name == SERVICE_ERROR_OBJECTIVE)
        .expect("the service error objective is declared");
    for environment in ENVIRONMENTS {
        let path = format!("infrastructure/gitops/envs/{environment}/slo-gate.yaml");
        let gate = read(&path);
        let kustomization = read(&format!(
            "infrastructure/gitops/envs/{environment}/kustomization.yaml"
        ));
        assert!(
            kustomization.contains("\n- slo-gate.yaml\n"),
            "{environment} does not apply the gate"
        );
        assert!(gate.contains("argocd.argoproj.io/hook: PostSync"), "{path}");
        assert!(
            gate.contains("argocd.argoproj.io/sync-wave: \"1\""),
            "{path} runs after prove-serving"
        );
        assert!(
            gate.contains("        image: google-cloud-cli\n"),
            "{path} runs no qip-* image"
        );
        assert!(
            gate.contains(&format!("SERVICE = \"qip-{environment}-api\"")),
            "{path}"
        );
        assert!(gate.contains(
            "target.get(\"tag\") == \"standby\" and int(target.get(\"percent\") or 0) > 0"
        ));
        assert!(gate.contains("resource.labels.task_id = starts_with(\"{}-\")"));
        assert!(gate.contains(&format!(
            "\"custom.googleapis.com/qip/{}\"",
            names::SERVICE_REQUESTS
        )));
        assert!(gate.contains(&format!(
            "\"custom.googleapis.com/qip/{}\"",
            names::SERVICE_ERRORS
        )));
        assert_eq!(
            scalar_after(&gate, "          AVAILABILITY = ")
                .map(|v| v.parse::<f64>().expect("a number")),
            Some(declared.slo.target),
            "{path}: the gate's objective is the declared one, not a figure of its own"
        );
        let soak: u64 = scalar_after(&gate, "          SOAK_SECONDS = ")
            .expect("a soak")
            .parse()
            .expect("whole seconds");
        assert!(
            soak >= 60,
            "{path}: a soak shorter than one push interval reads no point"
        );
        for refusal in [
            "if request_series == 0 or requests <= 0:\n              sys.exit(",
            "except Exception as error:\n              sys.exit(",
            "if availability < AVAILABILITY:\n              sys.exit(",
            "if not revision:\n              sys.exit(",
        ] {
            assert!(
                gate.contains(refusal),
                "{path} does not refuse on: {refusal}"
            );
        }
    }
}

// The gate can only select a canary's series if the series say which
// revision wrote them; `generic_task`'s task id leads with `K_REVISION`.
#[test]
fn the_pushed_series_carry_the_revision_the_gate_selects_by() {
    let drain = read("backend/crates/services/qip-telemetry-drain/src/cloud_monitoring.rs");
    assert!(drain.contains("pub const REVISION_VARIABLE: &str = \"K_REVISION\";"));
    assert!(drain.contains("|| name == REVISION_VARIABLE"));
    assert!(
        drain.contains("Some(revision) => format!(\"{revision}-{process_id}-{start_millis}\"),")
    );
    assert!(drain.contains("&config.task_id(std::process::id(), start.as_millis()),"));
    let module = read("infrastructure/terraform/modules/gitops-control-plane/main.tf");
    let grant = module
        .split("resource \"google_project_iam_member\" \"argocd_reads_metrics\" {")
        .nth(1)
        .expect("the Argo CD identity is granted the metrics read the gate needs");
    let grant = &grant[..grant.find('}').expect("block end")];
    assert!(
        grant.contains("role    = \"roles/monitoring.viewer\""),
        "read only: {grant}"
    );
    assert!(grant.contains("google_service_account.argocd.email"));
}
