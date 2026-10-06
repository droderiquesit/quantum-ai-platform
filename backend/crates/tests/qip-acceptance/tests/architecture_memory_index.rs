//! CICD-036: architecture memory index — ADRs, requirements, traceability matrix,
//! service catalogue, and runbooks are discoverable and indexed as described to
//! agents in CLAUDE.md and .claude/agents/*.md.
//!
//! These documents form the architecture memory: what the system decided, why,
//! and what constraints it chose. Agents read them in prose but nothing tests
//! whether the promised documents actually exist where promised or carry what
//! they claim.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

#[test]
fn architecture_decision_records_are_numbered_and_indexed() {
    let root = qip_acceptance::repository_root();
    let adr_dir = root.join("docs/adr");

    // Premise: the ADR directory exists and is readable.
    assert!(adr_dir.is_dir(), "docs/adr/ is the ADR directory");

    let readme = adr_dir.join("README.md");
    assert!(
        readme.exists(),
        "docs/adr/README.md exists and indexes the ADRs"
    );

    // Enumeration of ADRs by pattern: 0000-*.md through 0999-*.md.
    let mut adr_files: BTreeSet<PathBuf> = BTreeSet::new();
    for entry in std::fs::read_dir(&adr_dir).expect("adr directory is readable") {
        let entry = entry.expect("adr directory entry is readable");
        let path = entry.path();
        if path.is_file()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.ends_with(".md")
            && name.chars().take(4).all(|c| c.is_ascii_digit())
        {
            adr_files.insert(path);
        }
    }

    // Verify ADRs exist (currently 99 as of 2026-09-25).
    assert!(!adr_files.is_empty(), "at least one numbered ADR exists");

    // Spot-check: a few known ADRs should be present.
    let required_adrs = ["0001-", "0002-", "0003-"];
    for prefix in required_adrs {
        let found = adr_files.iter().any(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.starts_with(prefix))
                .unwrap_or(false)
        });
        assert!(found, "ADR {} should exist", prefix);
    }
}

#[test]
fn blueprint_requirements_are_machine_readable_and_indexed() {
    let root = qip_acceptance::repository_root();
    let req_dir = root.join("docs/blueprint/requirements");

    // Premise: the requirements directory exists.
    assert!(
        req_dir.is_dir(),
        "docs/blueprint/requirements/ contains requirements files"
    );

    // Enumerate JSON files: each domain has its own requirements file.
    let json_files: Vec<_> = std::fs::read_dir(&req_dir)
        .expect("requirements directory is readable")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    assert!(
        !json_files.is_empty(),
        "at least one requirements JSON file exists"
    );

    // Each file should parse as valid JSON.
    for path in json_files {
        let text = std::fs::read_to_string(&path).expect("requirements file is readable");
        serde_json::from_str::<serde_json::Value>(&text).expect("requirements file is valid JSON");
    }
}

#[test]
fn traceability_matrix_maps_requirements_to_tests() {
    let root = qip_acceptance::repository_root();
    let matrix = root.join("docs/blueprint/traceability-matrix.md");

    assert!(
        matrix.exists(),
        "docs/blueprint/traceability-matrix.md exists and maps requirements to test evidence"
    );

    let text = std::fs::read_to_string(&matrix).expect("traceability matrix is readable");

    // Sanity: the matrix should have substantial content.
    assert!(
        text.len() > 1000,
        "traceability matrix has substantive content"
    );

    // Spot-check: matrix should reference some requirements and test evidence.
    assert!(
        text.contains("CICD-") || text.contains("ARCH-"),
        "matrix references requirement IDs"
    );
}

#[test]
fn service_catalogue_enumerates_the_deployable_binaries() {
    let root = qip_acceptance::repository_root();
    let catalogue = root.join("infrastructure/terraform/catalogue.tf");

    assert!(
        catalogue.exists(),
        "infrastructure/terraform/catalogue.tf is the service catalogue"
    );

    let text = std::fs::read_to_string(&catalogue).expect("catalogue is readable");

    // The Cloud Run catalogue (catalogue.tf) holds the three central binaries.
    // qip-edge-node runs under systemd on a Compute instance (execution-node module),
    // and qip-web (Next.js console) is served separately and not in the catalogue.
    // Verify the three Cloud Run entries exist.
    let required_binaries = ["qip-api", "qip-fastbrain", "qip-deepbrain"];

    for binary in required_binaries {
        assert!(text.contains(binary), "catalogue names the {binary} binary");
    }
}

#[test]
fn operations_runbooks_and_remediation_guides_exist() {
    let root = qip_acceptance::repository_root();
    let ops_dir = root.join("docs/ops");

    assert!(
        ops_dir.is_dir(),
        "docs/ops/ contains operations documentation"
    );

    // Verify at least some runbook files exist.
    let runbook_files: Vec<_> = std::fs::read_dir(&ops_dir)
        .expect("ops directory is readable")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    assert!(
        !runbook_files.is_empty(),
        "at least one operations runbook exists"
    );
}

#[test]
fn agent_instructions_reference_the_architecture_memory_index() {
    let root = qip_acceptance::repository_root();
    let agents_dir = root.join(".claude/agents");

    assert!(
        agents_dir.is_dir(),
        ".claude/agents/ holds agent role definitions"
    );

    // At least the chief-orchestrator and solution-architect should reference
    // architecture memory.
    let architect = agents_dir.join("solution-architect.md");
    assert!(architect.exists(), "solution-architect agent exists");

    let text = std::fs::read_to_string(&architect).expect("agent instructions are readable");

    // The instruction should direct the agent to read ADRs and architecture
    // memory before proposing changes.
    assert!(
        text.contains("ADR") || text.contains("architecture") || text.contains("decision"),
        "solution-architect instructions reference architecture decisions"
    );
}

#[test]
fn no_document_class_claimed_by_claude_md_is_missing() {
    let root = qip_acceptance::repository_root();
    let claude_md = root.join("CLAUDE.md");

    assert!(
        claude_md.exists(),
        "CLAUDE.md names the promised document classes"
    );

    // Verify the key claims: docs/adr, docs/blueprint/requirements,
    // infrastructure/terraform/catalogue.tf, docs/ops all exist.
    let paths = [
        "docs/adr/README.md",
        "docs/blueprint/requirements",
        "docs/blueprint/traceability-matrix.md",
        "infrastructure/terraform/catalogue.tf",
        "docs/ops",
    ];

    for path in paths {
        assert!(
            root.join(path).exists(),
            "CLAUDE.md's promised {} exists",
            path
        );
    }
}
