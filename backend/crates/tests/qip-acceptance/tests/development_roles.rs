//! R12-CICD-10168: phase 9's exit test, named before the phase is entered.
//!
//! The requirement's autonomous-development loop is planner, coder, test,
//! security, reviewer, release and SRE agents running "through normal CI and
//! GitOps inside a sandbox with budgets". The owner's ruling (decision 10,
//! 2026-10-31) keeps ADR 0102's fleet at its eight roles, with no security,
//! merge or release role in it: the seven roles are the development agents
//! under `.claude/agents`, each acting only through a pull request that `ci`
//! and `agent boundary` gate, and a release is a person's dispatch of
//! `deploy.yml`, because no agent merges or deploys (GOV-9067). This test
//! holds that shape, so a change that hands the fleet a release role or lets
//! an agent dispatch a deploy fails here.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_acceptance::{read, repository_root};
use serde_json::Value;

/// Each of the requirement's seven roles, and the development agents that
/// play it.
const ROLES: [(&str, &[&str]); 7] = [
    (
        "planner",
        &[
            "product-manager",
            "chief-orchestrator",
            "solution-architect",
        ],
    ),
    (
        "coder",
        &[
            "backend-engineer",
            "frontend-engineer",
            "data-ai-engineer",
            "cloud-platform-engineer",
        ],
    ),
    ("test", &["test-engineer"]),
    ("security", &["security-engineer"]),
    ("reviewer", &["code-reviewer"]),
    ("release", &["sre-release-engineer"]),
    ("SRE", &["sre-release-engineer"]),
];

/// Mutation: drop `deploy.yml` from `dispatch_refused_for_agents`, or remove
/// one role's agent file. This fails naming it.
#[test]
fn phase_9_roles_are_development_agents_that_act_only_through_ci() {
    let root = repository_root();
    for (role, agents) in ROLES {
        assert!(
            agents
                .iter()
                .any(|agent| root.join(format!(".claude/agents/{agent}.md")).is_file()),
            "the {role} role has no development agent under .claude/agents ({agents:?})"
        );
    }

    // The sandbox and the budget ceiling exist.
    assert!(
        root.join(".github/workflows/agent-sandbox.yml").is_file(),
        "the agent sandbox workflow is gone"
    );
    let dispatch = read("scripts/fleet/dispatch.mjs");
    assert!(
        dispatch.contains("--daily-usd-ceiling") && dispatch.contains("ceilingRefusal"),
        "the fleet's daily budget ceiling is gone from scripts/fleet/dispatch.mjs"
    );

    // The fleet holds no security, merge or release role (ADR 0102).
    let worker = read("scripts/fleet/worker.mjs");
    let line = worker
        .lines()
        .find(|line| line.starts_with("export const ROLES"))
        .expect("premise: scripts/fleet/worker.mjs exports its ROLES");
    let roster: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
    assert_eq!(
        roster.len(),
        8,
        "premise: ADR 0102's eight roles: {roster:?}"
    );
    for forbidden in ["security", "merge", "release"] {
        assert!(
            !roster.iter().any(|role| role.contains(forbidden)),
            "the fleet's roster names a {forbidden} role: {roster:?}"
        );
    }

    // No agent identity may start a deploy, an infrastructure change or the
    // fleet itself.
    let policy: Value = serde_json::from_str(&read("scripts/agent-policy.json"))
        .expect("scripts/agent-policy.json parses");
    let refused: Vec<&str> = policy["dispatch_refused_for_agents"]["workflows"]
        .as_array()
        .expect("premise: the policy lists workflows refused to agents")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for workflow in ["deploy.yml", "infra.yml", "fleet.yml"] {
        assert!(
            refused.contains(&workflow),
            "{workflow} is no longer refused to agent identities: {refused:?}"
        );
    }
}
