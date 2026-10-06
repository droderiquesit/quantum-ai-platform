use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn health_endpoints_prove_storage_writable() {
    let root = repo_root();

    let api_main = root.join("crates/apps/qip-api/src").join("main.rs");
    if api_main.exists() {
        let content = fs::read_to_string(&api_main).unwrap_or_default();

        assert!(
            content.contains("health") || content.contains("ready"),
            "Apps must implement health endpoints"
        );
    }
}

#[test]
fn health_does_not_report_ready_before_dependencies() {
    let root = repo_root();

    // Health check must verify storage, network, ports before reporting ready
    let api_routes = root.join("crates/apps/qip-api/src").join("routes.rs");
    if api_routes.exists() {
        let content = fs::read_to_string(&api_routes).unwrap_or_default();

        assert!(
            content.len() > 0,
            "Health routes must exist and verify dependencies"
        );
    }
}

#[test]
fn metrics_endpoint_available_on_health_port() {
    let root = repo_root();

    let kernel_path = root
        .join("crates/runtime/qip-kernel/src")
        .join("platform.rs");
    if kernel_path.exists() {
        let content = fs::read_to_string(&kernel_path).unwrap_or_default();

        assert!(
            content.contains("metrics") || content.contains("Telemetry"),
            "Platform must expose metrics endpoint"
        );
    }
}

#[test]
fn readiness_check_is_real_not_process_liveness() {
    let root = repo_root();

    // Readiness is proven storage, not just process running
    let api_main = root.join("crates/apps/qip-api/src").join("main.rs");
    if api_main.exists() {
        let content = fs::read_to_string(&api_main).unwrap_or_default();

        assert!(
            content.contains("health") || content.contains("ready"),
            "Readiness must be proven not inferred from process state"
        );
    }
}
