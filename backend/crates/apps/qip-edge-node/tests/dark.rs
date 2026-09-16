//! §36.3's region wire, proven on the node's own seam with real files.
//!
//! `qip-edge`'s tests drive `Cell::apply_region_outlook` with readings they
//! build; what they cannot see is the mapping from a filesystem to those
//! readings, which is this crate's and is where a wrong answer would be
//! silent — a mount that went away reading as "every peer is answering" is a
//! cell that keeps taking one side of a mirror whose other side may be gone.
//! Every test here puts a real path in front of `DarkRegionWire::read` and
//! asserts what the assembled cell did about it.

use qip_contracts::venue::VenueId;
use qip_core::{Duration, SystemClock, Timestamp};
use qip_edge::cell::CellConfig;
use qip_edge::region::RegionOutlook;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::dark::{DECLARATION_FILE, DarkRegionWire};
use qip_edge_node::halt::HaltFlag;
use qip_edge_node::{NodeAssembly, assemble};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::labels;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const ABROAD: &str = "us-east1";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

/// A node holding one venue at home and one abroad, so a dark region is a
/// region its mirrors actually reach.
fn assembled() -> NodeAssembly {
    let config = CellConfig::new(CELL, REGION)
        .with_venue(VenueId::new("XLON"))
        .with_venue_in_region(VenueId::new("XNYS"), ABROAD)
        .expect("a well-formed region id");
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000")).expect("a positive amount");
    assemble(config, features, Arc::new(SystemClock), allocation, None)
        .expect("a well-formed cell assembles")
}

fn scratch(test: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("qip-edge-node-dark-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn dark_gauge(node: &NodeAssembly, source: &str) -> Option<f64> {
    node.scrape_registry().snapshot().gauge(
        "qip_edge_regions_dark",
        &labels([("cell", CELL), ("region", REGION), ("source", source)]),
    )
}

#[test]
fn the_wire_sits_beside_the_halt_flag_and_needs_no_variable_of_its_own() {
    let dir = scratch("beside");
    let flag = HaltFlag::at(dir.join("engaged")).expect("an absolute path");
    let wire = DarkRegionWire::beside(flag.path()).expect("the flag names a directory");
    assert_eq!(wire.path(), dir.join(DECLARATION_FILE));
}

#[test]
fn a_node_whose_declaration_names_a_region_suspends_the_mirrors_reaching_it_and_restores_them() {
    let dir = scratch("declared");
    let flag = HaltFlag::at(dir.join("engaged")).expect("an absolute path");
    let wire = DarkRegionWire::beside(flag.path()).expect("the flag names a directory");
    let mut node = assembled();

    // Absent first: a node whose operator has never written the file has
    // every peer answering, which is what every node has run as.
    assert_eq!(wire.poll(&mut node.cell, t(1)), RegionOutlook::AllLit);
    assert!(!node.cell.is_region_dark(ABROAD));
    assert_eq!(dark_gauge(&node, "declared"), Some(0.0));

    fs::write(wire.path(), format!("{ABROAD}\n")).expect("the declaration is written");
    let reading = wire.poll(&mut node.cell, t(2));
    assert_eq!(reading.named(), vec![ABROAD.to_string()]);
    assert!(
        node.cell.is_region_dark(ABROAD),
        "a declared region is not dark to the cell"
    );
    assert!(
        !node.cell.is_region_dark(REGION),
        "the cell read itself as dark"
    );
    assert_eq!(
        dark_gauge(&node, "declared"),
        Some(1.0),
        "the suspension is not on the registry the scrape serves"
    );

    // Removing the file restores the region, and the gauge falls rather than
    // going stale.
    fs::remove_file(wire.path()).expect("the declaration is removed");
    assert_eq!(wire.poll(&mut node.cell, t(3)), RegionOutlook::AllLit);
    assert!(!node.cell.is_region_dark(ABROAD));
    assert_eq!(dark_gauge(&node, "declared"), Some(0.0));
}

#[test]
fn a_node_whose_mount_is_gone_treats_every_other_region_as_dark() {
    // The failure this prevents, and the whole reason the reading is an enum
    // rather than a set: a wire that cannot be read is not a wire that says
    // nothing. A missing mount that read as "nothing is dark" is a cell that
    // keeps mirroring into a region nobody can tell it about.
    let dir = scratch("unmounted");
    let flag = HaltFlag::at(dir.join("mount").join("engaged")).expect("an absolute path");
    let wire = DarkRegionWire::beside(flag.path()).expect("the flag names a directory");
    // Premise: the directory really is absent, or this would be testing the
    // absent-file arm.
    assert!(!dir.join("mount").is_dir());
    let mut node = assembled();

    let reading = wire.poll(&mut node.cell, t(1));
    assert!(
        matches!(reading, RegionOutlook::Unreadable(_)),
        "a missing mount read as {reading:?}"
    );
    assert!(node.cell.is_region_dark(ABROAD));
    assert!(
        !node.cell.is_region_dark(REGION),
        "a cell is never dark to itself"
    );
    assert_eq!(dark_gauge(&node, "unreadable"), Some(1.0));
    assert_eq!(dark_gauge(&node, "declared"), Some(0.0));
}

#[test]
fn a_declaration_the_node_cannot_parse_suspends_every_mirror_rather_than_fewer() {
    // Fail closed on the content too. A parse that dropped what it could not
    // read would answer "fewer regions are dark", which is the one answer
    // that lets a cell trade one side of a mirror whose other side is gone.
    let dir = scratch("unparseable");
    let flag = HaltFlag::at(dir.join("engaged")).expect("an absolute path");
    let wire = DarkRegionWire::beside(flag.path()).expect("the flag names a directory");
    let mut node = assembled();

    fs::write(wire.path(), format!("  {ABROAD}\n")).expect("the declaration is written");
    let reading = wire.poll(&mut node.cell, t(1));
    assert!(
        matches!(reading, RegionOutlook::Unreadable(_)),
        "an indented region id read as {reading:?}, so it was trimmed into a region nobody named"
    );
    assert!(node.cell.is_region_dark(ABROAD));
    assert_eq!(dark_gauge(&node, "unreadable"), Some(1.0));

    // A directory where the file should be is the same answer.
    fs::remove_file(wire.path()).expect("the declaration is removed");
    fs::create_dir_all(wire.path()).expect("a directory in its place");
    let reading = wire.poll(&mut node.cell, t(2));
    assert!(
        matches!(reading, RegionOutlook::Unreadable(_)),
        "a directory where the declaration should be read as {reading:?}"
    );
}

#[test]
fn an_empty_declaration_is_the_operator_saying_nothing_is_dark() {
    // The other half of the fail-closed reading, and the one that would make
    // this wire unusable if it were wrong: an operator who empties the file
    // has released every region, and a present-but-empty file that read as
    // unreadable would suspend every mirror on the deployment's own
    // housekeeping.
    let dir = scratch("empty");
    let flag = HaltFlag::at(dir.join("engaged")).expect("an absolute path");
    let wire = DarkRegionWire::beside(flag.path()).expect("the flag names a directory");
    let mut node = assembled();

    fs::write(wire.path(), format!("{ABROAD}\n")).expect("the declaration is written");
    // Premise: the region really was dark before the file was emptied, or
    // the assertion below would pass on a cell that was never suspended.
    assert_eq!(
        wire.poll(&mut node.cell, t(1)).named(),
        vec![ABROAD.to_string()]
    );
    assert!(node.cell.is_region_dark(ABROAD));

    fs::write(wire.path(), "\n\n").expect("the declaration is emptied");
    assert_eq!(wire.poll(&mut node.cell, t(2)), RegionOutlook::AllLit);
    assert!(!node.cell.is_region_dark(ABROAD));
}
