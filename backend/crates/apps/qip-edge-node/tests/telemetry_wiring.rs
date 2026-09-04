//! The composition root's telemetry wiring, proven on the assembled pieces.
//!
//! The defect this guards is one nothing at runtime would report: a second
//! registry. If the cell recorded into one `Metrics` and the health thread
//! served another, every scrape would answer empty forever while the cell
//! recorded diligently — the exact gap this seam closes, rebuilt one level up.
//!
//! This file used to check the *source* of `main.rs` for the spelling of the
//! wiring: one `Telemetry::new(`, no `Metrics::new(`, four substrings. A
//! reviewer inserted `let other = Telemetry::silent(); let metrics =
//! Arc::clone(&other.metrics);` between the lines — the cell and the mesh
//! series recording into a registry the scrape never serves — and the check
//! stayed green, because every substring was still there and every one of
//! them also matched a comment. The assembly now lives in
//! [`qip_edge_node::assemble`], where a test can hold the property itself:
//! the registry the cell holds, the registry the mesh series holds, and the
//! registry the scrape serves are one `Arc`, by pointer identity, and a
//! series the cell wrote is readable through the responder the health thread
//! calls.

use qip_contracts::venue::VenueId;
use qip_core::{Duration, SystemClock};
use qip_edge::cell::CellConfig;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::telemetry::respond;
use qip_edge_node::{NodeAssembly, assemble};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{labels, names};
use std::fs;
use std::path::Path;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";

fn assembled() -> NodeAssembly {
    let config = CellConfig::new(CELL, REGION).with_venue(VenueId::new("XLON"));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000")).expect("a positive amount");
    assemble(config, features, Arc::new(SystemClock), allocation)
        .expect("a well-formed cell assembles")
}

#[test]
fn the_cell_and_the_mesh_series_record_into_the_registry_the_scrape_serves() {
    let node = assembled();

    // Pointer identity, not equality of contents: two empty registries are
    // equal and are exactly the defect.
    assert!(
        Arc::ptr_eq(node.scrape_registry(), node.cell.metrics_registry()),
        "the cell records into a registry the scrape does not serve"
    );
    assert!(
        Arc::ptr_eq(node.scrape_registry(), node.mesh_series.registry()),
        "the mesh series records into a registry the scrape does not serve"
    );

    // And behaviourally, through the responder `main.rs` routes a request
    // to: wiring the cell writes its halt gauge, so a scrape of the served
    // registry must already carry it. A registry the cell was not handed
    // answers this with nothing.
    let (content_type, exposition) = respond(
        b"GET /metrics HTTP/1.1\r\n\r\n",
        node.scrape_registry(),
        "{}",
    );
    assert_eq!(content_type, "text/plain; version=0.0.4; charset=utf-8");
    assert!(
        exposition.contains(names::EDGE_HALTED),
        "the scrape does not carry the gauge the cell wrote when it was wired:\n{exposition}"
    );
    assert_eq!(
        node.scrape_registry().snapshot().gauge(
            names::EDGE_HALTED,
            &labels([("cell", CELL), ("region", REGION), ("source", "policy")])
        ),
        Some(0.0),
        "the served registry does not hold the cell's halt gauge"
    );
}

/// Occurrences of `needle` that are not the tail of a longer identifier, so
/// `CellMetrics::new(` does not count as `Metrics::new(`.
fn whole_occurrences(haystack: &str, needle: &str) -> usize {
    haystack
        .match_indices(needle)
        .filter(|(at, _)| {
            haystack[..*at]
                .chars()
                .next_back()
                .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
        })
        .count()
}

#[test]
fn main_builds_no_registry_of_its_own() {
    // What the behavioural test above cannot see: `main.rs` is a binary no
    // test can call, so whether it uses `assemble` at all is out of reach.
    // This is the narrow remainder of the old source check, kept for the one
    // thing it can honestly say — that the binary constructs no telemetry and
    // no registry itself, so the only registry it can serve is the one the
    // assembly handed it. It counts constructors, not the spelling of the
    // wiring, which is what the reviewer's insertion slipped past.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let source =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    for constructor in ["Telemetry::new(", "Telemetry::silent(", "Metrics::new("] {
        assert_eq!(
            whole_occurrences(&source, constructor),
            0,
            "main.rs calls {constructor} itself, beside the registry the assembly serves"
        );
    }
    assert_eq!(
        whole_occurrences(&source, "assemble("),
        1,
        "main.rs does not take its cell and registry from the one assembly"
    );
}
