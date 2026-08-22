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

use qip_acceptance::{files_with_extension, read};

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
fn a_cluster_holding_a_book_cannot_be_deleted_by_accident() {
    let cluster = read("infrastructure/terraform/modules/cluster/main.tf");
    assert!(sets(&cluster, "deletion_protection", "true"));
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
    for environment in ["development", "staging", "production"] {
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
    for environment in ["development", "staging", "production"] {
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
        if !content.contains("kind: Deployment") {
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
        if !content.contains("kind: Deployment") {
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
        // And the tokens specifically come from a secret.
        if content.contains("QIP_TOKEN_") {
            assert!(
                content.contains("secretKeyRef"),
                "{} sets a token without a secret reference",
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
