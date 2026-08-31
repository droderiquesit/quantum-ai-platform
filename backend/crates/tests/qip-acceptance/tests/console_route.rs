//! Does the console's route to the platform exist in one piece?
//!
//! ADR 0018 gives the portal a path into the VPC. That path is described in
//! three files owned by three different tools, and none of them can read the
//! others:
//!
//! * `infrastructure/environments/dev/terraform.tfvars` — the address
//!   Terraform reserves and the subnet it creates.
//! * `infrastructure/helm/qip/values-dev.yaml` — the address the Service
//!   claims and the CIDR the NetworkPolicy admits.
//! * `scripts/deploy-frontends.sh` — the address the console is configured to
//!   call.
//!
//! Two of those hold literal copies of one fact. A third copy in the script
//! would be one more, so the script is required to derive its value instead,
//! and this file asserts that it does.
//!
//! The failure this prevents is specific and quiet. Change the reserved
//! address in Terraform, forget the Helm value, and every tool reports
//! success: Terraform reserves an address nothing claims, GKE asks for an
//! address that is not reserved and gets `SYNC_ERROR` on a Service nobody is
//! watching, and the console dials the old address until its ten-second
//! timeout. Nothing in any diff says the two disagree. The whole symptom is
//! that the portal is slow and then empty — which is what it looked like
//! before it had a route at all, so the obvious diagnosis is the wrong one.
//!
//! `manifest_wiring.rs` exists for the same class of drift on the mesh ports,
//! and says at length why a check like this is a test rather than a review
//! note.

// The workspace denies `panic_in_result_fn`. These tests return `()` and
// assert; the lint does not apply, and neither does the reason for it.

use qip_acceptance::read;
use std::net::Ipv4Addr;

const TFVARS: &str = "infrastructure/environments/dev/terraform.tfvars";
const HELM_VALUES: &str = "infrastructure/helm/qip/values-dev.yaml";
const DEPLOY_SCRIPT: &str = "scripts/deploy-frontends.sh";
const CHART_API: &str = "infrastructure/helm/qip/templates/api.yaml";

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// The value of a top-level `name = "value"` assignment in a tfvars file.
///
/// Anchored at column zero and matched against the whole identifier, because
/// `api_internal_address` is a suffix of nothing today and would be a suffix
/// of `legacy_api_internal_address` tomorrow. The architecture rule that
/// substring matching is a trap is not hypothetical here — `contains` on an
/// identifier has already passed a mutation in this repository that deleted
/// the value it was written to protect.
fn tfvar(source: &str, name: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let (identifier, rest) = line.split_once('=')?;
        if identifier.trim_end() != identifier.trim() || identifier.trim() != name {
            return None;
        }
        // Reject an indented line: a nested key inside a map block is not a
        // top-level assignment and must not answer for one.
        if line.starts_with(char::is_whitespace) {
            return None;
        }
        Some(rest.trim().trim_matches('"').to_string())
    })
}

/// The value of `key: "value"` nested one level under `parent:` in a values
/// file. Two levels is all this chart uses, and a real YAML parser would be a
/// dependency this workspace does not permit.
fn helm_value(source: &str, parent: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in source.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        if !trimmed.starts_with(char::is_whitespace) {
            // A new top-level key ends the block we were in. Commented-out
            // documentation of the same key in values.yaml is skipped above,
            // so the block found here is the one that renders.
            inside = trimmed.ends_with(':') && trimmed.trim_end_matches(':') == parent;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((found, value)) = trimmed.trim().split_once(':') else {
            continue;
        };
        if found.trim() == key {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// The `default` of a named `variable` block in a Terraform variables file.
///
/// The primary subnet range is not restated in any environment's tfvars, so
/// the root default is where it is actually decided. Reading it beats a
/// literal here, which would be a copy of exactly the kind this file exists
/// to refuse.
fn variable_default(source: &str, name: &str) -> Option<String> {
    let header = format!("variable \"{name}\" {{");
    let block = source.split(&header).nth(1)?.split("\n}").next()?;
    block.lines().find_map(|line| {
        let (identifier, rest) = line.split_once('=')?;
        (identifier.trim() == "default").then(|| rest.trim().trim_matches('"').to_string())
    })
}

/// An IPv4 CIDR as (network address, prefix length).
fn parse_cidr(text: &str) -> Option<(u32, u32)> {
    let (address, prefix) = text.split_once('/')?;
    let address: Ipv4Addr = address.parse().ok()?;
    let prefix: u32 = prefix.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    Some((u32::from(address), prefix))
}

fn contains_address(cidr: &str, address: &str) -> Option<bool> {
    let (network, prefix) = parse_cidr(cidr)?;
    let address: Ipv4Addr = address.parse().ok()?;
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Some((u32::from(address) & mask) == (network & mask))
}

fn ranges_overlap(left: &str, right: &str) -> Option<bool> {
    let (left_network, left_prefix) = parse_cidr(left)?;
    let (right_network, right_prefix) = parse_cidr(right)?;
    let prefix = left_prefix.min(right_prefix);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Some((left_network & mask) == (right_network & mask))
}

// ---------------------------------------------------------------------------
// The properties
// ---------------------------------------------------------------------------

#[test]
fn the_address_terraform_reserves_is_the_address_the_service_claims() {
    let tfvars = read(TFVARS);
    let values = read(HELM_VALUES);

    let reserved = tfvar(&tfvars, "api_internal_address")
        .expect("dev tfvars must set api_internal_address; ADR 0018 is what created it");
    let claimed = helm_value(&values, "consoleRoute", "apiInternalAddress")
        .expect("values-dev.yaml must set consoleRoute.apiInternalAddress");

    // Assert the premise before the property: two parsers that both returned
    // an empty string would agree, and would prove nothing.
    assert!(
        reserved.parse::<Ipv4Addr>().is_ok(),
        "api_internal_address in {TFVARS} is not an IPv4 address: {reserved:?}"
    );
    assert!(
        claimed.parse::<Ipv4Addr>().is_ok(),
        "consoleRoute.apiInternalAddress in {HELM_VALUES} is not an IPv4 address: {claimed:?}"
    );

    assert_eq!(
        reserved, claimed,
        "Terraform reserves {reserved} for qip-api's internal load balancer and the Helm \
         Service claims {claimed}. GKE will ask for an address nobody reserved, the Service \
         will sit in SYNC_ERROR, and the console will time out against an address that \
         answers nothing. Change both or neither."
    );
}

#[test]
fn the_subnet_terraform_creates_is_the_subnet_the_policy_admits() {
    let tfvars = read(TFVARS);
    let values = read(HELM_VALUES);

    let created = tfvar(&tfvars, "console_egress_cidr")
        .expect("dev tfvars must set console_egress_cidr; ADR 0018 is what created it");
    let admitted = helm_value(&values, "consoleRoute", "egressCidr")
        .expect("values-dev.yaml must set consoleRoute.egressCidr");

    assert!(
        parse_cidr(&created).is_some(),
        "console_egress_cidr in {TFVARS} is not a CIDR: {created:?}"
    );
    assert!(
        parse_cidr(&admitted).is_some(),
        "consoleRoute.egressCidr in {HELM_VALUES} is not a CIDR: {admitted:?}"
    );

    assert_eq!(
        created, admitted,
        "Cloud Run egresses from {created} and the NetworkPolicy admits {admitted}. Under \
         the namespace's default-deny the console's requests are dropped at the pod, which \
         presents as a timeout rather than as a refusal naming the policy."
    );
}

#[test]
fn the_reserved_address_sits_inside_the_primary_subnet_and_outside_the_console_subnet() {
    let tfvars = read(TFVARS);

    let address = tfvar(&tfvars, "api_internal_address").expect("api_internal_address");
    let console = tfvar(&tfvars, "console_egress_cidr").expect("console_egress_cidr");
    // subnet_cidr is not restated in the dev tfvars: it takes the root
    // variable's default, which is where the primary range is decided.
    let primary = tfvar(&tfvars, "subnet_cidr")
        .or_else(|| {
            variable_default(
                &read("infrastructure/terraform/variables.tf"),
                "subnet_cidr",
            )
        })
        .expect("the primary subnet range must be readable from tfvars or the root variable");

    assert!(
        parse_cidr(&primary).is_some(),
        "the primary subnet range parsed as {primary:?}, which is not a CIDR — this test \
         would otherwise prove nothing about containment"
    );

    assert_eq!(
        contains_address(&primary, &address),
        Some(true),
        "the reserved address {address} is outside the primary subnet {primary}. \
         google_compute_address refuses an address that is not in the subnetwork it names, \
         so this is an apply-time failure written into a committed file."
    );
    assert_eq!(
        contains_address(&console, &address),
        Some(false),
        "the reserved address {address} sits inside the console's own egress subnet \
         {console}. Cloud Run would be dialling an address in the range its own interface \
         is allocated from."
    );
    assert_eq!(
        ranges_overlap(&primary, &console),
        Some(false),
        "the console egress subnet {console} overlaps the primary subnet {primary}. Two \
         subnets cannot share a range in one VPC, and the apply fails on whichever is \
         created second."
    );
}

#[test]
fn the_deploy_script_derives_the_address_rather_than_restating_it() {
    let script = read(DEPLOY_SCRIPT);
    let tfvars = read(TFVARS);
    let address = tfvar(&tfvars, "api_internal_address").expect("api_internal_address");

    // The premise: the script does configure the console's upstream at all.
    assert!(
        script.contains("QIP_API_BASE_URL"),
        "{DEPLOY_SCRIPT} sets no QIP_API_BASE_URL, so the console it deploys has no \
         platform to read and answers 500 on every gateway call."
    );

    assert!(
        !script.contains(&address),
        "{DEPLOY_SCRIPT} contains the literal address {address}. Two copies of this fact \
         already exist and are reconciled by the tests above; a third in a shell script is \
         the one nobody thinks to change, because it is the only copy that is not \
         configuration. Read it from {TFVARS} instead."
    );
    assert!(
        script.contains(TFVARS) || script.contains("terraform.tfvars"),
        "{DEPLOY_SCRIPT} neither restates the address nor reads it from the tfvars, so \
         where it comes from is now a mystery this test cannot check."
    );
}

#[test]
fn the_console_reaches_the_api_over_a_load_balancer_that_is_internal() {
    let chart = read(CHART_API);

    // The premise: the Service this test is about is in the chart at all.
    assert!(
        chart.contains("name: qip-api-console"),
        "the chart declares no qip-api-console Service, so ADR 0018's route does not exist"
    );
    assert!(
        chart.contains("type: LoadBalancer"),
        "qip-api-console is not a LoadBalancer, so it has no address off the pod network"
    );

    // The property. An internal passthrough load balancer is the entire
    // security argument of ADR 0018: without this annotation GKE creates an
    // *external* one, and `POST /api/v1/kill-switch` acquires a public
    // address. The annotation is one line, and deleting it fails nothing else.
    assert!(
        chart.contains(r#"networking.gke.io/load-balancer-type: "Internal""#),
        "qip-api-console has no Internal load-balancer-type annotation. GKE's default for \
         a LoadBalancer Service is an external one, so removing this line does not disable \
         the route — it publishes the platform's operator interface, kill switch included, \
         on the internet."
    );
}

#[test]
fn the_policy_admitting_the_console_admits_only_the_console() {
    let chart = read(CHART_API);

    assert!(
        chart.contains("name: allow-api-ingress-from-console"),
        "the chart declares no ingress policy for the console route, so under the \
         namespace's default-deny the load balancer above admits nobody"
    );

    // A CIDR that is not the console's is the way this control stops being
    // one. `0.0.0.0/0` is the obvious mistake; `10.0.0.0/8` is the plausible
    // one, and it admits every pod, node and peered network in the VPC.
    for wide in ["0.0.0.0/0", "10.0.0.0/8", "0.0.0.0/1"] {
        assert!(
            !chart.contains(wide),
            "{CHART_API} admits {wide} to the API. The console's route is one /26; a \
             range wider than it is a hole in default-deny shaped like the network \
             rather than like the caller."
        );
    }
}
