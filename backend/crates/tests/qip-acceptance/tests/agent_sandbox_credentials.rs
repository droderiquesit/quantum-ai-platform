//! R12-SEC-9068: the coder agent's sandbox holds the contents:read token and
//! nothing else. `scripts/agent-guard.mjs sandbox` already refuses a sandbox
//! workflow that gains a secret, an OIDC token, an environment or a write, but
//! it is a node script a change to the same pull request could loosen. This
//! is the second, independent reader of the same file, in the Rust gate, so a
//! credential reaching an agent run is refused by two gates that share no code.

use std::path::Path;

fn sandbox() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../.github/workflows/agent-sandbox.yml");
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => panic!("agent-sandbox.yml must exist at .github/workflows: {e}"),
    }
}

/// Non-comment lines, trimmed, with their 1-based numbers.
fn code_lines(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .map(|(n, l)| (n + 1, l.trim().to_string()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

#[test]
fn the_agent_sandbox_workflow_grants_contents_read_and_nothing_else() {
    let lines = code_lines(&sandbox());
    // Premise: the file has code and a permissions block to judge.
    assert!(
        lines.iter().any(|(_, l)| l == "permissions:"),
        "premise: agent-sandbox.yml declares a permissions block"
    );
    // Every key under a `permissions:` line is `contents: read`: a second
    // key (id-token, pull-requests, packages) or a write is a new credential.
    let raw: Vec<&str> = sandbox_static();
    let mut grants = Vec::new();
    for (i, line) in raw.iter().enumerate() {
        if line.trim() == "permissions:" {
            let mut j = i + 1;
            while j < raw.len() && raw[j].starts_with(' ') && !raw[j].trim_start().starts_with('#')
            {
                grants.push(raw[j].trim().to_string());
                j += 1;
            }
        }
    }
    assert!(!grants.is_empty(), "premise: a permissions block has keys");
    for grant in &grants {
        assert_eq!(
            grant, "contents: read",
            "the sandbox may hold contents: read and no other permission, found `{grant}`"
        );
    }
}

fn sandbox_static() -> Vec<&'static str> {
    // Leaked once per test process: a test-only convenience to hand out
    // borrowed lines without threading the String through.
    let text: &'static str = Box::leak(sandbox().into_boxed_str());
    text.lines().collect()
}

#[test]
fn the_agent_sandbox_workflow_names_no_secret_oidc_token_or_environment() {
    let lines = code_lines(&sandbox());
    assert!(
        lines.iter().any(|(_, l)| l.starts_with("runs-on:")),
        "premise: the file has jobs to judge"
    );
    for (n, line) in &lines {
        assert!(
            !line.contains("secrets."),
            "agent-sandbox.yml:{n} references a secret: {line}"
        );
        assert!(
            !line.starts_with("id-token:"),
            "agent-sandbox.yml:{n} requests an OIDC token: {line}"
        );
        assert!(
            !line.starts_with("environment:"),
            "agent-sandbox.yml:{n} binds an environment, and with it secrets: {line}"
        );
        assert!(
            !line.starts_with("secrets:"),
            "agent-sandbox.yml:{n} passes secrets: {line}"
        );
    }
}

#[test]
fn every_checkout_in_the_agent_sandbox_drops_the_repository_token() {
    let lines = code_lines(&sandbox());
    let checkouts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, (_, l))| l.starts_with("- uses: actions/checkout@"))
        .map(|(i, _)| i)
        .collect();
    assert!(!checkouts.is_empty(), "premise: the sandbox checks code out");
    for i in checkouts {
        let window = &lines[i + 1..(i + 6).min(lines.len())];
        let kept = window
            .iter()
            .take_while(|(_, l)| !l.starts_with("- "))
            .any(|(_, l)| l == "persist-credentials: false");
        assert!(
            kept,
            "the checkout at agent-sandbox.yml:{} keeps the token, so lane code could push",
            lines[i].0
        );
    }
}
