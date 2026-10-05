//! Configuration-level proofs for blueprint SEC controls that are Terraform
//! text rather than Rust behaviour.
//!
//! These read the committed configuration. They prove the control is declared
//! and cannot be silently dropped; they do not prove a deployed project has it
//! (nothing is applied), and the register says so.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::read;

/// Configuration with comments removed, so a comment quoting a setting is not
/// mistaken for the setting.
fn code(path: &str) -> String {
    read(path)
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whitespace-free form, so the checks do not depend on alignment.
fn compact(text: &str) -> String {
    text.split_whitespace().collect()
}

#[test]
fn every_internet_facing_cloud_armor_policy_enables_layer_7_adaptive_protection() {
    let modules = [
        "infrastructure/terraform/modules/public-edge/main.tf",
        "infrastructure/terraform/modules/iap-edge/main.tf",
    ];
    for path in modules {
        let text = compact(&code(path));
        assert!(
            text.contains("resource\"google_compute_security_policy\""),
            "{path} declares no Cloud Armor policy, so there is nothing for adaptive \
             protection to be on"
        );
        assert!(
            text.contains("adaptive_protection_config{layer_7_ddos_defense_config{enable=true}}"),
            "{path} does not enable layer-7 adaptive protection; a flood would be found on \
             the bill rather than blocked"
        );
    }
}

#[test]
fn secret_manager_and_kms_data_reads_are_audit_logged() {
    let text = compact(&code("infrastructure/terraform/modules/secrets/main.tf"));
    assert!(
        text.contains("resource\"google_project_iam_audit_config\""),
        "no audit config exists, so a secret's value can be read with no record"
    );
    for service in ["secretmanager.googleapis.com", "cloudkms.googleapis.com"] {
        assert!(
            text.contains(&format!("\"{service}\"")),
            "{service} is not audit-configured"
        );
    }
    for log_type in ["\"DATA_READ\"", "\"DATA_WRITE\"", "\"ADMIN_READ\""] {
        assert!(
            text.contains(log_type),
            "audit log type {log_type} is not enabled; DATA_READ is the only log that \
             records who read a secret's value"
        );
    }
}

#[test]
fn the_container_scanning_api_is_enabled_for_the_registry() {
    let services = code("infrastructure/terraform/modules/services/main.tf");
    assert!(
        services.contains("\"containeranalysis.googleapis.com\""),
        "premise: the always-on API list is the one this test reads"
    );
    assert!(
        services.contains("\"containerscanning.googleapis.com\" ="),
        "Artifact Analysis scanning is not enabled, so a vulnerability disclosed after a \
         push is never recorded against the digest"
    );
}

// --- helpers for the tree-wide walks ----------------------------------------

/// Every Terraform file the repository commits, with comments removed. A
/// `.terraform` directory holds whatever `terraform init` fetched and is not
/// configuration anyone reviewed.
fn terraform_tree() -> Vec<(String, String)> {
    let root = qip_acceptance::repository_root();
    qip_acceptance::files_with_extension("infrastructure", "tf")
        .into_iter()
        .filter(|path| {
            !path
                .components()
                .any(|part| part.as_os_str() == ".terraform")
        })
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"));
            let text = text
                .lines()
                .map(|line| line.split('#').next().unwrap_or("").trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            (relative, text)
        })
        .collect()
}

/// The text between the braces of the block that opens at or after `from`.
fn braced(text: &str, from: usize) -> Option<&str> {
    let open = from + text[from..].find('{')?;
    let mut depth = 0usize;
    for (offset, character) in text[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open + 1..open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every `resource "<kind>" "<name>"` in comment-free Terraform, as its name
/// and the text between its own braces — not everything up to the next
/// resource, which would let a neighbouring `locals` block satisfy a check.
fn resources(text: &str, kind: &str) -> Vec<(String, String)> {
    let marker = format!("resource \"{kind}\" \"");
    text.match_indices(&marker)
        .filter_map(|(at, _)| {
            let after = at + marker.len();
            let name = text[after..].split('"').next()?;
            let body = braced(text, after + name.len())?;
            Some((name.to_string(), body.to_string()))
        })
        .collect()
}

/// The body of the first nested block called `name` inside `body`.
fn nested<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    body.match_indices(name).find_map(|(at, _)| {
        let rest = &body[at + name.len()..];
        // `name {`, not `name_suffix = …` and not `name = …`.
        rest.trim_start()
            .starts_with('{')
            .then(|| braced(body, at))
            .flatten()
    })
}

// --- SEC-070 ----------------------------------------------------------------

#[test]
fn every_virtual_machine_the_repository_defines_is_shielded_with_secure_boot_vtpm_and_integrity_monitoring()
 {
    // SEC-070. The execution-node test reads the one template that exists. A
    // Fabric broker VM, or any second template, would be written by somebody
    // copying the first and dropping a block they did not need to boot, and
    // nothing would say so: a machine without secure boot or a vTPM boots and
    // trades exactly like one with them, until the day its boot chain is the
    // question. So this walks every VM definition in the tree, and the one VM
    // a workflow creates by hand, rather than one module.
    const SETTINGS: [&str; 3] = [
        "enable_secure_boot",
        "enable_vtpm",
        "enable_integrity_monitoring",
    ];
    let mut machines = 0usize;
    for (path, text) in terraform_tree() {
        for kind in [
            "google_compute_instance",
            "google_compute_instance_template",
            "google_compute_region_instance_template",
        ] {
            for (name, body) in resources(&text, kind) {
                machines += 1;
                let shielded = nested(&body, "shielded_instance_config").unwrap_or_else(|| {
                    panic!("{path}: `{kind}.{name}` has no shielded_instance_config block")
                });
                let shielded = compact(shielded);
                for setting in SETTINGS {
                    assert!(
                        shielded.contains(&format!("{setting}=true")),
                        "{path}: `{kind}.{name}` does not set {setting} = true; Reflex and \
                         Fabric compute runs shielded or not at all (SEC-070)"
                    );
                }
            }
        }
    }
    assert!(
        machines >= 1,
        "premise: no VM definition was read under infrastructure/, so every assertion above \
         was vacuous; the walk is not reaching the modules"
    );

    let mut created = 0usize;
    for path in qip_acceptance::files_with_extension(".github/workflows", "yml") {
        let workflow = std::fs::read_to_string(&path).expect("a committed workflow is readable");
        for marker in [
            "gcloud compute instances create",
            "gcloud compute instance-templates create",
        ] {
            for create in workflow.split(marker).skip(1) {
                created += 1;
                // The command ends at the first line that does not continue.
                let command: Vec<&str> = create
                    .lines()
                    .scan(true, |more, line| {
                        let keep = *more;
                        *more = line.trim_end().ends_with('\\');
                        keep.then_some(line)
                    })
                    .flat_map(str::split_whitespace)
                    .collect();
                for flag in [
                    "--shielded-secure-boot",
                    "--shielded-vtpm",
                    "--shielded-integrity-monitoring",
                ] {
                    assert!(
                        command.contains(&flag),
                        "{}: `{marker}` creates a machine without {flag}",
                        path.display()
                    );
                }
            }
        }
    }
    assert!(
        created >= 1,
        "premise: no workflow creates a VM any more; the image bake's builder was the one this \
         check read, so it is now reading nothing"
    );
}

// --- SEC-012, SEC-016, SEC-014 ----------------------------------------------

#[test]
fn every_cloud_nat_translates_only_subnetworks_its_own_module_declares_and_logs_every_connection() {
    // SEC-012 names Cloud NAT as the controlled way out, and until now no test
    // read a `google_compute_router_nat` at all. Three ways it stops being
    // controlled, each of which plans cleanly:
    //
    //   * `ALL_SUBNETWORKS_ALL_IP_RANGES` — every subnet created later in the
    //     region gets internet egress by existing.
    //   * a gateway naming a subnetwork another module owns — the crawler
    //     path and the venue path become one gateway (SEC-016).
    //   * a custom `google_compute_route`, or a reserved external address
    //     bound to something other than a gateway — a second way out that no
    //     NAT log records.
    //
    // And a gateway that logs errors only keeps no record of the connections
    // that succeeded, which are the ones an audit asks about (SEC-014).
    let tree = terraform_tree();
    let mut gateways = 0usize;
    for (path, text) in &tree {
        let declared: Vec<String> = resources(text, "google_compute_subnetwork")
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        for (name, body) in resources(text, "google_compute_router_nat") {
            gateways += 1;
            let squeezed = compact(&body);
            assert!(
                squeezed.contains("source_subnetwork_ip_ranges_to_nat=\"LIST_OF_SUBNETWORKS\""),
                "{path}: NAT `{name}` does not list its subnetworks; any other setting hands \
                 internet egress to every subnet the region later acquires"
            );

            let block = nested(&body, "dynamic \"subnetwork\"")
                .or_else(|| nested(&body, "subnetwork"))
                .unwrap_or_else(|| panic!("{path}: NAT `{name}` names no subnetwork block"));
            let served: Vec<&str> = block
                .lines()
                .filter_map(|line| line.trim().strip_prefix("name"))
                .filter_map(|rest| rest.trim_start().strip_prefix('='))
                .map(str::trim)
                .collect();
            assert!(
                !served.is_empty(),
                "{path}: NAT `{name}` has a subnetwork block that names nothing"
            );
            for reference in served {
                let owner = reference
                    .strip_prefix("google_compute_subnetwork.")
                    .and_then(|rest| rest.split(['.', '[']).next())
                    .unwrap_or_else(|| {
                        panic!(
                            "{path}: NAT `{name}` serves `{reference}`, which is not a \
                             subnetwork resource; a variable here is a gateway whose reach \
                             the module cannot state"
                        )
                    });
                assert!(
                    declared.iter().any(|subnet| subnet == owner),
                    "{path}: NAT `{name}` serves google_compute_subnetwork.{owner}, which this \
                     module does not declare. A gateway carries its own module's subnets and \
                     no other: the zone path and the venue path stay two gateways (SEC-016)"
                );
            }

            let logging = compact(
                nested(&body, "log_config")
                    .unwrap_or_else(|| panic!("{path}: NAT `{name}` has no log_config")),
            );
            assert!(
                logging.contains("enable=true") && logging.contains("filter=\"ALL\""),
                "{path}: NAT `{name}` does not log every connection; ERRORS_ONLY keeps no \
                 record of the egress that succeeded"
            );
        }

        assert!(
            resources(text, "google_compute_route").is_empty(),
            "{path} declares a custom route; a route to the internet that is not a Cloud NAT \
             is a way out that no gateway log records"
        );

        for (name, _) in resources(text, "google_compute_address") {
            let reference = format!("google_compute_address.{name}");
            let on_a_gateway = text
                .lines()
                .any(|line| line.trim_start().starts_with("nat_ips") && line.contains(&reference));
            let elsewhere = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("nat_ips"))
                .any(|line| line.contains(&reference));
            assert!(
                on_a_gateway && !elsewhere,
                "{path}: the reserved address `{name}` is not held by a NAT gateway alone; an \
                 external address on anything else is internet egress with no gateway in front"
            );
        }
    }
    assert!(
        gateways >= 2,
        "premise: {gateways} NAT gateways were read; the zone gateway and the node gateway \
         are two, so the walk is not reaching the modules"
    );
}

/// Workloads whose internet-bound traffic does not pass a Cloud NAT, as the
/// register records them on 2026-10-04. **Open gaps, not accepted ones.**
///
/// * The fleet job (ADR 0102) is an agent workload with no `vpc_access` at
///   all, so it leaves through Cloud Run's own path, where no firewall rule,
///   gateway or allow-list applies. SEC-012's gap.
/// * The dev portal routes only private ranges through the VPC, so every
///   public destination leaves the same way. SEC-052's gap.
///
/// The check below fails on growth only. Parallel lanes merge into one
/// branch, and a lane that closes one of these must not be failed by a list
/// it has never read; strike the entry in the commit that closes it.
const OUTSIDE_THE_NAT: [&str; 2] = [
    "infrastructure/fleet/main.tf:google_cloud_run_v2_job.fleet",
    "infrastructure/gitops/envs/dev/portal.yaml",
];

#[test]
fn no_workload_beyond_the_two_the_register_records_can_reach_the_internet_outside_a_cloud_nat() {
    // A Cloud Run service or job reaches the internet through Google's own
    // egress unless it sends ALL_TRAFFIC through a VPC interface — and that
    // path meets no deny-egress rule, no gateway log and no allow-list. The
    // parity test in gitops.rs holds ALL_TRAFFIC on the catalogue's workloads;
    // it does not read the portal, and it cannot see a job declared in
    // Terraform. This reads every one of both.
    let root = qip_acceptance::repository_root();
    let mut outside: Vec<String> = Vec::new();
    let mut through_the_vpc = 0usize;

    for path in qip_acceptance::files_with_extension("infrastructure/gitops/envs", "yaml") {
        let relative = path
            .strip_prefix(&root)
            .expect("a manifest under the repository")
            .display()
            .to_string();
        let manifest = std::fs::read_to_string(&path).expect("a committed manifest is readable");
        for document in manifest.split("\n---") {
            if !document
                .lines()
                .any(|line| line.trim_end() == "kind: RunService")
            {
                continue;
            }
            let egress: Vec<&str> = document
                .lines()
                .filter_map(|line| line.trim().strip_prefix("egress:"))
                .map(str::trim)
                .collect();
            if egress == ["ALL_TRAFFIC"] {
                through_the_vpc += 1;
            } else {
                outside.push(relative.clone());
            }
        }
    }

    for (path, text) in terraform_tree() {
        for kind in [
            "google_cloud_run_v2_service",
            "google_cloud_run_v2_job",
            "google_cloud_run_service",
        ] {
            for (name, body) in resources(&text, kind) {
                let routed = nested(&body, "vpc_access")
                    .is_some_and(|block| compact(block).contains("egress=\"ALL_TRAFFIC\""));
                if routed {
                    through_the_vpc += 1;
                } else {
                    outside.push(format!("{path}:{kind}.{name}"));
                }
            }
        }
    }

    assert!(
        through_the_vpc >= 10,
        "premise: only {through_the_vpc} workloads were read as routing ALL_TRAFFIC through \
         the VPC; four environments carry three services each, so the walk is not reading \
         the manifests"
    );
    assert!(
        !outside.is_empty(),
        "premise: no workload was read as outside the NAT, but the fleet job and the dev \
         portal are; the walk is not reading what it claims to"
    );
    let unrecorded: Vec<&String> = outside
        .iter()
        .filter(|workload| !OUTSIDE_THE_NAT.contains(&workload.as_str()))
        .collect();
    assert!(
        unrecorded.is_empty(),
        "{unrecorded:?} can reach the internet without passing a Cloud NAT: it does not send \
         ALL_TRAFFIC through a VPC interface, so no deny-egress rule, gateway log or \
         allow-list sees what it dials. Route it through its zone, or argue the exception \
         in SEC-012's row"
    );
}

// --- SEC-050 ----------------------------------------------------------------

#[test]
fn every_internet_facing_cloud_armor_policy_bans_a_client_over_its_rate_with_429_counted_per_address()
 {
    // SEC-050. The plan tests assert that a rate rule exists and that its
    // action is a ban. Neither reads what the rule does to the client that
    // trips it or what it counts by, and both are one-word edits that plan
    // cleanly: `deny(403)` tells an honest client it is forbidden rather than
    // early, and a key of `ALL` counts every visitor in one bucket, so the
    // first busy minute bans the whole internet.
    for path in [
        "infrastructure/terraform/modules/public-edge/main.tf",
        "infrastructure/terraform/modules/iap-edge/main.tf",
    ] {
        let text = code(path);
        let policies = resources(&text, "google_compute_security_policy");
        assert!(
            !policies.is_empty(),
            "premise: {path} declares no Cloud Armor policy, so there is no rate rule to read"
        );
        let mut rate_limited = 0usize;
        for (name, body) in policies {
            let squeezed = compact(&body);
            // An edge-type policy is the only kind a backend bucket accepts,
            // and Cloud Armor gives it no rate rule to carry. The static
            // shell behind it is therefore not rate-limited per client, which
            // SEC-050's row records rather than this test pretending otherwise.
            if squeezed.contains("type=\"CLOUD_ARMOR_EDGE\"") {
                continue;
            }
            rate_limited += 1;
            assert!(
                squeezed.contains("action=\"rate_based_ban\""),
                "{path}: policy `{name}` has no rate_based_ban rule"
            );
            assert!(
                !squeezed.contains("action=\"throttle\""),
                "{path}: policy `{name}` throttles; a throttle lets the next minute start \
                 clean, which an automated client never notices"
            );
            let options = compact(nested(&body, "rate_limit_options").unwrap_or_else(|| {
                panic!("{path}: policy `{name}` has a ban rule with no rate_limit_options")
            }));
            for setting in [
                "conform_action=\"allow\"",
                "exceed_action=\"deny(429)\"",
                "enforce_on_key=\"IP\"",
                "interval_sec=60",
                "count=var.rate_limit_requests_per_minute",
            ] {
                assert!(
                    options.contains(setting),
                    "{path}: policy `{name}`'s rate rule does not set {setting}"
                );
            }
            let ban_seconds: u64 = options
                .split("ban_duration_sec=")
                .nth(1)
                .map(|rest| {
                    rest.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                })
                .and_then(|digits| digits.parse().ok())
                .unwrap_or(0);
            assert!(
                ban_seconds >= 60,
                "{path}: policy `{name}` bans for {ban_seconds}s; a ban shorter than the \
                 counting interval is a throttle under another name"
            );
        }
        assert!(
            rate_limited >= 1,
            "premise: {path} holds only edge-type policies, so no rate rule was read and \
             every assertion above was skipped"
        );
    }
}

// --- SEC-046 ----------------------------------------------------------------

/// Stores that name no customer-managed key, as the register records them on
/// 2026-10-04. **Open gaps in SEC-046, not accepted ones.** Fails on growth
/// only, for the reason `OUTSIDE_THE_NAT` gives.
const UNKEYED_STORES: [&str; 9] = [
    "infrastructure/fleet/main.tf:google_artifact_registry_repository.fleet",
    "infrastructure/fleet/main.tf:google_storage_bucket.fleet",
    "infrastructure/terraform/modules/cloudrun/main.tf:google_storage_bucket.collector_config",
    "infrastructure/terraform/modules/cloudrun/main.tf:google_storage_bucket.config_files",
    "infrastructure/terraform/modules/data/main.tf:google_bigtable_instance.timeseries",
    "infrastructure/terraform/modules/egress-proxy/main.tf:google_storage_bucket.bootstrap",
    "infrastructure/terraform/modules/image-bake/main.tf:google_storage_bucket.payload",
    "infrastructure/terraform/modules/public-edge/main.tf:google_storage_bucket.static_shell",
    "infrastructure/terraform/modules/registry/main.tf:google_artifact_registry_repository.images",
];

#[test]
fn no_data_store_beyond_those_the_register_records_is_left_without_a_customer_managed_key() {
    // SEC-046. Only the evidence bucket's key was asserted by a test; the
    // secrets, warehouse, ledger and model stores each named one and nothing
    // would have said so the day one stopped. A store created without an
    // `encryption` block is encrypted with a key Google holds and rotates,
    // which nobody here can disable when it matters and no audit log of ours
    // records the use of.
    const KINDS: [&str; 8] = [
        "google_storage_bucket",
        "google_bigquery_dataset",
        "google_spanner_database",
        "google_secret_manager_secret",
        "google_artifact_registry_repository",
        "google_bigtable_instance",
        "google_pubsub_topic",
        "google_alloydb_cluster",
    ];
    let mut keyed: Vec<String> = Vec::new();
    let mut unkeyed: Vec<String> = Vec::new();
    for (path, text) in terraform_tree() {
        for kind in KINDS {
            for (name, body) in resources(&text, kind) {
                // A key reference, not the attribute's name alone: `= null`
                // and `= ""` both plan as Google-managed encryption.
                let names_a_key = body.lines().any(|line| {
                    let line = line.trim();
                    (line.starts_with("kms_key_name") || line.starts_with("default_kms_key_name"))
                        && line.split_once('=').is_some_and(|(_, value)| {
                            value.trim().starts_with("google_kms_crypto_key.")
                        })
                });
                let entry = format!("{path}:{kind}.{name}");
                if names_a_key {
                    keyed.push(entry);
                } else {
                    unkeyed.push(entry);
                }
            }
        }
    }

    for kind in [
        "google_storage_bucket",
        "google_bigquery_dataset",
        "google_spanner_database",
        "google_secret_manager_secret",
    ] {
        assert!(
            keyed
                .iter()
                .any(|entry| entry.contains(&format!(":{kind}."))),
            "premise: no {kind} was read as naming a key, and at least one does; the walk is \
             not reading encryption blocks, so the absence below would be trivially true"
        );
    }

    let unrecorded: Vec<&String> = unkeyed
        .iter()
        .filter(|store| !UNKEYED_STORES.contains(&store.as_str()))
        .collect();
    assert!(
        unrecorded.is_empty(),
        "{unrecorded:?} names no customer-managed Cloud KMS key, so its contents are \
         encrypted under a key this platform cannot disable or audit. Give it a key from \
         the environment ring, with the service agent's encrypter grant ahead of it"
    );
}
