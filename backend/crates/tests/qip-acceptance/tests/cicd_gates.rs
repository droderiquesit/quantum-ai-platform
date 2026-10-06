//! CICD and review infrastructure tests.
//!
//! These tests verify that the platform's CI/CD infrastructure meets its
//! architectural requirements for separate review agents, gating, and
//! policy enforcement.

use qip_acceptance::repository_root;
use std::fs;
use std::path::PathBuf;

fn read_agent(name: &str) -> String {
    let path = repository_root()
        .join(".claude")
        .join("agents")
        .join(format!("{}.md", name));
    fs::read_to_string(&path).expect(&format!("readable {}", path.display()))
}

/// CICD-021: Multiple review passes by separate agents and models.
///
/// Code-reviewer and security-engineer must have distinct models configured
/// so that changes are reviewed by different model instances.
#[test]
fn code_reviewer_and_security_engineer_have_distinct_models() {
    let code_reviewer = read_agent("code-reviewer");
    let security_engineer = read_agent("security-engineer");

    // Extract model field from frontmatter
    let extract_model = |content: &str| -> Option<String> {
        content
            .lines()
            .find(|line| line.starts_with("model:"))
            .and_then(|line| line.split(':').nth(1))
            .map(|m| m.trim().to_string())
    };

    let code_reviewer_model = extract_model(&code_reviewer)
        .expect("code-reviewer must have a model field in its frontmatter");
    let security_engineer_model = extract_model(&security_engineer)
        .expect("security-engineer must have a model field in its frontmatter");

    assert_ne!(
        code_reviewer_model, security_engineer_model,
        "code-reviewer (uses {}) and security-engineer (uses {}) must have distinct models",
        code_reviewer_model, security_engineer_model
    );

    // Verify the models are valid Claude models
    let valid_models = [
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-4-5-20251001",
    ];
    assert!(
        valid_models.contains(&code_reviewer_model.as_str()),
        "code-reviewer model '{}' is not a known Claude model",
        code_reviewer_model
    );
    assert!(
        valid_models.contains(&security_engineer_model.as_str()),
        "security-engineer model '{}' is not a known Claude model",
        security_engineer_model
    );
}

/// Verify that both review agents exist and are properly named.
#[test]
fn review_agent_files_exist_with_correct_names() {
    let code_reviewer_path = repository_root()
        .join(".claude")
        .join("agents")
        .join("code-reviewer.md");
    let security_engineer_path = repository_root()
        .join(".claude")
        .join("agents")
        .join("security-engineer.md");

    assert!(
        code_reviewer_path.exists(),
        "code-reviewer.md must exist at {}",
        code_reviewer_path.display()
    );
    assert!(
        security_engineer_path.exists(),
        "security-engineer.md must exist at {}",
        security_engineer_path.display()
    );
}
