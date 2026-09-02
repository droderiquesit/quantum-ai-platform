//! Does the console's route to the platform exist in one piece?
//!
//! ADR 0018 gives the portal a path to the platform. On the GKE runtime that
//! path was an address written in three files owned by three tools — a
//! reserved internal-load-balancer address in Terraform, a Service claiming
//! it in Helm, a script configuring the console to dial it — and this file
//! existed to hold the three copies together.
//!
//! Under ADR 0024 there is no load balancer between the console and the API.
//! `qip-api` is a Cloud Run service with internal ingress, reached at its own
//! URL by the one identity the catalogue names as an invoker, and that URL is
//! a Terraform output rather than a fact anybody copies. What is left to hold
//! together is smaller and just as quiet when it breaks: the console's
//! subnet, the invoker grant, the internal posture, and the script deriving
//! the address rather than restating it. Change the invoker and forget the
//! script, and every tool reports success while the portal answers 403 on
//! every gateway call — which is what it looked like before it had a route
//! at all, so the obvious diagnosis is the wrong one.
//!
//! `manifest_wiring.rs` exists for the same class of drift on the workloads'
//! configuration, and says at length why a check like this is a test rather
//! than a review note.

// The workspace denies `panic_in_result_fn`. These tests return `()` and
// assert; the lint does not apply, and neither does the reason for it.

use qip_acceptance::read;

const TFVARS: &str = "infrastructure/environments/dev/terraform.tfvars";
const DEPLOY_SCRIPT: &str = "scripts/deploy-frontends.sh";
const CATALOGUE: &str = "infrastructure/terraform/catalogue.tf";
const NETWORK_MODULE: &str = "infrastructure/terraform/modules/network/main.tf";
const CLOUD_RUN_MODULE: &str = "infrastructure/terraform/modules/cloudrun/main.tf";
const CLOUD_RUN_VARIABLES: &str = "infrastructure/terraform/modules/cloudrun/variables.tf";
const OUTPUTS: &str = "infrastructure/terraform/outputs.tf";

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// A configuration with its comments removed.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The value of a top-level `name = "value"` assignment in a tfvars file.
///
/// Anchored at column zero and matched against the whole identifier, because
/// `console_egress_cidr` would be a suffix of `legacy_console_egress_cidr`
/// tomorrow. The architecture rule that substring matching is a trap is not
/// hypothetical here — `contains` on an identifier has already passed a
/// mutation in this repository that deleted the value it was written to
/// protect.
fn tfvar(source: &str, name: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let (identifier, rest) = line.split_once('=')?;
        if identifier.trim_end() != identifier.trim() || identifier.trim() != name {
            return None;
        }
        if line.starts_with(char::is_whitespace) {
            return None;
        }
        Some(rest.trim().trim_matches('"').to_string())
    })
}

/// The `value` expression of a named output.
fn output_value(name: &str) -> String {
    let text = without_comments(&read(OUTPUTS));
    let start = text
        .find(&format!("output \"{name}\" {{"))
        .unwrap_or_else(|| panic!("outputs.tf declares no output named `{name}`"));
    text[start..]
        .lines()
        .skip(1)
        .take_while(|line| !line.starts_with('}'))
        .find_map(|line| {
            let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
            collapsed.strip_prefix("value = ").map(str::to_string)
        })
        .unwrap_or_else(|| panic!("the `{name}` output has no value expression"))
}

/// One catalogue entry's body, comments stripped.
fn catalogue_entry(name: &str) -> String {
    let text = without_comments(&read(CATALOGUE));
    let opening = format!("    {name} = {{");
    let start = text
        .find(&opening)
        .unwrap_or_else(|| panic!("catalogue.tf has no `{name}` entry"));
    text[start + opening.len()..]
        .split("\n    }\n")
        .next()
        .expect("the entry closes")
        .to_string()
}

fn parse_cidr(text: &str) -> Option<(u32, u32)> {
    let (address, prefix) = text.split_once('/')?;
    let address: std::net::Ipv4Addr = address.parse().ok()?;
    let prefix: u32 = prefix.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    Some((u32::from(address), prefix))
}

// ---------------------------------------------------------------------------
// The properties
// ---------------------------------------------------------------------------

#[test]
fn the_subnet_terraform_creates_for_the_console_is_the_one_the_script_attaches_to() {
    let tfvars = read(TFVARS);
    let created = tfvar(&tfvars, "console_egress_cidr")
        .expect("dev tfvars must set console_egress_cidr; ADR 0018 is what created it");
    let (_, prefix) = parse_cidr(&created)
        .unwrap_or_else(|| panic!("console_egress_cidr in {TFVARS} is not a CIDR: {created:?}"));
    // A /26 is the smallest direct VPC egress accepts; anything smaller is
    // an apply-time failure written into a committed file.
    assert!(
        prefix <= 26,
        "the console subnet {created} is smaller than the /26 Cloud Run requires"
    );

    // The module names the subnet with the environment prefix, and the
    // script attaches the console to that name. Two copies, reconciled here.
    let network = without_comments(&read(NETWORK_MODULE));
    assert!(
        network.contains("name    = \"qip-${var.environment}-console-egress\""),
        "the network module no longer names the console's subnet qip-<env>-console-egress"
    );
    let script = read(DEPLOY_SCRIPT);
    assert!(
        script.contains("qip-dev-console-egress"),
        "{DEPLOY_SCRIPT} attaches the console to a subnet other than the one Terraform creates"
    );
    assert!(
        script.contains("console_egress_cidr") && script.contains("terraform.tfvars"),
        "{DEPLOY_SCRIPT} no longer reads the console subnet from {TFVARS}"
    );
}

#[test]
fn the_console_reaches_the_api_as_a_named_invoker_and_nothing_else_may() {
    // The whole access decision on this route. A Cloud Run service with
    // internal ingress and no invoker is reachable by nobody; one with
    // `allUsers` is reachable by the internet through any VPC path. The API
    // names exactly the console's identity.
    let api = catalogue_entry("api");
    assert!(
        api.contains("module.secrets.console_service_account_email"),
        "the API's invokers do not name the console's identity, so the portal gets 403 on every gateway call"
    );
    let invokers = api
        .lines()
        .skip_while(|line| !line.trim_start().starts_with("invokers"))
        .take_while(|line| !line.trim().starts_with("env"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        invokers.contains("serviceAccount:"),
        "the API's invoker is not a service account: {invokers}"
    );

    // The brains name no invoker at all: nothing calls them over HTTP.
    for name in ["fastbrain", "deepbrain"] {
        let entry = catalogue_entry(name);
        assert!(
            entry.contains("invokers     = []") || entry.contains("invokers      = []"),
            "{name} names an invoker; nothing calls the brains over HTTP and an invoker is a route in"
        );
    }

    // And the module refuses the two principals that would make the URL the
    // route in.
    let variables = read(CLOUD_RUN_VARIABLES);
    assert!(
        variables.contains("!contains(var.invokers, \"allUsers\") && !contains(var.invokers, \"allAuthenticatedUsers\")"),
        "the Cloud Run module no longer refuses an anonymous invoker"
    );
}

#[test]
fn the_api_is_reachable_only_from_inside_the_vpc_and_its_address_is_a_terraform_output() {
    // The security argument of ADR 0018, restated for Cloud Run: without
    // internal ingress the API's own URL answers the internet, and `POST
    // /api/v1/kill-switch` acquires a public address.
    let catalogue = without_comments(&read(CATALOGUE));
    assert!(
        catalogue.contains("ingress_posture = \"internal\""),
        "the catalogue no longer places the API behind the internal posture"
    );
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        module.contains("\"INGRESS_TRAFFIC_INTERNAL_ONLY\"")
            && !module.contains("INGRESS_TRAFFIC_ALL"),
        "the Cloud Run module can publish a service to the internet"
    );

    // The address the console dials is the API's URL as Terraform reports
    // it, never a literal. A copy in a script is the copy nobody thinks to
    // change, because it is the only one that is not configuration.
    assert_eq!(
        output_value("api_internal_base_url"),
        "module.cloud_run[\"api\"].uri",
        "the api_internal_base_url output no longer derives from the API's own Cloud Run URL"
    );
}

#[test]
fn the_deploy_script_derives_the_api_address_rather_than_restating_it() {
    let script = read(DEPLOY_SCRIPT);
    let tfvars = without_comments(&read(TFVARS));

    // The premise: the script does configure the console's upstream at all.
    assert!(
        script.contains("QIP_API_BASE_URL"),
        "{DEPLOY_SCRIPT} sets no QIP_API_BASE_URL, so the console it deploys has no \
         platform to read and answers 500 on every gateway call."
    );

    // No literal. Neither an address nor a run.app hostname belongs in the
    // script; both are facts Terraform owns.
    for line in script.lines() {
        if line.contains("QIP_API_BASE_URL=") {
            assert!(
                !line.contains("run.app") && !line.contains("://10."),
                "{DEPLOY_SCRIPT} restates the API's address in `{}`",
                line.trim()
            );
        }
    }

    // And whatever the script derives the address from actually exists.
    // Under the GKE runtime it read `api_internal_address` from the tfvars;
    // that key is gone with the load balancer it reserved an address for, and
    // a script reading a key that no longer exists deploys a console with an
    // empty upstream and exits with the message this test's premise names.
    // The address is now the `api_internal_base_url` Terraform output.
    let reads_a_tfvars_key = script
        .lines()
        .filter_map(|line| line.split("$(tfvar ").nth(1))
        .filter_map(|rest| rest.split(')').next())
        .map(str::trim)
        .filter(|key| key.contains("api"))
        .collect::<Vec<_>>();
    for key in &reads_a_tfvars_key {
        assert!(
            tfvar(&tfvars, key).is_some(),
            "{DEPLOY_SCRIPT} derives the API's address from the tfvars key `{key}`, which {TFVARS} \
             no longer sets. Read `terraform output -raw api_internal_base_url` instead: the \
             API is a Cloud Run service now and its URL is Terraform's to report."
        );
    }
    assert!(
        script.contains("api_internal_base_url") || !reads_a_tfvars_key.is_empty(),
        "{DEPLOY_SCRIPT} neither reads the api_internal_base_url output nor a tfvars key, so \
         where the console's upstream comes from is a mystery this test cannot check"
    );
}
