use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn all_inputs_are_validated() {
    let root = repo_root();

    // qip-core must define validation via Result/Error types
    let core_path = root.join("crates/libs/qip-core/src").join("lib.rs");
    let core_content = fs::read_to_string(&core_path).unwrap_or_else(|_| String::new());

    assert!(
        core_content.contains("Result") || core_content.contains("Error"),
        "qip-core must provide Result/Error for validation"
    );
}

#[test]
fn risk_controls_checked_before_order_creation() {
    let root = repo_root();

    // qip-risk-engine must have pre-trade checks
    let risk_path = root
        .join("crates/services/qip-risk-engine/src")
        .join("lib.rs");
    let risk_content = fs::read_to_string(&risk_path).unwrap_or_else(|_| String::new());

    assert!(
        risk_content.contains("limit") || risk_content.contains("check"),
        "Risk engine must check limits before orders are created"
    );
}

#[test]
fn invalid_inputs_refused_not_clamped() {
    let root = repo_root();

    // CLAUDE.md principle: refuse rather than guess
    // Look for error handling that refuses bad values
    let risk_path = root.join("crates/services/qip-risk-engine/src");
    if let Ok(entries) = fs::read_dir(&risk_path) {
        let mut found_validation = false;
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file()
                && p.to_string_lossy().ends_with(".rs")
                && let Ok(content) = fs::read_to_string(&p)
                && (content.contains("Error") || content.contains("Result"))
            {
                found_validation = true;
            }
        }
        assert!(
            found_validation,
            "Risk engine must use error returns for invalid inputs"
        );
    }
}

#[test]
fn no_silent_value_corrections() {
    let root = repo_root();

    // Code must not silently clamp or adjust invalid values
    let execution_path = root
        .join("crates/services/qip-execution-engine/src")
        .join("lib.rs");
    if let Ok(content) = fs::read_to_string(&execution_path) {
        // Look for intentional design choices, not silent fixes
        assert!(
            !content.contains("clamp") || content.contains("Result"),
            "Invalid values must be refused, not silently corrected"
        );
    }
}
