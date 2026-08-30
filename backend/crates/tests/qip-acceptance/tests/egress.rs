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
//!
//! That is not a hypothetical warning here; it is this file's own history. Six
//! of these checks were wrong the first time they were run, and every one of
//! them was wrong about how to read the manifest rather than about the manifest:
//!
//!   * Four read only the block form `key: value`, while the bootstrap writes
//!     every socket address, every port and every certificate matcher in the
//!     flow form `{ key: value }`. Each read an empty list for the key it was
//!     about, and each was one relaxed premise away from passing forever while
//!     guarding nothing.
//!   * One scraped every `cluster:` in the bootstrap and found the `node:`
//!     block's own statistics name, which is not a route to anywhere.
//!   * One could not tell the sentence `# No iam.gke.io/gcp-service-account
//!     annotation, deliberately` from the annotation itself, and so failed on
//!     the documentation of the property it existed to check.
//!
//! `values_of`, `key_count` and `declares_key` below are the fix, and all three
//! match a key in mapping position with its delimiter. That is not fastidious:
//! `address` is a suffix of `socket_address`, `port` of `targetPort`, and
//! `validation_context` of `common_tls_context`, so a substring match reports a
//! listener's bind address as an internet destination and a port name as a port
//! number.

use qip_acceptance::{files_with_extension, read};

/// The manifest under test.
const MANIFEST: &str = "infrastructure/kubernetes/base/egress.yaml";

/// The in-cluster name every adapter address must resolve through.
const PROXY_AUTHORITY: &str = "qip-egress.qip.svc.cluster.local";

/// The trust store every upstream is verified against.
///
/// The path the distroless Envoy image carries. A `validation_context` naming
/// a file that is not in the image fails closed — Envoy refuses to start — so
/// this is checked for drift rather than for danger.
const CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

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

/// Every place a key appears in mapping position, as the text following it.
///
/// Two forms are matched because the manifest uses both: the block form
/// `key: value`, and the flow form `socket_address: { address: X, port_value:
/// N }` that every address, port and certificate matcher in the bootstrap is
/// written in. A parser that read the block form alone returned nothing for
/// `address`, `port_value` and `exact` — the failure this file was committed
/// with, and the reason the premise assertions in every caller are load-bearing
/// rather than decorative.
///
/// The key is matched with its delimiter. What precedes it must be a mapping or
/// list boundary, so `address` does not match inside `socket_address` and
/// `port` does not match inside `targetPort`; what follows the value ends it,
/// so a flow mapping yields one value per key rather than the rest of the line.
///
/// Occurrences with an empty tail are kept here — they are the block keys whose
/// value is the indented block beneath them — because the fact that
/// `match_typed_subject_alt_names:` is present at all is exactly what one
/// caller needs to know.
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
///
/// For the keys whose value is a block rather than a scalar: their presence is
/// the property, and their absence is what the check is looking for.
fn key_count(text: &str, key: &str) -> usize {
    occurrences_of(text, key).len()
}

/// Whether the manifest declares a key, in live YAML or in the pod that is
/// committed commented out.
///
/// Two things have to be told apart here, and `contains` tells neither. A `#`
/// in front of a key is not an absence: the Deployment at the foot of this
/// manifest is real configuration waiting on four out-of-band edits, and an
/// operator will uncomment it as written. But prose about a key is not the key
/// — the ServiceAccount above it explains that it carries no
/// `iam.gke.io/gcp-service-account` annotation, and a check that read the raw
/// text failed on that sentence, which is to say it failed on the documentation
/// of the property it was written to enforce.
///
/// So: uncomment every line, then require the key in mapping position, which is
/// the only form Kubernetes would act on.
fn declares_key(manifest: &str, key: &str) -> bool {
    let mapping = format!("{key}:");
    manifest.lines().any(|line| {
        line.trim_start()
            .trim_start_matches(['#', ' '])
            .trim_start_matches("- ")
            .starts_with(&mapping)
    })
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

/// One block of the bootstrap, bounded by the block that follows it.
///
/// Bounded rather than open-ended, and the difference is not cosmetic: a check
/// reading "the listeners" that ran on past `clusters:` would read the five
/// upstream port 443s as listener ports, and a Service publishing 443 would
/// satisfy it. Both ends must be found, so a reshaped bootstrap fails here
/// rather than quietly widening what a caller is looking at.
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

/// The admin interface's own configuration.
fn admin_block(bootstrap: &str) -> String {
    section(bootstrap, "\n    admin:\n", Some("\n    static_resources:"))
}

/// The listeners, and nothing that follows them.
fn listeners_block(bootstrap: &str) -> String {
    section(bootstrap, "\n      listeners:\n", Some("\n      clusters:"))
}

/// The clusters, which are the last thing in the bootstrap.
fn clusters_block(bootstrap: &str) -> String {
    section(bootstrap, "\n      clusters:\n", None)
}

/// The top-level entries of a `listeners:` or `clusters:` block, as name and
/// body.
///
/// Split on the eight-space `- name:` that a top-level entry is indented with,
/// which is the only place that indent occurs: a network filter's `name:` sits
/// at sixteen and a transport socket's at twelve. Per-entry bodies rather than
/// one bag of values is what lets a check name *which* cluster lost its
/// certificate matcher, and stops five matchers on one cluster satisfying a
/// count of five.
fn entries_of(block: &str) -> Vec<(String, String)> {
    let padded = format!("\n{block}");
    padded
        .split("\n        - name: ")
        .skip(1)
        .map(|entry| {
            let (name, body) = entry.split_once('\n').unwrap_or((entry, ""));
            (name.trim().to_string(), body.to_string())
        })
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

    // Premise. Every address in this file is written inside a flow mapping, and
    // the parser this check first shipped with matched the block form only: it
    // read an empty list, filtered it to another empty list, and reported that
    // nothing outside the allowlist was dialled. Six binds and five upstreams
    // is eleven; fewer than ten means the socket addresses have moved and this
    // check is looking at nothing again.
    let addresses = values_of(&bootstrap, "address");
    assert!(
        addresses.len() >= 10,
        "only {addresses:?} were read out of the bootstrap; the socket_address \
         blocks have been reshaped and this check is filtering an empty list"
    );

    // `socket_address` carries both the listener binds and the upstream dials.
    // The binds are the wildcard and loopback; everything else is somewhere
    // this proxy will connect out to. A listener bound to a particular address
    // would be read here as an upstream and refused — which is the direction
    // this should fail in, because that is also a manifest nobody reviewed.
    let (bound, dialled): (Vec<String>, Vec<String>) = addresses
        .into_iter()
        .partition(|address| address == "0.0.0.0" || address == "127.0.0.1");

    // Premise, second half: the partition discriminates. Both sides being
    // populated is what separates a working filter from one that passed
    // everything or nothing, and only one of those two mistakes is visible in
    // the assertion below.
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
            dialled.iter().any(|upstream| upstream == host),
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
    // Premise: there is at least one rewrite per upstream. Every upstream here
    // is a public vendor that rejects a request whose `host:` header names a
    // cluster service, so this is also the check that the proxy works at all.
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
             whatever reaches it is addressed to the proxy's own cluster name \
             and answered with a 404 the operator will read as the vendor's"
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

    // Routes are read out of the listeners, not out of the whole bootstrap.
    // The `node:` block above them sets `cluster: qip-egress`, which is this
    // proxy's own name for its statistics and not a destination; the check that
    // scraped every `cluster:` in the file reported it as a route to an
    // undeclared upstream and failed for a reason that had nothing to do with
    // the manifest. That is the first thing to go wrong again if these blocks
    // move, so it is asserted rather than assumed.
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

    // A cluster is named twice — once as the cluster's own `name`, which is
    // what a route resolves against, and once as its `load_assignment`'s
    // `cluster_name`. Both are checked: endpoints filed under a name no cluster
    // has is the same 503 with a longer diagnosis.
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
    //
    // Checked per cluster rather than by counting the file's matchers, because
    // five matchers and five upstreams is also what two matchers on one cluster
    // and none on another look like from a distance.
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
        // Premise, per cluster: this really is a block that dials one host.
        // Everything below compares against that host, so reading no host means
        // comparing nothing against nothing.
        let dialled = values_of(body, "address");
        assert_eq!(
            dialled.len(),
            1,
            "the cluster {name} dials {dialled:?}. One cluster, one upstream \
             host: a second endpoint here is a destination that inherits the \
             first one's certificate matcher and is not the host it names."
        );
        let host = dialled[0].as_str();

        // Without a transport socket Envoy speaks plaintext to port 443, which
        // fails — the safe direction — but fails in a way that reads as the
        // vendor being down rather than as a deleted TLS context.
        assert_eq!(
            key_count(body, "transport_socket"),
            1,
            "the cluster {name} has no upstream TLS context, so it speaks \
             plaintext to a port that expects TLS"
        );
        assert_eq!(
            values_of(body, "sni"),
            vec![host.to_string()],
            "the cluster {name} dials {host} and offers SNI {:?}. An upstream \
             with no SNI, or with somebody else's, reaches a shared front end \
             that answers with the wrong certificate.",
            values_of(body, "sni")
        );

        // `validation_context` is a suffix-neighbour of `common_tls_context`,
        // which is why this is counted with a delimited match rather than a
        // `contains` — the enclosing block would have satisfied a `contains`
        // for as long as the file existed.
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

        // The part worth not deleting, and the reason this test has its name.
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
            "the cluster {name} accepts {:?}. TLS 1.0 and 1.1 are the versions \
             whose weaknesses are exploitable by whoever is between this pod \
             and the vendor, which on this path is a NAT gateway and the public \
             internet.",
            values_of(body, "tls_minimum_protocol_version")
        );
    }
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
fn no_manifest_sends_a_workload_out_of_the_cluster_except_through_the_proxy() {
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
            // The property is about addresses that *leave the cluster*, which
            // is what the comment above says and what the earlier version of
            // this assertion did not implement: it refused every `http://`
            // address, including in-cluster service DNS. The mesh's own peer
            // address — one pod calling another over the cluster network —
            // tripped it, and that packet never touches the internet, so no
            // credential crosses it in clear text.
            //
            // Matched on the authority's suffix rather than as a substring.
            // `contains(".svc.cluster.local")` would also accept
            // `vendor.example.com.svc.cluster.local.attacker.net`, which is a
            // public host wearing the suffix as a prefix of its own — exactly
            // the substring trap that has already let a mutation through
            // elsewhere in this repository.
            let authority = address
                .split_once("://")
                .map(|(_, rest)| rest)
                .unwrap_or(address)
                .split(['/', '?'])
                .next()
                .unwrap_or("")
                .rsplit_once(':')
                .map_or_else(
                    || {
                        address
                            .split_once("://")
                            .map(|(_, rest)| rest)
                            .unwrap_or(address)
                            .split(['/', '?'])
                            .next()
                            .unwrap_or("")
                            .to_string()
                    },
                    |(host, _port)| host.to_string(),
                );
            let stays_in_cluster = authority.ends_with(".svc.cluster.local")
                || authority.ends_with(".svc")
                || !authority.contains('.');
            assert!(
                address.contains(PROXY_AUTHORITY) || stays_in_cluster,
                "{} configures {address}, whose authority {authority} is \
                 neither the egress proxy nor an in-cluster name. An adapter \
                 pointed straight at a vendor over plaintext sends its \
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
    // `port` is a suffix of `targetPort`, and the target is a port *name*.
    // Reading one as the other would compare `gcp` against a listener's number
    // for as long as this file lasted, and never match.
    assert!(
        published
            .iter()
            .all(|port| port.chars().all(|c| c.is_ascii_digit())),
        "the Service publishes {published:?}, which includes something that is \
         not a port number"
    );

    // Listener ports come from the listeners block alone. Read from the whole
    // bootstrap they would include every upstream's 443, and a Service
    // publishing 443 would then satisfy this check.
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
    let mut listening: Vec<(String, String)> = Vec::new();
    for (name, body) in &listeners {
        let ports = values_of(body, "port_value");
        assert_eq!(
            ports.len(),
            1,
            "the listener {name} binds {ports:?}; one listener, one port is \
             what makes the port a destination selector"
        );
        listening.push((name.clone(), ports[0].clone()));
    }

    for port in &published {
        assert!(
            listening.iter().any(|(_, bound)| bound == port),
            "the Service publishes {port} and the bootstrap has no listener on \
             it. A Service port with nothing behind it is a connection that \
             hangs, which is the failure that takes longest to diagnose."
        );
    }

    // The health listener answers a probe and forwards to no cluster. It is
    // deliberately absent from the Service: a probe endpoint on a Service is a
    // thing that ends up behind a load balancer.
    let (_, health) = listening.iter().find(|(name, _)| name == "health").expect(
        "the bootstrap has no listener named `health`, so the pod's probes have \
         nothing to hit that is not the admin interface",
    );
    assert_eq!(
        health, "9900",
        "the health listener has moved to {health}; the Deployment's probes \
         name the port by its container-port name and would go on hitting \
         whatever is called `health` there"
    );
    assert!(
        !published.contains(health),
        "the Service publishes the health listener"
    );
    for (name, port) in &listening {
        assert!(
            name == "health" || published.contains(port),
            "the listener {name} binds {port} and no Service publishes it. A \
             listener with no Service in front is reachable by pod IP and \
             constrained by nothing anybody reviewed."
        );
    }

    // And the admin interface, which serves /quitquitquit and a config dump, is
    // bound to loopback and therefore cannot be published even by accident —
    // but the Service is checked anyway, because loopback and the Service are
    // two edits and only one of them is in this file's admin block.
    let admin = values_of(&admin_block(&bootstrap), "port_value");
    assert_eq!(
        admin,
        vec!["9901".to_string()],
        "the admin interface binds {admin:?}; this check is looking at the \
         wrong block or the interface has moved"
    );
    assert!(
        !published.contains(&admin[0]),
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
    let admin = admin_block(&bootstrap);
    // Premise: this block binds something. The check that read only block-form
    // mappings found no address in a flow mapping and compared an empty list
    // against loopback, which is the shape that passes the day somebody deletes
    // the binding entirely.
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
    //
    // Counted as it goes, because this half is a scan and a scan that selects
    // nothing reports no violation. If the label these policies are written
    // against ever changes, the loop below silently examines zero documents and
    // this test starts asserting that a set it never built contains nothing.
    let mut examined = 0usize;
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
            examined += 1;
            assert!(
                !document.contains("qip-egress"),
                "{} gives {selected} an egress rule naming the proxy",
                path.display()
            );
        }
    }
    assert!(
        examined >= 2,
        "only {examined} network policies governing the fast brain or an edge \
         cell were found; namespace.yaml carries an ingress and an egress rule \
         for the fast brain alone, so this scan is matching on something that \
         has been renamed and is checking nothing"
    );
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
    let edge = read("backend/crates/apps/qip-edge-node/src/main.rs");
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

    // Premise. Every assertion below is an absence, and an absence is worth
    // nothing unless the thing looking for the key can find one that is there.
    // The pod is committed commented out, so `declares_key` uncomments before
    // it matches; these three are what demonstrate it still does. If the pod
    // moves to another file, this fails here rather than reporting that a
    // manifest with no pod in it has no credential in it.
    for present in [
        "serviceAccountName",
        "automountServiceAccountToken",
        "readOnlyRootFilesystem",
    ] {
        assert!(
            declares_key(&manifest, present),
            "egress.yaml no longer declares {present}, so it no longer carries \
             the pod and every absence checked below is vacuous"
        );
    }

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
        // In mapping position, not anywhere in the text. The ServiceAccount in
        // this manifest explains in prose that it carries no
        // `iam.gke.io/gcp-service-account` annotation, and the check that read
        // the raw file failed on that sentence — on the documentation of the
        // property it exists to enforce, which is how a reviewer learns to
        // delete the sentence rather than keep the property.
        assert!(
            !declares_key(&manifest, marker),
            "egress.yaml carries {marker}: {why}"
        );
    }
    assert!(
        manifest
            .lines()
            .any(|line| line.trim_start_matches(['#', ' ']).trim_end()
                == "automountServiceAccountToken: false"),
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
