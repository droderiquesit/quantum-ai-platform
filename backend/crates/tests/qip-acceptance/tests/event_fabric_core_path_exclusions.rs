#![allow(clippy::unwrap_used, clippy::expect_used)]
//! FABRIC-102 and FABRIC-103: Eventarc and Cloud Tasks are not part of the
//! platform's internal eventing.
//!
//! Neither exists in the tree today, which is the strongest form of the rule
//! and not a gap in it. What was missing was anything that would notice the
//! first one arriving: a convenient `google_eventarc_trigger` aimed at a
//! warm service reintroduces a managed broker on the internal path, with
//! semantics somebody else can change, and review would have to spot it by
//! eye. This scan reads every Terraform file and workflow instead.

use qip_acceptance::{files_with_extension, read, repository_root};

/// Resource-type prefixes and API names that put either product on the path.
const FORBIDDEN: [(&str, &str); 4] = [
    ("google_eventarc_", "FABRIC-102 (Eventarc)"),
    ("eventarc.googleapis.com", "FABRIC-102 (Eventarc)"),
    ("google_cloud_tasks_", "FABRIC-103 (Cloud Tasks)"),
    ("cloudtasks.googleapis.com", "FABRIC-103 (Cloud Tasks)"),
];

/// Lines with their `#` comments removed, so prose that names a product to
/// say it is excluded does not trip the scan.
fn code(text: &str) -> String {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which forbidden names `text` uses, once per name.
fn offences(text: &str) -> Vec<&'static str> {
    let body = code(text).to_lowercase();
    FORBIDDEN
        .iter()
        .filter(|(needle, _)| body.contains(needle))
        .map(|(needle, _)| *needle)
        .collect()
}

fn scanned() -> Vec<(String, String)> {
    let root = repository_root();
    let mut out = Vec::new();
    for (dir, ext) in [
        ("infrastructure", "tf"),
        ("infrastructure", "tfvars"),
        (".github/workflows", "yml"),
    ] {
        for path in files_with_extension(dir, ext) {
            if path.components().any(|c| c.as_os_str() == ".terraform") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let rel = path.strip_prefix(&root).unwrap().display().to_string();
            out.push((rel, text));
        }
    }
    out
}

/// The scan fires on each forbidden name and ignores prose; without this a
/// scan that matched nothing would pass forever.
#[test]
fn the_scan_names_eventarc_and_cloud_tasks_and_ignores_comments() {
    assert_eq!(
        offences("resource \"google_eventarc_trigger\" \"t\" {}"),
        vec!["google_eventarc_"]
    );
    assert_eq!(
        offences("services = [\"cloudtasks.googleapis.com\"]"),
        vec!["cloudtasks.googleapis.com"]
    );
    assert_eq!(
        offences("resource \"google_cloud_tasks_queue\" \"q\" {}"),
        vec!["google_cloud_tasks_"]
    );
    assert!(
        offences("# no google_eventarc_trigger here\nresource \"google_pubsub_topic\" \"r\" {}")
            .is_empty()
    );
}

/// No Terraform or workflow file provisions Eventarc or Cloud Tasks.
///
/// Mutation: add `resource "google_eventarc_trigger" "x" {}` to any `.tf`
/// under `infrastructure/` — fails naming that file and FABRIC-102.
#[test]
fn no_terraform_or_workflow_file_provisions_eventarc_or_cloud_tasks() {
    let files = scanned();
    // Premise: the walk found real Terraform and a real workflow.
    assert!(files.len() > 20, "only {} files scanned", files.len());
    assert!(files.iter().any(|(p, _)| p.ends_with("main.tf")));
    assert!(
        files
            .iter()
            .any(|(p, _)| p.starts_with(".github/workflows/"))
    );
    assert!(read("infrastructure/terraform/main.tf").contains("module"));

    let mut found = Vec::new();
    for (path, text) in &files {
        for needle in offences(text) {
            let why = FORBIDDEN.iter().find(|(n, _)| *n == needle).unwrap().1;
            found.push(format!(
                "{path} uses {needle}, which {why} forbids on the internal path"
            ));
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}
