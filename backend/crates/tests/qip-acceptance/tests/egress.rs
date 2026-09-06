//! What the egress proxy is allowed to reach, and what it is not.
//!
//! `infrastructure/egress/envoy.yaml` is the only thing in this repository
//! that describes a path from a workload to the public internet. The adapters
//! that need it — `qip_storage::gcp`, `qip_training::vertex`,
//! `qip_quantum::provider` — refuse `https` by name and therefore hand a
//! plaintext credential to whatever that file points them at. Everything below
//! is a check on the one file that decides where that goes, and on the two
//! places it is rendered: the Envoy sidecar `modules/cloudrun` attaches beside
//! a service, and the `qip-egress.service` unit `modules/execution-node`
//! installs beside the binary.
//!
//! These are string checks on YAML and HCL rather than a parse, matching the
//! idiom in `infrastructure.rs`: they cannot understand the configuration, so
//! they can be fooled by somebody rewriting it. What they can do is fail when a
//! security property is deleted, which is the change that actually happens.
//!
//! Every test asserts its own premise before its conclusion. A check that reads
//! a file, filters it to nothing and then finds no violation is a check that
//! passes after somebody deletes the thing it was checking, and this file
//! covers a control whose absence is silent — a proxy that reaches one host too
//! many produces no error anywhere.
//!
//! That is not a hypothetical warning here; it is this file's own history. Six
//! of these checks were wrong the first time they were run, and every one of
//! them was wrong about how to read the bootstrap rather than about the
//! bootstrap: four read only the block form `key: value` while every socket
//! address is written in the flow form `{ key: value }`, one scraped the
//! `node:` block's own statistics name as a route, and one could not tell a
//! sentence about an annotation from the annotation. And for a year the
//! previous version of this suite asserted the shape of a proxy that was
//! committed commented out and never ran — a green run was evidence about a
//! design document. `values_of`, `key_count` and the premise assertions below
//! are the fix for the first; the tests at the foot of the file, which fire
//! when the proxy is detached from a workload that needs it, are the fix for
//! the second.

use qip_acceptance::{files_with_extension, read};

/// The one committed bootstrap.
const BOOTSTRAP: &str = "infrastructure/egress/envoy.yaml";

/// The list the pipeline mirrors and attests from, and the module reads the
/// proxy image out of.
const VENDORED: &str = "infrastructure/egress/vendored-images.txt";

const CATALOGUE: &str = "infrastructure/terraform/catalogue.tf";
const PROXY_MODULE: &str = "infrastructure/terraform/modules/egress-proxy/main.tf";
const PROXY_VARIABLES: &str = "infrastructure/terraform/modules/egress-proxy/variables.tf";
const PROXY_OUTPUTS: &str = "infrastructure/terraform/modules/egress-proxy/outputs.tf";
const CLOUD_RUN_MODULE: &str = "infrastructure/terraform/modules/cloudrun/main.tf";
const NODE_MODULE: &str = "infrastructure/terraform/modules/execution-node/main.tf";
const NODE_VARIABLES: &str = "infrastructure/terraform/modules/execution-node/variables.tf";
const NODE_STARTUP: &str =
    "infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl";
const TRUST_ZONES_MODULE: &str = "infrastructure/terraform/modules/trust-zones/main.tf";
const ROOT: &str = "infrastructure/terraform/main.tf";
const ROOT_VARIABLES: &str = "infrastructure/terraform/variables.tf";

/// Where every listener binds, and the only address a co-located adapter is
/// ever configured with.
const LOOPBACK: &str = "127.0.0.1";
/// The one address the bootstrap opens wider, and only for `health`.
const WILDCARD: &str = "0.0.0.0";

/// The trust store every upstream is verified against.
///
/// The path the distroless Envoy image carries. A `validation_context` naming
/// a file that is not in the image fails closed — Envoy refuses to start — so
/// this is checked for drift rather than for danger.
const CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

/// The vendor hosts the proxy may reach, and the adapter each was derived from.
///
/// Written here as a second copy of the bootstrap's list on purpose. The point
/// of an allowlist is that widening it is deliberate, and a test that read the
/// list out of the file it is checking would agree with every widening. Adding
/// a destination is therefore three edits — the bootstrap, the root's
/// `egress_allowed_upstreams`, and this — and a reviewer who sees all of them.
///
/// Each entry names where it came from, because "why is this host here" is the
/// question a reviewer of the *next* entry will need answered by example.
const ALLOWED_UPSTREAMS: [(&str, &str); 7] = [
    (
        "storage.googleapis.com",
        "qip_storage::gcp::storage — its own requirement string names the host, \
         and it is reached through QIP_GCP_ENDPOINT",
    ),
    (
        "bigquery.googleapis.com",
        "qip_storage::gcp::bigquery — same requirement string, same variable, \
         distinguished from Cloud Storage by path",
    ),
    (
        "europe-west2-aiplatform.googleapis.com",
        "qip_training::vertex documents {region}-aiplatform.googleapis.com; one \
         region, deliberately not a wildcard",
    ),
    (
        "quantum.cloud.ibm.com",
        "the HostedConfig in crates/libs/qip-quantum/tests/hosted.rs",
    ),
    (
        "api.quantum.ibm.com",
        "the HostedConfig in crates/libs/qip-quantum/tests/quantum.rs and in \
         crates/services/qip-optimization-engine/tests/optimization.rs",
    ),
    (
        "api.frankfurter.dev",
        "the shipped manifest of frankfurter-ecb-reference-rates, in \
         crates/services/qip-market-ingestion/src/connectors/manifests/, and \
         FrankfurterRatesConnector::UPSTREAM_HOST, which connector_feed.rs's \
         bridge opens by name; licensing class `public`, evaluated in \
         qip-data-finder/src/admission.rs before the source can open",
    ),
    (
        "router.huggingface.co",
        "HuggingFaceModel::UPSTREAM_HOST in \
         crates/services/qip-reasoning-engine/src/providers/huggingface.rs, the \
         hosted language-model adapter ADR 0037 decides on; constructed by the \
         deep brain alone, reached on POST /v1/chat/completions and nothing \
         else, and dark until an environment sets the variables and mounts the \
         secret, which none does",
    ),
];

/// The hosted language-model listener (ADR 0037). Restated rather than read
/// from the adapter, for the same reason as `ALLOWED_UPSTREAMS`; the test that
/// holds the adapter's own constant, the bootstrap and the root variable to
/// one value is
/// `the_hugging_face_host_is_one_value_in_the_adapter_the_bootstrap_and_the_allowlist`.
const HUGGING_FACE_LISTENER: &str = "huggingface";
const HUGGING_FACE_HOST: &str = "router.huggingface.co";
const HUGGING_FACE_CLUSTER: &str = "router_huggingface_co";
const HUGGING_FACE_ADAPTER: &str =
    "backend/crates/services/qip-reasoning-engine/src/providers/huggingface.rs";

/// The shipped manifest the market-data listener is derived from.
///
/// Read rather than restated: a route checked against a literal this test
/// carried itself would agree with a connector that had been repointed.
const FRANKFURTER_MANIFEST: &str = "backend/crates/services/qip-market-ingestion/src/\
                                    connectors/manifests/frankfurter-ecb-reference-rates.json";
const FRANKFURTER_TRANSPORT: &str =
    "backend/crates/services/qip-market-ingestion/src/connector/transport.rs";
const FRANKFURTER_LICENSING: &str = "backend/crates/services/qip-data-finder/src/admission.rs";
const FRANKFURTER_LISTENER: &str = "frankfurter";
/// Restated rather than read from the connector, for the same reason as
/// `ALLOWED_UPSTREAMS`: this file's checks are the reviewer's copy. The test
/// that holds the connector's own constant, the manifest, the bootstrap and
/// the root variable to one value is
/// `the_frankfurter_host_is_one_value_in_the_manifest_the_bootstrap_and_the_allowlist`.
const FRANKFURTER_HOST: &str = "api.frankfurter.dev";
const FRANKFURTER_CLUSTER: &str = "api_frankfurter_dev";

/// Whether a configuration sets a setting to a value.
///
/// Compares with whitespace collapsed, so a `terraform fmt` that realigns the
/// equals signs does not break a check reading this. Matches
/// `infrastructure.rs`'s own `sets` exactly; each test binary in this suite
/// reads Terraform as text independently, and duplicating the four-line
/// helper is cheaper than a shared dependency between two acceptance suites
/// that otherwise have nothing to say to each other.
fn sets(content: &str, setting: &str, value: &str) -> bool {
    content.lines().any(|line| {
        let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        collapsed == format!("{setting} = {value}")
    })
}

/// A configuration with its comments removed.
///
/// A comment naming a host is not a route to it, and a check that cannot tell
/// the difference makes it impossible to write down why a host was refused.
fn without_comments(content: &str) -> String {
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every place a key appears in mapping position, as the text following it.
///
/// Two forms are matched because the bootstrap uses both: the block form
/// `key: value`, and the flow form `socket_address: { address: X, port_value:
/// N }` that every address, port and certificate matcher is written in. The
/// key is matched with its delimiter: what precedes it must be a mapping or
/// list boundary, so `address` does not match inside `socket_address` and
/// `port` does not match inside `targetPort`; what follows the value ends it.
fn occurrences_of(text: &str, key: &str) -> Vec<String> {
    let needle = format!("{key}:");
    let mut found = Vec::new();
    for line in without_comments(text).lines() {
        for (index, _) in line.match_indices(needle.as_str()) {
            let boundary = line[..index]
                .chars()
                .next_back()
                .is_none_or(|previous| matches!(previous, ' ' | '{' | ','));
            if !boundary {
                continue;
            }
            let tail = &line[index + needle.len()..];
            let value = tail.split([',', '}']).next().unwrap_or(tail);
            found.push(value.trim().trim_matches('"').trim().to_string());
        }
    }
    found
}

/// The values of every `key:` in a block of configuration, comments excluded.
fn values_of(text: &str, key: &str) -> Vec<String> {
    occurrences_of(text, key)
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}

/// How many times a key appears, whatever it carries.
fn key_count(text: &str, key: &str) -> usize {
    occurrences_of(text, key).len()
}

/// The Envoy bootstrap, as committed.
fn bootstrap() -> String {
    let body = read(BOOTSTRAP);
    assert!(
        body.lines().count() > 100,
        "only {} lines of bootstrap were read; the file has been reshaped and \
         every check in this file is now looking at nothing",
        body.lines().count()
    );
    assert!(
        body.contains("\nstatic_resources:\n"),
        "{BOOTSTRAP} is not an Envoy bootstrap any more"
    );
    body
}

/// One block of the bootstrap, bounded by the block that follows it.
///
/// Bounded rather than open-ended, and the difference is not cosmetic: a check
/// reading "the listeners" that ran on past `clusters:` would read the six
/// upstream port 443s as listener ports. Both ends must be found, so a
/// reshaped bootstrap fails here rather than quietly widening what a caller is
/// looking at.
fn section(bootstrap: &str, opening: &str, closing: Option<&str>) -> String {
    let body = bootstrap.split(opening).nth(1).unwrap_or_else(|| {
        panic!(
            "the bootstrap has no `{}` block, so every check reading it is \
             reading nothing at all",
            opening.trim()
        )
    });
    match closing {
        None => body.to_string(),
        Some(end) => {
            let (block, _) = body.split_once(end).unwrap_or_else(|| {
                panic!(
                    "the `{}` block is not followed by `{}`; the bootstrap has \
                     been reshaped and this block now runs into the next",
                    opening.trim(),
                    end.trim()
                )
            });
            block.to_string()
        }
    }
}

fn admin_block(bootstrap: &str) -> String {
    section(bootstrap, "\nadmin:\n", Some("\nstatic_resources:"))
}

fn listeners_block(bootstrap: &str) -> String {
    section(bootstrap, "\n  listeners:\n", Some("\n  clusters:"))
}

fn clusters_block(bootstrap: &str) -> String {
    section(bootstrap, "\n  clusters:\n", None)
}

/// The top-level entries of a `listeners:` or `clusters:` block, as name and
/// body.
///
/// Split on the four-space `- name:` that a top-level entry is indented with,
/// which is the only place that indent occurs: a network filter's `name:` sits
/// at twelve and a transport socket's at eight.
fn entries_of(block: &str) -> Vec<(String, String)> {
    let padded = format!("\n{block}");
    padded
        .split("\n    - name: ")
        .skip(1)
        .map(|entry| {
            let (name, body) = entry.split_once('\n').unwrap_or((entry, ""));
            (name.trim().to_string(), body.to_string())
        })
        .collect()
}

/// The listeners as name and bound port, asserting one bind each.
fn listener_ports() -> Vec<(String, String)> {
    let bootstrap = bootstrap();
    let listeners = entries_of(&listeners_block(&bootstrap));
    assert_eq!(
        listeners.len(),
        7,
        "{:?} listeners were read; there are six destination listeners and \
         the health listener, and a listener this check cannot see is a port \
         nothing below constrains",
        listeners.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    listeners
        .into_iter()
        .map(|(name, body)| {
            let ports = values_of(&body, "port_value");
            assert_eq!(
                ports.len(),
                1,
                "the listener {name} binds {ports:?}; one listener, one port is \
                 what makes the port a destination selector"
            );
            (name, ports[0].clone())
        })
        .collect()
}

/// The `vendor/envoy` line of the vendored-images list, as source and digest.
///
/// Selects by destination rather than by position: a second entry
/// (`vendor/openobserve`, mirrored so its reviewed bytes exist in the
/// registry but not yet deployed — see the file's own comment on the two
/// decisions still outstanding before it can be) means "exactly one line"
/// stopped being a fact about this file the moment it was reviewed, but
/// "the envoy line is still the distroless proxy image" has to remain true
/// regardless of how many other images this platform goes on to adopt.
fn vendored_envoy() -> (String, String) {
    let list = read(VENDORED);
    let entries: Vec<&str> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert!(
        !entries.is_empty(),
        "{VENDORED} carries no entries; the egress proxy has to be vendored from somewhere"
    );
    let envoy_lines: Vec<&&str> = entries
        .iter()
        .filter(|line| line.split_whitespace().nth(1) == Some("vendor/envoy"))
        .collect();
    assert_eq!(
        envoy_lines.len(),
        1,
        "{VENDORED} carries {entries:?}; expected exactly one vendor/envoy line"
    );
    let fields: Vec<&str> = envoy_lines[0].split_whitespace().collect();
    assert_eq!(
        fields.len(),
        3,
        "{VENDORED} line is not `<source@digest> <dest> <tag>`: {entries:?}"
    );
    let (repository, digest) = fields[0]
        .split_once("@sha256:")
        .unwrap_or_else(|| panic!("{} is not pinned by digest", fields[0]));
    (repository.to_string(), digest.to_string())
}

/// The vendored OpenObserve line is well-formed and pinned by digest, and the
/// two decisions the vendoring comment once named as still needing a human —
/// a new top-level workload category, and how storage is authenticated
/// without a static key — are visibly made rather than defaulted past:
/// `modules/cloudrun` carries the `image_source = "vendored"` branch ADR 0028
/// decision 3 describes, and `catalogue.tf` wires it up on ephemeral,
/// ZO_S3_*-free storage per decision 4, gated on
/// `vendored_openobserve_image_digest` naming a digest.
///
/// This test used to assert the opposite of its second half — that nothing
/// in `infrastructure/terraform/**` referenced `vendor/openobserve` at all —
/// as a deliberate tripwire for the day OpenObserve stopped being merely
/// mirrored. That day is this commit, and the honest update named by the old
/// test's own comment is this one: assert the wiring is real and correct
/// instead of asserting its absence.
#[test]
fn the_vendored_openobserve_image_is_pinned_and_every_environment_that_names_it_names_the_reviewed_digest()
 {
    let list = read(VENDORED);
    let entries: Vec<&str> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let openobserve_lines: Vec<&&str> = entries
        .iter()
        .filter(|line| line.split_whitespace().nth(1) == Some("vendor/openobserve"))
        .collect();
    assert_eq!(
        openobserve_lines.len(),
        1,
        "{VENDORED} carries {entries:?}; expected exactly one vendor/openobserve line"
    );
    let fields: Vec<&str> = openobserve_lines[0].split_whitespace().collect();
    assert_eq!(
        fields.len(),
        3,
        "{VENDORED} line is not `<source@digest> <dest> <tag>`: {entries:?}"
    );
    fields[0]
        .split_once("@sha256:")
        .unwrap_or_else(|| panic!("{} is not pinned by digest", fields[0]));

    // The image, the posture and the storage are the manifest's since ADR
    // 0036: gitops.rs's OpenObserve test asserts the mirrored image at this
    // reviewed digest, both halves of ADR 0030's anonymous posture, and
    // ephemeral storage, on every environment's RunService. What Terraform
    // still holds is the identity and its grants, gated on the same root
    // variable that gates the deployment, and the ADR 0031 root login as
    // `secret_env` — the grant for which the module keys on that input.
    let catalogue = without_comments(&read(CATALOGUE));
    let openobserve_block = catalogue
        .split("module \"openobserve\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect(
            "catalogue.tf declares module \"openobserve\" with a closing brace on its own line",
        );
    assert!(
        openobserve_block
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")
                == "count = var.vendored_openobserve_image_digest != null ? 1 : 0"),
        "OpenObserve's identity is no longer gated on the root's digest variable, so an \
         environment that deploys nothing carries a principal for it"
    );
    assert!(
        openobserve_block.contains("ZO_ROOT_USER_EMAIL")
            && openobserve_block.contains("ZO_ROOT_USER_PASSWORD")
            && !openobserve_block.contains("image_source")
            && !openobserve_block.contains("ingress_posture"),
        "OpenObserve's catalogue block no longer names the ADR 0031 root login as secret_env, \
         or names an image or posture again: {openobserve_block}"
    );

    // The root's digest variable stays closed by default: setting it is what
    // creates the service, and no environment forces it.
    let root_variables = without_comments(&read(ROOT_VARIABLES));
    assert!(
        root_variables.contains("variable \"vendored_openobserve_image_digest\"")
            && sets(&root_variables, "default", "null"),
        "the root no longer declares vendored_openobserve_image_digest with a null default"
    );
    // An environment may pin one, and if it does it must be the digest this
    // repository reviewed. This used to assert that no environment pinned
    // anything at all, which was true until dev did, and was a tripwire for
    // exactly this moment rather than a rule. The rule it becomes is the
    // stronger half: `vendored-images.txt` is the review surface — its git
    // history is the answer to "which foreign code runs here" — so a digest
    // in a tfvars that appears nowhere in that file is bytes nobody read,
    // and Binary Authorization would refuse them at apply having already
    // let them through review.
    let vendored = read(VENDORED);
    let reviewed: Vec<String> = vendored
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| line.split_whitespace().nth(1) == Some("vendor/openobserve"))
        .filter_map(|line| line.split_whitespace().next())
        .filter_map(|source| source.split_once("@").map(|(_, digest)| digest.to_string()))
        .collect();

    for environment in ["dev", "test", "stage", "prod"] {
        let tfvars = read(&format!(
            "infrastructure/environments/{environment}/terraform.tfvars"
        ));
        let Some(pinned) = tfvars
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("vendored_openobserve_image_digest"))
        else {
            continue;
        };
        // The premise, asserted before the property: a review surface with no
        // OpenObserve line would make every pin below unreviewable, and an
        // empty `reviewed` would otherwise fail each one with a message about
        // the wrong file.
        assert!(
            !reviewed.is_empty(),
            "{VENDORED} carries no active vendor/openobserve line, so {environment}'s pinned \
             digest cannot be checked against anything: {pinned}"
        );
        assert!(
            reviewed
                .iter()
                .any(|digest| pinned.contains(digest.as_str())),
            "{environment} pins an OpenObserve digest that {VENDORED} has not reviewed. The \
             reviewed digests are {reviewed:?} and the pin is: {pinned}"
        );
    }
}

// ---------------------------------------------------------------------------
// The bootstrap
// ---------------------------------------------------------------------------

#[test]
fn the_egress_proxy_dials_only_the_vendor_hosts_the_adapters_named() {
    // The exfiltration question. A forward proxy that will connect anywhere is
    // a route out of a process holding trading credentials, and the thing that
    // makes this one not that is the set of clusters declared in its bootstrap.
    let bootstrap = bootstrap();

    // Premise. Every address in this file is written inside a flow mapping;
    // eight binds and seven upstreams is fifteen, and fewer than fourteen
    // means the socket addresses have moved and this check is looking at
    // nothing.
    let addresses = values_of(&bootstrap, "address");
    assert!(
        addresses.len() >= 14,
        "only {addresses:?} were read out of the bootstrap; the socket_address \
         blocks have been reshaped and this check is filtering an empty list"
    );

    // `socket_address` carries both the listener binds and the upstream dials.
    // The binds are loopback and nothing else — an interface bind is asserted
    // against below — so everything that is not loopback is somewhere this
    // proxy will connect out to.
    let (bound, dialled): (Vec<String>, Vec<String>) = addresses
        .into_iter()
        .partition(|address| address == LOOPBACK || address == WILDCARD);

    assert!(
        bound.len() >= 6,
        "only {bound:?} bind addresses were found beside {dialled:?}; the rule \
         that tells a bind from a dial has stopped telling them apart"
    );
    assert_eq!(
        dialled.len(),
        ALLOWED_UPSTREAMS.len(),
        "the bootstrap dials {dialled:?}; the allowlist this test carries is \
         {:?}. A destination added in one place and not the other is a \
         destination nobody reviewed.",
        ALLOWED_UPSTREAMS.map(|(host, _)| host)
    );

    for upstream in &dialled {
        assert!(
            ALLOWED_UPSTREAMS.iter().any(|(host, _)| host == upstream),
            "the proxy dials {upstream}, which no adapter in this workspace \
             names. An allowlist entry that was guessed is an allowlist entry \
             that is wrong, and this one is a route to the internet."
        );
        assert!(
            !upstream.contains('*'),
            "{upstream} carries a wildcard; the destination list stops being a \
             list the moment one entry matches a family of hosts"
        );
    }
    for (host, provenance) in ALLOWED_UPSTREAMS {
        assert!(
            dialled.iter().any(|upstream| upstream == host),
            "{host} is on this test's allowlist ({provenance}) and the \
             bootstrap does not dial it. Either the adapter stopped needing it \
             — in which case delete both — or the destination was dropped and \
             that adapter now fails with a 404 from a proxy nobody suspects."
        );
    }

    // And the root declares the same set, which the module's plan-time gate
    // compares against the file. Three copies, each a reviewer sees.
    let declared: Vec<String> = read(ROOT_VARIABLES)
        .split("variable \"egress_allowed_upstreams\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("the root declares egress_allowed_upstreams")
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"'))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect();
    let mut declared_sorted = declared.clone();
    declared_sorted.sort();
    let mut allowed_sorted: Vec<String> = ALLOWED_UPSTREAMS
        .iter()
        .map(|(host, _)| (*host).to_string())
        .collect();
    allowed_sorted.sort();
    assert_eq!(
        declared_sorted, allowed_sorted,
        "the root's egress_allowed_upstreams default and this allowlist disagree"
    );
    let module = without_comments(&read(PROXY_MODULE));
    assert!(
        module.contains("condition     = toset(local.dialled) == toset(var.allowed_upstreams)"),
        "the proxy module no longer refuses a bootstrap that dials a host the deployment did not declare"
    );
}

#[test]
fn the_market_data_listener_reaches_one_vendor_on_one_path_and_widening_it_fails_here() {
    // The first market-data egress path this platform has had, and the first
    // destination in this bootstrap that is neither Google nor IBM. The risk
    // it carries is not that it is wrong today; it is that "we already have a
    // vendor listener" is the sentence that precedes a second route on it, a
    // catch-all prefix, or a SAN matcher widened to a suffix. Each of those is
    // a separate assertion below, and each is a route to the internet out of a
    // process that holds trading state.
    //
    // Everything is derived from the shipped manifest rather than restated, so
    // a connector repointed at another path or another host fails here instead
    // of being agreed with.
    let manifest: serde_json::Value = serde_json::from_str(&read(FRANKFURTER_MANIFEST))
        .expect("the frankfurter manifest is JSON");
    let manifest_host = manifest["provider"]
        .as_str()
        .expect("the manifest names its provider");
    let manifest_path = manifest["endpoint"]["path"]
        .as_str()
        .expect("the manifest names the path it requests");

    // Premise, twice over. A manifest whose path were "/" or empty would make
    // the prefix comparison below vacuous — it would admit the catch-all this
    // test exists to refuse — and a manifest that stopped naming the host
    // would leave the allowlist entry with no provenance.
    assert!(
        manifest_path.starts_with('/') && manifest_path.len() > 1,
        "the manifest requests {manifest_path:?}; a path that is empty or a \
         bare `/` cannot distinguish a narrow route from a catch-all, and \
         every check below would pass on a proxy that forwards everything"
    );
    assert!(
        manifest_host.contains(FRANKFURTER_HOST),
        "the manifest's provider no longer names {FRANKFURTER_HOST}; the \
         allowlist entry for that host has lost the source it was derived from"
    );

    let bootstrap = bootstrap();
    let listeners = entries_of(&listeners_block(&bootstrap));
    let (_, listener) = listeners
        .iter()
        .find(|(name, _)| name == FRANKFURTER_LISTENER)
        .unwrap_or_else(|| {
            panic!(
                "no `{FRANKFURTER_LISTENER}` listener in the bootstrap, but \
                 {FRANKFURTER_HOST} is on the allowlist. An upstream declared \
                 with no listener in front of it is a host the proxy may dial \
                 and no reviewed route reaches — the widening without the use."
            )
        });

    // Loopback, and one port. The port is the whole destination selector here:
    // there is no host header a caller controls, so a wider bind is a route
    // anything on the network can take to a rewritten authority.
    assert_eq!(
        values_of(listener, "address"),
        vec![LOOPBACK.to_string()],
        "the {FRANKFURTER_LISTENER} listener does not bind {LOOPBACK} alone"
    );
    let ports = values_of(listener, "port_value");
    assert_eq!(
        ports.len(),
        1,
        "the {FRANKFURTER_LISTENER} listener binds {ports:?}; one listener, \
         one port is what makes the port a destination selector"
    );

    // One route. Not "every route points at Frankfurter" — one. A second route
    // on this listener is a second thing reachable through an address whose
    // whole review was "it is the rates feed".
    let routed = values_of(listener, "cluster");
    assert_eq!(
        routed,
        vec![FRANKFURTER_CLUSTER.to_string()],
        "the {FRANKFURTER_LISTENER} listener routes to {routed:?}. One vendor, \
         one cluster: this listener was reviewed as a path to the ECB's \
         reference rates and to nothing else."
    );
    assert_eq!(
        values_of(listener, "host_rewrite_literal"),
        vec![FRANKFURTER_HOST.to_string()],
        "the {FRANKFURTER_LISTENER} listener rewrites the authority to \
         something other than {FRANKFURTER_HOST}"
    );

    // One prefix, and it is the one the connector actually builds. This is the
    // assertion that refuses `prefix: "/"`: a catch-all makes the listener a
    // forward proxy to whatever the rewritten authority serves, which for a
    // vendor with more than one product is a different API entirely.
    assert_eq!(
        values_of(listener, "prefix"),
        vec![manifest_path.to_string()],
        "the {FRANKFURTER_LISTENER} listener's routes do not match exactly \
         {manifest_path:?}, the one path the shipped manifest requests. A \
         prefix wider than the manifest is a route nothing in this workspace \
         asks for."
    );
    assert!(
        !without_comments(listener).contains("regex"),
        "the {FRANKFURTER_LISTENER} listener matches with a regex; a route \
         whose extent a reviewer has to simulate is not a reviewed route"
    );

    // GET and only GET. Asserting the method at the proxy is what makes "this
    // path cannot carry an order" a property of the boundary rather than of
    // the vendor's product range: an order is a POST, and a POST to this port
    // gets a 404 before a socket is opened upstream.
    assert_eq!(
        values_of(listener, "exact"),
        vec!["GET".to_string()],
        "the {FRANKFURTER_LISTENER} listener no longer restricts the method to \
         GET. The connector transport issues Method::Get and only that; a \
         listener that forwards a POST forwards something this platform does \
         not send."
    );
    let transport = read(FRANKFURTER_TRANSPORT);
    assert!(
        transport.contains("Method::Get"),
        "{FRANKFURTER_TRANSPORT} no longer issues Method::Get, so the route's \
         method matcher above now describes a request nobody makes"
    );
    for method in [
        "Method::Post",
        "Method::Put",
        "Method::Delete",
        "Method::Patch",
    ] {
        assert!(
            !transport.contains(method),
            "{FRANKFURTER_TRANSPORT} issues {method}; the connector transport \
             has gained a write path, and a write path to a market-data vendor \
             is a decision nobody recorded"
        );
    }

    // The converse. Without this, a route added to the `gcp` listener naming
    // this cluster passes every check above, because every check above reads
    // only this listener.
    for (name, body) in &listeners {
        if name == FRANKFURTER_LISTENER {
            continue;
        }
        assert!(
            !values_of(body, "cluster").contains(&FRANKFURTER_CLUSTER.to_string()),
            "the {name} listener routes to {FRANKFURTER_CLUSTER}. The vendor is \
             reachable from one reviewed port, or it is reachable from anywhere \
             a route was added without one."
        );
        assert!(
            !values_of(body, "host_rewrite_literal").contains(&FRANKFURTER_HOST.to_string()),
            "the {name} listener rewrites the authority to {FRANKFURTER_HOST}"
        );
    }

    // The cluster: one host, one certificate name, matched exactly. A matcher
    // widened from `exact` to `suffix` accepts any certificate under the
    // domain, which on a proxy that terminates TLS means a DNS answer is
    // enough to redirect the request.
    let clusters = entries_of(&clusters_block(&bootstrap));
    let (_, cluster) = clusters
        .iter()
        .find(|(name, _)| name == FRANKFURTER_CLUSTER)
        .unwrap_or_else(|| panic!("no `{FRANKFURTER_CLUSTER}` cluster is declared"));
    assert_eq!(
        values_of(cluster, "address"),
        vec![FRANKFURTER_HOST.to_string()],
        "{FRANKFURTER_CLUSTER} dials somewhere other than {FRANKFURTER_HOST}"
    );
    assert_eq!(
        values_of(cluster, "sni"),
        vec![FRANKFURTER_HOST.to_string()],
        "{FRANKFURTER_CLUSTER}'s SNI is not {FRANKFURTER_HOST}"
    );
    assert_eq!(
        values_of(cluster, "exact"),
        vec![FRANKFURTER_HOST.to_string()],
        "{FRANKFURTER_CLUSTER} does not match the certificate name exactly"
    );
    for loose in ["suffix", "contains", "safe_regex", "ignore_case"] {
        assert_eq!(
            key_count(cluster, loose),
            0,
            "{FRANKFURTER_CLUSTER}'s certificate matcher carries `{loose}`. A \
             name matched loosely is every host that can obtain a certificate \
             under it, and this connection carries a request the platform acts \
             on."
        );
    }
    assert_eq!(
        values_of(cluster, "filename"),
        vec![CA_BUNDLE.to_string()],
        "{FRANKFURTER_CLUSTER} verifies against a trust store other than {CA_BUNDLE}"
    );

    // The manifest's own two claims first, then the cross-file drift check.
    // The order is deliberate: these two are self-contained, and putting the
    // check that reads another crate's source ahead of them would mean a
    // catalogue that had not been written yet masked a manifest that had
    // silently changed class.
    assert_eq!(
        manifest["licensing"].as_str(),
        Some("public"),
        "the manifest's licensing class is no longer `public`; the argument \
         for admitting this host ahead of every other candidate was that \
         `public` is the one posture already evaluated"
    );
    assert_eq!(
        manifest["auth"]["scheme"].as_str(),
        Some("none"),
        "the frankfurter connector now authenticates. This proxy terminates \
         TLS and its clients speak plaintext to it, so a credential on this \
         path is a credential in the sidecar's clear text — a different review \
         from the one this listener had."
    );

    // Licensing was evaluated before the source became reachable, which is the
    // ordering .claude/rules/domains/data-and-streaming.md requires. This is a
    // drift check on the catalogue's text, not a proof that admit() runs —
    // that proof lives in the data finder's own tests and in each root's.
    // What it catches is a listener landing here for a source the catalogue
    // never grew an entry for.
    let licensing = read(FRANKFURTER_LICENSING);
    let source_id = manifest["source_id"]
        .as_str()
        .expect("the manifest has a source_id");
    assert!(
        licensing.contains(source_id),
        "{FRANKFURTER_LICENSING} carries no entry for {source_id}, so the \
         bootstrap reaches a vendor whose terms nobody wrote down. \
         admission::admit refuses an uncatalogued source, so this listener \
         would be a route both roots are denied — the widening without the \
         use, again."
    );
}

#[test]
fn the_frankfurter_host_is_one_value_in_the_manifest_the_bootstrap_and_the_allowlist() {
    // ADR 0034 requires a vendor host to move in the manifest, the Envoy
    // cluster and the Terraform allowlist in one commit. On 2026-09-04 the
    // vendor moved from `api.frankfurter.app` to `api.frankfurter.dev`, the
    // old host began answering 301, and the transport — which never follows
    // a redirect — refused the source. The three files then had to change
    // together, and nothing named which of them disagreed with which: the
    // earlier test above checks each against a literal of its own, so a host
    // changed in two places and not the third failed with "does not dial",
    // which is true and unhelpful. This test takes the connector's own
    // constant as the one value and reports every place that differs from
    // it, by name.
    use qip_market_ingestion::connectors::FrankfurterRatesConnector;
    let host = FrankfurterRatesConnector::UPSTREAM_HOST;

    // Premise: the constant is a bare hostname. A URL or a path here would
    // never equal an Envoy `address:` and every mismatch below would fire
    // for the wrong reason.
    assert!(
        host.contains('.') && !host.contains('/') && !host.contains(':'),
        "UPSTREAM_HOST is {host:?}, which is not a bare hostname"
    );

    // The manifest, in the parentheses the provider convention uses. Matched
    // delimited: the host is a substring of longer names.
    let manifest: serde_json::Value = serde_json::from_str(&read(FRANKFURTER_MANIFEST))
        .expect("the frankfurter manifest is JSON");
    let provider = manifest["provider"]
        .as_str()
        .expect("the manifest names its provider");
    let manifest_host = provider
        .split('(')
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("the manifest's provider names its host in parentheses");

    // The bootstrap: the cluster that dials the host, its SNI, its
    // certificate name, and the route's rewritten authority. Four places
    // because Envoy reads four; a cluster dialling the new host with the old
    // SNI is a TLS handshake the vendor refuses.
    let bootstrap = bootstrap();
    let clusters = entries_of(&clusters_block(&bootstrap));
    let (_, cluster) = clusters
        .iter()
        .find(|(name, _)| name == FRANKFURTER_CLUSTER)
        .unwrap_or_else(|| panic!("no `{FRANKFURTER_CLUSTER}` cluster is declared"));
    let listeners = entries_of(&listeners_block(&bootstrap));
    let (_, listener) = listeners
        .iter()
        .find(|(name, _)| name == FRANKFURTER_LISTENER)
        .unwrap_or_else(|| panic!("no `{FRANKFURTER_LISTENER}` listener is declared"));

    // The root variable's default: the entry that names frankfurter, and
    // exactly one of them, because two entries would be the old and the new
    // host both allowed.
    let declared: Vec<String> = read(ROOT_VARIABLES)
        .split("variable \"egress_allowed_upstreams\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("the root declares egress_allowed_upstreams")
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"'))
        .filter_map(|rest| rest.split('"').next())
        .filter(|entry| entry.contains("frankfurter"))
        .map(str::to_string)
        .collect();
    assert_eq!(
        declared.len(),
        1,
        "{ROOT_VARIABLES} allows {declared:?}; exactly one frankfurter host is \
         the vendor, and two is the old one still reachable"
    );

    let claims: [(&str, Vec<String>); 6] = [
        ("the manifest's provider", vec![manifest_host.to_string()]),
        ("the Envoy cluster's address", values_of(cluster, "address")),
        ("the Envoy cluster's SNI", values_of(cluster, "sni")),
        (
            "the Envoy cluster's certificate name",
            values_of(cluster, "exact"),
        ),
        (
            "the frankfurter listener's rewritten authority",
            values_of(listener, "host_rewrite_literal"),
        ),
        ("the Terraform allowlist entry", declared),
    ];
    let disagreeing: Vec<String> = claims
        .iter()
        .filter(|(_, found)| found.as_slice() != [host.to_string()])
        .map(|(place, found)| format!("{place} says {found:?}"))
        .collect();
    assert!(
        disagreeing.is_empty(),
        "FrankfurterRatesConnector::UPSTREAM_HOST is {host:?} and {} disagree(s): {}. ADR 0034: \
         the manifest, the bootstrap and the allowlist move in one commit, and a host \
         changed in one of them is a 301 — or a certificate the proxy refuses — in the \
         others",
        disagreeing.len(),
        disagreeing.join("; ")
    );
}

#[test]
fn the_hugging_face_host_is_one_value_in_the_adapter_the_bootstrap_and_the_allowlist() {
    // ADR 0037 adds the first model vendor to this bootstrap and ADR 0034
    // requires the adapter's constant, the Envoy cluster and the Terraform
    // allowlist to move in one commit. The failure this prevents is the
    // Frankfurter one, one vendor later: a host changed in two places and not
    // the third, failing as "does not dial" with nothing naming which of the
    // three disagreed. The adapter's constant is the one value; every place
    // that differs is reported by name. And because this is the first
    // listener whose requests carry a credential, the route is held to one
    // exact path and one method: a second route on this port is a second
    // thing the bearer token is sent to.
    use qip_reasoning_engine::providers::huggingface::{CHAT_COMPLETIONS_PATH, HuggingFaceModel};
    let host = HuggingFaceModel::UPSTREAM_HOST;

    // Premise: the constant is a bare hostname. A URL or a path here would
    // never equal an Envoy `address:` and every mismatch below would fire
    // for the wrong reason. And the adapter really does build the path this
    // listener admits, or the route describes a request nobody makes.
    assert!(
        host.contains('.') && !host.contains('/') && !host.contains(':'),
        "UPSTREAM_HOST is {host:?}, which is not a bare hostname"
    );
    assert_eq!(
        host, HUGGING_FACE_HOST,
        "this file's copy of the host has drifted"
    );
    assert!(
        CHAT_COMPLETIONS_PATH.starts_with('/') && CHAT_COMPLETIONS_PATH.len() > 1,
        "CHAT_COMPLETIONS_PATH is {CHAT_COMPLETIONS_PATH:?}; a bare `/` cannot \
         distinguish a narrow route from a catch-all"
    );
    let adapter = read(HUGGING_FACE_ADAPTER);
    assert!(
        adapter.contains("Method::Post"),
        "{HUGGING_FACE_ADAPTER} no longer issues Method::Post, so the route's method \
         matcher describes a request nobody makes"
    );
    for method in [
        "Method::Get",
        "Method::Put",
        "Method::Delete",
        "Method::Head",
    ] {
        assert!(
            !adapter.contains(method),
            "{HUGGING_FACE_ADAPTER} issues {method}; the adapter has gained a request the \
             listener was not reviewed to carry"
        );
    }

    let bootstrap = bootstrap();
    let clusters = entries_of(&clusters_block(&bootstrap));
    let (_, cluster) = clusters
        .iter()
        .find(|(name, _)| name == HUGGING_FACE_CLUSTER)
        .unwrap_or_else(|| panic!("no `{HUGGING_FACE_CLUSTER}` cluster is declared"));
    let listeners = entries_of(&listeners_block(&bootstrap));
    let (_, listener) = listeners
        .iter()
        .find(|(name, _)| name == HUGGING_FACE_LISTENER)
        .unwrap_or_else(|| {
            panic!(
                "no `{HUGGING_FACE_LISTENER}` listener in the bootstrap, but {host} is on \
                 the allowlist. An upstream declared with no listener in front of it is a \
                 host the proxy may dial and no reviewed route reaches."
            )
        });

    // The root variable's default: exactly one entry names the vendor.
    let declared: Vec<String> = read(ROOT_VARIABLES)
        .split("variable \"egress_allowed_upstreams\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("the root declares egress_allowed_upstreams")
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"'))
        .filter_map(|rest| rest.split('"').next())
        .filter(|entry| entry.contains("huggingface"))
        .map(str::to_string)
        .collect();
    assert_eq!(
        declared.len(),
        1,
        "{ROOT_VARIABLES} allows {declared:?}; exactly one Hugging Face host is the vendor"
    );

    let claims: [(&str, Vec<String>); 5] = [
        ("the Envoy cluster's address", values_of(cluster, "address")),
        ("the Envoy cluster's SNI", values_of(cluster, "sni")),
        (
            "the Envoy cluster's certificate name",
            values_of(cluster, "exact"),
        ),
        (
            "the huggingface listener's rewritten authority",
            values_of(listener, "host_rewrite_literal"),
        ),
        ("the Terraform allowlist entry", declared),
    ];
    let disagreeing: Vec<String> = claims
        .iter()
        .filter(|(_, found)| found.as_slice() != [host.to_string()])
        .map(|(place, found)| format!("{place} says {found:?}"))
        .collect();
    assert!(
        disagreeing.is_empty(),
        "HuggingFaceModel::UPSTREAM_HOST is {host:?} and {} disagree(s): {}. ADR 0034: the \
         adapter, the bootstrap and the allowlist move in one commit",
        disagreeing.len(),
        disagreeing.join("; ")
    );

    // The listener: loopback, one port, one cluster, one exact path, POST
    // and only POST. `path` rather than `prefix`, because a prefix admits
    // everything under it and the router serves more than one thing under
    // `/v1/`.
    assert_eq!(
        values_of(listener, "address"),
        vec![LOOPBACK.to_string()],
        "the {HUGGING_FACE_LISTENER} listener does not bind {LOOPBACK} alone"
    );
    assert_eq!(values_of(listener, "port_value").len(), 1);
    assert_eq!(
        values_of(listener, "cluster"),
        vec![HUGGING_FACE_CLUSTER.to_string()],
        "the {HUGGING_FACE_LISTENER} listener routes to more than one cluster, or to another"
    );
    assert_eq!(
        values_of(listener, "path"),
        vec![CHAT_COMPLETIONS_PATH.to_string()],
        "the {HUGGING_FACE_LISTENER} listener's routes do not match exactly the one path \
         the adapter builds"
    );
    assert_eq!(
        key_count(listener, "prefix"),
        0,
        "the {HUGGING_FACE_LISTENER} listener matches a prefix; the rest of the router's \
         surface under it is then reachable through a port reviewed for one call"
    );
    assert!(
        !without_comments(listener).contains("regex"),
        "the {HUGGING_FACE_LISTENER} listener matches with a regex"
    );
    assert_eq!(
        values_of(listener, "exact"),
        vec!["POST".to_string()],
        "the {HUGGING_FACE_LISTENER} listener no longer restricts the method to POST alone"
    );
    // The converse: no other listener reaches the vendor.
    for (name, body) in &listeners {
        if name == HUGGING_FACE_LISTENER {
            continue;
        }
        assert!(
            !values_of(body, "cluster").contains(&HUGGING_FACE_CLUSTER.to_string())
                && !values_of(body, "host_rewrite_literal").contains(&host.to_string()),
            "the {name} listener reaches {host}; the vendor is reachable from one reviewed \
             port or from anywhere a route was added without one"
        );
    }
    // The cluster's certificate name is matched exactly and against the
    // system trust store — a loose matcher on a proxy carrying a bearer
    // token means a DNS answer is enough to redirect the credential.
    for loose in ["suffix", "contains", "safe_regex", "ignore_case"] {
        assert_eq!(
            key_count(cluster, loose),
            0,
            "{HUGGING_FACE_CLUSTER}'s certificate matcher carries `{loose}`"
        );
    }
    assert_eq!(values_of(cluster, "filename"), vec![CA_BUNDLE.to_string()]);
}

#[test]
fn the_proxy_rewrites_the_authority_to_a_host_on_the_same_allowlist() {
    // The clients send an origin-form request line with `host:` naming
    // loopback, so every request has to have its authority rewritten before it
    // leaves. That rewrite is a second, independent place a host is written
    // down, and a host that appears there and in no cluster is a route to
    // somewhere the previous test would not have seen.
    let bootstrap = bootstrap();
    let rewrites = values_of(&bootstrap, "host_rewrite_literal");
    assert!(
        rewrites.len() >= ALLOWED_UPSTREAMS.len(),
        "only {rewrites:?} routes rewrite the authority, for {} upstreams",
        ALLOWED_UPSTREAMS.len()
    );
    for rewrite in &rewrites {
        assert!(
            ALLOWED_UPSTREAMS.iter().any(|(host, _)| host == rewrite),
            "a route rewrites the authority to {rewrite}, which is not on the \
             allowlist. The rewrite decides which vendor believes it was \
             addressed."
        );
    }
    for (host, provenance) in ALLOWED_UPSTREAMS {
        assert!(
            rewrites.iter().any(|rewrite| rewrite == host),
            "no route rewrites the authority to {host} ({provenance}), so \
             whatever reaches it is addressed to loopback and answered with a \
             404 the operator will read as the vendor's"
        );
    }
}

#[test]
fn every_route_names_a_cluster_the_bootstrap_actually_declares() {
    // An Envoy route pointing at an undeclared cluster is a 503 at run time,
    // not a configuration error at load time, so a typo here is a destination
    // that silently stops working — which on this path reads as the vendor
    // being down.
    let bootstrap = bootstrap();
    let listeners = listeners_block(&bootstrap);
    let clusters = clusters_block(&bootstrap);

    let routed = values_of(&listeners, "cluster");
    assert!(
        routed.len() >= 6,
        "only {routed:?} routes were read; the route blocks have been reshaped"
    );
    assert!(
        !routed.iter().any(|cluster| cluster == "qip-egress"),
        "the routes read as {routed:?}, which includes the proxy's own node \
         name; the listeners block now contains the `node:` block and this \
         check is reading a statistics prefix as a destination"
    );

    let declared: Vec<String> = entries_of(&clusters)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        declared.len(),
        ALLOWED_UPSTREAMS.len(),
        "{declared:?} clusters were read, for {} allowlisted upstreams; the \
         cluster list has been reshaped and this check has stopped checking",
        ALLOWED_UPSTREAMS.len()
    );
    let assigned = values_of(&clusters, "cluster_name");
    assert_eq!(
        assigned, declared,
        "the clusters are named {declared:?} and their load assignments are \
         filed under {assigned:?}. Envoy resolves a route against the first and \
         finds its endpoints under the second."
    );

    for cluster in &routed {
        assert!(
            declared.contains(cluster),
            "a route sends traffic to {cluster}, which no cluster declares. \
             Envoy answers 503 for this rather than refusing to load."
        );
    }
    for cluster in &declared {
        assert!(
            routed.contains(cluster),
            "{cluster} is declared and no route reaches it. An unreachable \
             upstream is a destination somebody believes is in use, and the \
             first sign otherwise is that it has been in use all along."
        );
    }
}

#[test]
fn every_upstream_is_verified_against_a_named_certificate_and_not_merely_a_trusted_one() {
    // The failure this prevents: `validation_context` with a `trusted_ca` and
    // no name matcher checks that the peer holds *a* publicly-trusted
    // certificate, not that it is the vendor. On a proxy whose job is to carry
    // a bearer token onwards, a DNS answer would then be enough to redirect
    // that credential to anyone who can obtain a certificate for a host of
    // their own — which is everyone.
    let bootstrap = bootstrap();
    let clusters = entries_of(&clusters_block(&bootstrap));
    assert_eq!(
        clusters.len(),
        ALLOWED_UPSTREAMS.len(),
        "{:?} clusters were read, for {} allowlisted upstreams; a cluster this \
         check cannot see is a cluster whose certificate it cannot check",
        clusters.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        ALLOWED_UPSTREAMS.len()
    );

    for (name, body) in &clusters {
        let dialled = values_of(body, "address");
        assert_eq!(
            dialled.len(),
            1,
            "the cluster {name} dials {dialled:?}. One cluster, one upstream \
             host: a second endpoint here is a destination that inherits the \
             first one's certificate matcher and is not the host it names."
        );
        let host = dialled[0].as_str();

        assert_eq!(
            key_count(body, "transport_socket"),
            1,
            "the cluster {name} has no upstream TLS context, so it speaks \
             plaintext to a port that expects TLS"
        );
        assert_eq!(
            values_of(body, "sni"),
            vec![host.to_string()],
            "the cluster {name} dials {host} and offers SNI {:?}",
            values_of(body, "sni")
        );
        assert_eq!(
            key_count(body, "validation_context"),
            1,
            "the cluster {name} verifies nothing at all: there is no \
             validation context under its TLS context"
        );
        assert_eq!(
            values_of(body, "filename"),
            vec![CA_BUNDLE.to_string()],
            "the cluster {name} trusts {:?} rather than the image's CA bundle",
            values_of(body, "filename")
        );
        assert_eq!(
            key_count(body, "match_typed_subject_alt_names"),
            1,
            "the cluster {name} has no subject-alternative-name matcher. It \
             now accepts any certificate that chains to a public root, so a DNS \
             answer is enough to redirect the token it carries."
        );
        assert_eq!(
            values_of(body, "san_type"),
            vec!["DNS".to_string()],
            "the cluster {name} matches {:?} rather than a DNS name",
            values_of(body, "san_type")
        );
        assert_eq!(
            values_of(body, "exact"),
            vec![host.to_string()],
            "the cluster {name} dials {host} and accepts a certificate for \
             {:?}. A matcher that names another host is the same hole as no \
             matcher, with a line in the diff saying otherwise.",
            values_of(body, "exact")
        );
        assert!(
            ALLOWED_UPSTREAMS
                .iter()
                .any(|(allowed, _)| *allowed == host),
            "the cluster {name} pins a certificate for {host}, which is not on \
             the allowlist"
        );
        assert_eq!(
            values_of(body, "tls_minimum_protocol_version"),
            vec!["TLSv1_2".to_string()],
            "the cluster {name} accepts {:?}",
            values_of(body, "tls_minimum_protocol_version")
        );
    }
}

#[test]
fn the_proxy_image_is_pinned_by_digest_and_attested_rather_than_exempted() {
    // An admission policy that trusts a tag trusts whoever can push it. This
    // one runs a third-party image, so the tag is documentation and the digest
    // is the only part that names particular bytes — and the digest is what
    // vendor.yml mirrors into the platform's own registry and signs, so the
    // policy keeps one rule and no exemption.
    let (repository, digest) = vendored_envoy();
    assert!(
        repository.contains('/'),
        "{repository} names no registry; an unqualified name resolves to \
         whatever the default registry is"
    );
    assert_eq!(
        digest.len(),
        64,
        "{digest} is {} characters, not the 64 of a sha256 digest — which is \
         what a placeholder nobody replaced looks like",
        digest.len()
    );
    assert!(
        digest.chars().all(|c| c.is_ascii_hexdigit()),
        "{digest} is not hexadecimal, so it is a placeholder rather than a pin"
    );

    // The module runs the mirrored copy at that digest, read from the same
    // file, and never the upstream repository.
    let module = without_comments(&read(PROXY_MODULE));
    assert!(
        module.contains("egress/vendored-images.txt")
            && module.contains("entry[1] == \"vendor/envoy\""),
        "the proxy module no longer reads its image out of {VENDORED}"
    );
    assert!(
        module
            .contains("image         = \"${var.image_prefix}/vendor/envoy@${local.envoy_digest}\""),
        "the proxy module runs an image from somewhere other than the environment's own registry"
    );
    assert!(
        !module.contains("docker.io"),
        "the proxy module names the upstream registry, which Binary Authorization would refuse"
    );

    // No exemption pattern anywhere: the image is admitted because it was
    // signed, not because the policy stopped looking.
    let root = without_comments(&read(ROOT));
    assert!(
        !root.contains("exempt_image_patterns"),
        "the root passes an exemption to Binary Authorization; the proxy is vendored and attested instead"
    );
    let vendor = read(".github/workflows/vendor.yml");
    assert!(
        vendor.contains(VENDORED) && vendor.contains("sign-and-create"),
        "vendor.yml no longer mirrors and attests from {VENDORED}"
    );
}

// ---------------------------------------------------------------------------
// The renderings: where the proxy is, and where it is not
// ---------------------------------------------------------------------------

#[test]
fn every_listener_binds_loopback_and_only_the_destination_listeners_are_exposed() {
    // The port is the destination selector in this design: a process
    // permitted to reach one port has no way to express a wish to reach
    // another upstream. On loopback that only holds if every listener binds
    // loopback — an interface bind is an address every neighbour can reach —
    // and if the ports the module exposes to a workload are exactly the
    // destination listeners, with the health listener kept for the probe.
    let ports = listener_ports();
    let bootstrap = bootstrap();
    for (name, body) in entries_of(&listeners_block(&bootstrap)) {
        // `health` is the one exception and it is named, not matched by a
        // predicate: Cloud Run issues the sidecar's startup probe from
        // outside the container's network namespace, so a loopback bind
        // answers nothing at all and the instance never starts —
        // "STARTUP HTTP probe failed 15 times consecutively ...
        // ERROR_CONNECTION_FAILED", with Envoy up and this file loaded.
        // Every listener that carries traffic still binds loopback.
        let expected = if name == "health" { WILDCARD } else { LOOPBACK };
        assert_eq!(
            values_of(&body, "address"),
            vec![expected.to_string()],
            "the listener {name} binds {:?} and should bind {expected}. A \
             traffic listener on anything but loopback is reachable by \
             whatever shares the network, which on the node is everything in \
             the subnet.",
            values_of(&body, "address")
        );
    }
    let (_, health) = ports
        .iter()
        .find(|(name, _)| name == "health")
        .expect("the bootstrap has a listener named `health`");
    assert_eq!(health, "9900", "the health listener has moved to {health}");

    // The module's own gate says the same, at plan time.
    let module = without_comments(&read(PROXY_MODULE));
    assert!(
        module.contains("listener.address == (name == \"health\" ? \"0.0.0.0\" : \"127.0.0.1\")"),
        "the proxy module no longer refuses, at plan time, a traffic listener \
         bound to an interface address — or it no longer names `health` as \
         the single exception, which is how a second wide bind would arrive \
         without a reviewer seeing it"
    );

    // And what it exposes to a workload is every listener but the health one.
    let outputs = without_comments(&read(PROXY_OUTPUTS));
    assert!(
        outputs.contains("ports            = sort([for listener in values(local.destination_listeners) : tostring(listener.port)])"),
        "the proxy module's sidecar output no longer lists the destination listeners' ports"
    );
    assert!(
        module.contains("if name != \"health\""),
        "the destination listeners include the health listener, so the probe port is published as a destination"
    );

    // The admin interface, which serves /quitquitquit and a config dump, is
    // bound to loopback and is not a listener at all.
    let admin = values_of(&admin_block(&bootstrap), "port_value");
    assert_eq!(
        admin,
        vec!["9901".to_string()],
        "the admin interface binds {admin:?}"
    );
    assert!(
        !ports.iter().any(|(_, port)| port == &admin[0]),
        "a listener shares the admin interface's port"
    );
}

#[test]
fn the_admin_interface_is_bound_to_loopback_and_nothing_else() {
    // `/quitquitquit` needs no authentication, `/config_dump` prints every
    // upstream this proxy knows about, and `/logging` can turn on a log level
    // that prints request headers.
    let bootstrap = bootstrap();
    let admin = admin_block(&bootstrap);
    assert_eq!(
        key_count(&admin, "socket_address"),
        1,
        "the admin block binds {} socket addresses; it is not the block this \
         check thinks it is",
        key_count(&admin, "socket_address")
    );
    let addresses = values_of(&admin, "address");
    assert_eq!(
        addresses,
        vec![LOOPBACK.to_string()],
        "the admin interface binds {addresses:?}"
    );
    let module = without_comments(&read(PROXY_MODULE));
    assert!(
        module.contains("local.admin_binds[0] == \"127.0.0.1\""),
        "the proxy module no longer refuses an admin interface bound off loopback"
    );
}

#[test]
fn no_workload_is_configured_with_an_address_that_leaves_the_instance_except_through_the_proxy() {
    // The whole point of the proxy is defeated by one configuration that sets
    // an adapter's base URL to the vendor. Both failure shapes are checked:
    // an `https` address, which the transport refuses at construction and is
    // therefore merely an outage; and a plaintext address off the instance,
    // which is the one that silently sends a credential across the network in
    // clear text. Every outbound address any workload is configured with must
    // be one of the proxy's own loopback listeners.
    let ports: Vec<String> = listener_ports()
        .into_iter()
        .filter(|(name, _)| name != "health")
        .map(|(_, port)| port)
        .collect();
    assert_eq!(
        ports.len(),
        6,
        "six destination listeners were expected, found {ports:?}"
    );

    let mut checked = 0usize;

    // The catalogue's environment values.
    let catalogue = without_comments(&read(CATALOGUE));
    for line in catalogue.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !key.starts_with("QIP_") {
            continue;
        }
        let value = value.trim().trim_matches('"');
        if !value.starts_with("http://") && !value.starts_with("https://") {
            continue;
        }
        assert!(
            value.starts_with("http://127.0.0.1:")
                && ports
                    .iter()
                    .any(|port| value == format!("http://127.0.0.1:{port}")),
            "catalogue.tf sets {key} to {value}, which is not one of the proxy's loopback listeners"
        );
        checked += 1;
    }

    // The proxy module's own endpoints, which every consumer takes.
    let outputs = without_comments(&read(PROXY_OUTPUTS));
    assert!(
        outputs.contains("name => \"http://127.0.0.1:${listener.port}\""),
        "the proxy module's endpoints are not the loopback listeners"
    );
    checked += 1;

    // The node: its endpoint comes from those outputs, and its module refuses
    // anything that is not loopback.
    let node_variables = read(NODE_VARIABLES);
    assert!(
        node_variables.contains("startswith(endpoint, \"http://127.0.0.1:\")"),
        "the execution node accepts an egress endpoint off the machine"
    );
    // The node is handed the listener's address and, for now, does not write
    // it into node.env: `qip-edge-node` constructs no GCP client, and
    // `manifest_wiring` refuses a variable nothing reads. What is asserted is
    // that the address the script knows is the module's, so the day the line
    // is added it can only name the loopback proxy.
    let startup = read(NODE_STARTUP);
    assert!(
        startup.contains("${egress_endpoint}"),
        "the node's startup script no longer receives the module's egress endpoint"
    );
    assert!(
        !startup.contains("QIP_GCP_ENDPOINT="),
        "node.env sets QIP_GCP_ENDPOINT; if qip-edge-node now reads it, delete this assertion \
         and the comment above the heredoc that says it does not"
    );
    let root = without_comments(&read(ROOT));
    assert!(
        root.contains("egress_endpoints = module.egress_proxy.endpoints"),
        "the root hands the node endpoints from somewhere other than the proxy module"
    );
    checked += 1;

    // The one operator-settable outbound address, the market-data connector,
    // is refused unless it is a loopback listener.
    let root_variables = read(ROOT_VARIABLES);
    assert!(
        root_variables
            .contains("startswith(var.market_data_connector.base_url, \"http://127.0.0.1:\")"),
        "the market-data connector's base URL may point off the instance"
    );
    checked += 1;

    assert!(
        checked >= 3,
        "only {checked} outbound addresses were checked; the scan is not reaching the configuration"
    );
}

#[test]
fn the_egress_proxy_is_attached_to_the_workloads_that_need_it_and_to_nothing_else() {
    // The test that fires when the deployment state changes, in either
    // direction. The previous suite asserted the shape of a proxy committed
    // commented out and never ran; a green run was evidence about a design
    // document. This asserts where the proxy actually is: beside the API and
    // the deep brain, beside the execution node, and — deliberately — not
    // beside the fast brain, for which port 9102 is a route to a model API.
    let catalogue = without_comments(&read(CATALOGUE));
    let entry = |name: &str| -> String {
        let opening = format!("    {name} = {{");
        let start = catalogue
            .find(&opening)
            .unwrap_or_else(|| panic!("catalogue.tf has no `{name}` entry"));
        catalogue[start..]
            .split("\n    }\n")
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let attached = |body: &str| -> bool {
        body.lines().any(|line| {
            let (key, value) = line.split_once('=').unwrap_or(("", ""));
            key.trim() == "egress_proxy" && value.trim() == "true"
        })
    };
    for name in ["api", "deepbrain"] {
        assert!(
            attached(&entry(name)),
            "{name} no longer carries the egress proxy. Its outbound adapters — the audit \
             chain's Cloud Storage writer, the research workload's Vertex and IBM ports — \
             refuse at construction with no address to reach."
        );
    }
    assert!(
        !attached(&entry("fastbrain")),
        "the fast brain carries the egress proxy, which is a route to a language model API"
    );

    // The container is the manifest's since ADR 0036 — gitops.rs's parity
    // test asserts a `qip-egress` sidecar on exactly the workloads whose
    // entry says so, and its own test asserts the workload waits for it and
    // the sidecar probes its health listener. What the module still turns
    // the flag into is the grant that lets the sidecar read its bootstrap.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        sets(&module, "has_egress_sidecar", "var.egress_sidecar != null"),
        "the Cloud Run module no longer keys anything on egress_sidecar"
    );
    let grant = module
        .split("resource \"google_storage_bucket_iam_member\" \"egress_bootstrap\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("the Cloud Run module grants the workload its proxy's bootstrap bucket");
    assert!(
        grant.lines().any(|line| {
            line.split_whitespace().collect::<Vec<_>>().join(" ")
                == "count = local.has_egress_sidecar ? 1 : 0"
        }),
        "the bootstrap-bucket grant is not keyed on the workload carrying the proxy"
    );
    assert!(
        catalogue.contains(
            "egress_sidecar = each.value.egress_proxy ? module.egress_proxy.sidecar : null"
        ),
        "the catalogue no longer passes the proxy module's sidecar to the workloads that asked for it"
    );

    // And the node carries it as a unit the binary's unit requires.
    let startup = read(NODE_STARTUP);
    assert!(
        startup.contains("ExecStart=/usr/local/bin/envoy -c $EGRESS_DIR/envoy.yaml"),
        "the node no longer runs the proxy as a unit"
    );
    assert!(
        startup.contains("Requires=qip-egress.service"),
        "the node's unit no longer requires the proxy, so a node with no outbound path comes up healthy"
    );
    assert!(
        startup.contains("[ -x /usr/local/bin/envoy ] || fail"),
        "the node's image contract no longer includes the proxy binary"
    );
}

#[test]
fn neither_the_fast_path_nor_an_execution_node_can_reach_a_proxy_that_is_not_its_own() {
    // ADR 0008, consequence 3: nothing on the hot path consults a model. The
    // fast brain links `qip-ai` transitively, so what actually stops it
    // calling one is its start-up roster check and the fact that it can reach
    // nothing that serves one. On this runtime the proxy has no network
    // address at all — it is a loopback sidecar or a loopback unit — so the
    // question "who may reach the proxy" is answered by topology: only the
    // workload it is attached to, and the fast brain has none attached.
    let catalogue = without_comments(&read(CATALOGUE));
    assert!(
        catalogue.contains("condition     = !local.cloud_run_catalogue.fastbrain.egress_proxy"),
        "nothing at plan time refuses attaching the proxy to the fast brain"
    );

    // No address. The proxy module creates no service, no load balancer and
    // no endpoint group; a proxy with an address is a proxy something else
    // can be pointed at.
    let module = without_comments(&read(PROXY_MODULE));
    for resource in [
        "google_cloud_run_v2_service",
        "google_compute_forwarding_rule",
        "google_compute_region_network_endpoint_group",
        "google_compute_backend_service",
        "google_compute_address",
    ] {
        assert!(
            !module.contains(resource),
            "the proxy module creates a {resource}, which gives the proxy an address something else can reach"
        );
    }

    // The node's proxy is its own: no firewall rule to a proxy exists,
    // because there is no address to permit, and the endpoint must be loopback.
    let node = without_comments(&read(NODE_MODULE));
    assert!(
        !node.contains("resource \"google_compute_firewall\" \"egress_proxy\""),
        "the execution node has an egress rule to a proxy, which means the proxy has an address off the machine"
    );
    assert!(
        read(NODE_VARIABLES).contains("startswith(endpoint, \"http://127.0.0.1:\")"),
        "the node accepts an egress endpoint that is not on the machine"
    );
}

#[test]
fn the_proxy_offers_no_route_to_a_venue_and_cannot_carry_an_order() {
    // The shipped autonomy ceiling is paper trading, and a route from this
    // platform to a venue is a live-order submission path whatever the ceiling
    // says. `qip_brokers::rest` asks for a TLS-terminating proxy in the same
    // words every other adapter does, and it deliberately does not get one.
    let edge = read("backend/crates/apps/qip-edge-node/src/main.rs");
    for variable in ["QIP_VENUE_FEED_ENDPOINT", "QIP_DROP_COPY_ENDPOINT"] {
        assert!(
            edge.contains(variable),
            "{variable} is not named by the edge node any more, so this test is \
             checking for a venue path that has been renamed"
        );
    }

    let bootstrap = without_comments(&bootstrap());
    for variable in [
        "QIP_VENUE_FEED_ENDPOINT",
        "QIP_DROP_COPY_ENDPOINT",
        "QIP_SANDBOX_VENUE_ENDPOINT",
    ] {
        assert!(
            !bootstrap.contains(variable),
            "the egress proxy configures {variable}. Adding a venue to this \
             file is a decision for whoever owns the execution path, taken \
             with docs/operations/enabling-live-trading.md open."
        );
    }
    for (host, _) in ALLOWED_UPSTREAMS {
        assert!(
            !host.contains("venue") && !host.contains("broker") && !host.contains("exchange"),
            "{host} is on the allowlist and names a venue"
        );
    }
    // And the module refuses one at plan time, before a bootstrap can be
    // widened to match.
    let variables = read(PROXY_VARIABLES);
    assert!(
        variables.contains("!strcontains(host, \"venue\") && !strcontains(host, \"broker\") && !strcontains(host, \"exchange\")"),
        "the proxy module no longer refuses an allowlist entry naming a venue"
    );
}

#[test]
fn the_proxy_holds_no_credential_and_no_identity_of_its_own() {
    // This process terminates TLS, so every token its clients send passes
    // through it in clear text. That makes it the highest-value target on the
    // instance, and the mitigation is that compromising it yields nothing but
    // the traffic already flowing: no mounted secret, no environment, no
    // service account of its own, and no shell.
    // The Cloud Run sidecar is the manifest's since ADR 0036, and gitops.rs's
    // `the_egress_sidecar_in_every_manifest_holds_no_credential_and_the_workload_waits_for_it`
    // asserts, on every RunService that carries one, no environment, no
    // mount but its bootstrap, and the vendored image. Here: the module
    // renders no sidecar of its own, so there is no second copy to drift.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        !module.contains("containers {"),
        "modules/cloudrun renders a container again; the manifest is the one place the \
         sidecar is declared"
    );

    // No identity in the proxy module either.
    let proxy = without_comments(&read(PROXY_MODULE));
    assert!(
        !proxy.contains("google_service_account"),
        "the proxy module creates an identity"
    );

    // The node's unit: its own unprivileged user, no environment file, no
    // secret path.
    let startup = read(NODE_STARTUP);
    let unit = startup
        .split("cat >\"$EGRESS_UNIT\" <<EOF")
        .nth(1)
        .and_then(|rest| rest.split("\nEOF\n").next())
        .expect("the startup script writes the proxy unit");
    assert!(
        unit.contains("User=qip-egress"),
        "the proxy unit runs as something other than its own user"
    );
    assert!(
        !unit.contains("EnvironmentFile") && !unit.contains("$RUN_DIR"),
        "the proxy unit is given the node's configuration or its secrets"
    );
    assert!(
        unit.contains("NoNewPrivileges=yes"),
        "the proxy unit may gain privileges"
    );

    // And the image has no shell: the vendored line is the distroless one.
    let (repository, _) = vendored_envoy();
    let list = read(VENDORED);
    assert!(
        list.lines()
            .any(|line| line.contains(&repository) && line.contains("distroless")),
        "the vendored Envoy is not the distroless image; a shell on the most exposed process is a shell"
    );
}

#[test]
fn no_zone_or_node_has_an_egress_rule_that_bypasses_the_proxy() {
    // The proxy is only a control if it is the only way out. Every egress
    // allow rule the trust zones and the node write is therefore checked
    // against the shapes this platform permits: the restricted Google API
    // range, a declared zone's range, the central plane, a node's own venue
    // range, and an allowlist entry whose purpose its zone may hold. A rule
    // naming anything else is a route to the internet that does not pass
    // through the proxy.
    let permitted_destinations = [
        "[var.google_apis_range]",
        "[lookup(local.zone_cidr, each.value.to, \"192.0.2.0/32\")]",
        "[each.value.cidr]",
        "var.central_plane_ranges",
    ];
    let mut allows = 0usize;
    for path in [TRUST_ZONES_MODULE, NODE_MODULE] {
        let module = without_comments(&read(path));
        for block in module
            .split("resource \"google_compute_firewall\" \"")
            .skip(1)
        {
            let (name, body) = block.split_once('"').unwrap_or((block, ""));
            let body = body.split("\nresource ").next().unwrap_or(body);
            if !body.contains("direction = \"EGRESS\"") || !body.contains("allow {") {
                continue;
            }
            allows += 1;
            let destination = body
                .lines()
                .find_map(|line| line.trim().strip_prefix("destination_ranges = "))
                .unwrap_or_else(|| panic!("{path}: the egress rule `{name}` names no destination"));
            assert!(
                permitted_destinations.contains(&destination),
                "{path}: the egress rule `{name}` permits {destination}, which is not one of the \
                 shapes this platform allows out. A workload that can reach the internet can \
                 exfiltrate the position history it was trained on."
            );
        }
    }
    assert!(
        allows >= 5,
        "only {allows} egress allow rules were read; the walk is not reaching the modules"
    );

    // The external allowlist is bounded by zone and purpose at plan time.
    let zones = without_comments(&read(TRUST_ZONES_MODULE));
    assert!(
        zones.contains("contains(lookup(local.sanctioned_egress_purposes, each.value.zone, []), each.value.purpose)"),
        "an external-egress entry may name a purpose its zone does not hold"
    );
}

#[test]
fn every_rendering_derives_from_the_single_committed_bootstrap() {
    // One file, every rendering. A sidecar per workload and a unit per node
    // is N copies of the allowlist and N places for one to drift; the
    // mitigation is that none of them is a copy — each is the committed file,
    // read at plan time — and this is the check that it stays that way.
    let proxy = without_comments(&read(PROXY_MODULE));
    assert!(
        proxy.contains("bootstrap = file(\"${path.module}/../../../egress/envoy.yaml\")"),
        "the proxy module no longer reads the committed bootstrap"
    );
    let root = without_comments(&read(ROOT));
    assert!(
        root.contains("egress_bootstrap = file(\"${path.module}/../egress/envoy.yaml\")"),
        "the root hands the node a bootstrap from somewhere other than the committed file"
    );
    let startup = read(NODE_STARTUP);
    assert!(
        startup.contains("${egress_bootstrap}"),
        "the node's startup script no longer renders the bootstrap it was given"
    );

    // And there is no second bootstrap anywhere under infrastructure/.
    let mut bootstraps = Vec::new();
    for extension in ["yaml", "yml", "tf", "tftpl"] {
        for path in files_with_extension("infrastructure", extension) {
            let content = std::fs::read_to_string(&path).expect("readable");
            if content.contains("\nstatic_resources:\n")
                || content.contains("\n    static_resources:\n")
            {
                bootstraps.push(path.display().to_string());
            }
        }
    }
    assert_eq!(
        bootstraps,
        vec![
            qip_acceptance::repository_root()
                .join(BOOTSTRAP)
                .display()
                .to_string()
        ],
        "more than one Envoy bootstrap exists under infrastructure/: {bootstraps:?}. Two copies of an allowlist is two allowlists."
    );
}

#[test]
fn the_vendor_workflow_attests_every_platform_manifest_and_not_only_the_index() {
    // Cloud Run resolves a multi-arch index to the manifest for the platform
    // it runs and asks Binary Authorization about *that* digest. GKE asked
    // about the digest in the pod spec, which is the index, so attesting the
    // index alone was enough there and is not enough here. The first Cloud
    // Run apply of the migration was refused on it:
    //
    //   Image .../vendor/envoy@sha256:c8fecdf5... denied by attestor
    //   qip-dev-build: No attestations found that were valid and signed by a
    //   key trusted by the attestor
    //
    // — a digest that appears in no committed file, because it is the
    // linux/amd64 child of the index digest that does. A workflow that signs
    // only the index leaves the platform admitting nothing it vendored, and
    // the failure surfaces at apply rather than at review.
    let vendor = read(".github/workflows/vendor.yml");

    // Premise: the workflow still mirrors by digest and still signs. Without
    // both, what follows is a test about a workflow that does nothing.
    assert!(
        vendor.contains("crane copy") && vendor.contains("binauthz attestations sign-and-create"),
        "vendor.yml no longer mirrors and signs, so this guards nothing"
    );

    // The children are read from the mirrored image's own manifest rather
    // than from a list, and the index is signed alongside them.
    assert!(
        vendor.contains("crane manifest") && vendor.contains(".manifests[]?.digest"),
        "vendor.yml does not read the platform manifests out of the index, so \
         a multi-arch image is attested only at its index digest and Cloud \
         Run refuses every revision that runs it"
    );

    // And the signing call must be inside the loop over those digests, not
    // beside it: a loop that computes the children and then signs one fixed
    // reference is the same gap wearing a for statement.
    let signing = vendor
        .split_once("for digest in")
        .expect("vendor.yml signs inside a loop over the digests")
        .1;
    let sign_at = signing
        .find("sign-and-create")
        .expect("the loop body signs");
    let loop_end = signing.find("\n            done").unwrap_or(signing.len());
    assert!(
        sign_at < loop_end,
        "vendor.yml signs outside the loop over the index and its platform \
         manifests, so only one of them is attested"
    );
    // Read from the signing call itself, not from the loop body around it.
    // The first draft asserted over the whole body and a mutation that made
    // sign-and-create name the fixed index reference passed anyway, because
    // the idempotency lookup two lines above still named the loop variable.
    assert!(
        signing[sign_at..loop_end].contains("--artifact-url \"${artifact}\""),
        "sign-and-create names something other than the digest the loop is \
         on, so every iteration attests the same reference"
    );
}

#[test]
fn the_health_listener_is_the_only_wide_bind_and_it_forwards_nowhere() {
    // The trade this file makes, held in one place. `health` binds 0.0.0.0
    // because Cloud Run probes it from outside the container's namespace, and
    // that is the only listener here reachable by anything but the co-located
    // workload. What it may answer with is therefore the whole of the
    // exposure: a constant, and no route out.
    //
    // `modules/egress-proxy` cannot check this — it parses each listener's
    // name, address and port and never sees a body — so the property lives
    // here rather than as an unvalidatable regex in the plan path.
    let bootstrap = bootstrap();
    let listeners = entries_of(&listeners_block(&bootstrap));

    // Premise: there are several listeners and one is `health`, so what
    // follows compares a real exception against real neighbours.
    assert!(
        listeners.len() >= 6,
        "only {} listener(s) were read out of the bootstrap; the block has \
         been reshaped and this test is comparing nothing",
        listeners.len()
    );
    let (_, health) = listeners
        .iter()
        .find(|(name, _)| name == "health")
        .expect("the bootstrap has a listener named `health`");

    // Exactly one wide bind, and it is this one.
    let wide: Vec<&String> = listeners
        .iter()
        .filter(|(_, body)| values_of(body, "address") == vec![WILDCARD.to_string()])
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        wide,
        vec!["health"],
        "the listeners bound to {WILDCARD} are {wide:?}. Exactly one may be, \
         and it is the one that answers a probe with a constant."
    );

    // And it forwards to nothing. A `cluster` here would be an
    // unauthenticated way out of the network at the one address opened wider.
    // Comments stripped first: an entry's body runs to the next `- name:`,
    // so it carries the prose introducing the listener after it — and the
    // paragraph above `gcp` explains that it is "the only listener here with
    // more than one cluster behind it". Reading that as configuration failed
    // this assertion on a bootstrap that was correct.
    let configured = without_comments(health);
    assert!(
        !configured.contains("cluster"),
        "the health listener names a cluster: it is the only listener bound \
         to an interface address, and it may answer with a constant and \
         nothing else.\n{configured}"
    );
    assert!(
        health.contains("direct_response"),
        "the health listener no longer answers with a direct response, so \
         what the wide bind reaches is no longer a fixed reply.\n{health}"
    );
}

#[test]
fn the_node_module_admits_the_same_named_exception_it_shares_the_bootstrap_with() {
    // Run 23 of infra.yml found this the hard way: fixing
    // `modules/egress-proxy`'s bind gate was not enough, because
    // `modules/execution-node` reads the same committed bootstrap and
    // carried its own validation refusing any 0.0.0.0 listener outright —
    //
    //   Error: Invalid value for variable
    //   The egress bootstrap binds a listener to 0.0.0.0. On the node every
    //   listener is loopback...
    //   This was checked by the validation rule at
    //   modules/execution-node/variables.tf:276,3-13.
    //
    // — on `main.tf line 506`, which passes this same file to both modules.
    // One file, two independent gates; fixing one and not the other still
    // refuses the plan.
    let node_variables = read(NODE_VARIABLES);

    // Premise: the module still takes the bootstrap as a string variable
    // with validations on it, so this is a test about which validations.
    assert!(
        node_variables.contains("variable \"egress_bootstrap\""),
        "modules/execution-node no longer declares egress_bootstrap; this \
         test's premise needs rewriting"
    );

    // It no longer refuses 0.0.0.0 outright...
    assert!(
        !node_variables.contains("!strcontains(var.egress_bootstrap, \"address: 0.0.0.0\")"),
        "the node module still refuses any 0.0.0.0 listener outright, which \
         refuses the health listener's own bootstrap and blocks every apply"
    );
    // ...and the replacement names `health` as the one exception, matching
    // the shape `modules/egress-proxy` uses for the same file.
    assert!(
        node_variables.contains("line[0] == \"health\""),
        "the node module's replacement validation does not name `health` as \
         the exception, so either nothing is admitted (blocking every \
         apply) or every listener is (admitting a real mistake)"
    );
}

#[test]
fn the_node_variable_s_regex_uses_escapes_hcl_actually_accepts() {
    // Run 24 of infra.yml found this: the fix above swapped a blanket
    // refusal for a `regexall` over the bootstrap text, but HCL's own
    // string-escaping rules are not the regex engine's. `\n` is a real HCL
    // escape (a literal newline) and passed straight through, but `\s` and
    // `\{` are not on HCL's short list of valid escapes — `terraform init`
    // refused the module outright:
    //
    //   Error: Invalid escape sequence
    //   The symbol "s" is not a valid escape sequence selector.
    //
    // A backslash meant to reach the regex engine has to survive HCL's own
    // parse first, which means writing it doubled (`\\s`, `\\{`) so HCL
    // emits one literal backslash for `regexall` to see. A parenthesised
    // capture (`\(`) is not this class — the fixture below asserts on the
    // two cases the previous fix actually broke, not on every metacharacter.
    let node_variables = read(NODE_VARIABLES);
    let regexall_line = node_variables
        .lines()
        .find(|line| line.contains("for line in regexall("))
        .expect(
            "the validation's regexall call moved or was rewritten; this \
             test's premise needs rewriting",
        );

    assert!(
        regexall_line.contains("\\\\s+address"),
        "the regex uses a bare \\s for whitespace, which HCL parses as an \
         invalid escape sequence and refuses at `terraform init` before any \
         plan runs — every apply is blocked, not just a bad value admitted.\n\
         {regexall_line}"
    );
    assert!(
        regexall_line.contains("\\\\{ address"),
        "the regex uses a bare \\{{ for the literal brace, which HCL parses \
         as an invalid escape sequence for the same reason.\n{regexall_line}"
    );
}

/// `ignore_changes` names a static list, and nothing computed from an input.
///
/// This is not hypothetical. ADR 0028 decision 3 said a vendored workload
/// would skip the rule, and the module was written to do exactly that:
///
///   ignore_changes = var.source == "vendored" ? [] : [template[0].containers[0].image]
///
/// Terraform refuses that outright — "A static list expression is required" —
/// so `terraform validate` failed on every commit carrying it, the deploy
/// gate correctly refused to ship an unvalidated tree, and nothing reached
/// dev until it was made uniform. The rule is now the same for both sources
/// and the cost to a vendored workload is written at the rule itself.
///
/// A whole-file scan, not a scan of one known line: a second `lifecycle`
/// block added later with the same conditional shape would break the plan the
/// same way, and this test is the thing that has to notice.
#[test]
fn no_terraform_lifecycle_rule_ignores_an_image_because_terraform_no_longer_names_one() {
    // `modules/cloudrun` once ignored `template[0].containers[0].image` so an
    // apply would not roll a service back to the digest tfvars still named,
    // and this test kept that rule a static list terraform would accept.
    // ADR 0036 takes the service, and with it the image, out of Terraform:
    // the manifest names the digest and Kargo moves it. The property that
    // survives is the one the rule existed for — no Terraform resource names
    // an image a promotion also moves — and its shape now is that no
    // lifecycle rule anywhere ignores an image, because nothing declares one
    // to ignore. A rule reappearing is the first sign a service has come
    // back into Terraform beside its manifest.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        !module.contains("google_cloud_run_v2_service") && !module.contains("ignore_changes"),
        "{CLOUD_RUN_MODULE} declares a Cloud Run service or a lifecycle rule again; the \
         manifest is the one writer of the image"
    );
    let mut scanned = 0usize;
    for path in qip_acceptance::files_with_extension("infrastructure/terraform", "tf") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        scanned += 1;
        for line in content
            .lines()
            .filter(|line| line.trim_start().starts_with("ignore_changes"))
        {
            assert!(
                !line.contains("image"),
                "{} ignores changes to an image, so a Terraform resource names one a promotion \
                 also moves:\n{line}",
                path.display()
            );
        }
    }
    assert!(scanned >= 20, "only {scanned} Terraform files were scanned");
}
