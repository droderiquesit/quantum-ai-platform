//! R12-WORLD-3084 / ADR 0133 decision 2 / ADR 0506: the lattice's
//! microsecond row is promoted to the local reflex and nothing else, so
//! nothing central reads a `ReflexForecast`.
//!
//! The contract's one-variant `ReflexPromotion` makes a record that *claims*
//! another target undecodable. That does not stop a central crate from
//! importing the type and sizing, routing or allocating on a reflex forecast
//! that honestly says it is the reflex's — which is exactly "promoted beyond
//! the local reflex" by a reader rather than a writer. This suite closes the
//! reader side: outside the contract crate that defines it, the edge crate
//! that issues it and the edge node that hosts the issuer, no production
//! source in the workspace names it.
//!
//! The centre's after-the-fact scoring (ADR 0133) is not an exception
//! written in advance. When it lands it scores outcomes, not forecasts it
//! acts on, and this list is where it argues its way in — in a reviewed diff
//! naming the crate and why — rather than a crate quietly starting to read.

use qip_acceptance::{files_with_extension, production_text, repository_root};

/// The crates that may name a reflex forecast in production code: where it
/// is defined, where it is issued, and the node that hosts the issuer.
const READERS: [&str; 3] = ["qip-contracts", "qip-edge", "qip-edge-node"];

/// Inside `qip-edge`, the only files that may name it: the issuer and the
/// module declaration. ADR 0195 decision 1 adds no second forecast form to
/// the cell's decisions, so `Cell::work`, its gates and its sizing do not
/// read a reflex forecast; one that started to would be a second decision
/// input under a record's name, and it is refused here until an ADR says
/// otherwise.
const EDGE_FILES: [&str; 2] = ["qip-edge/src/reflex_forecast.rs", "qip-edge/src/lib.rs"];

/// The names a production reader would have to write.
const NAMES: [&str; 2] = ["ReflexForecast", "reflex_forecast"];

/// `backend/crates/<group>/<crate>/src/...` to `<crate>`.
fn crate_of(path: &std::path::Path) -> Option<String> {
    let crates = repository_root().join("backend/crates");
    let rel = path.strip_prefix(&crates).ok()?;
    let mut parts = rel.components();
    parts.next()?;
    Some(parts.next()?.as_os_str().to_string_lossy().into_owned())
}

#[test]
fn nothing_central_reads_a_reflex_forecast() {
    let files = files_with_extension("backend/crates", "rs");
    let mut issuer_seen = false;
    let mut scanned = 0usize;
    let mut readers = Vec::new();
    let mut decision_readers = Vec::new();
    for path in &files {
        // Production sources only: a crate's `tests/` and `benches/` may
        // build a forecast to prove a refusal, and that is not a read.
        let rel = path.to_string_lossy();
        if !rel.contains("/src/") {
            continue;
        }
        let Some(krate) = crate_of(path) else {
            continue;
        };
        let source = std::fs::read_to_string(path).unwrap_or_default();
        let text = production_text(&source);
        scanned += 1;
        let names_it = NAMES.iter().any(|n| text.contains(n));
        if krate == "qip-edge" && rel.ends_with("src/reflex_forecast.rs") && names_it {
            issuer_seen = true;
        }
        if names_it && !READERS.contains(&krate.as_str()) {
            readers.push(rel.into_owned());
        } else if names_it && krate == "qip-edge" && !EDGE_FILES.iter().any(|f| rel.ends_with(f)) {
            decision_readers.push(rel.into_owned());
        }
    }
    // Premise: the scan reached production sources and saw the issuer name
    // the contract, so an empty reader list is a finding, not a scan that
    // read nothing.
    assert!(scanned > 100, "premise: scanned only {scanned} sources");
    assert!(
        issuer_seen,
        "premise: qip-edge's reflex_forecast.rs did not name the contract; the scan or the \
         issuer has moved"
    );
    assert!(
        readers.is_empty(),
        "these production sources outside {READERS:?} name a reflex forecast; the microsecond \
         row is promoted to the local reflex only (R12-WORLD-3084, ADR 0506): {readers:#?}"
    );
    assert!(
        decision_readers.is_empty(),
        "these qip-edge sources name a reflex forecast outside the issuer; under ADR 0195 \
         decision 1 and ADR 0506 it is a record and evidence, not an input to the cell's \
         decisions: {decision_readers:#?}"
    );
}
