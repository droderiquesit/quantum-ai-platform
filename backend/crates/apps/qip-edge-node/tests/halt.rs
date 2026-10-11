//! The second halt wire, proven on the node's own seam with real files.
//!
//! `qip-edge`'s tests drive `Cell::apply_polled_halt` with readings they
//! build; what they cannot see is the mapping from a filesystem to those
//! readings, which is this crate's and is where a wrong answer would be
//! silent. Every test here puts a real path in front of `HaltFlag::read` —
//! present, absent, a directory where a file should be, a file whose
//! directory is gone — and asserts what the assembled cell did about it.
//! The wiring test and the mesh tests share nothing: no link is built here,
//! which is the independence §46.2 asks for, held by construction.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::venue::VenueId;
use qip_core::{Duration, SystemClock, Timestamp};
use qip_edge::cell::{CellConfig, PolledHalt};
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::halt::{FLAG_VARIABLE, HaltFlag};
use qip_edge_node::{NodeAssembly, assemble};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{labels, names};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn assembled() -> NodeAssembly {
    let config = CellConfig::new(CELL, REGION).with_venue(VenueId::new("XLON"));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    // Far above anything this suite sends, so the halt wire is what decides.
    let allocation = RegionCapital::read(Some("1000000000")).expect("a positive amount");
    assemble(config, features, Arc::new(SystemClock), allocation, None)
        .expect("a well-formed cell assembles")
}

/// A fresh directory for one test, so tests running in parallel never see
/// each other's flag.
fn scratch(test: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("qip-edge-node-halt-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn polled_gauge(node: &NodeAssembly) -> Option<f64> {
    node.scrape_registry().snapshot().gauge(
        names::EDGE_HALTED,
        &labels([("cell", CELL), ("region", REGION), ("source", "polled")]),
    )
}

#[test]
fn the_node_halts_the_cell_on_a_present_flag_and_releases_it_when_the_flag_is_removed() {
    let dir = scratch("present");
    let path = dir.join("halt");
    let flag = HaltFlag::at(&path).expect("an absolute path");
    let mut node = assembled();

    // Absent first: a node whose operator has never written a flag runs.
    assert_eq!(flag.poll(&mut node.cell, t(1)), PolledHalt::Absent);
    assert!(!node.cell.is_halted(), "an absent flag halted the cell");
    assert_eq!(polled_gauge(&node), Some(0.0));

    fs::write(&path, "engaged: drill\n").expect("the flag is written");
    let reading = flag.poll(&mut node.cell, t(2));
    assert_eq!(reading, PolledHalt::Engaged("drill".to_string()));
    assert!(
        node.cell.is_halted(),
        "a present flag did not halt the cell"
    );
    assert_eq!(
        polled_gauge(&node),
        Some(1.0),
        "the halt is not on the registry the scrape serves"
    );

    fs::remove_file(&path).expect("the flag is removed");
    assert_eq!(flag.poll(&mut node.cell, t(3)), PolledHalt::Absent);
    assert!(
        !node.cell.is_halted(),
        "removing the flag did not release the halt"
    );
    assert_eq!(polled_gauge(&node), Some(0.0));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_flag_that_cannot_be_read_halts_the_cell_rather_than_reading_as_absent() {
    // Two failures the filesystem can produce that are not "no flag": the
    // path is a directory, so `read` fails with something other than
    // not-found; and the directory that should carry the flag is gone, so
    // `read` fails with not-found for a reason that is the mount, not the
    // operator. Both must halt.
    let dir = scratch("unreadable");
    let as_directory = dir.join("halt-is-a-dir");
    fs::create_dir(&as_directory).expect("a directory where the flag should be");
    let flag = HaltFlag::at(&as_directory).expect("an absolute path");
    let mut node = assembled();
    let reading = flag.poll(&mut node.cell, t(1));
    assert!(
        matches!(reading, PolledHalt::Unreadable(_)),
        "a directory at the flag's path read as {reading:?}"
    );
    assert!(
        node.cell.is_halted(),
        "an unreadable flag did not halt the cell"
    );

    let missing_mount = dir.join("no-such-mount").join("halt");
    assert!(
        !missing_mount.parent().is_some_and(Path::is_dir),
        "the premise needs the flag's directory to be missing"
    );
    let flag = HaltFlag::at(&missing_mount).expect("an absolute path");
    let mut node = assembled();
    let reading = flag.poll(&mut node.cell, t(1));
    assert!(
        matches!(reading, PolledHalt::Unreadable(_)),
        "a flag whose directory is missing read as {reading:?}, so an unmounted volume \
         would release the kill switch"
    );
    assert!(node.cell.is_halted());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_flag_that_reads_released_does_not_halt_and_malformed_content_does() {
    let dir = scratch("content");
    let path = dir.join("halt");
    let flag = HaltFlag::at(&path).expect("an absolute path");
    let mut node = assembled();

    fs::write(&path, "released\n").expect("written");
    assert_eq!(flag.poll(&mut node.cell, t(1)), PolledHalt::Released);
    assert!(!node.cell.is_halted(), "a released flag halted the cell");

    fs::write(&path, "Released? maybe\n").expect("written");
    let reading = flag.poll(&mut node.cell, t(2));
    assert!(matches!(reading, PolledHalt::Unreadable(_)), "{reading:?}");
    assert!(
        node.cell.is_halted(),
        "malformed content did not halt the cell"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_or_relative_flag_path_is_refused_at_configuration() {
    assert!(HaltFlag::at("").is_err(), "an empty path was accepted");
    assert!(
        HaltFlag::at("run/halt").is_err(),
        "a relative path was accepted, and would name a different file under every supervisor"
    );
    assert!(HaltFlag::at("/run/qip/halt").is_ok());
}

/// The text of `source` from `signature` to the next item at the same
/// indent — one method's body, so a fragment found in it is in that method
/// and not merely somewhere in the file.
fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` is not in the scanned file"));
    let rest = &source[start + signature.len()..];
    let end = ["\n    fn ", "\n    pub fn ", "\n}\n"]
        .iter()
        .filter_map(|next| rest.find(next))
        .min()
        .unwrap_or(rest.len());
    &source[start..start + signature.len() + end]
}

fn read_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_binary_reads_the_flag_variable_and_polls_the_flag_in_its_loop() {
    // The binary's loop is not callable from a test, so this is a source
    // check, and it is narrower than a behavioural one. What changed: until
    // `670e94c7` the loop lived in `main.rs` and read the flag itself with
    // `flag.poll(cell, now)` on the decision thread, so this scanned
    // `main.rs` for that call — and admitted it could not show the poll was
    // in the loop or ahead of the flush. That commit moved the read onto
    // `control::Poller`'s own thread (a hung mount no longer holds the cell
    // deciding) and the application of the reading into
    // `scheduler::DecisionLoop::turn`. The property is unchanged — the flag
    // the variable names reaches the cell on every turn — and it is now
    // scanned where it lives, method by method, with the order asserted that
    // the old scan could not assert.
    let main = read_source("src/main.rs");
    let scheduler = read_source("src/scheduler.rs");
    let control = read_source("src/control.rs");

    // The variable is read by name, and the flag it names is handed to the
    // poller the decision loop holds.
    assert!(
        main.contains("std::env::var(FLAG_VARIABLE)"),
        "main.rs does not read {FLAG_VARIABLE}"
    );
    assert!(
        main.contains("decisions = decisions.with_poller(poller);"),
        "main.rs never hands the halt flag's poller to the decision loop"
    );

    // The poller applies its halt reading to the cell.
    let apply = body_of(
        &control,
        "pub fn apply(&self, cell: &mut Cell, now: Timestamp)",
    );
    assert!(
        apply.contains("cell.apply_polled_halt(readings.halt.value.clone(), now);"),
        "Poller::apply no longer hands the polled halt to the cell"
    );

    // The decision loop applies the poller every turn, before the pass and
    // before the flush: a halt read this turn stops this turn's pass.
    let wires = body_of(&scheduler, "fn apply_wires(&mut self, now: Timestamp)");
    assert!(
        wires.contains("poller.apply(&mut self.cell, now)"),
        "DecisionLoop::apply_wires never applies the poller to the cell"
    );
    let turn = body_of(&scheduler, "pub fn turn(&mut self, now: Timestamp)");
    let applied = turn
        .find("self.apply_wires(now)")
        .expect("DecisionLoop::turn never applies the halt flag");
    let pass = turn
        .find("run_pass(")
        .expect("the premise: the scan is reading the turn that runs the pass");
    let flush = turn
        .find("self.cell.flush(")
        .expect("the premise: the scan is reading the turn that flushes");
    assert!(
        applied < pass && applied < flush,
        "the halt flag is applied after the pass or the flush, so a halt read this turn \
         would not stop this turn's pass"
    );
}
