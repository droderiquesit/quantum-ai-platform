#![allow(clippy::unwrap_used, clippy::expect_used)]
//! FABRIC-043: no global exactly-once delivery claim.
//!
//! The fabric is at-least-once transport plus idempotent producers, fencing
//! and idempotent sinks. "Exactly-once" as a delivery property is the claim
//! ADR 0100 and every fabric module doc refuse, and until this test existed
//! that refusal was an inspection somebody had to remember to repeat. The
//! scan below fails the build on the commit that writes the claim, in the
//! source of every crate the fabric is built from and in ADR 0100.
//!
//! What counts as a claim: the hyphenated term of art `exactly-once`, or the
//! words `exactly once` on a line that also speaks of delivery. What does
//! not: a mention that negates it, which is how every module states the
//! refusal ("not exactly-once", "No exactly-once"). A mention is negated when
//! it, or one of the two lines above it (a wrapped doc paragraph), carries a
//! negating word. Mechanism statements such as "runs exactly once" or "every
//! sequence is applied exactly once" are about idempotency, not delivery, and
//! are not matched.

use std::fs;
use std::path::{Path, PathBuf};

const SCANNED: [&str; 7] = [
    "libs/qip-events/src",
    "libs/qip-transport/src",
    "services/qip-streaming/src",
    "services/qip-mesh/src",
    "apps/qip-fabricd/src",
    "apps/qip-edge-node/src",
    "apps/qip-cli/src",
];

const NEGATIONS: [&str; 8] = [
    "not ", "no ", "never", "neither", "without", "refus", "claims", "nothing",
];

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.map(|e| e.expect("entry").path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs" || x == "md") {
            out.push(p);
        }
    }
}

fn is_claim(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("exactly-once") || (lower.contains("exactly once") && lower.contains("deliver"))
}

fn is_negated(lines: &[&str], index: usize) -> bool {
    let from = index.saturating_sub(2);
    lines[from..=index].iter().any(|l| {
        // Markdown emphasis (`**not**`) must not hide the word from the match.
        let lower = format!("{} ", l.to_lowercase().replace('*', " "));
        NEGATIONS.iter().any(|n| lower.contains(n))
    })
}

/// Every un-negated claim in `text`, as `(line number, line)`.
fn claims(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    (0..lines.len())
        .filter(|&i| is_claim(lines[i]) && !is_negated(&lines, i))
        .map(|i| (i + 1, lines[i].trim().to_string()))
        .collect()
}

fn scanned_files() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    for rel in SCANNED {
        files(&crates.join(rel), &mut out);
    }
    let adr_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../docs/adr");
    let mut adrs = Vec::new();
    files(&adr_dir, &mut adrs);
    out.extend(adrs.into_iter().filter(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("0100"))
    }));
    out
}

/// No scanned file claims exactly-once delivery.
///
/// Mutation: add the doc line `//! Delivery is exactly-once.` to
/// `qip-streaming/src/event_fabric/producer.rs` — fails, naming that file
/// and line, because the line carries the term and nothing negating it.
#[test]
fn no_fabric_source_or_adr_claims_exactly_once_delivery() {
    let paths = scanned_files();
    // Premise: the walk found the fabric's sources and ADR 0100, or an empty
    // walk would pass the assertion below vacuously.
    assert!(paths.len() > 50, "walked only {} files", paths.len());
    assert!(
        paths.iter().any(|p| p.ends_with("0100-event-fabric.md")
            || p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("0100"))),
        "ADR 0100 was not scanned"
    );

    let mut mentions = 0usize;
    let mut offenders = Vec::new();
    for path in &paths {
        let text = fs::read_to_string(path).unwrap_or_default();
        mentions += text.lines().filter(|l| is_claim(l)).count();
        for (line, content) in claims(&text) {
            offenders.push(format!("{}:{line}: {content}", path.display()));
        }
    }
    // Premise: the detector sees the refusals the modules already write
    // ("not exactly-once"); a detector blind to the term would find nothing
    // to refuse either.
    assert!(
        mentions >= 3,
        "the scan found only {mentions} mentions of the term; it is blind"
    );
    assert!(
        offenders.is_empty(),
        "an exactly-once delivery claim is not allowed (FABRIC-043); the fabric is at-least-once \
         plus idempotency and fencing:\n{}",
        offenders.join("\n")
    );
}

/// The detector itself, on fixtures: it refuses a claim, and admits the three
/// negated forms the modules use. Without this a scan that flagged nothing
/// would look the same as a scan that found the tree clean.
#[test]
fn the_detector_refuses_a_claim_and_admits_a_negated_mention() {
    assert_eq!(claims("//! Delivery is exactly-once.").len(), 1);
    assert_eq!(
        claims("//! This transport delivers messages exactly once.").len(),
        1
    );
    assert!(claims("//! Delivery is at-least-once, and **not** exactly-once.").is_empty());
    assert!(claims("//! * **No exactly-once.** Delivery is at-least-once.").is_empty());
    assert!(
        claims("//! A wrapped paragraph that says it does not\n//! promise anything\n//! exactly-once here.")
            .is_empty()
    );
    // A mechanism statement is not a delivery claim.
    assert!(claims("// guaranteed to run exactly once").is_empty());
}
