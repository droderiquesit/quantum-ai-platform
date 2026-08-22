//! Checks that what the documentation claims matches what the code does.
//!
//! Documentation that has drifted from the code is worse than none, because
//! someone will believe it. These tests check the specific, checkable claims —
//! counts, defaults, names, thresholds — rather than trying to verify prose.
//!
//! Each test fails in one of two ways, and both are useful: the code changed
//! and the documentation did not, or the documentation was wrong to begin with.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read, repository_root};

// --- claims about the loop --------------------------------------------------

#[test]
fn the_readme_names_the_stages_the_code_actually_has() {
    let readme = read("README.md");
    let stages = qip_kernel::cycle::Stage::all();
    assert_eq!(stages.len(), 8, "the loop has eight stages");

    for stage in &stages {
        let upper = stage.as_str().to_uppercase();
        assert!(
            readme.contains(&upper),
            "the README does not mention the {upper} stage"
        );
    }

    // And the diagram is in the right order.
    let arrow_diagram = readme
        .lines()
        .find(|line| line.contains("SENSE") && line.contains("LEARN"))
        .expect("the README diagrams the loop");
    let mut cursor = 0usize;
    for stage in &stages {
        let upper = stage.as_str().to_uppercase();
        let position = arrow_diagram[cursor..]
            .find(&upper)
            .unwrap_or_else(|| panic!("{upper} is out of order in the diagram"));
        cursor += position + upper.len();
    }
}

#[test]
fn the_architecture_document_lists_a_crate_for_every_stage() {
    let architecture = read("docs/architecture/README.md");
    for stage in qip_kernel::cycle::Stage::all() {
        assert!(
            architecture.contains(&stage.as_str().to_uppercase()),
            "the architecture document does not cover {}",
            stage.as_str()
        );
    }
}

// --- claims about safety ----------------------------------------------------

#[test]
fn the_documented_default_autonomy_level_is_the_actual_default() {
    // The single most consequential claim in the documentation.
    let level = qip_risk_engine::autonomy::AutonomyLevel::DEFAULT;
    assert_eq!(
        level,
        qip_risk_engine::autonomy::AutonomyLevel::PaperTrading
    );

    for document in ["README.md", "docs/adr/0003-paper-trading-by-default.md"] {
        let content = read(document);
        assert!(
            content.contains("paper trading") || content.contains("PaperTrading"),
            "{document} does not state the default"
        );
    }
}

#[test]
fn the_documented_autonomy_levels_are_the_ones_the_code_declares() {
    let levels = qip_risk_engine::autonomy::AutonomyLevel::all();
    let variables = read("infrastructure/terraform/variables.tf");
    let operations = read("docs/operations/enabling-live-trading.md");

    for level in &levels {
        assert!(
            variables.contains(level.as_str()),
            "the Terraform ceiling variable does not accept {}",
            level.as_str()
        );
    }
    // The runbook names the level it tells an operator to set.
    assert!(operations.contains("supervised_live"));
}

#[test]
fn the_readme_lists_a_control_for_every_control_the_code_enforces() {
    let readme = read("README.md");
    for claim in [
        // 1: paper trading by default.
        "AutonomyLevel::DEFAULT",
        // 2: two operators.
        "second approver",
        // 3: no self-escalation.
        "change_autonomy_level",
        // 4: separation of duties.
        "AgentManifest::validate",
        // 5: numeric provenance.
        "observed",
        // 6: the asymmetric kill switch.
        "kill switch",
    ] {
        assert!(
            readme.contains(claim),
            "the README's safety section does not mention {claim}"
        );
    }
}

#[test]
fn no_agent_holds_the_capability_the_readme_says_none_holds() {
    // The README claims no agent of any role may hold `change_autonomy_level`.
    // The roster is the check.
    let roster = qip_investment_agents::manifests::roster(qip_core::Timestamp::from_secs(0));
    assert!(
        roster
            .holding(qip_agents::capability::Capability::ChangeAutonomyLevel)
            .is_empty(),
        "an agent holds change_autonomy_level, which the README says is impossible"
    );
    assert!(
        roster
            .holding(qip_agents::capability::Capability::OverrideRiskLimit)
            .is_empty()
    );
}

#[test]
fn the_documented_agent_count_matches_the_roster() {
    let roster = qip_investment_agents::manifests::roster(qip_core::Timestamp::from_secs(0));
    let count = roster.len();

    for document in ["README.md", "docs/architecture/README.md"] {
        let content = read(document).to_lowercase();
        assert!(
            content.contains(&format!("{count} ")) || content.contains("eighteen"),
            "{document} does not state the agent count of {count}"
        );
    }
}

// --- claims about dependencies ----------------------------------------------

#[test]
fn the_documented_dependencies_are_the_ones_in_the_manifest() {
    // The README and ADR 0002 both claim serde and serde_json and nothing
    // else. The workspace manifest is the authority.
    let manifest = read("Cargo.toml");
    let dependencies: Vec<&str> = manifest
        .split("[workspace.dependencies]")
        .nth(1)
        .expect("the workspace declares its dependencies")
        .lines()
        .take_while(|line| !line.trim().starts_with('['))
        .filter_map(|line| line.split('=').next())
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.starts_with('#') && !name.starts_with("qip-"))
        .collect();

    assert_eq!(
        dependencies,
        vec!["serde", "serde_json"],
        "the third-party dependencies have changed; the README and ADR 0002 need updating"
    );

    for document in ["README.md", "docs/adr/0002-two-dependencies.md"] {
        let content = read(document);
        assert!(content.contains("serde"), "{document} does not name serde");
        assert!(
            content.contains("serde_json"),
            "{document} does not name serde_json"
        );
    }
}

#[test]
fn the_documented_lockfile_size_is_current() {
    // ADR 0002 claims the lockfile is small enough to read. Checking the claim
    // rather than the sentiment.
    let lockfile = read("Cargo.lock");
    let third_party = lockfile
        .lines()
        .filter(|line| line.starts_with("name = "))
        .filter(|line| !line.contains("\"qip-"))
        .count();

    let adr = read("docs/adr/0002-two-dependencies.md");
    assert!(
        adr.contains(&format!("{third_party} packages")),
        "ADR 0002 claims a lockfile size that is no longer {third_party}. \
         The count is written as a numeral on purpose: a document saying \
         \"eleven\" would survive the count becoming twelve."
    );
}

// --- claims about the API ---------------------------------------------------

#[test]
fn every_documented_endpoint_exists() {
    let operations = files_with_extension("docs/operations", "md");
    let mut mentioned: Vec<String> = Vec::new();
    for path in &operations {
        let content = std::fs::read_to_string(path).expect("readable");
        for line in content.lines() {
            for token in line.split_whitespace() {
                let token = token.trim_matches(|c: char| !c.is_ascii_graphic());
                if let Some(index) = token.find("/api/v1") {
                    mentioned.push(token[index..].trim_end_matches(['`', '"', ')']).to_string());
                }
            }
        }
    }
    assert!(!mentioned.is_empty(), "the runbooks reference no endpoints");

    for path in mentioned {
        let suffix = path.trim_start_matches("/api/v1");
        if suffix.is_empty() {
            continue;
        }
        assert!(
            qip_api::ROUTES.iter().any(|route| route.pattern == suffix),
            "a runbook references {path}, which is not a route"
        );
    }
}

#[test]
fn the_documented_role_names_are_the_ones_the_code_defines() {
    let readme = read("docs/operations/README.md");
    for role in [
        qip_api::Role::Monitor,
        qip_api::Role::Viewer,
        qip_api::Role::Operator,
    ] {
        let variable = format!("QIP_TOKEN_{}", role.as_str().to_uppercase());
        assert!(
            readme.contains(&variable) || read("README.md").contains(&variable),
            "{variable} is not documented"
        );
    }
}

// --- claims about the escalation policy -------------------------------------

#[test]
fn the_runbook_describes_the_escalation_the_monitor_actually_performs() {
    let runbook = read("docs/operations/limit-breach.md");
    let policy = qip_risk_engine::monitor::MonitorPolicy::default();

    assert!(
        runbook.contains(&format!("{}", policy.breaches_before_halt)),
        "the runbook does not state the {} consecutive breaches before a halt",
        policy.breaches_before_halt
    );
    assert!(
        runbook.contains("Reduce-only"),
        "the runbook does not describe the reduce-only state"
    );
    // And the claim that reduce-only still permits reductions.
    assert!(
        qip_risk_engine::monitor::MonitorAction::ReduceOnly {
            breaches: Vec::new()
        }
        .permits_reduction(),
        "the runbook says reduce-only permits reductions and the code disagrees"
    );
}

#[test]
fn the_runbook_states_the_credential_freshness_the_code_requires() {
    // Fifteen minutes, in both places.
    let runbook = read("docs/operations/enabling-live-trading.md");
    assert!(
        runbook.contains("fifteen minutes"),
        "the runbook does not state the credential freshness requirement"
    );

    // And the code enforces it: a credential from an hour ago is refused.
    let now = qip_core::Timestamp::from_secs(1_000_000);
    let operator = qip_risk_engine::autonomy::OperatorIdentity::verified("a", "token", now)
        .with_second_approver("b");
    let mut controller = qip_risk_engine::autonomy::AutonomyController::with_live_ceiling(
        qip_risk_engine::autonomy::AutonomyLevel::SupervisedLive,
    );
    let much_later = now.saturating_add(qip_core::Duration::from_hours(1));
    assert!(
        controller
            .request_change(
                qip_risk_engine::autonomy::AutonomyLevel::SupervisedLive,
                &operator,
                "enabling live trading for the pilot",
                much_later,
            )
            .is_err(),
        "the code does not enforce the freshness the runbook promises"
    );
}

// --- the documentation's own hygiene ----------------------------------------

#[test]
fn every_internal_link_resolves() {
    // A broken link in a runbook is found by the person who needed it.
    let root = repository_root();
    let mut checked = 0usize;
    for path in files_with_extension("docs", "md")
        .into_iter()
        .chain(files_with_extension("ops", "md"))
        .chain([root.join("README.md")])
    {
        let content = std::fs::read_to_string(&path).expect("readable");
        let directory = path.parent().expect("a file has a parent");
        for line in content.lines() {
            let mut rest = line;
            while let Some(open) = rest.find("](") {
                let after = &rest[open + 2..];
                let Some(close) = after.find(')') else { break };
                let target = &after[..close];
                rest = &after[close..];

                if target.starts_with("http") || target.starts_with('#') {
                    continue;
                }
                let target = target.split('#').next().unwrap_or(target);
                let resolved = directory.join(target);
                assert!(
                    resolved.exists(),
                    "{} links to {target}, which does not exist",
                    path.display()
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 5, "only {checked} internal links were checked");
}

#[test]
fn every_decision_record_states_what_it_costs() {
    // A decision with no stated cost has not been thought about.
    for path in files_with_extension("docs/adr", "md") {
        if path.file_name().is_some_and(|name| name == "README.md") {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("readable");
        assert!(
            content.contains("## What it costs"),
            "{} does not state what the decision costs",
            path.display()
        );
        assert!(
            content.contains("## What would make this wrong"),
            "{} states no condition under which it should be revisited",
            path.display()
        );
    }
}

#[test]
fn every_decision_record_is_listed_in_the_index() {
    let index = read("docs/adr/README.md");
    for path in files_with_extension("docs/adr", "md") {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == "README.md" {
            continue;
        }
        assert!(
            index.contains(name),
            "{name} is not listed in the decision index"
        );
    }
}

#[test]
fn no_document_promises_something_the_platform_does_not_do() {
    // A narrow check for the specific overclaims that matter: anything
    // presenting the platform as production-ready or as having demonstrated
    // quantum advantage.
    for path in files_with_extension("docs", "md")
        .into_iter()
        .chain(files_with_extension("ops", "md"))
        .chain([repository_root().join("README.md")])
    {
        let content = std::fs::read_to_string(&path).expect("readable");
        let lowered = content.to_lowercase();
        for claim in [
            "production ready",
            "production-ready",
            "quantum advantage over",
            "guaranteed profit",
            "battle tested",
            "battle-tested",
        ] {
            assert!(
                !lowered.contains(claim),
                "{} claims \"{claim}\", which the platform has not demonstrated",
                path.display()
            );
        }
    }
}
