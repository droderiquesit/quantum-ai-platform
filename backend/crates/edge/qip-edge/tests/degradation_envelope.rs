//! Every input a reflex cell reads has a declared degradation envelope — a
//! freshness bound and what its stale and lost readings do to size — and a
//! table missing one is refused at configuration (R12-RES-8026).
//!
//! The failure these prevent: a cell depending on an input whose loss nobody
//! decided behaves however the surrounding code happens to fall. Until this
//! suite the envelope was spread across four configuration fields and a
//! contract table, and nothing refused a cell for which one of them said
//! nothing.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::degradation::{Capability, Freshness};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, dec};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::degradation::{DegradationEnvelope, EnvelopeRow, FreshnessBound};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_routing::health::HealthPolicy;

fn features() -> FeatureEngine {
    FeatureEngine::new(MarketState::default(), Duration::from_secs(5))
}

/// A cell configuration that reads every input a cell can: the journal wire
/// adds the ledger row and venue health the session row.
fn full_config() -> CellConfig {
    CellConfig::new("london-1", "europe-west2")
        .with_venue(VenueId::new("XLON"))
        .with_journal_wire()
        .with_venue_health(HealthPolicy::default())
}

#[test]
fn every_input_a_cell_reads_has_a_row_with_a_freshness_bound_and_a_multiplier() -> Result<()> {
    let config = full_config();
    let required = DegradationEnvelope::dependencies_of(&config);
    // Premise: the full configuration reads all nine inputs a cell can, so
    // the loop below is over every row and not a convenient subset.
    assert_eq!(
        required.len(),
        9,
        "the premise failed: {required:?} is not every input a cell can read"
    );
    let cell = Cell::new(config, features())?;
    let envelope = cell.degradation_envelope();
    assert_eq!(
        envelope.rows().keys().copied().collect::<Vec<_>>(),
        required.iter().copied().collect::<Vec<_>>(),
        "the assembled cell's table and its dependencies disagree"
    );
    for (capability, row) in envelope.rows() {
        if let Some(bound) = row.bound.duration() {
            assert!(
                bound > Duration::ZERO,
                "{} has a bound under which it is never fresh",
                capability.as_str()
            );
        } else {
            assert_eq!(
                *capability,
                Capability::Ledger,
                "only the ledger's bound is the node's spool reading"
            );
        }
        assert!(
            row.unavailable <= row.stale && row.stale <= Decimal::ONE,
            "{} has multipliers that widen",
            capability.as_str()
        );
    }
    // The values are the configuration's and the contract's, not restated.
    let feed = envelope.row(Capability::Feed).expect("a feed row");
    assert_eq!(feed.bound, FreshnessBound::Age(Duration::from_secs(5)));
    assert_eq!((feed.stale, feed.unavailable), (dec!("1"), dec!("0")));
    let clock = envelope.row(Capability::Clock).expect("a clock row");
    assert_eq!(
        (clock.stale, clock.unavailable),
        (dec!("0.75"), dec!("0.5"))
    );
    assert_eq!(
        envelope.row(Capability::Ingestion).map(|row| row.bound),
        Some(FreshnessBound::PerBook(Duration::from_secs(5)))
    );
    Ok(())
}

#[test]
fn a_cell_without_the_journal_wire_or_venue_health_declares_no_row_for_what_it_cannot_read()
-> Result<()> {
    let config = CellConfig::new("london-1", "europe-west2").with_venue(VenueId::new("XLON"));
    let cell = Cell::new(config, features())?;
    let envelope = cell.degradation_envelope();
    assert!(envelope.row(Capability::Feed).is_some(), "premise");
    assert_eq!(envelope.row(Capability::Ledger), None);
    assert_eq!(envelope.row(Capability::VenueSession), None);
    Ok(())
}

/// Mutation: make `DegradationEnvelope::new`'s missing-row loop `continue`
/// unconditionally — every removal below then assembles and the assertion
/// naming the dropped row fires.
#[test]
fn a_table_missing_the_row_for_any_input_the_cell_reads_is_refused_naming_it() {
    let config = full_config();
    let required = DegradationEnvelope::dependencies_of(&config);
    let complete = DegradationEnvelope::rows_for(&config);
    // Premise: the complete table is admitted, so each refusal below is
    // about the row removed and not about a table that never assembles.
    assert!(DegradationEnvelope::new(&required, complete.clone()).is_ok());
    assert!(!required.is_empty());
    for capability in &required {
        let mut rows = complete.clone();
        rows.remove(capability);
        let error = DegradationEnvelope::new(&required, rows).expect_err(&format!(
            "a table with no row for {} was admitted",
            capability.as_str()
        ));
        // The delimited token: `feed` would also match inside a longer word.
        assert!(
            error
                .message()
                .contains(&format!(" {} ", capability.as_str())),
            "the refusal for {} did not name it: {}",
            capability.as_str(),
            error.message()
        );
    }
}

/// Mutation: drop the `Capability::Ingestion` arm's message in
/// `DegradationEnvelope::new` so ingestion gets the generic refusal — the
/// `stale_book` assertion fires. Mutation: delete the per-book bound check —
/// the `Age` row is admitted and the second assertion fires.
#[test]
fn ingestion_omitted_or_stated_as_an_age_is_refused_as_covered_per_book_by_the_stale_book_gate() {
    let config = full_config();
    let required = DegradationEnvelope::dependencies_of(&config);
    assert!(required.contains(&Capability::Ingestion), "premise");

    let mut rows = DegradationEnvelope::rows_for(&config);
    rows.remove(&Capability::Ingestion);
    let omitted = DegradationEnvelope::new(&required, rows).expect_err("no ingestion row admitted");
    assert!(
        omitted.message().contains("stale_book gate") && omitted.message().contains("per book"),
        "the refusal did not say where ingestion's envelope lives: {}",
        omitted.message()
    );

    let mut rows = DegradationEnvelope::rows_for(&config);
    rows.insert(
        Capability::Ingestion,
        EnvelopeRow {
            bound: FreshnessBound::Age(Duration::from_secs(5)),
            stale: Decimal::ONE,
            unavailable: Decimal::ONE,
        },
    );
    let as_age = DegradationEnvelope::new(&required, rows)
        .expect_err("ingestion as a cell-wide age was admitted");
    assert!(
        as_age.message().contains("per-book bound"),
        "{}",
        as_age.message()
    );
}

/// Mutation: delete the `!required.contains(capability)` refusal — the
/// custody row is admitted and the assertion fires.
#[test]
fn a_row_for_an_input_the_cell_does_not_read_is_refused() {
    let config = full_config();
    let required = DegradationEnvelope::dependencies_of(&config);
    assert!(!required.contains(&Capability::Custody), "premise");
    let mut rows = DegradationEnvelope::rows_for(&config);
    rows.insert(
        Capability::Custody,
        EnvelopeRow {
            bound: FreshnessBound::Age(Duration::from_secs(60)),
            stale: dec!("0.75"),
            unavailable: dec!("0.5"),
        },
    );
    let error = DegradationEnvelope::new(&required, rows).expect_err("a stray row was admitted");
    assert!(
        error.message().contains("row for custody"),
        "{}",
        error.message()
    );
}

/// Mutation: change `row.unavailable > row.stale` to `false` in
/// `DegradationEnvelope::new` — the inverted clock row is admitted and the
/// assertion fires.
#[test]
fn a_row_whose_total_loss_narrows_less_than_its_partial_loss_is_refused() {
    let config = full_config();
    let required = DegradationEnvelope::dependencies_of(&config);
    let mut rows = DegradationEnvelope::rows_for(&config);
    rows.insert(
        Capability::Clock,
        EnvelopeRow {
            bound: FreshnessBound::Lead(Duration::from_millis(500)),
            stale: dec!("0.5"),
            unavailable: dec!("0.75"),
        },
    );
    let error =
        DegradationEnvelope::new(&required, rows).expect_err("an inverted row was admitted");
    assert!(
        error.message().contains("rewards the worse case"),
        "{}",
        error.message()
    );
}

/// The refusal reached through configuration rather than through a hand-built
/// table: a feed bound of zero gives the feed and ingestion rows a bound under
/// which nothing is ever fresh, and the cell does not assemble.
///
/// Mutation: delete the `duration <= Duration::ZERO` refusal in
/// `DegradationEnvelope::new` — the cell assembles and `expect_err` fires.
#[test]
fn a_cell_configured_with_a_zero_feed_bound_is_refused_at_assembly() {
    let mut config = CellConfig::new("london-1", "europe-west2").with_venue(VenueId::new("XLON"));
    // Premise: the same configuration with its default bound assembles.
    assert!(Cell::new(config.clone(), features()).is_ok());
    config.max_staleness = Duration::ZERO;
    let error = Cell::new(config, features()).expect_err("a zero feed bound assembled a cell");
    assert!(
        error.message().contains("freshness bound of 0 ms"),
        "{}",
        error.message()
    );
}

#[test]
fn the_multiplier_an_unobserved_row_applies_is_one_and_an_undeclared_one_is_zero() -> Result<()> {
    let cell = Cell::new(
        CellConfig::new("london-1", "europe-west2").with_venue(VenueId::new("XLON")),
        features(),
    )?;
    let envelope = cell.degradation_envelope();
    assert_eq!(envelope.multiplier(Capability::Clock, None), Decimal::ONE);
    assert_eq!(
        envelope.multiplier(Capability::Clock, Some(Freshness::Stale)),
        dec!("0.75")
    );
    assert_eq!(
        envelope.multiplier(Capability::Custody, Some(Freshness::Fresh)),
        Decimal::ZERO,
        "an input the cell never declared sized at something"
    );
    Ok(())
}
