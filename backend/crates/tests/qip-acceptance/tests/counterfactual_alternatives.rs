//! R12-CAPITAL-4091 (decision 17 of 2026-10-31): every central composition
//! root loads the committed counterfactual alternatives, so the different-
//! hedge and different-venue arms come from one declaration on every
//! default path rather than only where an operator set a hedge policy or an
//! arbitrage whitelist, which no environment does.
//!
//! Read from the roots' source, comments stripped, because `main` cannot be
//! run in a test; the kernel's own suite proves the declaration parses and
//! that a missing arm fails closed.

use std::path::PathBuf;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn without_line_comments(text: &str) -> String {
    text.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Mutation: delete the `with_counterfactual_alternatives(...committed()?)`
/// call from any one root and that root is named here.
#[test]
fn every_central_root_loads_the_committed_counterfactual_alternatives() {
    let call = "CounterfactualAlternatives::committed()?";
    let roots = [
        "backend/crates/apps/qip-api/src/main.rs",
        "backend/crates/apps/qip-fastbrain/src/main.rs",
        "backend/crates/apps/qip-deepbrain/src/main.rs",
    ];
    for root in roots {
        let text = std::fs::read_to_string(repository_root().join(root))
            .unwrap_or_else(|error| panic!("cannot read {root}: {error}"));
        let code = without_line_comments(&text);
        assert!(
            code.contains(".with_counterfactual_alternatives(") && code.contains(call),
            "{root} does not load the committed counterfactual alternatives into its \
             configuration, so its scored decisions carry no declared venue or hedge arm and no \
             stated reason for their absence"
        );
    }
}
