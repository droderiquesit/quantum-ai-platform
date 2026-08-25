//! Structural checks on the infrastructure configuration.
//!
//! `terraform validate` catches a configuration that will not parse or whose
//! references do not resolve. These catch a configuration that parses
//! perfectly and would deploy something unsafe — a public control plane, a
//! node with an external address, a container running as root.
//!
//! They are string checks on HCL and YAML rather than a parse, which is a
//! trade: they cannot understand the configuration, so they can be fooled by
//! someone rewriting it. What they can do is fail when a security property is
//! deleted, which is the change that actually happens.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};

/// Whether a configuration sets a boolean setting to a value.
///
/// Compares with whitespace collapsed, so a `terraform fmt` that realigns the
/// equals signs does not break a security check. A test that fails on
/// formatting is a test people learn to edit rather than read.
fn sets(content: &str, setting: &str, value: &str) -> bool {
    content.lines().any(|line| {
        let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        collapsed == format!("{setting} = {value}")
    })
}

/// A configuration with its comments removed.
///
/// A comment mentioning a dangerous value is not the same as setting one, and
/// a check that cannot tell the difference makes it impossible to document
/// why the value is refused.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

// --- the cluster ------------------------------------------------------------

#[test]
fn nodes_have_no_public_addresses_and_the_control_plane_is_private() {
    // A node that cannot be reached from the internet cannot be reached from
    // the internet, which is a stronger statement than any firewall rule.
    let cluster = read("infrastructure/terraform/modules/cluster/main.tf");
    assert!(
        sets(&cluster, "enable_private_nodes", "true"),
        "nodes must have no public addresses"
    );
    assert!(
        sets(&cluster, "enable_private_endpoint", "true"),
        "a private cluster with a public control plane is private in name only"
    );
}

#[test]
fn the_control_plane_is_never_reachable_from_the_whole_internet() {
    // The validation rule that refuses 0.0.0.0/0, and the empty default that
    // makes forgetting to set it safe rather than dangerous.
    let variables = read("infrastructure/terraform/variables.tf");
    assert!(
        variables.contains(r#"network.cidr_block != "0.0.0.0/0""#),
        "the authorised-networks validation must refuse the whole internet"
    );
    assert!(
        variables.contains("default     = []"),
        "authorised networks must default to none rather than to everything"
    );
}

#[test]
fn the_cluster_enforces_the_controls_that_contain_a_compromised_pod() {
    let cluster = read("infrastructure/terraform/modules/cluster/main.tf");
    for (setting, why) in [
        (
            "network_policy",
            "without it a compromised research pod can reach the execution pod",
        ),
        (
            "workload_identity_config",
            "without it a key file lives on disk and never expires",
        ),
        (
            "database_encryption",
            "without a key we control, revoking access to etcd is not possible",
        ),
        (
            "binary_authorization",
            "without it an unsigned image can run",
        ),
        (
            "enable_secure_boot",
            "without it the boot chain is unverified",
        ),
        (
            "GKE_METADATA",
            "without it a compromised pod reads the node's credentials",
        ),
        (
            "disable-legacy-endpoints",
            "the legacy metadata endpoints are an authentication bypass",
        ),
    ] {
        assert!(
            cluster.contains(setting),
            "the cluster is missing {setting}: {why}"
        );
    }
}

#[test]
fn the_node_pool_does_not_use_the_default_service_account() {
    // The default compute service account has far more permission than any
    // workload needs.
    let cluster = read("infrastructure/terraform/modules/cluster/main.tf");
    assert!(sets(&cluster, "remove_default_node_pool", "true"));
    assert!(cluster.contains("service_account = var.service_account"));
}

#[test]
fn the_throwaway_default_pool_boots_from_the_same_disks_the_real_one_does() {
    // `remove_default_node_pool` deletes the default pool, but GKE creates it
    // first, and on a regional cluster `initial_node_count = 1` is one node
    // per zone. Left unconfigured those three take GKE's default disk,
    // `pd-balanced` at 100GB — 300GB against the 250GB SSD_TOTAL_GB a fresh
    // project gets, since pd-balanced draws on that quota exactly as pd-ssd
    // does. Cluster creation then fails on quota, and it fails identically
    // whatever the node pool's own disks say, because the default pool has
    // never read them. Two applies died that way, the second after the node
    // pool was already on pd-standard.
    //
    // So the cluster's own node_config must exist and must read the same
    // variables: an environment that shrinks its disks to fit a ceiling has
    // to shrink both, or it fixes the half that was never the problem.
    let file = read("infrastructure/terraform/modules/cluster/main.tf");
    let cluster = file
        .split("resource \"google_container_node_pool\"")
        .next()
        .expect("the cluster module no longer declares a node pool after the cluster");

    assert!(
        cluster.contains("node_config {"),
        "the cluster declares no node_config, so its throwaway default pool \
         takes GKE's default disks and can exceed a quota the real pool fits"
    );
    for setting in [
        "disk_type    = var.node_disk_type",
        "disk_size_gb = var.node_disk_size_gb",
    ] {
        assert!(
            cluster.contains(setting),
            "the cluster's default pool does not take `{setting}`, so it can \
             disagree with the pool that replaces it"
        );
    }
}

#[test]
fn a_cluster_holding_a_book_cannot_be_deleted_by_accident() {
    // Was a hardcoded `true`. It is a variable now — `infra.yml down` and the
    // recovery from a tainted cluster both need to turn it off in the
    // environments that need it off — so the safety property this test pins
    // moved from "the module always says true" to "the module wires the
    // field to the variable, and the variable defaults to true". Either
    // check would pass with the field simply deleted, which is why both are
    // asserted rather than one implying the other.
    let cluster = read("infrastructure/terraform/modules/cluster/main.tf");
    assert!(
        cluster.contains("deletion_protection = var.cluster_deletion_protection"),
        "the cluster no longer wires deletion_protection to the variable"
    );

    let variables = read("infrastructure/terraform/modules/cluster/variables.tf");
    let declaration = block_under(&variables, "variable \"cluster_deletion_protection\" {");
    assert!(
        sets(&declaration, "default", "true"),
        "cluster_deletion_protection no longer defaults to true, so a new \
         environment's tfvars silently inherits a cluster nothing protects"
    );
}

#[test]
fn the_control_plane_is_logged_as_well_as_the_workloads() {
    // An audit trail that omits the control plane omits exactly the events an
    // attacker would generate.
    let cluster = read("infrastructure/terraform/modules/cluster/main.tf");
    for component in ["APISERVER", "CONTROLLER_MANAGER", "SCHEDULER"] {
        assert!(
            cluster.contains(component),
            "{component} logging is not enabled"
        );
    }
}

// --- secrets ----------------------------------------------------------------

#[test]
fn no_secret_value_appears_in_the_terraform() {
    // The rule the secrets module exists to enforce: a state file that leaks
    // contains the shape of the deployment and none of its credentials.
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = std::fs::read_to_string(&path).expect("readable");
        assert!(
            !content.contains("google_secret_manager_secret_version"),
            "{} creates a secret version, which puts its value in state",
            path.display()
        );
        assert!(
            !content.contains("secret_data"),
            "{} sets secret data in Terraform",
            path.display()
        );
    }
}

#[test]
fn the_venue_credential_is_unreadable_where_live_trading_is_impossible() {
    // The infrastructure half of the live-trading control. The application
    // refuses a live order below a live autonomy level; this makes the
    // credential unreadable in an environment that could not use it anyway.
    let secrets = read("infrastructure/terraform/modules/secrets/main.tf");
    assert!(
        secrets.contains("count = var.venue_credential_readable ? 1 : 0"),
        "the venue credential's IAM binding must be conditional"
    );

    let root = read("infrastructure/terraform/main.tf");
    assert!(
        root.contains(r#"venue_credential_readable = var.autonomy_ceiling != "paper_trading""#),
        "the condition must be the autonomy ceiling"
    );
}

#[test]
fn every_key_rotates_and_the_ones_that_hold_data_cannot_be_destroyed() {
    let secrets = read("infrastructure/terraform/modules/secrets/main.tf");
    assert_eq!(
        secrets.matches("rotation_period").count(),
        3,
        "both keys and the secrets rotate"
    );
    assert_eq!(
        secrets.matches("prevent_destroy = true").count(),
        2,
        "destroying the key that encrypts etcd destroys the cluster's data"
    );
}

#[test]
fn each_deployable_has_its_own_service_account() {
    // A compromised component has only its own permissions, which is the
    // entire argument for not sharing one.
    let root = read("infrastructure/terraform/main.tf");
    for deployable in ["qip-api", "qip-fastbrain", "qip-deepbrain"] {
        assert!(
            root.contains(deployable),
            "{deployable} has no service account"
        );
    }
}

// --- the environments -------------------------------------------------------

#[test]
fn every_environment_ships_with_a_paper_trading_ceiling() {
    // The one line in the repository that decides whether the platform can
    // reach a real venue. Production included: raising it is a separate,
    // reviewed change.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        ));
        assert!(
            tfvars.contains(r#"autonomy_ceiling = "paper_trading""#),
            "{environment} does not default to paper trading"
        );
    }
}

#[test]
fn no_environment_authorises_the_whole_internet() {
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = without_comments(&read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )));
        assert!(
            !tfvars.contains("0.0.0.0/0"),
            "{environment} authorises the whole internet"
        );
    }
}

#[test]
fn only_prod_lets_the_provider_refuse_to_destroy_its_cluster() {
    // `deletion_protection` is a GKE provider setting, separate from and in
    // addition to Terraform's own `prevent_destroy` — the module default is
    // true, and it refuses a destroy with the same message whether the
    // destroy is `infra.yml down` tearing a dev cluster down on purpose, or
    // Terraform replacing a cluster a failed create left `tainted`. The first
    // real teardown attempt hit the second case: a cluster nobody could
    // recover from, in an environment with no live book to protect.
    //
    // dev, test and stage — every environment `infra.yml` will ever run
    // `down` against — turn it off. prod does not: `infra.yml` already
    // refuses prod outright, so this is defence in depth, not the only
    // thing standing between an agent and a production cluster.
    for environment in ["dev", "test", "stage"] {
        let tfvars = without_comments(&read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )));
        assert!(
            tfvars.contains("cluster_deletion_protection = false"),
            "{environment} leaves deletion_protection at its true default, so \
             infra.yml's own down action — and recovery from a tainted \
             cluster — would be refused by the provider"
        );
    }

    let prod_tfvars = without_comments(&read("infrastructure/environments/prod/terraform.tfvars"));
    assert!(
        !prod_tfvars.contains("cluster_deletion_protection"),
        "prod overrides deletion_protection; it should inherit the module's \
         true default rather than a line that could be edited to false"
    );
}

#[test]
fn the_autonomy_ceiling_variable_accepts_only_the_declared_levels() {
    // The six levels the application declares, and no seventh that would be
    // silently ignored.
    let variables = read("infrastructure/terraform/variables.tf");
    for level in [
        "observation",
        "advisory",
        "paper_trading",
        "supervised_live",
        "limited_autonomous_live",
        "autonomous_live",
    ] {
        assert!(
            variables.contains(level),
            "the ceiling variable does not accept {level}"
        );
    }
    // And the same six the code knows about, so the two cannot drift.
    let levels = qip_risk_engine::autonomy::AutonomyLevel::all();
    assert_eq!(levels.len(), 6);
    for level in levels {
        assert!(
            variables.contains(level.as_str()),
            "{} is a level in the code that the infrastructure does not accept",
            level.as_str()
        );
    }
}

#[test]
fn no_environment_can_be_applied_at_a_ceiling_that_reaches_a_real_venue() {
    // `every_environment_ships_with_a_paper_trading_ceiling` asserts what the
    // four committed files say today. This asserts what the *fifth* one, and
    // any edit to the four, will be allowed to say — which is the part that
    // survives somebody adding an environment.
    //
    // The refusal lives in Terraform because it is the earliest place a live
    // value can be stopped: before the apply writes it into the `qip-config`
    // ConfigMap that every workload reads its ceiling from. It is not the only
    // place, and is not meant to be — Terraform cannot see a `kubectl edit
    // configmap`, so the composition roots refuse the same values at start-up.
    // This layer catches the reviewed, committed mistake; that one catches the
    // unreviewed live edit.
    let variables = read("infrastructure/terraform/variables.tf");

    // The premise: there is a `validation` block that names the negation. A
    // test that only searched for the three level names would pass on the
    // *other* validation, the one that lists all six as permitted spellings.
    assert!(
        variables.contains("condition = !contains(["),
        "no validation refuses anything; the ceiling variable admits every \
         level it can spell"
    );

    // Each live rung named in that refusal. `AutonomyLevel::is_live` is the
    // authority on which those are, so a seventh level added to the ladder as
    // live is a level this test starts requiring — rather than one it silently
    // stops covering, which a hardcoded list of three would do.
    let live: Vec<_> = qip_risk_engine::autonomy::AutonomyLevel::all()
        .into_iter()
        .filter(|level| level.is_live())
        .collect();
    assert_eq!(
        live.len(),
        3,
        "the ladder's live rungs changed; this test's premise needs rewriting"
    );
    let refusal = variables
        .split("condition = !contains([")
        .nth(1)
        .unwrap_or_default();
    // Terminated on the argument that closes the call, not on `])`: the list
    // ends `], var.autonomy_ceiling)`, so a `])` delimiter finds nothing here
    // and swallows the rest of the file — including the *other* validation,
    // which names all six levels and would satisfy every assertion below.
    let refusal = refusal
        .split("], var.autonomy_ceiling)")
        .next()
        .unwrap_or_default();
    for level in live {
        // The quoted, comma-terminated token, not the bare name.
        // `limited_autonomous_live` contains `autonomous_live` as a substring,
        // so a bare `contains` reports the most permissive rung on the list
        // when only the middle one is — which is how the first version of this
        // test passed a mutation that deleted `autonomous_live` outright.
        let entry = format!("\"{}\",", level.as_str());
        assert!(
            refusal.contains(&entry),
            "{} reaches a real venue and the ceiling variable does not refuse it",
            level.as_str()
        );
    }

    // And the refusal stops at the live rungs. A validation that also refused
    // `paper_trading` would pass every assertion above and make the platform
    // unapplyable, which is a different failure and a worse one to debug.
    for permitted in ["observation", "advisory", "paper_trading"] {
        assert!(
            !refusal.contains(&format!("\"{permitted}\",")),
            "the ceiling variable refuses {permitted}, at which no order reaches a venue"
        );
    }
}

// --- Kubernetes -------------------------------------------------------------

#[test]
fn the_namespace_denies_all_traffic_by_default() {
    let namespace = read("infrastructure/kubernetes/base/namespace.yaml");
    assert!(
        namespace.contains("name: default-deny"),
        "without a default deny, every flow is permitted"
    );
    assert!(
        namespace
            .lines()
            .any(|line| line.trim() == "pod-security.kubernetes.io/enforce: restricted")
    );
}

#[test]
fn no_container_runs_as_root_or_can_escalate() {
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        if !is_workload(&content) {
            continue;
        }
        for (setting, why) in [
            (
                "runAsNonRoot: true",
                "a container running as root is a node compromise away",
            ),
            (
                "allowPrivilegeEscalation: false",
                "without it a setuid binary inside the container escalates",
            ),
            (
                "readOnlyRootFilesystem: true",
                "a writable root filesystem lets an attacker persist",
            ),
            (
                "drop: [\"ALL\"]",
                "a container needs none of the default capabilities",
            ),
            (
                "seccompProfile",
                "without one the container can make any syscall",
            ),
        ] {
            assert!(
                content.contains(setting),
                "{} is missing {setting}: {why}",
                path.display()
            );
        }
    }
}

#[test]
fn every_container_has_both_a_cpu_and_a_memory_limit() {
    // A memory limit without a CPU limit lets a busy pod starve its
    // neighbours; a CPU limit without a memory limit lets a leak take down the
    // node.
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        if !is_workload(&content) {
            continue;
        }
        assert!(
            content.contains("limits:"),
            "{} has no limits",
            path.display()
        );
        let after_limits = content.split("limits:").nth(1).unwrap_or("");
        assert!(
            after_limits.contains("cpu:"),
            "{} has no CPU limit",
            path.display()
        );
        assert!(
            after_limits.contains("memory:"),
            "{} has no memory limit",
            path.display()
        );
    }
}

#[test]
fn no_credential_appears_in_a_kubernetes_manifest() {
    // Every credential comes from a secret reference, never from a literal.
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        for line in content.lines() {
            let line = line.trim();
            if !line.starts_with("value:") {
                continue;
            }
            let value = line.trim_start_matches("value:").trim().trim_matches('"');
            assert!(
                !looks_like_a_credential(value),
                "{} has what looks like a literal credential: {line}",
                path.display()
            );
        }
        // And the tokens specifically come from the secret store, as files
        // projected by the CSI driver. `secretKeyRef` was the earlier shape
        // and is refused now: it reads a synced Kubernetes Secret, which puts
        // the plaintext in etcd and does not exist on a fresh cluster until
        // after the first pod has already failed.
        if content.contains("QIP_TOKEN_") {
            assert!(
                content.contains("secrets-store.csi.k8s.io"),
                "{} sets a token without projecting it from the secret store",
                path.display()
            );
            assert!(
                !content.contains("secretKeyRef"),
                "{} reads a credential through a synced Kubernetes Secret; \
                 project it as a file through the CSI driver instead",
                path.display()
            );
        }
    }
}

/// Whether a manifest value looks like a credential rather than configuration.
///
/// Deliberately narrow: a check that flags every long string produces a wall of
/// false positives, and a wall of false positives is a check people stop
/// reading.
fn looks_like_a_credential(value: &str) -> bool {
    value.len() >= 24
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
        && value.chars().any(|c| c.is_ascii_digit())
        && value.chars().any(|c| c.is_ascii_uppercase())
}

#[test]
fn the_autonomy_ceiling_comes_from_a_named_resource_rather_than_a_command_line() {
    // Changing what the platform is permitted to do should appear in a diff
    // and in an audit log.
    let api = read("infrastructure/kubernetes/base/api.yaml");
    assert!(
        api.contains("configMapKeyRef"),
        "the ceiling must come from a config map"
    );
    let config = read("infrastructure/kubernetes/base/config.yaml");
    assert!(
        config.contains(r#"autonomy_ceiling: "paper_trading""#),
        "the shipped config map must be paper trading"
    );
}

#[test]
fn the_api_pod_mounts_no_service_account_token() {
    // The workload authenticates through workload identity; a mounted token is
    // a credential nothing needs.
    let api = read("infrastructure/kubernetes/base/api.yaml");
    assert!(
        api.lines()
            .any(|line| line.trim() == "automountServiceAccountToken: false")
    );
}

// --- CI ---------------------------------------------------------------------

#[test]
fn the_pipeline_gates_on_everything_it_claims_to() {
    let workflow = read(".github/workflows/ci.yml");
    for gate in [
        "cargo fmt --all --check",
        "cargo clippy --workspace --all-targets",
        "cargo test --workspace",
        "cargo build --workspace --release --locked",
        "check-dependencies.sh",
        "cargo audit",
        "cargo cyclonedx",
        "terraform",
        "check-secrets.sh",
    ] {
        assert!(workflow.contains(gate), "the pipeline does not run {gate}");
    }
}

#[test]
fn the_pipeline_fails_on_warnings() {
    // A warning nobody has to fix is a warning everybody stops reading.
    let workflow = read(".github/workflows/ci.yml");
    assert!(workflow.contains(r#"RUSTFLAGS: "-D warnings""#));
}

#[test]
fn the_pipeline_starts_with_the_least_permission_it_can() {
    let workflow = read(".github/workflows/ci.yml");
    assert!(
        workflow.contains("contents: read"),
        "the default token must be read-only"
    );
}

#[test]
fn the_dependency_policy_is_enforced_rather_than_documented() {
    let script = read("scripts/check-dependencies.sh");
    assert!(script.contains("set -euo pipefail"));
    assert!(script.contains("readonly PERMITTED"));
    // And it actually reads the lockfile rather than the manifests, so a
    // transitive dependency cannot slip past.
    assert!(script.contains("Cargo.lock"));
}

// --- the workloads and the binaries they run --------------------------------
//
// The bug these exist to catch is already in the repository's history: an
// `allow-deepbrain-egress` NetworkPolicy governing a pod that no Deployment
// creates. A rule for a workload that does not exist is not harmless — it is a
// reviewer reading the namespace and concluding the deep brain is deployed and
// constrained, when it is neither.
//
// So the correspondence is checked in both directions. A test that only asked
// "does every binary have a Deployment" would have passed on that namespace
// without noticing, because the missing half was the Deployment.

/// Binaries in the workspace that are deliberately not deployed as a workload.
///
/// Kept as a list with a reason attached rather than as a filter in the test,
/// because "why is this one exempt" is the question the next person will have
/// and a predicate cannot answer it.
const NOT_A_WORKLOAD: &[(&str, &str)] = &[(
    // `qip-cli` builds a binary called `qip`.
    "qip",
    "an operator's tool, run by a person against a cluster rather than \
     scheduled in one",
)];

/// Binaries the workspace builds that the pipeline deliberately builds no image
/// for.
///
/// The sibling of `NOT_A_WORKLOAD`, and deliberately a second list rather than
/// a reuse of the first: "nothing schedules it" and "nothing builds it" are
/// different decisions, and a crate could sensibly be one without the other —
/// an operator tool distributed as an image and run as a `Job` would be in the
/// matrix and absent from the manifests.
///
/// Each entry carries the crate it comes from as well as the binary, because
/// the two differ exactly where this matters: `qip-cli` builds `qip`, and a
/// reader searching the decision record for "qip" finds every line in it.
///
/// The reasons are the short form. The decision is
/// `docs/adr/0010-what-gets-deployed.md`, and
/// `every_deployment_exclusion_is_recorded_as_a_decision` is what keeps the two
/// from drifting.
const NOT_IN_THE_IMAGE_MATRIX: &[(&str, &str, &str)] = &[(
    "qip-cli",
    "qip",
    "an operator's tool, run by a person against a cluster rather than \
     scheduled in one",
)];

/// Deployments whose binary is not in the workspace yet.
///
/// This list has to shrink to nothing, and
/// `every_pending_workload_is_still_actually_pending` is what makes it: that
/// test fails the moment the crate lands, so the exemption is removed by
/// whoever lands it rather than surviving as a permanent hole in the check
/// above.
const AWAITING_ITS_CRATE: &[(&str, &str)] = &[];

/// Workloads whose binary has no serving loop, so no probe can be written.
///
/// Same discipline: a probe pointed at an endpoint that does not exist looks
/// like coverage and is not, and an exemption that cannot expire is a
/// permanent one.
const DOES_NOT_SERVE_YET: &[(&str, &str)] = &[
    // Empty, and the list stays here rather than being deleted with its last
    // entry: the discipline it encodes — an exemption states its reason and
    // expires when the reason does — is what the next unserving binary needs,
    // and re-deriving it under deadline is how a permanent hole gets opened.
    //
    // `qip-fastbrain` was the first to leave. `qip-deepbrain` was the last: it
    // now runs a bounded research loop behind its roster validation and serves
    // `/health` and `/ready`, so its Deployment carries real probes and the
    // exemption would be describing a binary that no longer exists.
];

/// The binaries the workspace actually builds, by binary name.
///
/// Read from the manifests rather than from a list here, because a list here
/// would be a second copy of the truth and the whole point of this section is
/// that the two copies drift.
fn workspace_binaries() -> Vec<String> {
    let mut found = Vec::new();
    for path in files_with_extension("crates", "toml") {
        if path.file_name().is_none_or(|name| name != "Cargo.toml") {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("readable");
        let Some(directory) = path.parent() else {
            continue;
        };
        // A `[[bin]]` section names the binary explicitly; a `src/main.rs`
        // with no section produces one named after the package. Both count,
        // because both are something a Deployment could run.
        let declared: Vec<String> = content
            .split("[[bin]]")
            .skip(1)
            .filter_map(|section| {
                section
                    .lines()
                    .find(|line| line.trim_start().starts_with("name"))
                    .and_then(|line| line.split('"').nth(1))
                    .map(str::to_string)
            })
            .collect();
        if !declared.is_empty() {
            found.extend(declared);
            continue;
        }
        if directory.join("src/main.rs").exists() {
            let name = content
                .lines()
                .find(|line| line.trim_start().starts_with("name"))
                .and_then(|line| line.split('"').nth(1))
                .expect("a package declares a name");
            found.push(name.to_string());
        }
    }
    found.sort();
    found.dedup();
    assert!(
        found.len() >= 4,
        "only {found:?} were found; the manifest walk is not reaching the apps"
    );
    found
}

/// Every YAML document in every manifest, paired with the file it came from.
fn manifest_documents() -> Vec<(std::path::PathBuf, String)> {
    let mut documents = Vec::new();
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        for document in content.split("\n---\n") {
            documents.push((path.clone(), document.to_string()));
        }
    }
    assert!(
        documents.len() > 10,
        "only {} documents were found; the manifest walk is not reaching them",
        documents.len()
    );
    documents
}

/// Documents of one kind.
/// Documents whose **own** kind is `kind`.
///
/// The match is anchored at column zero rather than trimmed, because `kind:`
/// appears nested inside several Kubernetes objects and a trimmed match reads
/// those as the document's own kind. A `HorizontalPodAutoscaler` names its
/// target in a `scaleTargetRef` containing `kind: Deployment`, so the trimmed
/// version pulled every HPA into the Deployment set and then panicked on it
/// having no container — which made a correctly written HPA fail four tests
/// that have nothing to do with autoscaling.
///
/// The workaround at the time was to write `scaleTargetRef` as an inline flow
/// mapping so the line never appeared on its own. That is a coupling between a
/// manifest's formatting and a test's parser, and the kind of thing that holds
/// until somebody reformats a file for good reasons.
fn documents_of_kind(kind: &str) -> Vec<(std::path::PathBuf, String)> {
    manifest_documents()
        .into_iter()
        .filter(|(_, document)| document.lines().any(|line| line == format!("kind: {kind}")))
        .collect()
}

/// The value of the first `key:` in a document, at any indentation.
fn first_value(document: &str, key: &str) -> Option<String> {
    document.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix(&format!("{key}:"))
            .map(|value| value.trim().trim_matches('"').to_string())
    })
}

/// The lines nested under `key`, by indentation.
///
/// Splitting on a key and then on a fixed indent looks simpler and is wrong:
/// the remainder begins with the newline the split left behind, so the first
/// element is empty and the check quietly passes on nothing.
fn block_under(text: &str, key: &str) -> String {
    let mut lines = text.lines();
    let opening = lines.find(|line| line.trim() == key);
    let Some(opening) = opening else {
        return String::new();
    };
    let indent = opening.len() - opening.trim_start().len();
    lines
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() > indent)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a manifest declares a workload that runs containers.
///
/// A `StatefulSet` is a `Deployment` that keeps its volumes, and every rule in
/// this file — no root, no escalation, limits, a probe, a pinned image — is
/// about the container rather than about which controller manages it. Keying
/// these checks on `kind: Deployment` alone is how a workload converted to a
/// `StatefulSet` would quietly stop being checked.
fn is_workload(content: &str) -> bool {
    content.contains("kind: Deployment") || content.contains("kind: StatefulSet")
}

/// The kinds of workload the manifests may declare.
const WORKLOAD_KINDS: [&str; 2] = ["Deployment", "StatefulSet"];

/// Every workload document in the manifests, of either kind.
///
/// The single place that knows which controllers run containers, so a third
/// kind is one edit rather than nine.
fn workload_documents() -> Vec<(std::path::PathBuf, String)> {
    WORKLOAD_KINDS
        .iter()
        .flat_map(|kind| documents_of_kind(kind))
        .collect()
}

/// The container blocks inside a workload document.
///
/// Split on the fixed indentation these manifests use. That is brittle on
/// purpose: a reindented manifest makes this return nothing, and every caller
/// asserts it found at least one container, so the failure is loud rather than
/// a check that quietly stops checking.
fn containers(document: &str) -> Vec<String> {
    let Some(start) = document.find("\n      containers:\n") else {
        return Vec::new();
    };
    let body = &document[start..];
    let body = body.split("\n      volumes:").next().unwrap_or(body);
    body.split("\n        - name: ")
        .skip(1)
        .map(str::to_string)
        .collect()
}

/// The binary a container runs, taken from its image reference.
///
/// The image is the honest link between a Deployment and a binary: a container
/// name is a label somebody chose, and an image is the thing that will actually
/// execute.
fn container_binary(container: &str) -> String {
    let image = container
        .lines()
        .find_map(|line| line.trim().strip_prefix("image:"))
        .unwrap_or_else(|| panic!("a container declares an image:\n{container}"))
        .trim();
    let without_tag = image.split(':').next().unwrap_or(image);
    without_tag
        .rsplit('/')
        .next()
        .unwrap_or(without_tag)
        .to_string()
}

/// Every binary a Deployment in the manifests runs.
fn deployed_binaries() -> Vec<String> {
    let mut deployed: Vec<String> = WORKLOAD_KINDS
        .iter()
        .flat_map(|kind| documents_of_kind(kind))
        .collect::<Vec<_>>()
        .iter()
        .flat_map(|(path, document)| {
            let found = containers(document);
            assert!(
                !found.is_empty(),
                "{} has a workload with no container the split could find; \
                 the manifest's indentation has changed and this check has \
                 stopped checking",
                path.display()
            );
            found
        })
        .map(|container| container_binary(&container))
        .collect();
    deployed.sort();
    deployed.dedup();
    deployed
}

#[test]
fn every_deployable_binary_in_the_workspace_has_a_deployment() {
    let deployed = deployed_binaries();
    for binary in workspace_binaries() {
        if NOT_A_WORKLOAD.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            deployed.contains(&binary),
            "{binary} is a binary this workspace builds and nothing deploys it. \
             Either give it a Deployment or add it to NOT_A_WORKLOAD with the \
             reason it is not one."
        );
    }
}

#[test]
fn every_deployment_runs_a_binary_that_exists() {
    // The other direction, and the one the repository actually got wrong.
    let binaries = workspace_binaries();
    for binary in deployed_binaries() {
        if AWAITING_ITS_CRATE.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            binaries.contains(&binary),
            "a Deployment runs {binary}, which no crate in this workspace \
             builds. A manifest for a binary that does not exist reads to a \
             reviewer as a component that is deployed and constrained."
        );
    }
}

#[test]
fn every_pending_workload_is_still_actually_pending() {
    // What keeps AWAITING_ITS_CRATE from becoming permanent. When the crate
    // lands this fails, and the person who landed it deletes the entry — at
    // which point `every_deployment_runs_a_binary_that_exists` starts covering
    // it for real.
    let binaries = workspace_binaries();
    for (binary, reason) in AWAITING_ITS_CRATE {
        assert!(
            !binaries.contains(&(*binary).to_string()),
            "{binary} now exists in the workspace, so the exemption \"{reason}\" \
             is stale. Delete it from AWAITING_ITS_CRATE."
        );
    }
}

#[test]
fn every_workload_that_cannot_be_probed_says_why_and_stops_being_exempt_when_it_can() {
    // A Deployment with no liveness probe is normally a mistake. Here three of
    // them are deliberate, because the binaries do not serve yet, and a probe
    // written against an endpoint that does not exist is worse than none: it
    // looks like coverage.
    for (path, document) in workload_documents() {
        let workload = first_value(&document, "name").expect("a Deployment is named");
        let binary = containers(&document)
            .first()
            .map(|container| container_binary(container))
            .unwrap_or_else(|| panic!("{} has a Deployment with no container", path.display()));

        let exempt = DOES_NOT_SERVE_YET.iter().any(|(name, _)| *name == binary);
        let probed = document.contains("livenessProbe") && document.contains("readinessProbe");

        if exempt {
            assert!(
                !probed,
                "{binary} carries a probe and is still listed in \
                 DOES_NOT_SERVE_YET. If it serves now, delete the entry."
            );
            assert!(
                document.contains("No liveness or readiness probe"),
                "{} does not say why {workload} has no probe",
                path.display()
            );
        } else {
            assert!(
                probed,
                "{workload} has no liveness and readiness probe, and is not \
                 listed in DOES_NOT_SERVE_YET with a reason"
            );
        }
    }
}

// --- identity ---------------------------------------------------------------

#[test]
fn every_service_account_terraform_creates_is_used_by_exactly_one_workload() {
    // Two identities existed with nothing attached to them before this. An
    // unused service account is not merely tidy-up: it is a set of permissions
    // nobody is watching, and the first sign that it is being used is that
    // something has used it.
    let root = read("infrastructure/terraform/main.tf");
    let block = root
        .split("service_accounts = {")
        .nth(1)
        .expect("the root declares its service accounts")
        .split('}')
        .next()
        .expect("the block closes");

    let declared: Vec<String> = block
        .lines()
        .filter_map(|line| line.split('=').nth(1))
        .map(|value| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
        .collect();
    assert!(
        declared.len() >= 3,
        "only {declared:?} were parsed out of the service-account map"
    );

    // Every account has a workload naming it, exactly once.
    let service_accounts: Vec<String> = workload_documents()
        .iter()
        .filter_map(|(_, document)| first_value(document, "serviceAccountName"))
        .collect();

    for account in &declared {
        let uses = service_accounts
            .iter()
            .filter(|name| *name == account)
            .count();
        assert_eq!(
            uses, 1,
            "{account} is created in Terraform and used by {uses} workloads. \
             An identity with nothing attached is permission nobody is watching."
        );
    }

    // And every workload's account is one Terraform creates. An edge cell's
    // account is created in its own module rather than in this map, because a
    // cell is created and destroyed as a unit.
    for account in &service_accounts {
        assert!(
            declared.contains(account) || account.starts_with("qip-edge-"),
            "{account} is named by a workload and created by no Terraform"
        );
    }
}

#[test]
fn every_workload_has_its_own_service_account_and_mounts_no_token() {
    // Sharing an account would undo the entire argument for having several,
    // and a mounted token is a credential nothing here needs: every workload
    // authenticates to Google through workload identity and none of them talks
    // to the Kubernetes API.
    let mut seen: Vec<String> = Vec::new();
    for (path, document) in workload_documents() {
        let workload = first_value(&document, "name").expect("a Deployment is named");
        let account = first_value(&document, "serviceAccountName").unwrap_or_else(|| {
            panic!("{workload} runs under the namespace's default service account")
        });
        assert!(
            !seen.contains(&account),
            "{workload} shares the service account {account} with another workload"
        );
        seen.push(account);

        assert!(
            document
                .lines()
                .any(|line| line.trim() == "automountServiceAccountToken: false"),
            "{} mounts a service-account token for {workload}",
            path.display()
        );
    }
    assert!(
        seen.len() >= 4,
        "only {} workloads were checked",
        seen.len()
    );
}

// --- what a container may do ------------------------------------------------

#[test]
fn every_container_has_a_cpu_and_a_memory_request_as_well_as_a_limit() {
    // The existing check reads the first `limits:` block in a file. This one
    // reads every container, and checks requests too: a container with limits
    // and no requests is scheduled as if it were free, which is how a node ends
    // up with more promised to it than it has.
    let mut checked = 0usize;
    for (path, document) in workload_documents() {
        let found = containers(&document);
        assert!(
            !found.is_empty(),
            "{} has a Deployment whose containers could not be read",
            path.display()
        );
        for container in found {
            let name = container.lines().next().unwrap_or("?").trim();
            let resources = block_under(&container, "resources:");
            assert!(
                !resources.is_empty(),
                "{name} in {} has no resources",
                path.display()
            );
            for section in ["requests:", "limits:"] {
                let block = block_under(&resources, section);
                assert!(!block.is_empty(), "{name} has no {section}");
                assert!(block.contains("cpu:"), "{name} has no cpu in {section}");
                assert!(
                    block.contains("memory:"),
                    "{name} has no memory in {section}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 4, "only {checked} containers were checked");
}

#[test]
fn no_container_in_any_workload_runs_as_root_or_can_escalate() {
    // The existing check asks whether the settings appear anywhere in a file.
    // This one asks per container, so a second container added to a Deployment
    // that already has the settings once cannot arrive without them.
    for (path, document) in workload_documents() {
        assert!(
            document.contains("runAsNonRoot: true"),
            "{} does not set runAsNonRoot",
            path.display()
        );
        assert!(
            !document.contains("runAsUser: 0"),
            "{} runs a container as uid 0",
            path.display()
        );
        for container in containers(&document) {
            let name = container.lines().next().unwrap_or("?").trim();
            for (setting, why) in [
                (
                    "allowPrivilegeEscalation: false",
                    "a setuid binary inside the container escalates without it",
                ),
                (
                    "readOnlyRootFilesystem: true",
                    "a writable root filesystem lets an attacker persist",
                ),
                (
                    "drop: [\"ALL\"]",
                    "a container needs none of the default capabilities",
                ),
            ] {
                assert!(
                    container.contains(setting),
                    "the {name} container in {} is missing {setting}: {why}",
                    path.display()
                );
            }
        }
    }
}

// --- what a workload may reach ----------------------------------------------

/// The `app` label a NetworkPolicy selects, and which directions it governs.
fn policy_targets() -> Vec<(String, bool, bool)> {
    documents_of_kind("NetworkPolicy")
        .iter()
        .filter_map(|(_, document)| {
            let selector = document
                .split("podSelector:")
                .nth(1)
                .and_then(|rest| rest.split("policyTypes:").next())
                .unwrap_or("");
            let app = selector
                .lines()
                .find_map(|line| line.trim().strip_prefix("app:"))
                .map(|value| value.trim().to_string())?;
            let types = block_under(document, "policyTypes:");
            Some((app, types.contains("- Ingress"), types.contains("- Egress")))
        })
        .collect()
}

#[test]
fn every_workload_is_covered_by_both_an_ingress_and_an_egress_policy() {
    // The default is deny, so a workload with no matching policy cannot be
    // reached and cannot reach anything — and it fails by hanging rather than
    // by erroring, which is the failure that takes longest to diagnose. A
    // missing egress rule for the API is exactly what was wrong here.
    let policies = policy_targets();
    let mut checked = 0usize;
    for (_, document) in workload_documents() {
        let app = document
            .split("labels:")
            .nth(1)
            .and_then(|rest| {
                rest.lines()
                    .find_map(|line| line.trim().strip_prefix("app:"))
            })
            .map(|value| value.trim().to_string())
            .expect("a Deployment carries an app label");

        assert!(
            policies
                .iter()
                .any(|(target, ingress, _)| *target == app && *ingress),
            "{app} is covered by no ingress policy, so nothing can reach it and \
             the connection hangs rather than failing"
        );
        assert!(
            policies
                .iter()
                .any(|(target, _, egress)| *target == app && *egress),
            "{app} is covered by no egress policy, so it can reach nothing — \
             not Secret Manager, not the API, not telemetry"
        );
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} workloads were checked");

    // And the default is still deny, so all of the above means something.
    let namespace = read("infrastructure/kubernetes/base/namespace.yaml");
    assert!(namespace.contains("name: default-deny"));
    let deny = namespace
        .split("name: default-deny")
        .nth(1)
        .expect("the default-deny policy exists")
        .split("\n---")
        .next()
        .expect("the document ends");
    assert!(
        deny.contains("podSelector: {}"),
        "the default deny does not select every pod"
    );
    assert!(
        deny.contains("- Ingress") && deny.contains("- Egress"),
        "the default deny does not cover both directions"
    );
}

#[test]
fn no_network_policy_permits_the_whole_internet() {
    // A `0.0.0.0/0` in an egress rule undoes every other rule in the file, and
    // it is one line that looks like the others.
    for (path, document) in documents_of_kind("NetworkPolicy") {
        let content = without_comments(&document);
        assert!(
            !content.contains("0.0.0.0/0"),
            "{} permits the whole internet",
            path.display()
        );
        assert!(
            !content.contains("::/0"),
            "{} permits the whole internet over IPv6",
            path.display()
        );
    }
}

/// The `to:` destinations a named egress policy permits: `ipBlock` CIDRs and
/// the `app` labels of the pods it names.
///
/// Searched across every manifest rather than in one file. A cell's policies
/// live with the cell because they name that cell's venues, and a check that
/// only looked in namespace.yaml would stop finding them the moment they moved.
fn egress_destinations(policy: &str) -> (Vec<String>, Vec<String>) {
    let document = manifest_documents()
        .into_iter()
        .find(|(_, document)| {
            document
                .lines()
                .any(|line| line.trim() == format!("name: {policy}"))
        })
        .map(|(_, document)| document)
        .unwrap_or_else(|| panic!("{policy} exists"));
    let egress = document
        .split("  egress:")
        .nth(1)
        .unwrap_or_else(|| panic!("{policy} has an egress section"))
        .to_string();
    let egress = without_comments(&egress);

    let cidrs = egress
        .lines()
        .filter_map(|line| line.trim().strip_prefix("cidr:"))
        .map(|value| value.trim().to_string())
        .collect();
    let apps = egress
        .lines()
        .filter_map(|line| line.trim().strip_prefix("app:"))
        .map(|value| value.trim().to_string())
        .collect();
    (cidrs, apps)
}

#[test]
fn an_edge_cell_may_reach_its_venues_and_the_central_plane_and_nothing_else() {
    // The most security-relevant rule in the manifests. A cell holds the whole
    // hot path and decides without asking anyone; this policy and the egress
    // firewall in modules/edge-cell are the only two things between a
    // compromised cell and an arbitrary outbound connection.
    let (cidrs, apps) = egress_destinations("allow-edge-egress");

    for cidr in &cidrs {
        assert!(
            cidr == "VENUE_CIDR" || cidr == "199.36.153.8/30",
            "an edge cell may egress to {cidr}, which is neither one of its \
             venues nor the private Google API endpoint"
        );
    }
    assert!(
        cidrs.iter().any(|cidr| cidr == "VENUE_CIDR"),
        "the edge policy names no venue destination at all"
    );

    for app in &apps {
        assert_eq!(
            app, "qip-api",
            "an edge cell may reach {app}. The central plane is the API; a cell \
             that can reach anything else in the namespace is a cell that can \
             reach what that thing can reach."
        );
    }

    // Specifically not the deep brain, and specifically not another cell.
    assert!(
        !apps.iter().any(|app| app == "qip-deepbrain"),
        "an edge cell may reach the deep brain, which can call a language model"
    );
    assert!(
        !apps.iter().any(|app| app == "qip-edge-node"),
        "an edge cell may reach another edge cell; cells are meant to be \
         independent, and a path between them is a path a partition does not cut"
    );
}

#[test]
fn the_fast_brain_cannot_reach_anything_that_could_serve_a_language_model() {
    // ADR 0008, consequence 3: nothing on the hot path consults a model. The
    // binary refuses to start if an agent it hosts holds `call_language_model`;
    // this is the deployment saying the same thing three more ways.
    let (cidrs, apps) = egress_destinations("allow-fastbrain-egress");

    // 1. It may not reach the one workload in the namespace that can call a
    //    model. This is the check that would catch somebody "just letting the
    //    fast brain ask the deep brain".
    for app in &apps {
        assert_ne!(
            app, "qip-deepbrain",
            "the fast brain may reach the deep brain, which is the workload \
             that may call a language model"
        );
    }

    // 2. It has no route off the VPC except private Google access — no
    //    third-party model endpoint, no general egress.
    for cidr in &cidrs {
        assert_eq!(
            cidr, "199.36.153.8/30",
            "the fast brain may egress to {cidr}, which is not private Google \
             access. Any other range is a route to something that could serve a \
             model."
        );
    }

    // 3. It carries nothing that could authenticate to one. No secret at all,
    //    and no environment variable naming a provider or an endpoint.
    let fastbrain = read("infrastructure/kubernetes/base/fastbrain.yaml");
    assert!(
        !fastbrain.contains("secretKeyRef"),
        "the fast brain mounts a secret; it is meant to hold no credential it \
         could call a model with, and to read the venue credential through \
         workload identity where the IAM binding exists at all"
    );
    let stripped = without_comments(&fastbrain).to_lowercase();
    for token in [
        "openai",
        "anthropic",
        "vertex",
        "aiplatform",
        "model_endpoint",
        "llm",
    ] {
        assert!(
            !stripped.contains(token),
            "the fast brain's manifest mentions {token}"
        );
    }

    // 4. And the honest limit of all of the above is written down rather than
    //    left for somebody to discover: private Google access is one range for
    //    every Google API, Vertex AI included, so this layer cannot finish the
    //    job on its own.
    let gaps = read("docs/operations/external-dependencies.md");
    assert!(
        gaps.contains("VPC Service Controls"),
        "the gap document does not name the control that would actually close \
         the fast brain's egress to a model API"
    );
}

// --- the evidence store -----------------------------------------------------

#[test]
fn the_evidence_bucket_is_versioned_and_retained() {
    // `EvidenceStore` is write-once by construction: no delete, no overwrite,
    // a second write of different bytes refused by digest. That guarantee is
    // worth exactly as much as the bucket underneath it.
    let evidence = read("infrastructure/terraform/modules/evidence/main.tf");
    assert!(
        evidence.contains("versioning {"),
        "an overwrite that somehow happens would replace the original"
    );
    assert!(
        evidence.contains("retention_policy {"),
        "without a retention policy a delete is refused by an IAM binding \
         somebody can change rather than by the storage service"
    );
    assert!(
        sets(&evidence, "uniform_bucket_level_access", "true"),
        "a per-object ACL could grant what the bucket policy refuses"
    );
    assert!(
        sets(&evidence, "public_access_prevention", "\"enforced\""),
        "the evidence store must not be reachable without authentication"
    );
    assert!(
        sets(&evidence, "force_destroy", "false"),
        "force_destroy empties the bucket before deleting it, which is a delete \
         with extra steps"
    );
    assert!(
        evidence.contains("default_kms_key_name"),
        "the bucket is not encrypted with a key we control"
    );

    // And the restrictive defaults: the retention policy is locked, which is
    // irreversible and is the point.
    let variables = read("infrastructure/terraform/modules/evidence/variables.tf");
    assert!(
        sets(&variables, "default", "true"),
        "the retention policy does not default to locked"
    );
}

#[test]
fn no_workload_identity_can_delete_from_the_evidence_bucket() {
    // An append-only store whose writer holds a delete permission is not
    // append-only; it is a store nobody has deleted from yet. Each of the roles
    // below carries `storage.objects.delete`.
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        for role in [
            "roles/storage.objectAdmin",
            "roles/storage.objectUser",
            "roles/storage.admin",
            "roles/storage.legacyBucketOwner",
            "roles/owner",
            "roles/editor",
        ] {
            assert!(
                !content.contains(role),
                "{} grants {role}, which can delete an object from the evidence \
                 store",
                path.display()
            );
        }
    }

    // And what is granted is the narrow one.
    let evidence = read("infrastructure/terraform/modules/evidence/main.tf");
    assert!(
        evidence.contains("roles/storage.objectCreator"),
        "the evidence writers hold something other than object creation"
    );
}

// --- the registry -----------------------------------------------------------

#[test]
fn the_image_registry_is_not_world_readable_and_nothing_can_delete_from_it() {
    let registry = read("infrastructure/terraform/modules/registry/main.tf");
    assert!(
        sets(&registry, "immutable_tags", "true"),
        "without immutable tags, the image that was tested and the image that \
         was deployed can be different bytes under the same name"
    );
    assert!(
        registry.contains("roles/artifactregistry.writer"),
        "the pipeline cannot push"
    );
    assert!(
        registry.contains("roles/artifactregistry.reader"),
        "the cluster cannot pull"
    );
    assert!(
        !registry.contains("roles/artifactregistry.repoAdmin"),
        "repoAdmin can delete a version, and a pipeline that can delete a tag \
         can delete the evidence of what it shipped"
    );

    // World-readable, in either of the two ways an IAM binding can be.
    for path in files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        for member in ["allUsers", "allAuthenticatedUsers"] {
            assert!(
                !content.contains(member),
                "{} grants a role to {member}, which tells an attacker exactly \
                 what is running and lets them read it",
                path.display()
            );
        }
    }
}

// --- the edge-cell module ---------------------------------------------------

#[test]
fn the_edge_cells_are_one_module_rather_than_seven_copies() {
    // Seven copies of a network policy is seven places for one of them to be
    // wrong, and the wrong one is the one nobody reads.
    let root = read("infrastructure/terraform/main.tf");
    assert!(
        root.contains("source   = \"./modules/edge-cell\""),
        "there is no edge-cell module"
    );
    assert!(
        root.contains("for_each = var.edge_cells"),
        "the cells are not instantiated from a variable"
    );

    // One cell is configured, and adding the others is a variable change: the
    // runbook carries the map, so the seventh cell is an entry rather than a
    // directory.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        ));
        assert!(
            tfvars.contains("edge_cells = {"),
            "{environment} configures no edge cells"
        );
    }
    let runbook = read("docs/operations/deploying-an-edge-cell.md");
    for cell in [
        "dallas-1",
        "chicago-1",
        "newyork-1",
        "london-1",
        "frankfurt-1",
        "singapore-1",
        "tokyo-1",
        "saopaulo-1",
        "dubai-1",
    ] {
        assert!(
            runbook.contains(cell),
            "the runbook does not name the {cell} cell"
        );
    }
}

#[test]
fn a_cell_gets_its_own_subnet_its_own_identity_and_its_own_binding() {
    // The point of a cell being a module is that a compromised one holds one
    // cell's permissions and one cell's address range.
    let module = read("infrastructure/terraform/modules/edge-cell/main.tf");
    for (resource, why) in [
        (
            "google_compute_subnetwork",
            "without its own subnet a cell's traffic is not separable in a flow log",
        ),
        (
            "google_service_account",
            "sharing an account means a compromised cell holds every cell's permissions",
        ),
        (
            "google_service_account_iam_member",
            "without a workload identity binding the pod needs a key file on disk",
        ),
        (
            "google_compute_firewall",
            "the network half of the constraint the NetworkPolicy makes at the pod level",
        ),
    ] {
        assert!(
            module.contains(resource),
            "the edge-cell module has no {resource}: {why}"
        );
    }

    // Egress is denied and then named, rather than named and then hoped about.
    assert!(
        module.contains("deny {"),
        "the cell's egress is not denied by default"
    );

    // An empty venue map is no venues, not all of them — the same reading
    // `CapitalEnvelope` takes of an empty venue list, for the same reason.
    let variables = read("infrastructure/terraform/modules/edge-cell/variables.tf");
    assert!(
        variables.contains("default = {}"),
        "the venue map does not default to empty"
    );
    assert!(
        variables.contains("0.0.0.0/0"),
        "nothing refuses a venue range of the whole internet"
    );
}

// --- the deployment pipeline ------------------------------------------------

#[test]
fn nothing_deploys_that_has_not_passed_the_test_suite() {
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("workflows: [\"ci\"]"),
        "the deploy workflow is not triggered by the test suite"
    );
    assert!(
        deploy.contains("github.event.workflow_run.conclusion"),
        "the automatic path does not check that ci succeeded"
    );
    assert!(
        deploy.contains("actions/workflows/ci.yml/runs?head_sha="),
        "the manual path does not check that ci succeeded for the commit; \
         somebody pressing a button is not evidence"
    );
    // Both build and deploy hang off the gate.
    assert!(
        deploy.matches("needs: gate").count() >= 1 && deploy.contains("needs: images"),
        "a job can run without the gate"
    );
}

#[test]
fn no_workflow_depends_on_a_repository_variable() {
    // Repository variables were the one input to this pipeline that nothing
    // reviewed. They are written by the bootstrap's last step, which a failed
    // bootstrap never reaches — and once, a bootstrap that DID reach it ran
    // against Cloud Shell's `terraform` stub and captured several lines of apt
    // install instructions into the workload-identity variable. Non-empty, so
    // every check that only asked whether they were set waved it through, and
    // both workflows then failed on an audience nobody could explain.
    //
    // Both derive their identity from the environment's committed tfvars now,
    // where every value is reviewed like any other configuration and a broken
    // bootstrap cannot reach it. A `vars.` creeping back into either workflow
    // reintroduces the entire failure mode.
    for workflow_file in [
        ".github/workflows/infra.yml",
        ".github/workflows/deploy.yml",
    ] {
        let workflow = read(workflow_file);
        assert!(
            !workflow.contains("${{ vars."),
            "{workflow_file} reads a repository variable; derive the value from \
             the environment's tfvars instead"
        );
        // And the derivation is real: both fields it constructs from must be
        // read, or the assertion above is satisfied by a workflow that
        // authenticates with nothing at all.
        for field in ["project_id", "project_number"] {
            assert!(
                workflow.contains(field),
                "{workflow_file} no longer derives {field} from the tfvars"
            );
        }
    }
}

#[test]
fn the_teardown_writes_the_flag_to_state_before_it_reads_it() {
    // `down` is two commands, and the order is the whole point. GKE reads
    // `deletion_protection` from prior state during a destroy, never from
    // configuration, so an environment whose tfvars say false but whose state
    // still says true refuses its own teardown — which is exactly how the
    // first real teardown attempt failed, twice, once before the flag was a
    // variable at all and once after. The targeted apply is what writes false
    // into state; delete it and `down` breaks again, silently, and only for
    // environments that have not been applied since.
    let infra = read(".github/workflows/infra.yml");
    let down = block_under(&infra, "- name: down");

    let apply = down.find("terraform -chdir=infrastructure/terraform apply");
    let destroy = down.find("terraform -chdir=infrastructure/terraform destroy");

    let apply = apply.expect(
        "infra.yml's down no longer applies before destroying, so deletion_protection \
         never reaches state and the teardown refuses itself",
    );
    let destroy = destroy.expect("infra.yml's down no longer destroys anything");
    assert!(
        apply < destroy,
        "infra.yml's down destroys before it applies, so the destroy still reads \
         the old deletion_protection out of state"
    );
}

#[test]
fn the_infrastructure_workflow_cannot_touch_production() {
    // infra.yml holds the identity that can reshape an environment, which is
    // exactly why it must never offer prod. Two layers: prod absent from the
    // dispatch choices, and a step that refuses it even if the choices are
    // edited. Both checked, because either alone is one edit from gone.
    let infra = read(".github/workflows/infra.yml");
    let choices = block_under(&infra, "environment:");
    assert!(
        !choices.contains("- prod"),
        "infra.yml offers prod as a dispatch choice"
    );
    assert!(
        infra.contains("inputs.environment }}\" = \"prod\" ]"),
        "infra.yml no longer refuses prod in a step, so a fork that adds the \
         choice gets it"
    );
    // And the destructive action is targeted, never a full destroy: KMS keys
    // and the workload identity pool are soft-deleted by name, so a full
    // destroy/apply cycle collides with its own remains — including this
    // workflow's own authentication.
    assert!(
        infra.contains("-target=module.cluster"),
        "infra.yml's down is no longer targeted at the cluster"
    );
    assert!(
        !without_comments(&infra)
            .contains("destroy -input=false -auto-approve \\\n            -var-file"),
        "infra.yml runs an untargeted destroy"
    );
}

#[test]
fn every_credential_a_workload_mounts_exists_in_terraform_and_is_readable_by_it() {
    // The chain this pins: a manifest names a path under the CSI mount; the
    // SecretProviderClass projects a Secret Manager secret to that path; the
    // secrets module creates that secret; and an IAM binding lets the
    // workload's identity read it. Each link lived in a different file and
    // nothing held them together, which is how the platform shipped with the
    // API's tokens named in a Secret that nothing created.
    let provider_classes = read("infrastructure/kubernetes/base/secrets.yaml");
    let terraform_root = read("infrastructure/terraform/main.tf");
    let secrets_module = read("infrastructure/terraform/modules/secrets/main.tf");

    // Every path a SecretProviderClass projects, with the secret it comes from.
    let mut projected: Vec<(String, String)> = Vec::new();
    let mut resource = None;
    for line in provider_classes.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("- resourceName:") {
            resource = Some(rest.trim().trim_matches('"').to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("path:")
            && let Some(secret) = resource.take()
        {
            projected.push((secret, rest.trim().trim_matches('"').to_string()));
        }
    }
    assert!(
        projected.len() >= 6,
        "only {} projections found in secrets.yaml; the parse is broken, not the file",
        projected.len()
    );

    for (secret, _path) in &projected {
        // The resource is projects/PROJECT/secrets/<name>-ENVIRONMENT/versions/…
        // and Terraform creates it as "<name>-${var.environment}", so the
        // in-repo spelling to look for is the bare name.
        let name = secret
            .split("/secrets/")
            .nth(1)
            .and_then(|rest| rest.split("/versions").next())
            .and_then(|with_environment| with_environment.strip_suffix("-ENVIRONMENT"))
            .unwrap_or_else(|| panic!("{secret} is not a Secret Manager version reference"));
        assert!(
            terraform_root.contains(&format!("\"{name}\"")),
            "secrets.yaml projects {name} and the secrets module is never told to create it"
        );
    }

    // Every workload that mounts a provider class can read what it projects.
    // The envelope key is projected to every mount, so its IAM grant must
    // cover every workload identity rather than one.
    for manifest in ["api.yaml", "fastbrain.yaml", "deepbrain.yaml"] {
        let content = read(&format!("infrastructure/kubernetes/base/{manifest}"));
        if content.contains("capital-envelope-key") {
            let grant = secrets_module
                .split(
                    "resource \"google_secret_manager_secret_iam_member\" \"capital_envelope_key\"",
                )
                .nth(1)
                .and_then(|rest| rest.split("\nresource ").next())
                .unwrap_or("");
            assert!(
                grant.contains("for_each = var.service_accounts"),
                "{manifest} mounts the capital-envelope key and the secrets module does not \
                 grant every workload identity read on it; the CSI driver would fail the \
                 mount and the pod would sit in ContainerCreating"
            );
        }
    }

    // And the cells still get theirs through their own module, which is the
    // one identity not covered by the central grant.
    let edge = read("infrastructure/terraform/modules/edge-cell/main.tf");
    assert!(
        edge.contains("capital_envelope_key"),
        "the edge-cell module no longer grants its cell read on the envelope key"
    );
}

#[test]
fn the_cluster_runs_the_driver_that_projects_the_secrets_the_manifests_mount() {
    // The manifests ask for `secrets-store.csi.k8s.io` volumes. That driver is
    // a cluster add-on, and a manifest that mounts it on a cluster without it
    // produces pods stuck in ContainerCreating with an event nobody reads
    // until the rollout times out. The two facts live in different languages
    // in different directories, so this is the only place they meet.
    let cluster = without_comments(&read("infrastructure/terraform/modules/cluster/main.tf"));
    let manifests_mount_the_driver = manifest_documents()
        .iter()
        .any(|(_, document)| document.contains("secrets-store.csi.k8s.io"));
    assert!(
        manifests_mount_the_driver,
        "no manifest mounts the secret-store driver any more; if the credential \
         delivery changed shape, retire this test alongside secret_manager_config"
    );
    assert!(
        cluster.contains("secret_manager_config"),
        "the manifests mount secrets-store.csi.k8s.io volumes and the cluster \
         never enables the Secret Manager CSI add-on"
    );
}

#[test]
fn production_is_never_deployed_automatically() {
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("TARGET_ENVIRONMENT: ${{ inputs.environment || 'dev' }}"),
        "the automatic path does not default to dev"
    );
    assert!(
        deploy.contains("refuse a production deployment that nobody dispatched"),
        "nothing refuses an automatic production deployment"
    );
    assert!(
        deploy.contains(
            "env.TARGET_ENVIRONMENT == 'prod' && github.event_name != 'workflow_dispatch'"
        ),
        "the refusal does not test what it claims to"
    );
    assert!(
        deploy.contains("environment: ${{ inputs.environment || 'dev' }}"),
        "the deploy job names no GitHub environment, so required reviewers have \
         nothing to attach to"
    );
}

#[test]
fn the_pipeline_authenticates_without_a_long_lived_key() {
    // A service-account key in a repository secret is a credential that never
    // expires, is copied by anyone who can read the secret, and leaves no trace
    // of which run used it.
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("id-token: write"),
        "the pipeline cannot mint an OIDC token"
    );
    assert!(
        deploy.contains("workload_identity_provider:"),
        "the pipeline does not use workload identity federation"
    );
    assert!(
        !deploy.contains("credentials_json"),
        "the pipeline authenticates with a key"
    );
    assert!(
        !deploy.contains("service_account_key"),
        "the pipeline authenticates with a key"
    );

    // And the pool refuses every repository but this one. Without the
    // condition, any GitHub repository in the world can present a valid token.
    let cicd = read("infrastructure/terraform/modules/cicd/main.tf");
    assert!(
        cicd.contains("attribute_condition"),
        "the workload identity pool accepts a token from any repository"
    );
    assert!(
        cicd.contains("attribute.repository == '${var.github_repository}'"),
        "the pool's condition does not pin the repository"
    );
    let variables = read("infrastructure/terraform/modules/cicd/variables.tf");
    assert!(
        !variables.contains("default     = \""),
        "the repository has a default, and a default here is a repository \
         somebody else could be running"
    );

    // What is missing is written down rather than invented.
    let gaps = read("docs/operations/external-dependencies.md");
    assert!(
        gaps.contains("GCP_WORKLOAD_IDENTITY_PROVIDER"),
        "the variables the pipeline needs are not documented anywhere"
    );
}

// --- what deploys, and what deliberately does not ---------------------------
//
// Three lists have to agree: the binaries the workspace builds, the images the
// pipeline pushes, and the workloads the manifests declare. Every pair of them
// can disagree in both directions, and each of the six failures is quiet:
//
//   * a binary with no image — the crate ships nowhere, and the deploy that was
//     supposed to include it succeeds;
//   * an image with no manifest — a build that costs money and ships nothing,
//     and reads to a reviewer as a deployed component;
//   * a manifest with no image — a rollout waiting for a tag that will never
//     exist;
//   * a workload the rollout check never waits on — a pipeline that reports
//     success for a container that never started.
//
// None of these produces an error anywhere. They produce a deployment that is
// missing something, and a review in which everything present is correct.

/// The binaries the deploy workflow builds an image for.
///
/// Parsed out of the matrix rather than listed here, for the same reason
/// `workspace_binaries` is parsed out of the manifests: a list here would be a
/// third copy of the truth, and this whole section exists because copies drift.
fn image_matrix() -> Vec<String> {
    let deploy = read(".github/workflows/deploy.yml");
    let block = deploy
        .split("\n        binary:\n")
        .nth(1)
        .expect("the image job declares a matrix of binaries")
        .split("\n    steps:")
        .next()
        .expect("the matrix ends where the job's steps begin");
    let mut binaries: Vec<String> = block
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- "))
        .map(str::to_string)
        .collect();
    binaries.sort();
    binaries.dedup();
    assert!(
        binaries.len() >= 4,
        "only {binaries:?} were parsed out of the image matrix; the workflow's \
         indentation has changed and this check has stopped checking"
    );
    binaries
}

/// The manifests the deploy pipeline renders but deliberately does not apply.
///
/// Read off the `case` in the render step, so a manifest that stops being
/// skipped stops being exempt here in the same commit.
fn manifests_the_pipeline_skips() -> Vec<String> {
    let deploy = read(".github/workflows/deploy.yml");
    let skipped: Vec<String> = deploy
        .lines()
        .filter_map(|line| line.trim().strip_suffix(") continue ;;"))
        .map(str::to_string)
        .collect();
    assert!(
        !skipped.is_empty(),
        "no manifest is skipped by the render step. Either every manifest is \
         applied now — in which case the rollout check must wait on all of \
         them — or the `case` has been rewritten and this walk reads nothing."
    );
    skipped
}

/// The workloads the pipeline waits for a rollout of.
fn rollout_workloads() -> Vec<String> {
    let deploy = read(".github/workflows/deploy.yml");
    let line = deploy
        .lines()
        .find(|line| line.trim().starts_with("for workload in "))
        .expect("the pipeline waits for a rollout");
    line.trim()
        .trim_start_matches("for workload in ")
        .split(';')
        .next()
        .expect("the loop's list ends at a semicolon")
        .split_whitespace()
        // Entries are kind-qualified (`deployment/qip-api`), because
        // `rollout status` needs the kind. The name is what this file
        // compares against the manifests.
        .map(|entry| entry.rsplit('/').next().unwrap_or(entry).to_string())
        .collect()
}

/// Every workload the pipeline actually applies, by workload name.
fn workloads_the_pipeline_applies() -> Vec<String> {
    let skipped = manifests_the_pipeline_skips();
    let applied: Vec<String> = workload_documents()
        .into_iter()
        .filter(|(path, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !skipped.iter().any(|skip| skip == name))
        })
        .map(|(_, document)| first_value(&document, "name").expect("a Deployment is named"))
        .collect();
    assert!(
        applied.len() >= 3,
        "only {applied:?} would be applied; the manifest walk is not reaching them"
    );
    applied
}

#[test]
fn every_binary_the_workspace_builds_is_in_the_image_matrix_or_excluded_by_name() {
    // The direction nobody notices: a new deployable lands, the manifests are
    // written, and the matrix is not touched. The apply then references an
    // image tag that was never pushed.
    let matrix = image_matrix();
    for binary in workspace_binaries() {
        if NOT_IN_THE_IMAGE_MATRIX
            .iter()
            .any(|(_, name, _)| *name == binary)
        {
            continue;
        }
        assert!(
            matrix.contains(&binary),
            "{binary} is a binary this workspace builds and deploy.yml builds \
             no image for it. Either add it to the matrix, or add it to \
             NOT_IN_THE_IMAGE_MATRIX with the reason it is not deployed and \
             record that reason in docs/adr/0010-what-gets-deployed.md."
        );
    }
}

#[test]
fn nothing_is_excluded_from_the_image_matrix_and_built_by_it() {
    // What stops the exclusion list becoming a place things are left. An entry
    // that is also in the matrix is an exclusion somebody undid without
    // deleting, and an entry for a binary the workspace no longer builds is one
    // whose crate has gone.
    let matrix = image_matrix();
    let binaries = workspace_binaries();
    for (crate_name, binary, reason) in NOT_IN_THE_IMAGE_MATRIX {
        assert!(
            !matrix.contains(&(*binary).to_string()),
            "{binary} is excluded from the image matrix for the reason \
             \"{reason}\" and the matrix builds it anyway. Delete the entry."
        );
        assert!(
            binaries.contains(&(*binary).to_string()),
            "NOT_IN_THE_IMAGE_MATRIX excludes {binary}, which no crate in this \
             workspace builds. An exclusion for something that does not exist \
             hides the next thing that takes its name."
        );
        assert!(
            repository_root()
                .join(format!("crates/apps/{crate_name}/Cargo.toml"))
                .exists(),
            "NOT_IN_THE_IMAGE_MATRIX names the crate {crate_name}, which is not \
             under crates/apps"
        );
    }
}

#[test]
fn qip_web_is_a_library_and_stops_being_exempt_the_moment_it_is_not() {
    // The one crate under crates/apps with no image, no manifest and no
    // service account — and the one whose absence most looks like an
    // oversight. It is a library qip-api links and renders from, so its pages
    // are already served by the qip-api image on qip-api's port, and there is
    // no second process to schedule.
    //
    // This is checked rather than left in a comment because the day it grows a
    // `main.rs` is the day four files need changing together: the matrix, a
    // manifest, the service-account map in Terraform, and the rollout check.
    // Without this the crate would simply build a binary nothing deploys.
    let manifest = read("crates/apps/qip-web/Cargo.toml");
    assert!(
        !manifest.contains("[[bin]]"),
        "qip-web declares a binary, and the deployment configuration has no \
         entry for it anywhere: no image in deploy.yml, no manifest, no \
         service account in Terraform. Either give it all of them or take the \
         binary back out. See docs/adr/0010-what-gets-deployed.md."
    );
    assert!(
        !workspace_binaries().contains(&"qip-web".to_string()),
        "qip-web now produces a binary. See docs/adr/0010-what-gets-deployed.md \
         for the four places that have to change with it."
    );

    // And it is still linked by the thing that serves it. A library nothing
    // links is not a decision not to deploy it; it is dead code.
    let api = read("crates/apps/qip-api/Cargo.toml");
    assert!(
        api.contains("qip-web.workspace = true"),
        "qip-api no longer links qip-web, so nothing renders its pages and \
         nothing serves them. Either something else does — and then that is \
         what deploys them — or the crate is unused."
    );
}

#[test]
fn every_image_the_matrix_builds_has_a_manifest_that_runs_it() {
    // An image nobody deploys is a build that costs money and ships nothing.
    // Worse, it reads to a reviewer as a component that is deployed: the
    // pipeline visibly builds and pushes it, and nothing says the cluster never
    // asks for it.
    let deployed = deployed_binaries();
    for binary in image_matrix() {
        assert!(
            deployed.contains(&binary),
            "the pipeline builds and pushes {binary} and no manifest runs it. \
             Either write the manifest or take it out of the matrix."
        );
    }
}

#[test]
fn the_pipeline_builds_an_image_for_every_workload_it_deploys() {
    // The reverse, and the one that fails a deployment rather than wasting a
    // build: a Deployment whose image nothing pushes is a rollout waiting for a
    // tag that will never exist.
    let matrix = image_matrix();
    for binary in deployed_binaries() {
        if AWAITING_ITS_CRATE.iter().any(|(name, _)| *name == binary) {
            continue;
        }
        assert!(
            matrix.contains(&binary),
            "{binary} has a Deployment and the pipeline builds no image for it"
        );
    }
}

#[test]
fn every_manifest_pulls_from_the_repository_the_pipeline_pushes_to() {
    // Three places name the same repository and none of them can see the other
    // two: the workflow builds `<region>-docker.pkg.dev/<project>/qip-<env>`,
    // Terraform creates `qip-<env>`, and every manifest writes `IMAGE_PREFIX`
    // for the workflow to substitute. If any drifts, every pull in every
    // environment fails with a 404 that names neither of the two that disagree.
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains(
            "docker.pkg.dev/${{ steps.identity.outputs.project }}/qip-${TARGET_ENVIRONMENT}"
        ),
        "the pipeline no longer pushes to qip-<environment> in the project's \
         Artifact Registry"
    );
    let registry = read("infrastructure/terraform/modules/registry/main.tf");
    assert!(
        sets(&registry, "repository_id", r#""qip-${var.environment}""#),
        "the registry Terraform creates is not the one the pipeline pushes to"
    );

    // And no manifest pins a registry of its own. A hard-coded prefix survives
    // the pipeline's placeholder check — there is no placeholder left in it —
    // and pulls from wherever it says, in every environment.
    let mut checked = 0usize;
    for (path, document) in workload_documents() {
        for container in containers(&document) {
            let image = container
                .lines()
                .find_map(|line| line.trim().strip_prefix("image:"))
                .map(str::trim)
                .unwrap_or_else(|| panic!("a container declares an image"));
            assert!(
                image.starts_with("IMAGE_PREFIX/"),
                "{} pulls {image}, which names a registry rather than deferring \
                 to the one the pipeline substitutes",
                path.display()
            );
            assert!(
                image.ends_with(":IMAGE_TAG"),
                "{} pulls {image}, which pins a tag rather than the commit \
                 being deployed",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked >= 4, "only {checked} images were checked");
}

#[test]
fn the_rollout_waits_on_every_workload_the_pipeline_applies() {
    // `kubectl apply` returns when the API server has accepted the objects, not
    // when the containers are running. Without this step a pipeline reports
    // success for an image that crashes on start-up, which is the failure the
    // whole deployment exists to catch.
    //
    // Both directions matter. A workload missing from the list is never
    // checked; a workload on the list that the pipeline did not apply makes
    // `rollout status` wait on a Deployment nobody created until it times out,
    // which fails the deployment for a reason that is not the real one.
    let waited = rollout_workloads();
    let applied = workloads_the_pipeline_applies();

    for workload in &applied {
        assert!(
            waited.contains(workload),
            "the pipeline applies {workload} and never waits for its rollout, \
             so a {workload} that does not start is a deployment that reports \
             success"
        );
    }
    for workload in &waited {
        assert!(
            applied.contains(workload),
            "the rollout waits for {workload}, which this pipeline does not \
             apply. `kubectl rollout status` on a Deployment nobody created \
             waits until it times out."
        );
    }
}

#[test]
fn every_manifest_is_somewhere_something_applies_it() {
    // `infrastructure/kubernetes/overlays` was an empty directory nothing
    // referenced — no kustomization, no pipeline step, no runbook. Empty it was
    // harmless. With a manifest in it, it would have been a set of resources a
    // reviewer reads as deployed and that nothing applies.
    let deploy = read(".github/workflows/deploy.yml");
    assert!(
        deploy.contains("for manifest in infrastructure/kubernetes/base/*.yaml"),
        "the pipeline no longer renders infrastructure/kubernetes/base/*.yaml, \
         so this check is looking at the wrong directory"
    );

    let skipped = manifests_the_pipeline_skips();
    let runbooks: String = files_with_extension("docs/operations", "md")
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect();

    let mut checked = 0usize;
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a manifest has a name")
            .to_string();
        assert!(
            path.parent()
                .is_some_and(|parent| parent.ends_with("infrastructure/kubernetes/base")),
            "{} is a manifest outside the one directory the pipeline renders. \
             Nothing applies it.",
            path.display()
        );
        if skipped.contains(&name) {
            assert!(
                runbooks.contains(&name),
                "{name} is skipped by the deploy pipeline and named by no \
                 runbook in docs/operations, so nothing applies it and nobody \
                 is told to"
            );
        }
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} manifests were checked");
}

/// The environment variables a binary refuses to start without.
///
/// Read out of the source rather than listed here. `required("QIP_X", &mut
/// missing)` is the call that puts a variable on the list the binary exits on,
/// so matching it is matching the actual requirement rather than a second
/// statement of it.
fn variables_the_binary_refuses_to_start_without(source: &str) -> Vec<String> {
    // `required_secret(` is the credential variant of the same refusal, and a
    // split on `required(` alone does not match it — which is exactly how the
    // envelope key fell out of this walk when it moved to `qip_core::secret`
    // and took a `_FILE` alternative. Both sites put the variable on the list
    // the binary exits on, so both belong here.
    ["required(\"", "required_secret(\""]
        .iter()
        .flat_map(|call| {
            source
                .split(call)
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn every_variable_a_deployable_refuses_to_start_without_is_set_by_its_manifest() {
    // The bug this was written for: `edge-cell.yaml` set QIP_CELL_ID,
    // QIP_CELL_REGION and QIP_CAPITAL_ENVELOPE_KEY and not QIP_VENUES, which
    // `qip-edge-node` also requires. The container would have exited with a
    // configuration error and been restarted for ever.
    //
    // Nothing else would have caught it. The manifest is not applied by the
    // pipeline, so no rollout check runs against it; the runbook that does
    // apply it substituted the placeholders that were there.
    let mut checked = 0usize;
    for (path, document) in workload_documents() {
        for container in containers(&document) {
            let binary = container_binary(&container);
            let source = format!("crates/apps/{binary}/src/main.rs");
            if !repository_root().join(&source).exists() {
                continue;
            }
            for variable in variables_the_binary_refuses_to_start_without(&read(&source)) {
                // `- name: {variable}_FILE` also satisfies the requirement:
                // `qip_core::secret` accepts the file variant, and it is the
                // one the manifests use for anything projected by the CSI
                // driver. The plain match below matches it too, as a prefix —
                // stated here so a reader does not think the file variant
                // slips through unchecked.
                assert!(
                    container.contains(&format!("- name: {variable}")),
                    "{binary} refuses to start without {variable} and {} does \
                     not set it. The container exits with a configuration error \
                     and is restarted for ever, which looks like a crash loop \
                     rather than a missing value.",
                    path.display()
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 4,
        "only {checked} required variables were checked; the walk from a \
         container to the source of the binary it runs is finding nothing"
    );
}

#[test]
fn every_deployment_exclusion_is_recorded_as_a_decision() {
    // A reason in a `const` is read by whoever is already editing this file.
    // The person asking why the operator CLI is not in the cluster, or why
    // there are six application crates and four images, is not that person —
    // and a comment in a build matrix is not where they will look.
    let adr = read("docs/adr/0010-what-gets-deployed.md");

    for (crate_name, _, _) in NOT_IN_THE_IMAGE_MATRIX {
        assert!(
            adr.contains(crate_name),
            "{crate_name} is excluded from the image matrix and the decision \
             record does not mention it"
        );
    }
    for (binary, _) in NOT_A_WORKLOAD {
        assert!(
            adr.contains(binary),
            "{binary} is deployed by no workload and the decision record does \
             not mention it"
        );
    }
    assert!(
        adr.contains("qip-web"),
        "the decision record does not say why qip-web is neither built nor \
         deployed, which is the exclusion that most looks like an oversight"
    );

    // And the record lists what *is* deployed too. A record of only the
    // exceptions cannot be checked against the thing it describes.
    for binary in image_matrix() {
        assert!(
            adr.contains(&binary),
            "the decision record does not name {binary}, which the pipeline \
             builds and deploys"
        );
    }
}

// --- the safety property that outranks all of this --------------------------

#[test]
fn nothing_added_here_raises_the_autonomy_ceiling_anywhere() {
    // Everything above adds workloads, identities and network paths. None of it
    // is allowed to change the one line that decides whether the platform can
    // reach a real venue — and the venue credential's IAM binding must stay
    // absent wherever the ceiling is paper trading.
    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = without_comments(&read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        )));
        assert!(
            tfvars.contains(r#"autonomy_ceiling = "paper_trading""#),
            "{environment} does not ship with a paper-trading ceiling"
        );
        for level in [
            "supervised_live",
            "limited_autonomous_live",
            "autonomous_live",
        ] {
            assert!(
                !tfvars.contains(level),
                "{environment} names {level}, which is above paper trading"
            );
        }
    }

    // The config map the workloads read is paper trading too, and every new
    // workload reads it from that config map rather than from its own default.
    let config = read("infrastructure/kubernetes/base/config.yaml");
    assert!(config.contains(r#"autonomy_ceiling: "paper_trading""#));
    for manifest in ["fastbrain.yaml", "deepbrain.yaml", "edge-cell.yaml"] {
        let content = read(&format!("infrastructure/kubernetes/base/{manifest}"));
        assert!(
            content.contains("key: autonomy_ceiling"),
            "{manifest} does not take the ceiling from the config map"
        );
        assert!(
            !content.contains("supervised_live"),
            "{manifest} names a live autonomy level"
        );
    }

    // And the binding is still conditional on the ceiling, with nothing new
    // granting the venue credential unconditionally.
    let secrets = read("infrastructure/terraform/modules/secrets/main.tf");
    let bindings = secrets.matches("qip-venue-credential").count();
    assert_eq!(
        bindings, 1,
        "the venue credential is named {bindings} times in the secrets module; \
         exactly one of them is the conditional IAM binding"
    );
}
