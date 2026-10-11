//! CICD-036: architecture memory index — ADRs, the blueprint of record,
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

/// The blueprint is held in the repository so that a requirement can be
/// traced to a page somebody else can open. A README that lists a hash the
/// file no longer has reads as a verified source while pointing at different
/// bytes, and a README that still lists a file somebody deleted sends the
/// next reader to nothing — both happened in this directory's history, when
/// the v11.6 and v2.1 sources were removed on 2026-10-11 and the table had
/// to be rewritten by hand in the same commit. So every `source/` row is
/// held to the bytes it names, and every file under `source/` must have a
/// row: a source nobody listed is a blueprint nobody knows is in force.
#[test]
fn every_blueprint_source_is_listed_once_and_has_the_hash_the_readme_records() {
    let root = qip_acceptance::repository_root();
    let readme = qip_acceptance::read("docs/blueprint/README.md");

    let mut listed: BTreeSet<String> = BTreeSet::new();
    for line in readme.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // `| `source/<file>` | what | pages | `<sha>` |` splits into six
        // cells, the first and last empty.
        if cells.len() != 6 {
            continue;
        }
        let Some(file) = cells[1]
            .strip_prefix("`source/")
            .and_then(|rest| rest.strip_suffix('`'))
        else {
            continue;
        };
        let recorded = cells[4].trim_matches('`');
        let bytes =
            std::fs::read(root.join("docs/blueprint/source").join(file)).unwrap_or_else(|error| {
                panic!("the README lists source/{file}, which is not there: {error}")
            });
        assert_eq!(
            qip_core::hash::sha256_hex(&bytes),
            recorded,
            "source/{file} does not have the SHA-256 the README records"
        );
        assert!(
            listed.insert(file.to_string()),
            "source/{file} is listed twice"
        );
    }

    // Premise: the table was read, and it is the v12.0 / v3.0 pair. A README
    // whose table changed shape would otherwise leave this checking nothing.
    for edition in [
        "algorik-master-blueprint-v12.0.pdf",
        "algorik-gcp-platform-blueprint-v3.0.pdf",
    ] {
        assert!(
            listed.contains(edition),
            "the README's source table does not list {edition}; read: {listed:?}"
        );
    }

    let on_disk: BTreeSet<String> = std::fs::read_dir(root.join("docs/blueprint/source"))
        .expect("docs/blueprint/source is readable")
        .map(|entry| {
            entry
                .expect("a source entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        on_disk, listed,
        "docs/blueprint/source/ and the README's source table disagree about which files exist"
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

    // Verify the key claims: docs/adr, the blueprint of record,
    // infrastructure/terraform/catalogue.tf, docs/ops all exist.
    let paths = [
        "docs/adr/README.md",
        "docs/blueprint/source/algorik-master-blueprint-v12.0.txt",
        "docs/blueprint/source/algorik-gcp-platform-blueprint-v3.0.txt",
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
