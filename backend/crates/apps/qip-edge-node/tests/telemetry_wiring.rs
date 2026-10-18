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
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

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
    assemble(config, features, Arc::new(SystemClock), allocation, None)
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
    // `assemble_with_risk` since the Risk Gate became a parameter of the
    // assembly (R12-RISK-4093): the binary passes the limits it read, and
    // `assemble` is the same function with the shipped set.
    assert_eq!(
        whole_occurrences(&source, "assemble_with_risk("),
        1,
        "main.rs does not take its cell and registry from the one assembly"
    );
}

/// Two linked peers measure each other's clock, and the nearest one's round
/// trip reaches the registry the scrape serves. Before this the link counted
/// `clocks_measured` and nothing recorded how far away the peer was, so a
/// cell could be joined to a peer a continent off and chart as healthy.
///
/// Mutation: drop the `clock_round_trip_ns` store in
/// `qip-peer-link/src/host.rs::record` and the gauge is never written, so the
/// wait times out with the series absent.
#[test]
fn a_peer_clock_exchange_records_the_measured_round_trip_in_the_scrape_registry() {
    use qip_edge_node::telemetry::PeerSeries;
    use qip_peer_link::{DEFAULT_CLOCK_BOUND, PeerSettings};
    use qip_transport::tls::LinkSecurity;
    use std::collections::BTreeMap;
    use std::net::TcpListener;
    use std::time::{Duration as Wait, Instant};

    let addr = || {
        TcpListener::bind("127.0.0.1:0")
            .expect("a loopback port")
            .local_addr()
            .expect("its address")
    };
    let (a_at, b_at) = (addr(), addr());
    let spawn = |node: &str, at, peer: &str, peer_at| {
        qip_peer_link::spawn(
            &PeerSettings {
                node: node.into(),
                listen: at,
                peers: BTreeMap::from([(peer.to_string(), peer_at)]),
                security: LinkSecurity::plaintext_for_tests(),
                clock_bound: DEFAULT_CLOCK_BOUND,
            },
            Arc::new(SystemClock),
        )
        .expect("a peer node spawns on a free loopback port")
        .0
    };
    let first = spawn("london-1", a_at, "newyork-1", b_at);
    let _second = spawn("newyork-1", b_at, "london-1", a_at);

    let node = assembled();
    let series = PeerSeries::new(Arc::clone(node.scrape_registry()), CELL, REGION);
    let gauge_labels = labels([("cell", CELL), ("region", REGION)]);
    let read = || {
        node.scrape_registry()
            .snapshot()
            .gauge(names::EDGE_PEER_ROUND_TRIP_MS, &gauge_labels)
    };
    series.observe(&qip_peer_link::boundary::PeerStatus::default());
    assert!(
        read().is_none(),
        "premise: a link that has measured no clock records nothing, not a zero"
    );

    let deadline = Instant::now() + Wait::from_secs(20);
    loop {
        let status = first.status();
        series.observe(&status);
        if status.clocks_measured >= 1 {
            let recorded = read().expect("a measured clock was recorded");
            assert!(
                status.clock_round_trip_ns > 0 && recorded > 0.0,
                "{status:?} recorded {recorded}"
            );
            assert!(
                recorded < 20_000.0,
                "a loopback round trip of {recorded} ms is not a measurement"
            );
            return;
        }
        assert!(Instant::now() < deadline, "no clock measured: {status:?}");
        std::thread::sleep(Wait::from_millis(5));
    }
}
