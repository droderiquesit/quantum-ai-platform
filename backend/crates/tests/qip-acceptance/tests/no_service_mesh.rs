//! Fabric and Reflex traffic never routes through a service-mesh sidecar
//! (R12-GCP-8119, R12-SEC-10031).
//!
//! The requirement allows a mesh for warm services only, and ADR 0112's hop
//! table refuses one altogether: warm hops are IAM-authenticated Cloud Run
//! calls and Compute Engine hops are rustls mTLS. So the property worth a
//! test is the prohibition's: nothing the repository deploys — Terraform, the
//! GitOps manifests, the execution node's and the broker's startup templates
//! — enrols a workload in a mesh or asks for a sidecar to be injected. A mesh
//! added for a warm service would arrive as one of these markers, and the
//! execution, broker and ledger machines are declared in the same trees, so
//! one scan holds both halves until somebody deliberately narrows it.

use qip_acceptance::files_with_extension;

/// Text that only a service-mesh enrolment or sidecar injection writes. Each
/// is a delimited token of the product's own configuration vocabulary, not a
/// bare word: "mesh" alone is the reflex peer mesh (ADR 0116) and "sidecar"
/// alone is the egress proxy beside the API (ADR 0024), neither of which is
/// a service mesh.
const MESH_MARKERS: &[&str] = &[
    "istio-injection",
    "sidecar.istio.io",
    "istio.io/rev",
    "linkerd.io/inject",
    "servicemesh",
    "google_network_services_mesh",
    "google_network_services_grpc_route",
    "google_network_services_http_route",
    "anthos-service-mesh",
    "mesh.cloud.google.com",
];

/// Every marker found in `text`, with the line it is on.
fn mesh_markers(text: &str) -> Vec<(usize, &'static str)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let lowered = line.to_ascii_lowercase();
        for marker in MESH_MARKERS {
            if lowered.contains(marker) {
                found.push((index + 1, *marker));
            }
        }
    }
    found
}

fn deployed_configuration() -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for extension in ["tf", "tftpl", "yaml", "yml", "json"] {
        files.extend(files_with_extension("infrastructure", extension));
    }
    files
}

#[test]
fn nothing_the_repository_deploys_enrols_a_workload_in_a_service_mesh() {
    let files = deployed_configuration();
    // Premise: the scan reads the trees that declare the execution node, the
    // broker and the GitOps manifests, so "nothing found" is not "nothing read".
    for needle in [
        "modules/execution-node/main.tf",
        "modules/event-fabric-broker/main.tf",
        "modules/ledgerd/main.tf",
        "gitops/envs/dev/portal.yaml",
    ] {
        assert!(
            files.iter().any(|f| f.to_string_lossy().ends_with(needle)),
            "premise: the scan does not read {needle}"
        );
    }
    let mut enrolled = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        for (line, marker) in mesh_markers(&text) {
            enrolled.push(format!("{}:{line} {marker}", file.display()));
        }
    }
    assert!(
        enrolled.is_empty(),
        "service-mesh enrolment or sidecar injection is declared: {enrolled:#?}. The mesh is \
         refused (ADR 0112); Fabric and Reflex traffic must never route through a sidecar"
    );
}

#[test]
fn the_scan_finds_a_sidecar_injection_label_and_a_mesh_feature() {
    let manifest = "metadata:\n  labels:\n    istio-injection: enabled\n";
    assert_eq!(mesh_markers(manifest), vec![(3, "istio-injection")]);
    let terraform = "resource \"google_gke_hub_feature\" \"m\" {\n  name = \"servicemesh\"\n}\n";
    assert_eq!(mesh_markers(terraform), vec![(2, "servicemesh")]);
    // The reflex peer mesh and the egress sidecar are not a service mesh.
    assert!(mesh_markers("QIP_MESH_PEER=x\n# the egress proxy sidecar\n").is_empty());
}
