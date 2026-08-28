//! What the egress proxy is allowed to reach, and what it is not.
//!
//! `infrastructure/kubernetes/base/egress.yaml` is the only thing in this
//! repository that describes a path from a pod to the public internet. The
//! adapters that need it — `qip_storage::gcp`, `qip_training::vertex`,
//! `qip_quantum::provider` — refuse `https` by name and therefore hand a
//! plaintext credential to whatever that file points them at. Everything below
//! is a check on the one file that decides where that goes.
//!
//! These are string checks on YAML rather than a parse, matching the idiom in
//! `infrastructure.rs`: they cannot understand the configuration, so they can
//! be fooled by somebody rewriting it. What they can do is fail when a security
//! property is deleted, which is the change that actually happens.
//!
//! Every test asserts its own premise before its conclusion. A check that reads
//! a file, filters it to nothing and then finds no violation is a check that
//! passes after somebody deletes the thing it was checking, and this file
//! covers a control whose absence is silent — a proxy that reaches one host too
//! many produces no error anywhere.

use qip_acceptance::{files_with_extension, read};

/// The manifest under test.
const MANIFEST: &str = "infrastructure/kubernetes/base/egress.yaml";

/// The in-cluster name every adapter address must resolve through.
const PROXY_AUTHORITY: &str = "qip-egress.qip.svc.cluster.local";

/// The vendor hosts the proxy may reach, and the adapter each was derived from.
///
/// Written here as a second copy of the manifest's list on purpose. The point
/// of an allowlist is that widening it is deliberate, and a test that read the
/// list out of the file it is checking would agree with every widening. Adding
/// a destination is therefore two edits and a reviewer who sees both.
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
        "qip_training::vertex documents {region}-aiplatform.googleapis.com, and \
         every file in infrastructure/environments sets region = europe-west2",
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

/// The Envoy bootstrap, as the text of the block scalar that carries it.
///
/// Sliced out of the manifest rather than parsed, and bounded by the document
/// separator that follows it, so that a bootstrap moved into a second ConfigMap
/// makes this return nothing and every caller's premise assertion fire.
fn bootstrap() -> String {
    let manifest = read(MANIFEST);
    let marker = "  envoy.yaml: |\n";
    let start = manifest
        .find(marker)
        .expect("egress.yaml carries the Envoy bootstrap under `envoy.yaml: |`");
    let body = manifest[start + marker.len()..]
        .split("\n---\n")
        .next()
        .expect("the ConfigMap document ends")
        .to_string();
    assert!(
        body.lines().count() > 100,
        "only {} lines of bootstrap were read; the block scalar has moved and \
         every check in this file is now looking at nothing",
        body.lines().count()
    );
    body
}

/// The values of every `key:` in a block of configuration, comments excluded.
///
/// Trailing `}` and quotes are trimmed because the manifest writes some of
/// these inside flow mappings — `matcher: { exact: host }` — and a check that
/// only matched the block form would silently stop matching the day one was
/// reformatted.
fn values_of(text: &str, key: &str) -> Vec<String> {
    without_comments(text)
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix(&format!("{key}:"))
                .map(|value| value.trim().trim_matches(['"', '}', ' ']).to_string())
        })
        .filter(|value| !value.is_empty())
        .collect()
}

/// The image the proxy pod runs.
///
/// Read out of the commented-out Deployment at the foot of the manifest, which
/// is where it lives until the four out-of-band edits named in that comment
/// have landed. Commented out is not the same as unreviewed: the digest is the
/// thing an operator will uncomment, and it is checked now rather than on the
/// day somebody is in a hurry.
fn proxy_image() -> String {
    let manifest = read(MANIFEST);
    let line = manifest
        .lines()
        .map(|line| line.trim_start_matches(['#', ' ']))
        .find(|line| line.starts_with("image:"))
        .expect("egress.yaml declares the proxy image");
    line.trim_start_matches("image:").trim().to_string()
}

#[test]
fn the_egress_proxy_dials_only_the_vendor_hosts_the_adapters_named() {
    // The exfiltration question. A forward proxy that will connect anywhere is
    // a route out of a cluster holding trading credentials, and the thing that
    // makes this one not that is the set of clusters declared in its bootstrap.
    let bootstrap = bootstrap();

    // Premise: the file really does declare upstream addresses. `socket_address`
    // also carries the listener bind addresses, so the upstreams are the ones
    // that are not a bind address — filtered here by taking the `address:`
    // values that are hostnames rather than 0.0.0.0 or loopback.
    let addresses = values_of(&bootstrap, "address");
    assert!(
        addresses.len() >= 10,
        "only {addresses:?} were read out of the bootstrap; the socket_address \
         blocks have been reshaped and this check is filtering an empty list"
    );
    let upstreams: Vec<String> = addresses
        .into_iter()
        .filter(|address| address != "0.0.0.0" && address != "127.0.0.1")
        .collect();
    assert_eq!(
        upstreams.len(),
        ALLOWED_UPSTREAMS.len(),
        "the bootstrap dials {upstreams:?}; the allowlist this test carries is \
         {:?}. A destination added in one place and not the other is a \
         destination nobody reviewed.",
        ALLOWED_UPSTREAMS.map(|(host, _)| host)
    );

    for upstream in &upstreams {
        assert!(
            ALLOWED_UPSTREAMS.iter().any(|(host, _)| host == upstream),
            "the proxy dials {upstream}, which no adapter in this workspace \
             names. An allowlist entry that was guessed is an allowlist entry \
             that is wrong, and this one is a route to the internet."
        );
        // A wildcard here would not widen Envoy's DNS lookup — it would simply
        // fail — but it would defeat every reader of this file, which is the
        // control that actually holds.
        assert!(
            !upstream.contains('*'),
            "{upstream} carries a wildcard; the destination list stops being a \
             list the moment one entry matches a family of hosts"
        );
    }

    for (host, provenance) in ALLOWED_UPSTREAMS {
        assert!(
            upstreams.iter().any(|upstream| upstream == host),
            "{host} is on this test's allowlist ({provenance}) and the \
             bootstrap does not dial it. Either the adapter stopped needing it \
             — in which case delete both — or the destination was dropped and \
             that adapter now fails with a 404 from a proxy nobody suspects."
        );
    }
}

#[test]
fn the_proxy_rewrites_the_authority_to_a_host_on_the_same_allowlist() {
    // The clients send an origin-form request line with `host:` naming the
    // proxy, so every request has to have its authority rewritten before it
    // leaves. That rewrite is a second, independent place a host is written
    // down, and a host that appears there and in no cluster is a route to
    // somewhere the previous test would not have seen.
    let bootstrap = bootstrap();
    let rewrites = values_of(&bootstrap, "host_rewrite_literal");
    assert!(
        !rewrites.is_empty(),
        "no route rewrites the authority. Every upstream here is a public \
         vendor that rejects a request whose `host:` header names a cluster \
         service, so this is also the check that the proxy works at all."
    );
    for rewrite in &rewrites {
        assert!(
            ALLOWED_UPSTREAMS.iter().any(|(host, _)| host == rewrite),
            "a route rewrites the authority to {rewrite}, which is not on the \
             allowlist. The rewrite decides which vendor believes it was \
             addressed."
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
    let routed = values_of(&bootstrap, "cluster");
    let declared = values_of(&bootstrap, "cluster_name");
    assert!(
        routed.len() >= 5,
        "only {routed:?} routes were read; the route blocks have been reshaped"
    );
    assert!(
        declared.len() >= 5,
        "only {declared:?} clusters were read; `load_assignment` has been \
         reshaped and this check has stopped checking"
    );
    for cluster in &routed {
        assert!(
            declared.contains(cluster),
            "a route sends traffic to {cluster}, which no `load_assignment` \
             declares. Envoy answers 503 for this rather than refusing to load."
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
    let snis = values_of(&bootstrap, "sni");
    let matched = values_of(&bootstrap, "exact");
    let cas = values_of(&bootstrap, "trusted_ca");
    let minimums = values_of(&bootstrap, "tls_minimum_protocol_version");

    assert_eq!(
        snis.len(),
        ALLOWED_UPSTREAMS.len(),
        "{snis:?} carry an SNI. An upstream with no SNI reaches a shared \
         front end that answers with the wrong certificate, and the connection \
         fails in a way that reads as the vendor being broken."
    );
    assert_eq!(
        matched.len(),
        ALLOWED_UPSTREAMS.len(),
        "{matched:?} subject-alternative-name matchers were found, for {} \
         upstreams. An upstream with no matcher accepts any certificate that \
         chains to a public root.",
        ALLOWED_UPSTREAMS.len()
    );
    assert_eq!(
        cas.len(),
        ALLOWED_UPSTREAMS.len(),
        "{cas:?} trusted CA bundles were found. An upstream TLS context with \
         no validation context verifies nothing at all."
    );

    for sni in &snis {
        assert!(
            ALLOWED_UPSTREAMS.iter().any(|(host, _)| host == sni),
            "the proxy offers SNI {sni}, which is not an allowlisted host"
        );
        assert!(
            matched.contains(sni),
            "the proxy offers SNI {sni} and accepts a certificate that does \
             not have to carry that name"
        );
    }
    for minimum in &minimums {
        assert_eq!(
            minimum, "TLSv1_2",
            "an upstream accepts {minimum}. TLS 1.0 and 1.1 are the versions \
             whose weaknesses are exploitable by whoever is between this pod \
             and the vendor, which on this path is a NAT gateway and the \
             public internet."
        );
    }
    assert_eq!(
        minimums.len(),
        ALLOWED_UPSTREAMS.len(),
        "{minimums:?} minimum protocol versions were set, for {} upstreams",
        ALLOWED_UPSTREAMS.len()
    );
}

#[test]
fn the_proxy_image_is_pinned_by_digest_rather_than_by_a_tag() {
    // An admission policy that trusts a tag trusts whoever can push it. This
    // one runs a third-party image, so the tag is documentation and the digest
    // is the only part that names particular bytes.
    let image = proxy_image();
    assert!(
        !image.is_empty(),
        "no image was read out of the manifest at all"
    );
    let (repository, digest) = image
        .split_once("@sha256:")
        .unwrap_or_else(|| panic!("{image} is not pinned by digest"));
    assert!(
        repository.contains('/'),
        "{repository} names no registry; an unqualified name resolves to \
         whatever the node's default registry is"
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
}

#[test]
fn every_address_the_manifests_hand_a_workload_points_at_the_proxy() {
    // The whole point of the proxy is defeated by one manifest that sets an
    // adapter's base URL to the vendor. Both failure shapes are checked: an
    // `https` address, which the transport refuses at construction and is
    // therefore merely an outage; and a plaintext address at a public host,
    // which is the one that silently sends a credential across the internet in
    // clear text.
    let mut checked = 0usize;
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = std::fs::read_to_string(&path).expect("readable");
        for line in without_comments(&content).lines() {
            let trimmed = line.trim();
            let Some(index) = trimmed.find("http") else {
                continue;
            };
            let address = trimmed[index..].trim_matches(['"', ' ']);
            if !address.starts_with("http://") && !address.starts_with("https://") {
                continue;
            }
            assert!(
                !address.starts_with("https://"),
                "{} configures {address}. `qip_transport::http` refuses `https` \
                 by name, so this is a pod that will not start rather than one \
                 that works.",
                path.display()
            );
            assert!(
                address.contains(PROXY_AUTHORITY),
                "{} configures {address}, which is not the egress proxy. An \
                 adapter pointed straight at a vendor over plaintext sends its \
                 credential across the internet in clear text.",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 4,
        "only {checked} addresses were checked, and the endpoint config map \
         alone declares four. The scan is not reaching the manifests."
    );
}

#[test]
fn every_port_the_proxy_publishes_is_a_listener_and_the_health_port_is_not_published() {
    // The port is the destination selector in this design: a workload
    // permitted to reach one port has no way to express a wish to reach
    // another upstream. That only holds if the published ports and the
    // listeners are the same set — a Service port with no listener behind it
    // is a connection that hangs, and a listener with no Service in front is a
    // destination reachable by pod IP and constrained by nothing anybody read.
    let manifest = read(MANIFEST);
    let service = manifest
        .split("kind: Service\n")
        .nth(1)
        .expect("egress.yaml declares a Service")
        .split("\n---\n")
        .next()
        .expect("the Service document ends")
        .to_string();

    let published: Vec<String> = values_of(&service, "port");
    assert!(
        published.len() >= 4,
        "only {published:?} ports are published; the Service has been reshaped"
    );

    let listening: Vec<String> = values_of(&bootstrap(), "port_value");
    assert!(
        listening.len() >= 5,
        "only {listening:?} listener ports were read out of the bootstrap"
    );

    for port in &published {
        assert!(
            listening.contains(port),
            "the Service publishes {port} and the bootstrap has no listener on \
             it. A Service port with nothing behind it is a connection that \
             hangs, which is the failure that takes longest to diagnose."
        );
    }
    // The health listener answers a probe and forwards to no cluster. It is
    // deliberately absent from the Service: a probe endpoint on a Service is a
    // thing that ends up behind a load balancer.
    assert!(
        listening.contains(&"9900".to_string()),
        "the bootstrap has no health listener, so the pod's probes have \
         nothing to hit that is not the admin interface"
    );
    assert!(
        !published.contains(&"9900".to_string()),
        "the Service publishes the health listener"
    );
    // And the admin interface, which serves /quitquitquit and a config dump, is
    // bound to loopback and therefore cannot be published even by accident.
    assert!(
        listening.contains(&"9901".to_string()),
        "the admin port was not read; this check is looking at the wrong block"
    );
    assert!(
        !published.contains(&"9901".to_string()),
        "the Service publishes the Envoy admin interface, which is an \
         unauthenticated stop button and a dump of every upstream"
    );
}

#[test]
fn the_admin_interface_is_bound_to_loopback_and_nothing_else() {
    // `/quitquitquit` needs no authentication, `/config_dump` prints every
    // upstream this proxy knows about, and `/logging` can turn on a log level
    // that prints request headers. Bound to the pod address, all three are
    // reachable by anything the ingress policy admits.
    let bootstrap = bootstrap();
    let admin = bootstrap
        .split("\n    admin:\n")
        .nth(1)
        .expect("the bootstrap declares an admin interface")
        .split("\n    static_resources:")
        .next()
        .expect("the admin block ends before the resources")
        .to_string();
    let addresses = values_of(&admin, "address");
    assert_eq!(
        addresses,
        vec!["127.0.0.1".to_string()],
        "the admin interface binds {addresses:?}. Anything other than loopback \
         is an unauthenticated stop button on the only pod with a route off \
         the cluster."
    );
}

#[test]
fn neither_the_fast_path_nor_an_edge_cell_may_reach_the_proxy() {
    // ADR 0008, consequence 3: nothing on the hot path consults a model. The
    // fast brain links `qip-ai` transitively through `qip-kernel` and
    // `qip-agents`, so what actually stops it calling one is its start-up
    // roster check and the fact that it can reach nothing that serves one.
    // Port 9102 on this proxy is exactly such a thing.
    //
    // The edge cell is excluded for a different reason: a cell is meant to keep
    // trading through a partition, and a shared proxy in the central plane is a
    // dependency a partition cuts.
    let manifest = read(MANIFEST);
    let ingress = manifest
        .split("name: allow-egress-proxy-ingress\n")
        .nth(1)
        .expect("egress.yaml declares who may reach the proxy")
        .split("\n---\n")
        .next()
        .expect("the policy document ends")
        .to_string();
    let ingress = without_comments(&ingress);
    let admitted: Vec<String> = values_of(&ingress, "app");
    assert!(
        admitted.len() >= 2,
        "only {admitted:?} may reach the proxy; the policy has been reshaped \
         and this check is filtering an empty list"
    );
    assert!(
        admitted.iter().any(|app| app == "qip-egress"),
        "the ingress policy does not select the proxy itself, so it governs \
         some other workload and this check is reading the wrong document"
    );
    for refused in ["qip-fastbrain", "qip-edge-node"] {
        assert!(
            !admitted.iter().any(|app| app == refused),
            "{refused} may reach the egress proxy. For the fast brain that is \
             a route to a language model API by way of a component that has \
             one; for an edge cell it is a central-plane dependency on the \
             workload whose whole design is that a partition does not stop it."
        );
    }

    // And nothing gave it the other half either: an egress rule on the fast
    // brain naming the proxy would be the same hole approached from the client
    // side, and it would be invisible to the check above.
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        for document in content.split("\nkind: NetworkPolicy\n").skip(1) {
            let document = document.split("\n---\n").next().unwrap_or(document);
            let Some(selector) = document.split("podSelector:").nth(1) else {
                continue;
            };
            let selected = selector
                .split("policyTypes:")
                .next()
                .unwrap_or("")
                .lines()
                .find_map(|line| line.trim().strip_prefix("app:"))
                .map(str::trim)
                .unwrap_or("");
            if selected != "qip-fastbrain" && selected != "qip-edge-node" {
                continue;
            }
            assert!(
                !document.contains("qip-egress"),
                "{} gives {selected} an egress rule naming the proxy",
                path.display()
            );
        }
    }
}

#[test]
fn the_proxy_offers_no_route_to_a_venue_and_cannot_carry_an_order() {
    // The shipped autonomy ceiling is paper trading, and a route from this
    // cluster to a venue is a live-order submission path whatever the ceiling
    // says. `qip_brokers::rest` asks for a TLS-terminating proxy in the same
    // words every other adapter does, and it deliberately does not get one.
    //
    // Premise first: these really are the variables the edge node's order-entry
    // path is configured through, read out of the source rather than asserted
    // from memory.
    let edge = read("crates/apps/qip-edge-node/src/main.rs");
    for variable in ["QIP_VENUE_FEED_ENDPOINT", "QIP_DROP_COPY_ENDPOINT"] {
        assert!(
            edge.contains(variable),
            "{variable} is not named by the edge node any more, so this test is \
             checking for a venue path that has been renamed"
        );
    }

    let manifest = without_comments(&read(MANIFEST));
    for variable in [
        "QIP_VENUE_FEED_ENDPOINT",
        "QIP_DROP_COPY_ENDPOINT",
        "QIP_SANDBOX_VENUE_ENDPOINT",
    ] {
        assert!(
            !manifest.contains(variable),
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
}

#[test]
fn the_proxy_holds_no_credential_and_no_identity_in_the_project() {
    // This pod terminates TLS, so every token its clients send passes through
    // it in clear text. That makes it the highest-value target in the
    // namespace, and the mitigation is that compromising it yields nothing but
    // the traffic already flowing: no mounted secret, no service-account token,
    // no workload-identity binding to a Google service account, and no shell.
    let manifest = read(MANIFEST);
    for (marker, why) in [
        (
            "iam.gke.io/gcp-service-account",
            "a workload-identity binding would give the one pod that talks to \
             the internet an identity in the project",
        ),
        (
            "secretProviderClass",
            "a projected secret is a credential this pod has no use for and an \
             attacker does",
        ),
        (
            "secretKeyRef",
            "the same, read out of etcd instead of the secret store",
        ),
    ] {
        assert!(
            !manifest.contains(marker),
            "egress.yaml carries {marker}: {why}"
        );
    }
    assert!(
        manifest.lines().any(
            |line| line.trim_start_matches(['#', ' ']) == "automountServiceAccountToken: false"
        ),
        "the proxy pod mounts a service-account token, which is a credential \
         for the Kubernetes API on the pod most exposed to the internet"
    );
}

#[test]
fn no_workload_in_the_namespace_has_an_egress_rule_that_bypasses_the_proxy() {
    // The proxy is only a control if it is the only way out. Every egress
    // destination in every policy is therefore checked against the four shapes
    // this platform permits: the private Google access range, a pod in the
    // namespace, kube-dns, and the venue range an edge cell reaches directly —
    // which is a placeholder in the committed state and is the one exception,
    // because a cell's venue is a cross-connect rather than a route through the
    // central plane.
    let mut destinations = Vec::new();
    let mut policies = 0usize;
    for path in files_with_extension("infrastructure/kubernetes", "yaml") {
        let content = without_comments(&std::fs::read_to_string(&path).expect("readable"));
        for document in content.split("\nkind: NetworkPolicy\n").skip(1) {
            let document = document.split("\n---\n").next().unwrap_or(document);
            policies += 1;
            let Some(egress) = document.split("\n  egress:").nth(1) else {
                continue;
            };
            destinations.extend(values_of(egress, "cidr"));
        }
    }
    assert!(
        policies >= 8,
        "only {policies} network policies were read; the walk is not reaching \
         them and this check has stopped checking"
    );
    assert!(
        !destinations.is_empty(),
        "no egress destination was read at all"
    );
    for destination in &destinations {
        assert!(
            destination == "199.36.153.8/30" || destination == "VENUE_CIDR",
            "a policy permits egress to {destination}. Anything that is not \
             private Google access or an edge cell's own venue range is a \
             route to the internet that does not pass through the proxy, and a \
             workload that can reach the internet can exfiltrate the position \
             history it was trained on."
        );
    }
    assert!(
        destinations.iter().any(|d| d == "199.36.153.8/30"),
        "no policy names private Google access, so this check is reading \
         something other than the egress rules"
    );
}
