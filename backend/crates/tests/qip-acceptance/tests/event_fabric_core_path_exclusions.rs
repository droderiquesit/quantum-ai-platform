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

/// `text` with `//` line comments removed, so prose naming a type to say it
/// is not used does not count as using it.
fn rust_code(text: &str) -> String {
    text.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of the first `fn {name}(` in `source`, through its closing brace.
fn body_of<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("no fn {name} in the scanned source"));
    let mut depth = 0usize;
    let mut opened = false;
    for (offset, ch) in source[start..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                opened = true;
            }
            '}' => {
                depth -= 1;
                if opened && depth == 0 {
                    return &source[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("fn {name} never closes");
}

/// COVERAGE-7193: a fabric outage cannot block market-event-to-order
/// execution, because the reflex node's decision thread reaches the fabric
/// only through a bounded handoff it never waits on. Crate tests prove the
/// behaviour (a stalled writer, a stalled store, a silent centre); this
/// proves the composite's shape across the crate boundary: the one handoff
/// the decision thread holds offers with `try_send`, and neither the loop
/// nor the pass names the spool or its writer, so no edit inside either can
/// append to the spool on the decision thread without this failing.
///
/// Mutation: make `HandoffSender::offer` call the blocking `send` and the
/// "waits on the writer" assertion fails; name `SegmentLog` anywhere in
/// `scheduler.rs` outside a comment and the per-file assertion does.
#[test]
fn the_reflex_decision_thread_reaches_the_fabric_only_through_a_handoff_it_never_waits_on() {
    let writer = rust_code(&read(
        "backend/crates/apps/qip-edge-node/src/event_fabric/writer.rs",
    ));
    let offer = body_of(&writer, "offer");
    assert!(
        offer.contains("Offer::Full"),
        "premise: the scan is reading HandoffSender::offer:\n{offer}"
    );
    assert!(
        !offer.replace(".try_send(", "").contains(".send("),
        "HandoffSender::offer waits on the writer:\n{offer}"
    );
    assert!(
        offer.contains("self.inner.try_send("),
        "HandoffSender::offer no longer offers without waiting:\n{offer}"
    );
    assert!(
        writer.contains("sync_channel("),
        "the handoff is no longer a bounded channel"
    );

    for file in [
        "backend/crates/apps/qip-edge-node/src/scheduler.rs",
        "backend/crates/apps/qip-edge-node/src/pass.rs",
    ] {
        let source = rust_code(&read(file));
        assert!(
            source.contains("run_pass") || source.contains("fn turn("),
            "premise: {file} is on the decision thread"
        );
        for name in ["SpoolWriter", "SegmentLog", "HandoffReceiver"] {
            assert!(
                !source.contains(name),
                "{file} names {name}: the decision thread can reach the spool without the handoff"
            );
        }
    }
}
