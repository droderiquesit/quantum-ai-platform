//! Exhaustive sweeps over every surface a person or the internet could
//! reach, read from the committed manifests and Terraform.
//!
//! The per-service suites (`gitops.rs`, `console_route.rs`) each prove one
//! door is shut. What none of them can say is that there is no door they do
//! not know about: a new `RunService` added beside the others, a new
//! certificate uploaded by hand, a second cluster with node-wide keys. These
//! walk the whole tree, so a surface nobody listed is a surface that fails.
//!
//! - R12-SEC-9136 — internal and admin web access is IAP, and no management
//!   endpoint is public.
//! - R12-SEC-10021 — public TLS certificates are issued and rotated by Google,
//!   never uploaded.
//! - R12-SEC-9125 — every GKE cluster maps pods to Google identities through
//!   Workload Identity, and its nodes hold no broad credential.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};

/// A file's text with `#` comments removed, so prose quoting a value is not
/// mistaken for the value. Works for both YAML and HCL in this tree, neither
/// of which carries a `#` inside a value the checks read.
fn uncommented(text: &str) -> String {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn relative(path: &std::path::Path) -> String {
    let root = repository_root();
    path.strip_prefix(&root)
        .map_or_else(|_| path.display().to_string(), |p| p.display().to_string())
}

/// Every YAML document under `infrastructure/gitops/envs`, as (file, text).
fn environment_documents() -> Vec<(String, String)> {
    let mut documents = Vec::new();
    for path in files_with_extension("infrastructure/gitops/envs", "yaml") {
        let file = relative(&path);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
        for document in uncommented(&text).split("\n---") {
            documents.push((file.clone(), document.to_string()));
        }
    }
    documents
}

/// The value of the first `<key>: <value>` line in a YAML document.
fn yaml_value<'a>(document: &'a str, key: &str) -> Option<&'a str> {
    document.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

/// Every committed Terraform file under `infrastructure/terraform`, comments
/// removed.
fn terraform_tree() -> Vec<(String, String)> {
    files_with_extension("infrastructure/terraform", "tf")
        .into_iter()
        .filter(|path| {
            !path
                .components()
                .any(|part| part.as_os_str() == ".terraform")
        })
        .map(|path| {
            let file = relative(&path);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {file}: {error}"));
            (file, uncommented(&text))
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

/// Every `resource "<kind>" "<name>"` in the tree, as (file, name, body).
fn resources(kind: &str) -> Vec<(String, String, String)> {
    let marker = format!("resource \"{kind}\" \"");
    let mut found = Vec::new();
    for (file, text) in terraform_tree() {
        for (at, _) in text.match_indices(&marker) {
            let after = at + marker.len();
            let Some(name) = text[after..].split('"').next() else {
                continue;
            };
            if let Some(body) = braced(&text, after + name.len()) {
                found.push((file.clone(), name.to_string(), body.to_string()));
            }
        }
    }
    found
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect()
}

// --- R12-SEC-9136 -------------------------------------------------------------

#[test]
fn every_cloud_run_service_in_every_environment_is_internal_or_behind_iap() {
    // A `RunService` with `INGRESS_TRAFFIC_ALL` answers the internet at its
    // run.app URL. Exactly one service is allowed that: the console, because
    // Identity-Aware Proxy on Cloud Run enforces on every ingress path and
    // the root module turns IAP on for a service of exactly that name. Every
    // other service must be internal. A service with no `ingress` line is
    // `INGRESS_TRAFFIC_ALL` by default, so a missing line is a failure here
    // rather than a pass.
    let root = compact(&uncommented(&read("infrastructure/terraform/main.tf")));
    assert!(
        root.contains("module\"portal_iap_run\"{source=\"./modules/iap-run\"")
            && root.contains("service_name=\"qip-${var.environment}-portal\""),
        "premise: the root no longer puts IAP on qip-<env>-portal, so the one public \
         ingress this test admits is admitted for a reason that has gone"
    );

    let services: Vec<_> = environment_documents()
        .into_iter()
        .filter(|(_, document)| yaml_value(document, "kind") == Some("RunService"))
        .collect();
    assert!(
        services.len() >= 8,
        "premise: only {} RunService manifests were found across the environments; the \
         sweep is reading the wrong tree",
        services.len()
    );

    for (file, document) in &services {
        let name = yaml_value(document, "name")
            .unwrap_or_else(|| panic!("{file}: a RunService with no name"));
        let ingress = yaml_value(document, "ingress");
        match ingress {
            Some("INGRESS_TRAFFIC_INTERNAL_ONLY" | "INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER") => {}
            Some("INGRESS_TRAFFIC_ALL")
                if name.starts_with("qip-") && name.ends_with("-portal") =>
            {
                let environment = file
                    .split('/')
                    .rev()
                    .nth(1)
                    .unwrap_or_else(|| panic!("{file}: not under an environment directory"));
                assert_eq!(
                    name,
                    format!("qip-{environment}-portal"),
                    "{file}: a public portal whose name is not the one the root puts IAP on"
                );
            }
            other => panic!(
                "{file}: RunService {name} has ingress {other:?}. Only qip-<env>-portal, which \
                 the root fronts with IAP, may take internet traffic; every other service is \
                 INGRESS_TRAFFIC_INTERNAL_ONLY or INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER"
            ),
        }
    }
}

#[test]
fn no_manifest_or_terraform_grant_admits_an_anonymous_or_every_account_caller() {
    // The other way an internal service becomes public: an `allUsers`
    // invoker. IAP in front of the console means nothing if the service
    // also answers anyone who reaches it. Every environment's IAM manifests
    // and every committed Terraform grant are swept.
    let mut members = 0usize;
    for (file, document) in environment_documents() {
        if let Some(member) = yaml_value(&document, "member") {
            members += 1;
            assert!(
                member != "allUsers" && member != "allAuthenticatedUsers",
                "{file}: an IAM member manifest admits {member}"
            );
        }
    }
    assert!(
        members > 0,
        "premise: no IAM member manifest was found, so the sweep checked nothing"
    );
    for (file, text) in terraform_tree() {
        let code = compact(&text);
        for open in ["member=\"allUsers\"", "member=\"allAuthenticatedUsers\""] {
            assert!(
                !code.contains(open),
                "{file} grants {open} by literal; no management surface is public"
            );
        }
    }
}

#[test]
fn no_kubernetes_service_or_route_exposes_a_controller_outside_the_cluster() {
    // Argo CD, Kargo and Config Connector administer the platform. A Service
    // of type LoadBalancer or NodePort, or an HTTPRoute with a backend and no
    // IAP backend policy beside it, would publish one of them. The only route
    // committed today is the HTTP-to-HTTPS redirect, which has no backend.
    let mut routes = 0usize;
    for path in files_with_extension("infrastructure/gitops", "yaml") {
        let file = relative(&path);
        if file.contains("/upstream/") {
            // Vendored controller manifests are pinned by digest and covered
            // by `gitops.rs`'s controller sweep; their CRD schemas mention
            // every type name in prose.
            continue;
        }
        let text = uncommented(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {file}: {error}")),
        );
        for document in text.split("\n---") {
            let kind = yaml_value(document, "kind");
            if kind == Some("Service") {
                let service_type = yaml_value(document, "type").unwrap_or("ClusterIP");
                assert!(
                    service_type != "LoadBalancer" && service_type != "NodePort",
                    "{file}: a Service of type {service_type} publishes a controller outside \
                     the cluster"
                );
            }
            if kind == Some("HTTPRoute") {
                routes += 1;
                assert!(
                    !document.contains("backendRefs"),
                    "{file}: an HTTPRoute sends traffic to a backend; a controller reached \
                     through the gateway must have a GCPBackendPolicy enabling IAP, and this \
                     sweep must be extended to check it before the route lands"
                );
            }
        }
    }
    assert!(
        routes >= 1,
        "premise: the gateway's redirect route was not found, so the route check saw nothing"
    );
}

// --- R12-SEC-10021 ------------------------------------------------------------

#[test]
fn every_public_tls_certificate_is_google_managed_and_none_is_uploaded() {
    // An uploaded certificate is a private key somebody held, copied into
    // state, and a renewal somebody has to remember. The front doors use
    // Google-managed certificates — classic managed SSL certificates on the
    // two IAP doors and a Certificate Manager certificate on the public edge —
    // and this refuses any other shape anywhere in the tree.
    for kind in [
        "google_compute_ssl_certificate",
        "google_compute_region_ssl_certificate",
    ] {
        let uploaded = resources(kind);
        assert!(
            uploaded.is_empty(),
            "{kind} is a self-managed certificate (its private key lives in state): {:?}",
            uploaded
                .iter()
                .map(|(file, name, _)| format!("{file} {name}"))
                .collect::<Vec<_>>()
        );
    }

    let managed = resources("google_compute_managed_ssl_certificate");
    let manager = resources("google_certificate_manager_certificate");
    assert!(
        managed.len() >= 2 && !manager.is_empty(),
        "premise: the IAP doors' managed certificates ({}) and the public edge's Certificate \
         Manager certificate ({}) were not all found",
        managed.len(),
        manager.len()
    );
    for (file, name, body) in managed.iter().chain(manager.iter()) {
        let code = compact(body);
        assert!(
            code.contains("managed{"),
            "{file}: certificate {name} has no `managed` block, so nothing issues or renews it"
        );
        for forbidden in [
            "self_managed",
            "private_key",
            "pem_private_key",
            "certificate=",
        ] {
            assert!(
                !code.contains(forbidden),
                "{file}: certificate {name} carries `{forbidden}`; a public certificate is \
                 issued by Google, never uploaded"
            );
        }
    }

    // And every HTTPS proxy terminates on one of those, by reference.
    for (file, name, body) in resources("google_compute_target_https_proxy") {
        let code = compact(&body);
        assert!(
            code.contains("google_compute_managed_ssl_certificate.")
                || code.contains("certificate_map="),
            "{file}: HTTPS proxy {name} terminates TLS on something other than a \
             Google-managed certificate or a Certificate Manager map"
        );
    }
}

// --- R12-SEC-9125 -------------------------------------------------------------

#[test]
fn every_gke_cluster_enables_workload_identity_and_runs_its_nodes_as_a_narrow_identity() {
    // `gitops.rs` and `infrastructure.rs` prove this of the GitOps control
    // plane by module path. This proves it of every cluster the tree declares,
    // so a second cluster cannot arrive with the default Compute Engine
    // account (`roles/editor` in a project with no organisation policy) on
    // every node and every pod able to read it from the metadata server.
    let clusters = resources("google_container_cluster");
    assert!(
        !clusters.is_empty(),
        "premise: no google_container_cluster was found"
    );
    for (file, name, body) in &clusters {
        let code = compact(body);
        assert!(
            code.contains(
                "workload_identity_config{workload_pool=\"${var.project_id}.svc.id.goog\"}"
            ),
            "{file}: cluster {name} does not map pods to Google identities through this \
             project's workload pool"
        );
        assert!(
            code.contains("auto_provisioning_defaults{service_account=google_service_account.")
                || code.contains("node_config{service_account=google_service_account."),
            "{file}: cluster {name} does not name a dedicated node identity, so its nodes run \
             as the project's default Compute Engine account"
        );
    }
    assert!(
        resources("google_container_node_pool").is_empty(),
        "a node pool is declared; it must name its own narrow service account and this sweep \
         must be extended to check it"
    );
    let keys: Vec<String> = resources("google_service_account_key")
        .into_iter()
        .map(|(file, name, _)| format!("{file} {name}"))
        .collect();
    assert!(
        keys.is_empty(),
        "a google_service_account_key mints a long-lived key; pods and nodes reach Google \
         through Workload Identity only: {keys:?}"
    );
}

// --- R12-SEC-9135 -------------------------------------------------------------

#[test]
fn every_service_that_changes_what_runs_records_its_data_writes_in_every_environment() {
    // Admin activity — a deploy, a cluster or instance change — is logged by
    // Google whatever is configured. DATA_WRITE is not, and it is what makes
    // a change an agent made through Cloud Run, the control plane's
    // Kubernetes API or a Compute Engine machine attributable below that.
    // Ported from recover/l016-cicd-037, whose `ADMIN_WRITE` entries were a
    // log type Google refuses; this also refuses that type anywhere, and a
    // service configured twice, which the API resolves by letting one
    // config silently replace the other.
    let configs = resources("google_project_iam_audit_config");
    let runtime = configs
        .iter()
        .find(|(file, name, _)| {
            file == "infrastructure/terraform/audit-logs.tf" && name == "runtime_changes"
        })
        .unwrap_or_else(|| panic!("the root declares no runtime_changes audit config"));
    let body = compact(&runtime.2);
    for service in [
        "\"run.googleapis.com\"",
        "\"container.googleapis.com\"",
        "\"compute.googleapis.com\"",
    ] {
        assert!(
            body.contains(service),
            "{service} is not in the runtime audit config; a change made through it is \
             recorded only as admin activity"
        );
    }
    assert!(
        body.contains("audit_log_config{log_type=\"DATA_WRITE\"}"),
        "the runtime audit config does not enable DATA_WRITE"
    );
    assert!(
        !body.contains("count=") && body.contains("for_each=toset([\"run.googleapis.com\""),
        "the runtime audit config is switched by a variable; visibility is not optional per \
         environment: {body}"
    );

    let mut services: Vec<String> = Vec::new();
    for (file, name, body) in &configs {
        let code = compact(body);
        assert!(
            !code.contains("\"ADMIN_WRITE\""),
            "{file}: {name} names log type ADMIN_WRITE, which Google refuses at apply; admin \
             activity is always logged"
        );
        for service in code
            .split('"')
            .filter(|piece| piece.ends_with(".googleapis.com"))
        {
            assert!(
                !services.iter().any(|seen| seen == service),
                "{file}: {name} configures {service} a second time; one config would replace \
                 the other"
            );
            services.push(service.to_string());
        }
    }
    assert!(
        services.len() >= 5,
        "premise: the secrets module's and the root's audit configs were not both read \
         ({services:?})"
    );
}
