//! The §31.1 cross-region mirror, read from a declaration and assembled into
//! the cell the binary builds.
//!
//! `qip-edge/tests/cross_region.rs` proves what a cell *given* a mirror does:
//! it composes mirror edges, is assigned path 3, and refuses the leg its
//! inventory band forbids. What it cannot see is whether any deployed cell is
//! ever given one — and until this suite existed none was. Every caller of
//! `Cell::install_mirror` and of `CellConfig::venue_regions` was a test, so a
//! node this binary built placed all of its venues in its own region, every
//! hop composed a transport edge, and §30.2's mirrored rows were reachable
//! from a harness and nowhere else.
//!
//! Each test here goes through the same `CrossRegionMirror::read` and
//! `assemble` the binary goes through, and asserts on what the assembled cell
//! then holds.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Duration;
use qip_core::{Decimal, SystemClock, dec};
use qip_edge::cell::CellConfig;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::cross_region::{CrossRegionMirror, MAX_DECLARATION_BYTES, MIRROR_VARIABLE};
use qip_edge_node::{NodeAssembly, assemble};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const CELL: &str = "london-1";
/// This cell's own region. Every refusal about "the cell's own region" is
/// about this string and not about whatever the router defaults to.
const HOME: &str = "europe-west2";
const ABROAD: &str = "us-east4";
/// A venue at home and a venue abroad, both of which the cell may trade.
const VENUE_HOME: &str = "XLON";
const VENUE_ABROAD: &str = "XNYS";

static FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A declaration written to a file, the way a deployment mounts one.
///
/// Written to disk rather than parsed from a string on purpose: the path is
/// what the deployment supplies, and a reader tested only against bytes would
/// not exercise the refusals about a file that is missing or too large.
fn declaration_at(body: &str) -> PathBuf {
    let unique = FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "qip-cross-region-{}-{unique}.json",
        std::process::id()
    ));
    std::fs::write(&path, body).expect("the test fixture is writable");
    path
}

fn venues() -> Vec<VenueId> {
    vec![VenueId::new(VENUE_HOME), VenueId::new(VENUE_ABROAD)]
}

/// The declaration this suite treats as correct. Every refusal test below is
/// this document with exactly one thing changed, so a test that fires is
/// firing on the change rather than on a document that was never admissible.
const GOOD: &str = r#"{
  "venue_regions": { "XNYS": "us-east4" },
  "round_trips_ms": { "us-east4": 28 },
  "instruments": {
    "obj-ACME": {
      "soft_band": "5",
      "hard_band": "20",
      "market": "ACME-USD",
      "threshold": "0.5"
    }
  }
}"#;

fn read(body: &str) -> Result<Option<CrossRegionMirror>> {
    let path = declaration_at(body);
    CrossRegionMirror::read(path.to_str(), HOME, &venues())
}

/// Assemble a node exactly as `main` does, with `mirror` as read.
fn assembled(mirror: Option<CrossRegionMirror>) -> Result<NodeAssembly> {
    let mut config = CellConfig::new(CELL, HOME);
    for venue in venues() {
        config = config.with_venue(venue);
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    // Far above anything this suite does with capital; the mirror is what is
    // under test, not the ceiling.
    let allocation = RegionCapital::read(Some("1000000000"))?;
    assemble(config, features, Arc::new(SystemClock), allocation, mirror)
}

#[test]
fn a_declaration_the_deployment_wrote_reaches_the_assembled_cell_as_a_mirror_arrangement()
-> Result<()> {
    // The row this lane exists to open, and the test the unwiring mutation
    // is aimed at: before it, `Cell::install_mirror` was called by no
    // composition root at all, so no deployed cell could hold a band and
    // §30.2's rows 3 and 4 were unreachable outside a harness.
    let mirror = read(GOOD)?.expect("a declaration was named, so one was read");
    // Premise: the declaration itself carries the instrument, so a cell that
    // ends up holding nothing is the assembly dropping it rather than the
    // fixture being empty.
    assert_eq!(mirror.instruments(), 1);

    let node = assembled(Some(mirror))?;
    let held = node
        .cell
        .mirror()
        .expect("the assembled cell holds the arrangement the deployment declared");

    // The numbers are the file's, not a default: a band nobody wrote is the
    // whole failure this module refuses.
    let discipline = held
        .instrument(&ObjectId::from_string("obj-ACME"))
        .expect("the declared instrument is mirrored");
    assert_eq!(discipline.market(), &ObjectId::from_string("ACME-USD"));
    assert_eq!(discipline.threshold(), dec!("0.5"));
    let band = discipline.band_around(Decimal::ZERO)?;
    assert_eq!(band.soft(), dec!("5"));
    assert_eq!(band.hard(), dec!("20"));
    assert_eq!(held.round_trip(ABROAD)?, Duration::from_millis(28));

    // And the other half: the venue is abroad in the config the cycle router
    // is built from, so a hop to it composes a mirror edge rather than a
    // transport edge. One half without the other is worse than neither.
    assert_eq!(
        node.cell.config().venue_regions.get(VENUE_ABROAD),
        Some(&ABROAD.to_string()),
        "the declared venue is not placed abroad in the cell's own configuration"
    );
    assert_eq!(
        node.cell.config().venue_regions.get(VENUE_HOME),
        None,
        "a venue the declaration never named was placed somewhere"
    );
    Ok(())
}

#[test]
fn a_node_with_no_declaration_holds_no_arrangement_and_places_every_venue_at_home() -> Result<()> {
    // The half that proves the test above is not asserting something the
    // assembly does regardless. This is also every node deployed to date.
    let node = assembled(None)?;
    assert!(
        node.cell.mirror().is_none(),
        "a node given no declaration invented an arrangement"
    );
    assert!(
        node.cell.config().venue_regions.is_empty(),
        "a node given no declaration placed a venue abroad: {:?}",
        node.cell.config().venue_regions
    );
    Ok(())
}

#[test]
fn a_cell_assembled_with_a_mirror_is_still_paper_trading_only() -> Result<()> {
    // §31.1 is the closest this binary's configuration comes to the venue
    // seam, and the boundary is structural: `Cell` has no constructor taking
    // a ceiling other than paper trading, and nothing in a declaration names
    // one. Asserted on the assembled cell so that a future parameter that
    // could carry a ceiling fails here.
    let mirrored = assembled(Some(read(GOOD)?.expect("a declaration")))?;
    let plain = assembled(None)?;
    assert!(
        !mirrored.cell.autonomy().ceiling().is_live(),
        "a mirrored cell reported a live-capable ceiling"
    );
    assert_eq!(
        mirrored.cell.autonomy().ceiling(),
        plain.cell.autonomy().ceiling(),
        "installing a mirror moved the cell's autonomy ceiling"
    );
    Ok(())
}

#[test]
fn an_unset_declaration_is_admitted_and_a_named_file_that_is_absent_is_refused() -> Result<()> {
    // Both halves in one test because each is meaningless alone: a reader
    // that refused the unset case would break every node deployed today, and
    // one that admitted a missing file would leave a node running with every
    // venue at home while its configuration said otherwise.
    assert!(
        CrossRegionMirror::read(None, HOME, &venues())?.is_none(),
        "an unset variable should be a cell with no foreign venue"
    );
    let refusal =
        CrossRegionMirror::read(Some("/nonexistent/qip-cross-region.json"), HOME, &venues())
            .expect_err("a declaration that was named and is not there is a fault");
    assert_eq!(refusal.code(), "io");
    assert!(
        refusal.message().contains(MIRROR_VARIABLE),
        "the refusal should name the variable that pointed at nothing: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_declaration_placing_a_venue_this_cell_may_not_trade_is_refused_rather_than_reaching_it()
-> Result<()> {
    // The failure this prevents is the one the region map must never cause:
    // a region annotation says *where* a venue is and never that the cell
    // may reach it. A declaration that could add a venue would be a file on
    // a node's disk widening what the cell trades.
    let refusal = read(&GOOD.replace("XNYS", "XPAR"))
        .expect_err("a venue outside QIP_VENUES cannot be placed anywhere");
    assert_eq!(refusal.code(), "denied");
    assert!(
        refusal
            .message()
            .contains("not a venue this cell may trade"),
        "the refusal should say why: {}",
        refusal.message()
    );
    // The half that proves the gate admits a good value: the same document
    // with a venue the cell may trade is read.
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_declaration_placing_a_venue_in_this_cells_own_region_is_refused_as_an_edge_nothing_routes()
-> Result<()> {
    // A mirror edge is one asset in two regions, and the router refuses one
    // whose ends share a region. Admitted here, the annotation would sit in
    // the config looking like a control and compose nothing.
    let refusal = read(&GOOD.replace("us-east4", HOME))
        .expect_err("a venue in the cell's own region composes no mirror edge");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("this cell's own region"),
        "the refusal should say why: {}",
        refusal.message()
    );
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_region_with_a_venue_and_no_measured_round_trip_is_refused_at_start_and_not_on_a_pass()
-> Result<()> {
    // §30.2's row 6 is decided on that measurement. Left out, the cell
    // refuses the cycle at pass time with a message naming the centre, and
    // an operator reads a market event where a configuration fault happened.
    let refusal = read(&GOOD.replace(
        r#""round_trips_ms": { "us-east4": 28 }"#,
        r#""round_trips_ms": {}"#,
    ))
    .expect_err("a venue abroad with no measured round trip cannot be routed to");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("measures no round trip to it"),
        "the refusal should say why: {}",
        refusal.message()
    );
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_round_trip_to_a_region_no_venue_sits_in_is_refused_as_a_measurement_nothing_reads()
-> Result<()> {
    // The other direction, and it is not symmetry for its own sake: a
    // measurement nothing looks up cannot be told from one nobody took, so a
    // region renamed on one side of the document and not the other would
    // read as configured and gate nothing.
    let refusal = read(&GOOD.replace(
        r#""round_trips_ms": { "us-east4": 28 }"#,
        r#""round_trips_ms": { "us-east4": 28, "ap-south1": 91 }"#,
    ))
    .expect_err("a round trip to a region holding no venue is read by nothing");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("places no venue there"),
        "the refusal should say why: {}",
        refusal.message()
    );
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_declaration_with_an_unrecognised_key_is_refused_rather_than_ignored() -> Result<()> {
    // The failure this prevents: a key the parser does not know, silently
    // dropped, leaves an operator believing they stated something.
    //
    // The key added here is a *surplus* one, and that is the whole point of
    // this test. Written the obvious way — misspelling `hard_band` as
    // `hard_bnad` — it passed with `deny_unknown_fields` removed, because the
    // misspelling also takes a required field away and serde refuses the
    // missing one. The test then guarded nothing it claimed to guard, which
    // is the class of defect only a mutation finds; the mutation was run and
    // this is the test that survived it.
    let refusal = read(&GOOD.replace(
        r#""hard_band": "20""#,
        r#""hard_band": "20", "reduced_size": "0.5""#,
    ))
    .expect_err("a key this parser does not know is not a key the operator stated");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("is not a mirror declaration"),
        "the refusal should name the document: {}",
        refusal.message()
    );
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_declaration_holding_only_one_of_the_two_halves_is_refused_in_both_directions() -> Result<()> {
    // Venues abroad with no instrument refuses every cross-region cycle for
    // a missing band; an instrument with no venue abroad is a band nothing
    // ever looks up. Either alone looks configured and gates nothing.
    let no_instrument = read(
        r#"{"venue_regions": {"XNYS": "us-east4"}, "round_trips_ms": {"us-east4": 28}, "instruments": {}}"#,
    )
    .expect_err("a venue abroad with no mirrored instrument refuses every cycle");
    assert_eq!(no_instrument.code(), "invalid");
    assert!(
        no_instrument
            .message()
            .contains("names no mirrored instrument"),
        "the refusal should say why: {}",
        no_instrument.message()
    );

    let no_venue = read(&GOOD.replace(
        r#""venue_regions": { "XNYS": "us-east4" }"#,
        r#""venue_regions": {}"#,
    ))
    .expect_err("an instrument mirrored against no foreign venue is looked up by nothing");
    assert_eq!(no_venue.code(), "invalid");
    assert!(
        no_venue.message().contains("places no venue abroad"),
        "the refusal should say why: {}",
        no_venue.message()
    );
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_hard_band_inside_the_soft_band_is_refused_with_the_file_that_declared_it_named() -> Result<()>
{
    // `MirroredInstrument::new` already refuses this; what this asserts is
    // that the refusal reaches the operator as a document to edit rather
    // than as an unattributed band error at some later pass.
    let refusal = read(&GOOD.replace(r#""hard_band": "20""#, r#""hard_band": "1""#))
        .expect_err("a hard band inside the soft band can never be reached");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal
            .message()
            .contains("is not wider than the soft band"),
        "the library's own reason should survive: {}",
        refusal.message()
    );
    assert!(
        refusal
            .message()
            .contains("cross-region mirror declaration at"),
        "the file should be named: {}",
        refusal.message()
    );
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_round_trip_of_zero_milliseconds_is_refused_rather_than_corrected_into_a_measurement()
-> Result<()> {
    // Not corrected here and not clamped: a zero makes every remote quote
    // look as though it outlasts the wire. The arrangement's own constructor
    // holds this, and the declaration hands it the operator's number
    // untouched — which is what this asserts.
    let refusal = read(&GOOD.replace(r#""us-east4": 28"#, r#""us-east4": 0"#))
        .expect_err("zero is not a measurement");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("is not a measurement"),
        "the refusal should say why: {}",
        refusal.message()
    );
    // A negative count reaches the same refusal rather than being made
    // positive on the way.
    let negative = read(&GOOD.replace(r#""us-east4": 28"#, r#""us-east4": -28"#))
        .expect_err("a negative round trip is not a measurement either");
    assert!(
        negative.message().contains("is not a measurement"),
        "a negative measurement should refuse for the same reason: {}",
        negative.message()
    );
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_declaration_larger_than_this_node_reads_is_refused_whole_rather_than_read_in_part()
-> Result<()> {
    // A discipline parsed from the beginning of a larger file is a
    // discipline nobody wrote. The padding is inside a JSON string so the
    // document stays well-formed: what refuses it is the size, not the shape.
    let padding =
        "x".repeat(usize::try_from(MAX_DECLARATION_BYTES).expect("the bound fits a usize") + 1);
    let oversized = GOOD.replace(
        r#""market": "ACME-USD""#,
        &format!(r#""market": "ACME-USD{padding}""#),
    );
    // Premise: the fixture really is over the bound, so this is the bound
    // firing and not a malformed document.
    assert!(u64::try_from(oversized.len()).expect("a length fits a u64") > MAX_DECLARATION_BYTES);
    let refusal = read(&oversized).expect_err("a file past the bound is refused whole");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("refused whole rather than read"),
        "the refusal should say why: {}",
        refusal.message()
    );
    // The half that proves the bound admits a document under it.
    assert!(u64::try_from(GOOD.len()).expect("a length fits a u64") < MAX_DECLARATION_BYTES);
    assert!(read(GOOD)?.is_some());
    Ok(())
}

#[test]
fn a_region_id_carrying_surrounding_whitespace_is_refused_rather_than_trimmed() -> Result<()> {
    // Two ids differing by a space are two regions to a mirror edge. Trimmed
    // here, the cell would look up a round trip under a name the router
    // never produces — and the trim would have corrected a configuration bug
    // into one that survives every later pass.
    let refusal = read(&GOOD.replace(r#""XNYS": "us-east4""#, r#""XNYS": " us-east4""#))
        .expect_err("whitespace makes it a different region");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        refusal.message().contains("surrounding whitespace"),
        "the refusal should say why: {}",
        refusal.message()
    );
    assert!(read(GOOD)?.is_some());
    Ok(())
}
