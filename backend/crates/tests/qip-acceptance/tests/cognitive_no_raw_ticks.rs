//! The cognitive layer takes no raw tick (R12-TICK-2041, ADR 0151 decision 2).
//!
//! Raw market-event storage is the edge's and the lake's: the cognitive
//! layer consumes time-aligned states, regimes, reaction summaries and
//! episodes. `lane_placement.rs` already keeps replay and training crates out
//! of the reflex binary, which is half of the requirement. Nothing kept the
//! other half: the kernel links `qip-simulation-engine` and `qip-contracts`,
//! so a convenience `fn observe_ticks(&mut self, ticks: Vec<MarketMessage>)`
//! in the REASON stage would compile, and the cognitive layer would start
//! reasoning over raw venue messages with every test still green.
//!
//! This suite reads the production text of each cognitive crate and refuses
//! any raw-tick type by name. The names are the types a raw tick travels as
//! in this tree: the venue message, the admitted market event, the lake's
//! decoded tick, the replay tick, the captured lake event, and a decoded
//! protocol packet.

use qip_acceptance::{files_with_extension, production_text, repository_root};

/// The crates that reason: the kernel's cycle, the world model, the
/// reasoning engine, the opportunity engine, the learning engine and the
/// agent crates. Every one sits above the lake and below no venue.
const COGNITIVE: &[&str] = &[
    "backend/crates/runtime/qip-kernel/src",
    "backend/crates/services/qip-world-model/src",
    "backend/crates/services/qip-reasoning-engine/src",
    "backend/crates/services/qip-opportunity-engine/src",
    "backend/crates/services/qip-learning-engine/src",
    "backend/crates/libs/qip-agents/src",
    "backend/crates/agents/qip-investment-agents/src",
];

/// Raw-tick types, as identifiers. `MarketEvent` alone is not here because
/// the opportunity engine's `catalyst::MarketEvent` is a news-derived
/// catalyst, not a tick; the admitted tick is named by its module path.
const RAW_TICK_IDENTIFIERS: &[&str] = &[
    "MarketMessage",
    "RecordedTick",
    "ReplayTick",
    "CapturedMarketEvent",
    "TickLakeWriter",
];
const RAW_TICK_PATHS: &[&str] = &["market_event::MarketEvent", "qip_protocols::"];

fn is_identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Every raw-tick name `text` uses, as a delimited token: `MarketMessage`
/// matches, `MarketMessageCount` does not.
fn raw_tick_names(text: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    for name in RAW_TICK_IDENTIFIERS {
        let hit = text.match_indices(name).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + name.len()..].chars().next();
            !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
        });
        if hit {
            found.push(*name);
        }
    }
    for path in RAW_TICK_PATHS {
        if text.contains(path) {
            found.push(*path);
        }
    }
    found
}

fn sources(relative: &str) -> Vec<(String, String)> {
    files_with_extension(relative, "rs")
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let shown = path
                .strip_prefix(repository_root())
                .unwrap_or(&path)
                .display()
                .to_string();
            (shown, production_text(&text))
        })
        .collect()
}

/// Mutation: add `pub fn observe_ticks(&mut self, _ticks: Vec<qip_contracts::message::MarketMessage>) {}`
/// to `impl Platform` in qip-kernel's platform.rs — this test names the file
/// and `MarketMessage`.
#[test]
fn no_cognitive_crate_takes_a_raw_tick_type() {
    // Premise: the detector sees raw-tick types where they do live, so an
    // empty result below is the cognitive layer's, not a blind scanner's.
    let raw_homes = [
        "backend/crates/libs/qip-contracts/src",
        "backend/crates/libs/qip-storage/src",
        "backend/crates/services/qip-simulation-engine/src",
    ];
    for home in raw_homes {
        let seen = sources(home)
            .iter()
            .any(|(_, text)| !raw_tick_names(text).is_empty());
        assert!(
            seen,
            "premise: no raw-tick type found in {home}; the scan is blind"
        );
    }

    let mut scanned = 0usize;
    let mut offenders = Vec::new();
    for crate_src in COGNITIVE {
        let files = sources(crate_src);
        assert!(
            !files.is_empty(),
            "premise: {crate_src} holds no source; the cognitive list is stale"
        );
        for (path, text) in files {
            scanned += 1;
            let names = raw_tick_names(&text);
            if !names.is_empty() {
                offenders.push(format!("{path}: {names:?}"));
            }
        }
    }
    assert!(
        scanned > 50,
        "premise: only {scanned} cognitive source files were read"
    );
    assert!(
        offenders.is_empty(),
        "a cognitive crate names a raw-tick type; it must consume time-aligned states, \
         regimes, summaries or episodes instead (R12-TICK-2041):\n{}",
        offenders.join("\n")
    );
}

/// The token rule is what keeps a longer name from reading as a raw tick.
#[test]
fn the_raw_tick_detector_matches_whole_tokens_only() {
    assert_eq!(
        raw_tick_names("fn f(m: MarketMessage)"),
        vec!["MarketMessage"]
    );
    assert!(raw_tick_names("let market_message_count = 1;").is_empty());
    assert!(raw_tick_names("struct MarketMessageSummary;").is_empty());
    assert!(raw_tick_names("use qip_opportunity_engine::catalyst::MarketEvent;").is_empty());
    assert_eq!(
        raw_tick_names("use qip_contracts::market_event::MarketEvent;"),
        vec!["market_event::MarketEvent"]
    );
}
