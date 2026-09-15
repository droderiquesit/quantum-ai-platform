//! The §31.1 cross-region mirror this cell takes part in, as the deployment
//! declares it.
//!
//! `qip-edge` holds the discipline — `MirrorArrangement`, `Cell::install_mirror`
//! and the band and direction gate `Cell::work` routes against — and
//! `qip-edge/tests/cross_region.rs` proves it. What the library cannot do is
//! give itself the arrangement, for the reason `allocation` gives about the
//! region ceiling: a cell that decided which of its venues were abroad would
//! be deciding where it is allowed to trade, and a band a cell chose for
//! itself is not a band.
//!
//! Until this module existed no composition root gave one. `main.rs` built
//! its `CellConfig` with `with_venue` alone, so **every venue a deployed node
//! could reach sat in the node's own region**, every hop composed a transport
//! edge, and §30.2's rows 3 and 4 — the mirrored-inventory rows — were
//! reachable from a test harness and nowhere else.
//!
//! # Two facts, one file, and why neither is useful alone
//!
//! *Where a venue is* decides whether a hop to it composes a transport edge
//! or a mirror edge. *What this region's discipline is* decides whether that
//! mirror edge may be taken. A deployment stating only the first would
//! compose mirror edges and refuse every one of them for want of a band; a
//! deployment stating only the second would hold a band nothing looks up. So
//! both are declared in one document, and a document holding either half
//! without the other is refused.
//!
//! ```json
//! {
//!   "venue_regions": { "XNYS": "us-east" },
//!   "round_trips_ms": { "us-east": 28 },
//!   "instruments": {
//!     "obj-ACME": {
//!       "soft_band": "5",
//!       "hard_band": "20",
//!       "market": "ACME-USD",
//!       "threshold": "0.5"
//!     }
//!   }
//! }
//! ```
//!
//! # What this can never do
//!
//! It cannot widen what the cell may reach. A venue named here that is not in
//! `QIP_VENUES` is refused outright rather than added: a region annotation
//! says *where* a venue is and never *whether* the cell may trade there, and
//! `Cell::install_arbitrage` holds the same line at runtime for the caller
//! that sets the field directly. Nor can it touch the paper-trading boundary:
//! `Cell` has no constructor taking a ceiling other than paper trading,
//! nothing here constructs a cell, an order or a size, and everything
//! declared here reaches `qip_routing::extension::check`, which can only
//! refuse.
//!
//! # Refused, never defaulted
//!
//! Unset is a cell with no foreign venue — what every node has run as, and
//! announced at start-up rather than assumed. Set, every value is checked
//! before a port is bound: a band that could not gate anything, a venue the
//! cell may not trade, a region that is the cell's own, a region placed
//! abroad with no measured round trip to it, a measurement no venue reads.
//! Each stops the process with `configuration:`, so an orchestrator reads it
//! as "deployed wrong" rather than as a crash. A default would be a risk
//! appetite nobody stated, sitting where a direction gate is decided.

use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::Duration;
use qip_edge::mirror::{MirrorArrangement, MirroredInstrument};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Names the file declaring which of this node's venues are abroad and under
/// what discipline this region mirrors each instrument.
pub const MIRROR_VARIABLE: &str = "QIP_CROSS_REGION_MIRROR_PATH";

/// The most bytes a declaration may hold. Read whole and refused above this
/// rather than read in part: a declaration is one region's venues and the
/// handful of instruments it mirrors, and a discipline parsed from the
/// beginning of a larger file is a discipline nobody wrote.
pub const MAX_DECLARATION_BYTES: u64 = 64 * 1024;

/// The document on disk.
///
/// `deny_unknown_fields` because a misspelled key is the failure this module
/// exists to stop: `hard_bnad` silently absent leaves serde filling nothing
/// and an operator believing a hard band is in force.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    /// Venue id to the region it sits in. Every venue here must be one
    /// `QIP_VENUES` already names, and no region here may be the cell's own.
    venue_regions: BTreeMap<String, String>,
    /// Region to the measured round trip to it, in whole milliseconds. §31
    /// puts New York to London at roughly 28 ms each way.
    round_trips_ms: BTreeMap<String, i64>,
    /// Instrument id to the discipline this region holds it under.
    instruments: BTreeMap<String, InstrumentDocument>,
}

/// One mirrored instrument, as the deployment writes it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstrumentDocument {
    /// Half-width around the centre's target that counts as at target.
    soft_band: Decimal,
    /// Half-width past which the direction is forced whatever the price says.
    hard_band: Decimal,
    /// The book this cell reads a local price from. An inventory is held in
    /// an instrument and a price is quoted on a market, and the two are not
    /// the same identifier.
    market: String,
    /// How far from the distributed reference is worth trading.
    threshold: Decimal,
}

/// A declaration that passed every refusal in [`Self::read`], and the only
/// way to obtain one.
///
/// Both fields are private and there is no constructor from parts: a
/// `CrossRegionMirror` in hand is proof that the venues it places abroad are
/// venues this node may trade, that every region it names has a measured
/// round trip, and that every band in it could gate something. That is what
/// lets [`crate::assemble`] take it as a fact rather than re-check it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossRegionMirror {
    venue_regions: BTreeMap<String, String>,
    arrangement: MirrorArrangement,
}

impl CrossRegionMirror {
    /// Interpret the variable's value, against the region this cell is in and
    /// the venues it may trade.
    ///
    /// `Ok(None)` is the variable unset or blank: a cell with no foreign
    /// venue, which is every node deployed to date. Every refusal starts with
    /// `configuration:` so `main` exits with `EX_CONFIG`.
    pub fn read(value: Option<&str>, region: &str, venues: &[VenueId]) -> Result<Option<Self>> {
        let Some(path) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let document = Self::parse(Path::new(path))?;
        Self::from_document(document, path, region, venues).map(Some)
    }

    /// Read and deserialise, with the path in every message.
    ///
    /// Separated from the checks below so that "the file is not there" and
    /// "the file says something this cell refuses" are two different reports.
    /// An operator sent looking for a malformed band when the mount is simply
    /// missing loses the morning.
    fn parse(path: &Path) -> Result<Document> {
        let length = std::fs::metadata(path)
            .map_err(|error| {
                Error::io(format!(
                    "configuration: the cross-region mirror declaration at {} cannot be read: \
                     {error}. {MIRROR_VARIABLE} names it, and a node that started without it \
                     would run with every venue in its own region — the state the variable was \
                     set to leave",
                    path.display()
                ))
            })?
            .len();
        if length > MAX_DECLARATION_BYTES {
            return Err(Error::invalid(format!(
                "configuration: the cross-region mirror declaration at {} is {length} bytes and \
                 this node reads at most {MAX_DECLARATION_BYTES}; it is refused whole rather \
                 than read in part",
                path.display()
            )));
        }
        let bytes = std::fs::read(path).map_err(|error| {
            Error::io(format!(
                "configuration: the cross-region mirror declaration at {} cannot be read: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<Document>(&bytes).map_err(|error| {
            Error::invalid(format!(
                "configuration: the cross-region mirror declaration at {} is not a mirror \
                 declaration: {error}. It holds `venue_regions`, `round_trips_ms` and \
                 `instruments` and nothing else — an unrecognised key is refused rather than \
                 ignored, because a misspelled band is a band nobody is held to",
                path.display()
            ))
        })
    }

    /// Every check that needs to know what this cell is.
    fn from_document(
        document: Document,
        path: &str,
        region: &str,
        venues: &[VenueId],
    ) -> Result<Self> {
        if document.venue_regions.is_empty() {
            return Err(Error::invalid(format!(
                "configuration: the cross-region mirror declaration at {path} places no venue \
                 abroad, so no hop this cell can make composes a mirror edge and the discipline \
                 it declares would be looked up by nothing. Name the venue that is abroad, or \
                 unset {MIRROR_VARIABLE}"
            )));
        }
        if document.instruments.is_empty() {
            return Err(Error::invalid(format!(
                "configuration: the cross-region mirror declaration at {path} places a venue \
                 abroad and names no mirrored instrument, so every cross-region cycle would be \
                 refused for a missing band. Name the instruments this region mirrors, or unset \
                 {MIRROR_VARIABLE}"
            )));
        }

        // The venue half. Checked against the list the operator already wrote
        // rather than added to it: this document says where a venue is and
        // never that the cell may reach it.
        for (venue, placed_in) in &document.venue_regions {
            if !venues.iter().any(|known| known.as_str() == venue) {
                return Err(Error::denied(format!(
                    "configuration: the cross-region mirror declaration at {path} places venue \
                     {venue} in region {placed_in}, and it is not a venue this cell may trade. A \
                     region annotation says where a venue is and never that the cell may reach \
                     it, so add {venue} to QIP_VENUES deliberately or remove the annotation"
                )));
            }
            if placed_in.is_empty() || placed_in.trim() != placed_in.as_str() {
                return Err(Error::invalid(format!(
                    "configuration: the cross-region mirror declaration at {path} places venue \
                     {venue} in region {placed_in:?}, which is empty or carries surrounding \
                     whitespace. Two region ids differing by a space are two regions to a mirror \
                     edge, so supply it exactly rather than relying on this to trim"
                )));
            }
            if placed_in == region {
                return Err(Error::invalid(format!(
                    "configuration: the cross-region mirror declaration at {path} places venue \
                     {venue} in region {placed_in}, which is this cell's own region. A mirror \
                     edge is one asset in two regions and the router refuses one whose ends \
                     share a region, so this annotation could only compose an edge nothing \
                     routes; a venue at home is left out of this map"
                )));
            }
        }

        // The measurement half, refused in both directions. A region a venue
        // sits in with no round trip is a cycle that reaches the pass and is
        // refused there, naming the centre; a round trip to a region no venue
        // sits in is a number nothing looks up, and a measurement nothing
        // reads cannot be told from one nobody took.
        let abroad: BTreeMap<&str, &str> = document
            .venue_regions
            .iter()
            .map(|(venue, placed_in)| (placed_in.as_str(), venue.as_str()))
            .collect();
        for (placed_in, venue) in &abroad {
            if !document.round_trips_ms.contains_key(*placed_in) {
                return Err(Error::invalid(format!(
                    "configuration: the cross-region mirror declaration at {path} places venue \
                     {venue} in region {placed_in} and measures no round trip to it. §30.2's \
                     row 6 is decided on that measurement and there is no default for it — a \
                     defaulted round trip makes a remote quote look as though it outlasts the \
                     wire. Measure it, or do not place a venue there"
                )));
            }
        }
        let mut arrangement = MirrorArrangement::new();
        for (measured, millis) in &document.round_trips_ms {
            if !abroad.contains_key(measured.as_str()) {
                return Err(Error::invalid(format!(
                    "configuration: the cross-region mirror declaration at {path} measures a \
                     round trip to region {measured} and places no venue there, so nothing ever \
                     looks it up. Place a venue in {measured} or drop the measurement"
                )));
            }
            // `Duration::from_millis` takes a signed count, so a negative
            // measurement survives to `with_round_trip`, which refuses it
            // beside zero. Nothing is corrected here first.
            arrangement = arrangement
                .with_round_trip(measured.as_str(), Duration::from_millis(*millis))
                .map_err(|error| Self::in_file(path, &error))?;
        }

        // The discipline half. `MirroredInstrument::new` refuses a band that
        // could not gate anything and a threshold that would make every tick
        // a dislocation.
        for (object, declared) in &document.instruments {
            if object.is_empty() || declared.market.is_empty() {
                return Err(Error::invalid(format!(
                    "configuration: the cross-region mirror declaration at {path} names \
                     instrument {object:?} quoted on market {:?}, and an empty id matches no \
                     holding and no book. Supply the instrument this region mirrors and the \
                     market it is quoted on",
                    declared.market
                )));
            }
            let discipline = MirroredInstrument::new(
                declared.soft_band,
                declared.hard_band,
                ObjectId::from_string(declared.market.clone()),
                declared.threshold,
            )
            .map_err(|error| Self::in_file(path, &error))?;
            arrangement =
                arrangement.with_instrument(ObjectId::from_string(object.clone()), discipline);
        }

        Ok(Self {
            venue_regions: document.venue_regions,
            arrangement,
        })
    }

    /// A refusal from `qip-edge` with this file named.
    ///
    /// Re-raised as `invalid` rather than by the original class, and that is
    /// exact rather than lossy: the two constructors this wraps —
    /// `MirrorArrangement::with_round_trip` and `MirroredInstrument::new` —
    /// refuse only malformed values, which is what `invalid` means. A caller
    /// matching on the class is told the truth, and an operator is told which
    /// file to edit.
    fn in_file(path: &str, error: &Error) -> Error {
        Error::invalid(format!(
            "configuration: the cross-region mirror declaration at {path} is refused: {}",
            error.message()
        ))
    }

    /// Where each venue placed abroad sits, keyed by venue id.
    ///
    /// A `BTreeMap` because it becomes `CellConfig::venue_regions`, which the
    /// cycle router is built from in iteration order, and a replay that
    /// reorders is not a replay.
    pub const fn venue_regions(&self) -> &BTreeMap<String, String> {
        &self.venue_regions
    }

    /// How many instruments this region mirrors.
    pub fn instruments(&self) -> usize {
        self.arrangement.len()
    }

    /// The arrangement `Cell::install_mirror` takes.
    pub fn into_arrangement(self) -> MirrorArrangement {
        self.arrangement
    }

    /// One start-up line naming what a reviewer can check against the file.
    pub fn banner_line(&self) -> String {
        let abroad: Vec<String> = self
            .venue_regions
            .iter()
            .map(|(venue, region)| format!("{venue}@{region}"))
            .collect();
        format!(
            "qip-edge-node: cross-region mirror: {} instrument(s) mirrored, venue(s) abroad [{}]",
            self.arrangement.len(),
            abroad.join(", ")
        )
    }
}

/// What a node with no declaration prints.
///
/// Spelled here rather than in `main.rs` so the sentence a node prints when
/// it has no declaration sits beside the one it prints when it has one, and
/// the two cannot drift into describing different features.
pub fn no_declaration_line(region: &str) -> String {
    format!(
        "qip-edge-node: no cross-region mirror declared ({MIRROR_VARIABLE} unset): every venue \
         is in region {region}, and a cycle crossing a region boundary would be refused whole"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_variable_is_a_cell_with_no_foreign_venue_rather_than_a_refusal() {
        // The failure this prevents: a node that has never mirrored anything
        // refusing to start because a new variable arrived unset.
        for value in [None, Some(""), Some("   ")] {
            assert_eq!(
                CrossRegionMirror::read(value, "eu-west", &[VenueId::new("XNYS")])
                    .expect("an unset declaration is not a refusal"),
                None,
                "{value:?} should read as no declaration"
            );
        }
    }

    #[test]
    fn a_path_that_names_nothing_is_refused_rather_than_read_as_unset() {
        // Premise: the same reader admits the unset case above, so this is
        // the path being absent and not the reader refusing everything.
        let refusal = CrossRegionMirror::read(
            Some("/nonexistent/qip-cross-region.json"),
            "eu-west",
            &[VenueId::new("XNYS")],
        )
        .expect_err("a named file that is not there is a configuration fault");
        assert_eq!(refusal.code(), "io");
        assert!(
            refusal.message().starts_with("configuration:"),
            "the refusal must exit EX_CONFIG: {}",
            refusal.message()
        );
    }
}
