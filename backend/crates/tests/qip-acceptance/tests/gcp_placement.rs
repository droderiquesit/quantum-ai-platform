//! Placement properties of the Google Cloud configuration, walked across the
//! whole tree rather than read from one named module.
//!
//! `infrastructure.rs` already holds most of what is below for the module
//! that existed when each check was written: the execution node declares no
//! load-balancer hop, the execution node boots shielded, two gateways each
//! have a plan harness asserting a reserved address. A property held for a
//! named file is held by nothing for the next file written, and the next
//! file here is the Fabric broker's — the blueprint puts it on the same kind
//! of machine under the same prohibitions, and no existing test would have
//! opened it.
//!
//! So every check walks every `.tf` under `infrastructure/`, and each asserts
//! its premise first: that it found the gateways, the machines and the load
//! balancers it is about to make a claim over. A walk that reaches nothing
//! passes everything.
//!
//! These are string checks on HCL, with the trade `infrastructure.rs` names:
//! they cannot understand the configuration, and they do fail when a property
//! is deleted, which is the change that actually happens.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::{files_with_extension, repository_root};
use std::collections::BTreeSet;

/// One `resource "<kind>" "<name>" { … }` block and the file it was read from.
struct Resource {
    file: String,
    kind: String,
    name: String,
    body: String,
}

impl Resource {
    /// How a failure names this block, so the reader can open it.
    fn at(&self) -> String {
        format!("{}: `{}.{}`", self.file, self.kind, self.name)
    }

    /// The right-hand side of the first `key = …` line in the block.
    fn setting(&self, key: &str) -> Option<String> {
        assignment(&self.body, key)
    }

    /// The body of the first `name { … }` block nested in this one, or of
    /// its `dynamic "name" { … }` spelling.
    fn block(&self, name: &str) -> Option<String> {
        braces(&self.body, &format!("{name} {{"))
            .or_else(|| braces(&self.body, &format!("\"{name}\" {{")))
    }
}

/// The resource name following `<kind>.` in an expression, if it names one.
fn reference<'a>(expression: &'a str, kind: &str) -> Option<&'a str> {
    let rest = expression.split(&format!("{kind}.")).nth(1)?;
    rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .next()
        .filter(|name| !name.is_empty())
}

/// The right-hand side of the first `key = …` line, whitespace collapsed so a
/// `terraform fmt` that realigns an equals sign does not change the answer.
///
/// The whole key is matched, never a prefix of it: `enable = ` must not be
/// satisfied by `enable_cdn = `.
fn assignment(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        line.strip_prefix(&format!("{key} = ")).map(str::to_string)
    })
}

/// The text between the brace that ends `opening` and the brace matching it.
fn braces(text: &str, opening: &str) -> Option<String> {
    let start = text.find(opening)? + opening.len();
    let mut depth = 1usize;
    for (offset, character) in text[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..start + offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Every resource block in every Terraform file under `infrastructure/`,
/// comments removed — a comment naming a refused value is not a setting.
fn resources() -> Vec<Resource> {
    let root = repository_root();
    let mut found = Vec::new();
    for path in files_with_extension("infrastructure", "tf") {
        let text: String = std::fs::read_to_string(&path)
            .expect("readable")
            .lines()
            .map(|line| line.split('#').next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let file = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for (at, _) in text.match_indices("resource \"") {
            // A block opens at the start of a line; anything else is a
            // string that happens to quote one.
            if at != 0 && text.as_bytes()[at - 1] != b'\n' {
                continue;
            }
            let header = &text[at..];
            let mut quoted = header.split('"');
            let (Some(kind), Some(name)) = (quoted.nth(1), quoted.nth(1)) else {
                continue;
            };
            let Some(body) = braces(header, "{") else {
                continue;
            };
            found.push(Resource {
                file: file.clone(),
                kind: kind.to_string(),
                name: name.to_string(),
                body,
            });
        }
    }
    found
}

/// The resource types that are a machine, or the group that creates one.
const MACHINE_KINDS: [&str; 5] = [
    "google_compute_instance",
    "google_compute_instance_template",
    "google_compute_region_instance_template",
    "google_compute_instance_group_manager",
    "google_compute_region_instance_group_manager",
];

/// Whether a resource type is something a request passes through on its way
/// to a backend: a balancer's listener, its backend, its proxy, or a mesh.
fn is_a_proxy_hop(kind: &str) -> bool {
    kind.contains("forwarding_rule")
        || kind.contains("backend_service")
        || kind.contains("url_map")
        || kind.contains("_proxy")
        || kind.contains("target_pool")
        || kind.contains("network_endpoint_group")
        || kind.contains("service_attachment")
        || kind.starts_with("google_network_services")
        || kind.contains("mesh")
}

#[test]
fn every_cloud_nat_translates_named_subnets_to_a_reserved_address_in_its_own_region_and_logs_it() {
    // GCP-013: outbound traffic leaves through a gateway whose address is a
    // fact somebody can allow-list and whose every translation is on record.
    // Each of the two gateways has a plan harness asserting MANUAL_ONLY. What
    // neither harness can see is a third gateway: one added in a new module
    // with Google's defaults allocates its address automatically, translates
    // every subnet in the region and logs nothing, and both harnesses pass.
    let all = resources();
    let gateways: Vec<&Resource> = all
        .iter()
        .filter(|r| r.kind == "google_compute_router_nat")
        .collect();
    assert!(
        gateways.len() >= 2,
        "only {} Cloud NAT gateways were read; the trust zones and the execution node each \
         declare one, so the walk is not reaching the modules",
        gateways.len()
    );

    for nat in gateways {
        let at = nat.at();
        let region = nat
            .setting("region")
            .unwrap_or_else(|| panic!("{at} names no region"));

        assert_eq!(
            nat.setting("nat_ip_allocate_option").as_deref(),
            Some("\"MANUAL_ONLY\""),
            "{at} does not allocate its addresses MANUAL_ONLY. An automatic address is one Google \
             replaces as the gateway scales, so nothing outbound is attributable to it"
        );

        // Every address it translates to is one its own module reserved.
        let addresses = nat.setting("nat_ips").unwrap_or_default();
        let named: Vec<&str> = addresses
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .collect();
        assert!(
            !named.is_empty(),
            "{at} lists no nat_ips on one line; write the reserved addresses as a single-line \
             list so this check can read them"
        );
        for entry in named {
            let name = entry
                .starts_with("google_compute_address.")
                .then(|| reference(entry, "google_compute_address"))
                .flatten()
                .unwrap_or_else(|| {
                    panic!(
                        "{at} translates to `{entry}`, which is not an address this configuration \
                         reserved; name a google_compute_address declared beside the gateway"
                    )
                });
            let address = all
                .iter()
                .find(|r| {
                    r.kind == "google_compute_address" && r.name == name && r.file == nat.file
                })
                .unwrap_or_else(|| {
                    panic!(
                        "{at} translates to google_compute_address.{name}, which its own module \
                         does not declare"
                    )
                });
            assert_eq!(
                address.setting("address_type").as_deref(),
                Some("\"EXTERNAL\""),
                "{} is not an external address, so it cannot be an egress identity",
                address.at()
            );
            assert_eq!(
                address.setting("region"),
                Some(region.clone()),
                "{} is reserved in a different region from the gateway that translates to it; a \
                 region's egress identity belongs to that region",
                address.at()
            );
        }

        assert_eq!(
            nat.setting("source_subnetwork_ip_ranges_to_nat").as_deref(),
            Some("\"LIST_OF_SUBNETWORKS\""),
            "{at} translates subnets it does not name. A subnet created later would acquire \
             internet egress by existing"
        );

        let log = nat.block("log_config").unwrap_or_default();
        assert!(
            assignment(&log, "enable").as_deref() == Some("true")
                && assignment(&log, "filter").as_deref() == Some("\"ALL\""),
            "{at} does not log every translation (log_config {{ enable = true, filter = \"ALL\" }}); \
             an egress path with no record is not a controlled one"
        );
    }

    // And no route of this repository's own. A custom route is how a subnet's
    // default path is pointed at something that is not the gateway above.
    let routes: Vec<String> = all
        .iter()
        .filter(|r| r.kind == "google_compute_route")
        .map(Resource::at)
        .collect();
    assert!(
        routes.is_empty(),
        "{routes:?} declares a custom route. The only way out of a subnet here is the VPC's \
         default route through a Cloud NAT gateway; show where this one leads before adding it"
    );
}

#[test]
fn no_machine_the_repository_defines_sits_behind_a_load_balancer_or_inside_a_mesh() {
    // GCP-029: nothing stands between tick and order, or between a producer
    // and the Fabric, except the connection itself. `infrastructure.rs` reads
    // the execution-node module for a hop of its own. It cannot see the two
    // ways the prohibition is actually broken: a backend service in some
    // *other* file taking a machine group as its backend — the node's group
    // already publishes a named port and the root already outputs its
    // `instance_group`, which is everything a balancer needs — and a second
    // machine module, the Fabric broker's, that nothing reads at all.
    let all = resources();
    let backends: Vec<&Resource> = all
        .iter()
        .filter(|r| {
            r.kind == "google_compute_backend_service"
                || r.kind == "google_compute_region_backend_service"
        })
        .collect();
    assert!(
        backends.len() >= 2,
        "only {} backend services were read; the public edge and the console's edge each declare \
         one, so this check has no load balancer to hold the prohibition against",
        backends.len()
    );
    let machine_files: BTreeSet<&str> = all
        .iter()
        .filter(|r| MACHINE_KINDS.contains(&r.kind.as_str()))
        .map(|r| r.file.as_str())
        .collect();
    assert!(
        !machine_files.is_empty(),
        "no machine definition was read; the walk is not reaching the execution-node module"
    );

    // A balancer's backend is a serverless endpoint group and nothing else.
    for backend in &backends {
        let groups: Vec<String> = backend
            .body
            .lines()
            .filter_map(|line| assignment(line, "group"))
            .collect();
        assert!(
            !groups.is_empty(),
            "{} names no backend group on a `group = ` line; this check can no longer read what \
             stands behind it",
            backend.at()
        );
        for group in groups {
            assert!(
                group.starts_with("google_compute_region_network_endpoint_group."),
                "{} takes `{group}` as a backend. Only a serverless endpoint group may stand \
                 behind a load balancer here: a machine group there puts a proxy on the Reflex or \
                 Fabric path, whose latency and failure nobody measured",
                backend.at()
            );
        }
    }
    for group in all
        .iter()
        .filter(|r| r.kind.contains("network_endpoint_group"))
    {
        assert_eq!(
            group.setting("network_endpoint_type").as_deref(),
            Some("\"SERVERLESS\""),
            "{} is not a serverless endpoint group, so it can name a machine's address as a \
             balancer's backend",
            group.at()
        );
    }
    // A target pool takes instances by name: the one balancer that needs no
    // group and so would pass the check above.
    let pools: Vec<String> = all
        .iter()
        .filter(|r| r.kind.contains("target_pool"))
        .map(Resource::at)
        .collect();
    assert!(
        pools.is_empty(),
        "{pools:?} declares a target pool, a load balancer that takes machines by name"
    );

    // And no file that defines a machine declares a hop of its own — every
    // such file, so the Fabric broker's module is held the day it is written.
    for resource in all
        .iter()
        .filter(|r| machine_files.contains(r.file.as_str()))
    {
        assert!(
            !is_a_proxy_hop(&resource.kind),
            "{} is a load-balancer or mesh hop declared beside a machine. A Reflex or Fabric \
             machine is reached directly or not at all",
            resource.at()
        );
    }
}

#[test]
fn every_machine_group_the_repository_defines_consumes_a_specific_reservation_sized_to_its_target()
{
    // GCP-015: hot capacity is held, not hoped for. The execution node's plan
    // harness proves its own reservation is sized and consumed. It says
    // nothing about the next group written: a Fabric broker group with no
    // reservation plans cleanly, and finds out about the zonal stockout on
    // the day a broker has to be replaced. So every group manager in the tree
    // is held to the same four facts — its template names a reservation, by
    // SPECIFIC_RESERVATION, that its own module declares for that group
    // alone, sized to the group's target and to the template's machine.
    let all = resources();
    let groups: Vec<&Resource> = all
        .iter()
        .filter(|r| r.kind.ends_with("instance_group_manager"))
        .collect();
    assert!(
        !groups.is_empty(),
        "no instance group manager was read; the walk is not reaching the execution-node module"
    );
    for group in groups {
        let at = group.at();
        let beside = |kind: &str, name: &str| {
            all.iter()
                .find(|r| r.kind.ends_with(kind) && r.name == name && r.file == group.file)
        };
        let target = group
            .setting("target_size")
            .unwrap_or_else(|| panic!("{at} states no target_size to size a reservation to"));
        let template = group
            .setting("instance_template")
            .as_deref()
            .and_then(|e| reference(e, "instance_template"))
            .and_then(|name| beside("instance_template", name))
            .unwrap_or_else(|| {
                panic!("{at} boots from a template its own module does not declare")
            });

        let affinity = template.block("reservation_affinity").unwrap_or_else(|| {
            panic!(
                "{} carries no reservation_affinity, so {at} takes on-demand capacity and a \
                 stockout can refuse the machine it needs",
                template.at()
            )
        });
        assert_eq!(
            assignment(&affinity, "type").as_deref(),
            Some("\"SPECIFIC_RESERVATION\""),
            "{} does not consume a SPECIFIC_RESERVATION",
            template.at()
        );
        let reservation = reference(&affinity, "google_compute_reservation")
            .and_then(|name| beside("google_compute_reservation", name))
            .unwrap_or_else(|| {
                panic!(
                    "{} names no google_compute_reservation its own module declares",
                    template.at()
                )
            });
        assert_eq!(
            reservation
                .setting("specific_reservation_required")
                .as_deref(),
            Some("true"),
            "{} admits any matching instance in the project, so another workload can spend the \
             capacity {at} is meant to own",
            reservation.at()
        );
        let held = reservation
            .block("specific_reservation")
            .unwrap_or_default();
        assert_eq!(
            assignment(&held, "count"),
            Some(target.clone()),
            "{} is not sized to {at}'s target_size `{target}`",
            reservation.at()
        );
        assert_eq!(
            assignment(&held, "machine_type"),
            template.setting("machine_type"),
            "{} holds a different machine from the one {} boots",
            reservation.at(),
            template.at()
        );
    }
}

#[test]
fn every_machine_the_repository_defines_boots_with_secure_boot_a_vtpm_and_integrity_monitoring() {
    // GCP-059: the hot path and the journal run on machines, not on an
    // admission-controlled cluster, so boot integrity is what stops a
    // tampered image running there. `infrastructure.rs` asserts the three
    // settings on the execution node by path. A Fabric broker template added
    // beside it would be a second critical machine that test never opens,
    // and Google's default leaves secure boot off.
    let all = resources();
    let machines: Vec<&Resource> = all
        .iter()
        .filter(|r| {
            [
                "google_compute_instance",
                "google_compute_instance_template",
                "google_compute_region_instance_template",
            ]
            .contains(&r.kind.as_str())
        })
        .collect();
    assert!(
        !machines.is_empty(),
        "no machine definition was read; the walk is not reaching the execution-node module"
    );
    for machine in machines {
        let at = machine.at();
        let shield = machine
            .block("shielded_instance_config")
            .unwrap_or_else(|| {
                panic!("{at} has no shielded_instance_config block, so it boots unverified")
            });
        for setting in [
            "enable_secure_boot",
            "enable_vtpm",
            "enable_integrity_monitoring",
        ] {
            assert_eq!(
                assignment(&shield, setting).as_deref(),
                Some("true"),
                "{at} does not set `{setting} = true` inside its shielded_instance_config"
            );
        }
    }
}

#[test]
fn no_shared_vpc_is_declared_so_no_host_project_lends_a_subnet_across_a_folder() {
    // GCP-026 permits Shared VPC inside one domain or folder and never across
    // them. This tree declares no folder, so a host project attached here
    // could not be shown to share one with its service projects, and the
    // first attachment written would span whatever the two projects happen to
    // be. Until the folder model exists the only form of the bound that can
    // be held is the refusal: no host, no service-project attachment, and no
    // subnet lent to another principal by IAM, which is the same thing done
    // without the attachment.
    let all = resources();
    assert!(
        all.iter().any(|r| r.kind == "google_compute_network")
            && all.iter().any(|r| r.kind == "google_compute_subnetwork"),
        "no network or subnet was read; the walk is not reaching the network modules"
    );
    for resource in &all {
        assert!(
            !resource.kind.contains("shared_vpc")
                && !resource.kind.starts_with("google_compute_subnetwork_iam"),
            "{} shares a VPC or a subnet with another project. Shared VPC is permitted only \
             inside one folder (GCP-026) and nothing here declares a folder to confine it to: \
             land the folder model first (GCP-016), then replace this refusal with a check that \
             the host and every service project sit in the same one, and that no shared subnet \
             carries both Reflex and Fabric machines",
            resource.at()
        );
    }
}

#[test]
fn every_backend_bucket_is_served_through_cloud_cdn_with_an_explicit_cache_mode_and_no_backend_service_is()
 {
    // GCP-055: public static assets go through Cloud CDN. The plan harness
    // asserts `enable_cdn`, and nothing asserted the cache mode — the half
    // that decides *what* is cached. Left out, the provider picks one; set to
    // FORCE_CACHE_ALL, the cache serves a response whatever its headers say,
    // which is how a private response is handed to the next client. The
    // other direction is held too: a backend service carries a session, and
    // one behind a cache can serve one session's response to another.
    let all = resources();
    let buckets: Vec<&Resource> = all
        .iter()
        .filter(|r| r.kind == "google_compute_backend_bucket")
        .collect();
    assert!(
        !buckets.is_empty(),
        "no backend bucket was read; the public edge declares the static shell's, so the walk is \
         not reaching the module"
    );
    for bucket in buckets {
        let at = bucket.at();
        assert_eq!(
            bucket.setting("enable_cdn").as_deref(),
            Some("true"),
            "{at} serves static assets without Cloud CDN"
        );
        let policy = bucket.block("cdn_policy").unwrap_or_else(|| {
            panic!("{at} has no cdn_policy block, so its cache mode is implied")
        });
        let mode = assignment(&policy, "cache_mode").unwrap_or_default();
        assert!(
            mode == "\"CACHE_ALL_STATIC\"" || mode == "\"USE_ORIGIN_HEADERS\"",
            "{at} has cache_mode `{mode}`. Say CACHE_ALL_STATIC or USE_ORIGIN_HEADERS: an absent \
             mode is the provider's choice, and FORCE_CACHE_ALL caches a response whatever its \
             headers say"
        );
    }

    let services: Vec<&Resource> = all
        .iter()
        .filter(|r| {
            r.kind == "google_compute_backend_service"
                || r.kind == "google_compute_region_backend_service"
        })
        .collect();
    assert!(
        services.len() >= 2,
        "only {} backend services were read; the walk is not reaching both edges",
        services.len()
    );
    for service in services {
        assert_eq!(
            service.setting("enable_cdn").as_deref(),
            Some("false"),
            "{} does not say `enable_cdn = false`. A backend service here carries a session, and \
             a cached one can serve one session's response to another",
            service.at()
        );
    }
}
