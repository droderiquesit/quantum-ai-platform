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
const ALLOWED_UPSTREAMS: [(&str, &str); 5] = [
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
];

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
/// reading "the listeners" that ran on past `clusters:` would read the five
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
        5,
        "{:?} listeners were read; there are four destination listeners and \
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
fn vendored_envoy() -> (String, String) {
    let list = read(VENDORED);
    let entries: Vec<&str> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "{VENDORED} carries {entries:?}; one image is vendored on this platform and a second is a decision"
    );
    let fields: Vec<&str> = entries[0].split_whitespace().collect();
    assert_eq!(
        fields.len(),
        3,
        "{VENDORED} line is not `<source@digest> <dest> <tag>`: {entries:?}"
    );
    assert_eq!(
        fields[1], "vendor/envoy",
        "the vendored image is not the egress proxy"
    );
    let (repository, digest) = fields[0]
        .split_once("@sha256:")
        .unwrap_or_else(|| panic!("{} is not pinned by digest", fields[0]));
    (repository.to_string(), digest.to_string())
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
    // six binds and five upstreams is eleven, and fewer than ten means the
    // socket addresses have moved and this check is looking at nothing.
    let addresses = values_of(&bootstrap, "address");
    assert!(
        addresses.len() >= 10,
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
        bound.len() >= 5,
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
        routed.len() >= 5,
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
        4,
        "four destination listeners were expected, found {ports:?}"
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

    // The module turns the flag into a container the workload waits for.
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    assert!(
        module.contains("for_each = local.has_egress_sidecar ? [var.egress_sidecar] : []"),
        "the Cloud Run module no longer renders the sidecar from egress_sidecar"
    );
    assert!(
        module.contains("depends_on = local.has_egress_sidecar ? [local.sidecar_name] : null"),
        "the workload container no longer waits for the proxy, so its first outbound call after a cold start hits a sidecar that is not listening"
    );
    assert!(
        module.contains("path = \"/healthz\"")
            && module.contains("port = containers.value.health_port"),
        "the sidecar's startup probe no longer hits the health listener"
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
    let module = without_comments(&read(CLOUD_RUN_MODULE));
    let sidecar = module
        .split("for_each = local.has_egress_sidecar ? [var.egress_sidecar] : []")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("the Cloud Run module renders the sidecar");
    // Premise: this really is the sidecar's block, with its image and probe.
    assert!(
        sidecar.contains("image = containers.value.image") && sidecar.contains("startup_probe {"),
        "the sidecar block has been reshaped; every absence below is vacuous"
    );
    for (marker, why) in [
        (
            "secret",
            "a mounted secret is a credential this process has no use for and an attacker does",
        ),
        (
            "env {",
            "an environment value is one more thing in /proc/<pid>/environ on the most exposed process",
        ),
        (
            "service_account",
            "an identity of its own would give the one process that talks to the internet a principal in the project",
        ),
    ] {
        assert!(
            !sidecar.contains(marker),
            "the egress sidecar carries `{marker}`: {why}"
        );
    }
    assert!(
        sidecar.contains("name       = \"egress-bootstrap\""),
        "the sidecar mounts something other than its bootstrap"
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
        listeners.len() >= 5,
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
