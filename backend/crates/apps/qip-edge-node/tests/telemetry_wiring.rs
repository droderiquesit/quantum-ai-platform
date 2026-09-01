//! The composition root's telemetry wiring, checked in the source.
//!
//! `run` and `serve` live in `main.rs`, where no test can call them, and the
//! defect this guards is one nothing at runtime would report: a second
//! registry. If the cell recorded into one `Metrics` and the health thread
//! served another, every scrape would answer empty forever while the cell
//! recorded diligently — the exact gap this seam closes, rebuilt one level up.
//! `qip-fastbrain` and `qip-deepbrain` are wired the same way, and this is
//! the same check they would want.

use std::fs;
use std::path::Path;

fn main_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
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
fn the_node_hands_the_cell_the_one_registry_its_scrape_surface_serves() {
    let source = main_source();

    assert_eq!(
        whole_occurrences(&source, "Telemetry::new("),
        1,
        "the premise failed: the node should construct exactly one Telemetry"
    );
    assert_eq!(
        whole_occurrences(&source, "Metrics::new("),
        0,
        "a registry built beside the telemetry is one the cell writes to and nothing serves"
    );
    assert!(
        source.contains("Arc::clone(&telemetry.metrics)"),
        "the registry handle must be the telemetry's own, taken before it is used"
    );
    assert!(
        source.contains(".with_metrics(Arc::clone(&metrics))"),
        "the cell is not handed the registry the scrape serves"
    );
    assert!(
        source.contains("MeshSeries::new(Arc::clone(&metrics)"),
        "the mesh series is not recorded into the registry the scrape serves"
    );
    assert!(
        source.contains("respond(&buffer[..read], metrics, &body)"),
        "the health server does not route a scrape through the tested responder"
    );
}
