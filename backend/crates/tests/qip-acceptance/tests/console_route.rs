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
//! Under ADR 0036 the Cloud Run service itself is no longer Terraform's. The
//! catalogue entry still names the invokers and `modules/cloudrun` still
//! creates the identity, but the resource that carries the ingress posture
//! and the resource that carries the invoker grant are Config Connector
//! manifests under `infrastructure/gitops/envs/<env>/` — `api.yaml` and
//! `invokers.yaml` — reconciled by Argo CD. Two of the tests below used to
//! read the posture and the refusals out of the module's own text; the
//! module has no service to put them on now, so they read the manifests.
//! The property did not weaken; it moved, and a test still pointed at the
//! old location would have failed on the move rather than on a breach —
//! or, re-aimed carelessly at the module's remaining text, passed forever.
//!
//! `manifest_wiring.rs` exists for the same class of drift on the workloads'
//! configuration, and says at length why a check like this is a test rather
//! than a review note. `gitops.rs` holds every property the module held for
//! every service in one parity test; the two tests here hold the console's
//! route specifically, and name what `gitops.rs` already pins rather than
//! asserting it twice.

// The workspace denies `panic_in_result_fn`. These tests return `()` and
// assert; the lint does not apply, and neither does the reason for it.

use qip_acceptance::{files_with_extension, read, repository_root};

const TFVARS: &str = "infrastructure/environments/dev/terraform.tfvars";
const DEPLOY_SCRIPT: &str = "scripts/deploy-frontends.sh";
const CATALOGUE: &str = "infrastructure/terraform/catalogue.tf";
const NETWORK_MODULE: &str = "infrastructure/terraform/modules/network/main.tf";
const OUTPUTS: &str = "infrastructure/terraform/outputs.tf";
const GITOPS: &str = "infrastructure/gitops";
const ENVS: &str = "infrastructure/gitops/envs";

/// The one ingress a service on the console's route may carry.
const INTERNAL_ONLY: &str = "INGRESS_TRAFFIC_INTERNAL_ONLY";
/// The ingress ADR 0030 grants exactly one service, OpenObserve.
const ALL_TRAFFIC: &str = "INGRESS_TRAFFIC_ALL";

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

/// One YAML document under `infrastructure/gitops`, read as its non-comment
/// lines.
///
/// `gitops.rs` carries a reader for the manifest subset into `serde_json`;
/// it is private to that file, and this one needs five scalars per
/// document — `kind`, `metadata.name`, and three fixed-indent `spec` fields
/// — so they are read by line rather than by duplicating it. The shape read
/// is the one every manifest under `envs/` has: `kind:` and `metadata:` at
/// column zero, `  name:` two in under `metadata:`. A document outside that
/// shape reads as one with no kind and no name, and the premise assertions
/// (an API `RunService` in every environment that has an `api.yaml`) catch
/// the reader going blind rather than letting it pass over nothing.
struct Document {
    path: String,
    kind: String,
    name: String,
    lines: Vec<String>,
}

impl Document {
    /// `kind` and name together, for messages.
    fn describe(&self) -> String {
        format!("{} `{}` in {}", self.kind, self.name, self.path)
    }

    /// The value of `key:` at exactly `indent` spaces, quotes stripped.
    fn field(&self, indent: usize, key: &str) -> Option<String> {
        let prefix = format!("{}{key}: ", " ".repeat(indent));
        self.lines.iter().find_map(|line| {
            line.strip_prefix(&prefix).map(|value| {
                value
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string()
            })
        })
    }

    /// The `name` under an `IAMPolicyMember`'s `spec.resourceRef`: the
    /// service the grant is on.
    fn resource_ref_name(&self) -> Option<String> {
        self.lines
            .iter()
            .skip_while(|line| line.as_str() != "  resourceRef:")
            .find_map(|line| line.strip_prefix("    name: "))
            .map(|value| value.trim().to_string())
    }
}

/// The `  name:` directly under a column-zero `metadata:`.
fn metadata_name(lines: &[String]) -> String {
    let mut in_metadata = false;
    for line in lines {
        if line == "metadata:" {
            in_metadata = true;
            continue;
        }
        if in_metadata {
            if let Some(name) = line.strip_prefix("  name: ") {
                return name.trim().to_string();
            }
            if !line.is_empty() && !line.starts_with(' ') {
                in_metadata = false;
            }
        }
    }
    String::new()
}

/// Every YAML document under a directory, split on `---`, comment lines
/// dropped.
fn documents_under(relative: &str) -> Vec<Document> {
    let root = repository_root();
    let mut found = Vec::new();
    for extension in ["yaml", "yml"] {
        for path in files_with_extension(relative, extension) {
            let display = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {display}: {error}"));
            for chunk in source.split("\n---") {
                let lines: Vec<String> = chunk
                    .lines()
                    .filter(|line| !line.trim_start().starts_with('#'))
                    .map(str::to_string)
                    .collect();
                let kind = lines
                    .iter()
                    .find_map(|line| line.strip_prefix("kind: "))
                    .map(|value| value.trim().to_string())
                    .unwrap_or_default();
                let name = metadata_name(&lines);
                found.push(Document {
                    path: display.clone(),
                    kind,
                    name,
                    lines,
                });
            }
        }
    }
    found
}

/// Every non-comment line under a directory that names `principal` as a
/// whole token, as `path:line: text`.
///
/// A token, not a substring: `allUsers` is a suffix of nothing here today,
/// but `contains` on a principal is exactly the check that has already
/// passed a mutation in this repository, and the delimiter costs one line.
fn lines_naming_principal(relative: &str, principal: &str) -> Vec<String> {
    let root = repository_root();
    let mut found = Vec::new();
    for extension in ["yaml", "yml"] {
        for path in files_with_extension(relative, extension) {
            let display = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {display}: {error}"));
            for (index, line) in source.lines().enumerate() {
                if line.trim_start().starts_with('#') {
                    continue;
                }
                if line
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|token| token == principal)
                {
                    found.push(format!("{display}:{}: {}", index + 1, line.trim()));
                }
            }
        }
    }
    found
}

/// The environments under `envs/` that carry an `api.yaml`, which must be
/// at least one: a test that loops over none asserts nothing.
fn environments_with_an_api() -> Vec<String> {
    let envs = repository_root().join(ENVS);
    assert!(
        envs.is_dir(),
        "{ENVS} does not exist; ADR 0036 decision 4 puts one directory per environment there"
    );
    let mut found: Vec<String> = std::fs::read_dir(&envs)
        .expect("readable")
        .flatten()
        .filter(|entry| entry.path().join("api.yaml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    found.sort();
    assert!(
        !found.is_empty(),
        "no directory under {ENVS} carries an api.yaml; ADR 0036 moved the API's service there \
         and every assertion below would be skipped"
    );
    found
}

/// One environment's tfvars, comments stripped.
fn tfvars_of(environment: &str) -> String {
    without_comments(&read(&format!(
        "infrastructure/environments/{environment}/terraform.tfvars"
    )))
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

    // The catalogue is what a reviewer reads; the grant that exists is the
    // `IAMPolicyMember` in each environment's `invokers.yaml`, since ADR
    // 0036 decision 5 released `_iam_member.invokers` from the module. So
    // the same decision is read again where it is applied, per environment:
    // exactly one grant on the API, `roles/run.invoker`, to the console's
    // own account — and only where the tfvars create that account, because
    // `console_enabled = var.console_egress_cidr != null` in the root, and a
    // grant to an identity that does not exist is one Config Connector
    // cannot apply. An environment with no console carries no grant on the
    // API, so the API there is reachable by nobody, which is the honest
    // state rather than a placeholder principal.
    //
    // What used to refuse the two principals that turn the URL into the
    // route in — `allAuthenticatedUsers` outright, `allUsers` outside the
    // open-anonymous posture — was a precondition on the service resource,
    // and left with it. The refusal is now a read of every manifest: the
    // manifests are what is applied, and a principal that is not in them is
    // not on any service.
    let environments = environments_with_an_api();
    let mut grants_checked = 0usize;
    for environment in &environments {
        let tfvars = tfvars_of(environment);
        let project = tfvar(&tfvars, "project_id")
            .unwrap_or_else(|| panic!("{environment}'s tfvars set no project_id"));
        let console_enabled = tfvar(&tfvars, "console_egress_cidr").is_some();
        let api = format!("qip-{environment}-api");
        let openobserve = format!("qip-{environment}-openobserve");
        let expected_member =
            format!("serviceAccount:qip-{environment}-console@{project}.iam.gserviceaccount.com");
        let documents = documents_under(&format!("{ENVS}/{environment}"));
        let mut on_api = Vec::new();
        for document in &documents {
            if !document.kind.starts_with("IAM") {
                continue;
            }
            // One shape only. An authoritative `IAMPolicy` carries a
            // `bindings` list this reader does not walk, and a grant it
            // could not see is a grant this test would call absent.
            assert_eq!(
                document.kind,
                "IAMPolicyMember",
                "{} is an IAM grant in a shape this test does not read; write it as an \
                 IAMPolicyMember so who may call what is one line per grant",
                document.describe()
            );
            let target = document.resource_ref_name().unwrap_or_else(|| {
                panic!("{} names no spec.resourceRef.name", document.describe())
            });
            if target == api {
                on_api.push(document);
            } else if target == openobserve {
                // ADR 0030's one anonymous grant. `gitops.rs`'s
                // `openobserve_is_deployed_at_the_reviewed_digest_anonymous_as_adr_0030_records_and_on_ephemeral_storage`
                // pins it to exactly one `allUsers` per environment, on that
                // service, and refuses `allUsers` wherever OpenObserve is not
                // deployed. Not asserted twice here.
            } else {
                // The brains, or anything else: nothing calls them over HTTP,
                // and a grant on one is a route in that the catalogue's
                // `invokers = []` says does not exist.
                panic!(
                    "{} grants {} on `{target}`, which is not the API; the brains take no \
                     invoker (catalogue.tf: `invokers = []`) and nothing else under {ENVS}/\
                     {environment} may be called",
                    document.describe(),
                    document.field(2, "role").unwrap_or_default()
                );
            }
        }
        if console_enabled {
            assert_eq!(
                on_api.len(),
                1,
                "{environment} creates a console identity (console_egress_cidr is set) and its \
                 invokers.yaml carries {} grant(s) on {api}; exactly one, to the console, is the \
                 route the portal has — none is a 403 on every gateway call, two is a second \
                 caller",
                on_api.len()
            );
            let grant = on_api[0];
            assert_eq!(
                grant.field(2, "role").as_deref(),
                Some("roles/run.invoker"),
                "{} grants a role other than roles/run.invoker on the API",
                grant.describe()
            );
            assert_eq!(
                grant.field(2, "member").as_deref(),
                Some(expected_member.as_str()),
                "{} names a principal other than the console's own account; the API's invoker \
                 is exactly `{expected_member}`",
                grant.describe()
            );
            grants_checked += 1;
        } else {
            assert!(
                on_api.is_empty(),
                "{environment} sets no console_egress_cidr, so no console identity exists, yet \
                 {} grant(s) name {api}: {:?}",
                on_api.len(),
                on_api.iter().map(|d| d.describe()).collect::<Vec<_>>()
            );
        }
    }
    assert!(
        grants_checked >= 1,
        "no environment under {ENVS} enables the console, so the positive half of this test \
         — the grant that exists is the console's — was never checked"
    );

    // `allAuthenticatedUsers` has no exception anywhere: it is every Google
    // account on earth, and reads to a reviewer as if it were a restriction.
    // Over the whole of {GITOPS}, not only `envs/`, because a grant is a
    // grant wherever a manifest declares it.
    let all_authenticated = lines_naming_principal(GITOPS, "allAuthenticatedUsers");
    assert!(
        all_authenticated.is_empty(),
        "allAuthenticatedUsers is named under {GITOPS}; it has no exception on any service, \
         and the module refusal that used to catch it left with the service resource (ADR \
         0036):\n{}",
        all_authenticated.join("\n")
    );
    // `allUsers` outside `envs/` has nothing to be bound to; inside, the
    // gitops.rs test named above holds it to OpenObserve's one grant.
    let anonymous_outside_envs: Vec<String> = lines_naming_principal(GITOPS, "allUsers")
        .into_iter()
        .filter(|line| !line.starts_with(&format!("{ENVS}/")))
        .collect();
    assert!(
        anonymous_outside_envs.is_empty(),
        "allUsers is named under {GITOPS} outside {ENVS}, where no service exists for ADR \
         0030's one exception to apply to:\n{}",
        anonymous_outside_envs.join("\n")
    );
}

#[test]
fn the_api_is_reachable_only_from_inside_the_vpc_and_its_address_is_a_terraform_output() {
    // The security argument of ADR 0018, restated for Cloud Run: without
    // internal ingress the API's own URL answers the internet, and `POST
    // /api/v1/kill-switch` acquires a public address.
    //
    // Read from `api.yaml` in every environment that has one, because that
    // is the resource that carries the posture since ADR 0036 decision 4;
    // the module this test used to read has no service resource to put an
    // ingress on, and `catalogue.tf` names no posture for the API at all.
    // Asserted as equality with the internal value, never as the absence of
    // `INGRESS_TRAFFIC_ALL`: a `RunService` with no `ingress` field at all
    // is one Cloud Run defaults to all traffic, so a deleted line is the
    // public address, not a safe omission.
    let environments = environments_with_an_api();
    for environment in &environments {
        let api = format!("qip-{environment}-api");
        let openobserve = format!("qip-{environment}-openobserve");
        let documents = documents_under(&format!("{ENVS}/{environment}"));
        let services: Vec<&Document> = documents
            .iter()
            .filter(|document| document.kind == "RunService")
            .collect();
        let api_service = services
            .iter()
            .find(|service| service.name == api)
            .unwrap_or_else(|| {
                panic!(
                    "{ENVS}/{environment}/api.yaml exists and no RunService named `{api}` was \
                     read out of {ENVS}/{environment}; the manifest has changed shape and this \
                     test is blind to it"
                )
            });
        let ingress = api_service.field(2, "ingress");
        assert_eq!(
            ingress.as_deref(),
            Some(INTERNAL_ONLY),
            "{} carries ingress {ingress:?}; anything but {INTERNAL_ONLY} — including no \
             `ingress` line, which Cloud Run reads as all traffic — puts the kill switch on a \
             public address",
            api_service.describe()
        );

        // The neighbours, so that the API's posture cannot be relaxed by
        // moving its route onto another service. `gitops.rs`'s
        // `every_run_service_holds_the_invariants_its_catalogue_entry_and_the_cloud_run_module_held`
        // pins each catalogue workload — the two brains included — to
        // internal ingress; this is the complement, over every RunService
        // the environment declares whatever it is named: the one that may
        // answer the internet is OpenObserve (ADR 0030, revisited by ADR
        // 0033), it does so through exactly the one value, and no other
        // service carries any other posture.
        for service in &services {
            let ingress = service.field(2, "ingress");
            if service.name == openobserve {
                assert_eq!(
                    ingress.as_deref(),
                    Some(ALL_TRAFFIC),
                    "{} is ADR 0030's anonymous service and carries ingress {ingress:?} rather \
                     than {ALL_TRAFFIC}; a public grant behind an internal posture is a public \
                     403 that reads as an exposure",
                    service.describe()
                );
                continue;
            }
            assert_eq!(
                ingress.as_deref(),
                Some(INTERNAL_ONLY),
                "{} carries ingress {ingress:?}; only `{openobserve}` may answer the internet \
                 (ADR 0030), and every other service under {ENVS}/{environment} is \
                 {INTERNAL_ONLY}",
                service.describe()
            );
        }
    }

    // The address the console dials is the API's URL as Terraform reports
    // it, never a literal. A copy in a script is the copy nobody thinks to
    // change, because it is the only one that is not configuration. Since
    // ADR 0036 the module computes the URL from the project number rather
    // than reading it back from a service it no longer owns; the output is
    // still the module's, and still the API's.
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
