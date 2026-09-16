//! The New York Fed's effective federal funds rate through the connector
//! runtime, with no network.
//!
//! The recorded fixture is the body `markets.newyorkfed.org` served on
//! 2026-09-16 at 01:57 UTC, byte for byte. What these tests hold is the part a
//! contract harness cannot: that each daily rate comes out stamped with both
//! instants and withheld until the later of them, that the rates are published
//! **oldest first** whatever order the vendor wrote its array in, that a row
//! about some other reference rate is refused rather than filed under
//! `POLICY_RATE.US.EFFR`, and that the vendor's own revision flag reaches the
//! record instead of every row being declared final.

// The workspace denies `panic_in_result_fn` for production code. In a test the
// assertion is the deliverable, and `?` keeps the fixtures readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::connector::checkpoint::Cursor;
use qip_market_ingestion::connector::emulator::SourceEmulator;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector::{ConnectorRuntime, RuntimeConfig, SourceConnector};
use qip_market_ingestion::connectors::{NyFedEffrConnector, nyfed_effr};
use qip_transport::RecordingSleeper;
use std::sync::Arc;

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a fixture timestamp is valid RFC 3339")
}

/// An instant at which nine of the recording's ten effective dates have passed
/// their five-day dissemination delay and the newest has not.
///
/// Chosen so that one poll shows both halves of the knowability gate at once:
/// 2026-09-11 became knowable at midnight on 2026-09-16 and 2026-09-14 does
/// not until 2026-09-19.
fn horizon() -> Timestamp {
    at("2026-09-16T09:00:00Z")
}

fn connector() -> Result<(NyFedEffrConnector, SourceEmulator)> {
    let connector = NyFedEffrConnector::new(NyFedEffrConnector::shipped_manifest()?)?;
    Ok((connector, SourceEmulator::from_json(nyfed_effr::FIXTURE)?))
}

fn runtime_for(connector: &dyn SourceConnector) -> Result<ConnectorRuntime> {
    let config = RuntimeConfig::seeded(11).with_sleeper(Arc::new(RecordingSleeper::new()));
    ConnectorRuntime::new(connector.manifest().clone(), config)
}

/// The body the fixture recorded, as JSON, for the tests that edit it.
fn recorded() -> Result<serde_json::Value> {
    let script: serde_json::Value = serde_json::from_str(nyfed_effr::FIXTURE)?;
    let body = script["exchanges"][0]["answers"][0]["body"]
        .as_str()
        .expect("the fixture records a body");
    Ok(serde_json::from_str(body)?)
}

#[test]
fn the_recorded_response_decodes_into_one_rate_per_business_day_with_the_new_york_feds_own_figures()
-> Result<()> {
    let (mut connector, mut emulator) = connector()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;
    let outcome = runtime.poll(&mut connector, transport, horizon())?;

    // Premise: something was admitted at all, and something was withheld, so
    // the assertions below are about the contents rather than about an empty
    // list — and the knowability gate is proven to be doing work on this very
    // poll rather than on a different one.
    assert_eq!(
        outcome.admitted.len(),
        9,
        "the recorded response produced {:?}",
        outcome.admitted
    );
    assert_eq!(
        outcome.withheld, 1,
        "the newest effective date was released before its dissemination delay elapsed"
    );

    let mut dates = Vec::new();
    for envelope in &outcome.admitted {
        let SensedRecord::Macro(observation) = envelope.record() else {
            panic!(
                "an effective federal funds rate decoded into {:?}",
                envelope
            );
        };
        assert_eq!(observation.series_id, "POLICY_RATE.US.EFFR");
        assert_eq!(observation.region, "US");
        assert_eq!(observation.unit, "percent per annum, USD");
        assert_eq!(observation.provenance.source, "nyfed-effr");
        // The level the New York Fed published on every one of these dates.
        // A decode that read a percentile column instead of `percentRate`
        // would still produce nine numbers in the right shape — 3.60, 3.62,
        // 3.64 are all plausible rates — so the value is asserted and not
        // merely the count.
        assert!(
            (observation.value - 3.63).abs() < f64::EPSILON,
            "the level for {} is {}",
            observation.reference_date.to_date_string(),
            observation.value
        );
        // Neither invented: the endpoint publishes no consensus and no prior
        // level, and a surprise built from one nobody forecast is a signal
        // out of nothing.
        assert_eq!(observation.consensus, None);
        assert_eq!(observation.previous, None);
        // Nothing in the recording carries a revision indicator, so nothing
        // may claim to be a revision.
        assert!(!observation.is_revision);
        dates.push(observation.reference_date.to_date_string());
    }
    assert_eq!(
        dates,
        vec![
            "2026-08-31",
            "2026-09-01",
            "2026-09-02",
            "2026-09-03",
            "2026-09-04",
            "2026-09-08",
            "2026-09-09",
            "2026-09-10",
            "2026-09-11",
        ],
        "the nine released dates are not the nine the recording carries"
    );
    Ok(())
}

#[test]
fn the_rates_are_published_oldest_first_however_the_vendor_ordered_its_array() -> Result<()> {
    // The failure this prevents is not hypothetical arithmetic: the markets
    // API serves its array **newest first**, and
    // `qip_capital_fabric::tolerance::PolicyRateTable::record` refuses a
    // figure stamped earlier than the one it already holds. Published in the
    // vendor's own order, the newest rate would land first and every older one
    // would then be refused as a superseded replay — filling the kernel's
    // capture problems on a completely healthy poll, and leaving the table
    // right for the wrong reason.
    let (connector, _) = connector()?;
    let payload = recorded()?;
    // Premise: the vendor really does serve newest first, so the ordering
    // below is the connector's work and not the vendor's.
    let served: Vec<&str> = payload["refRates"]
        .as_array()
        .expect("the recording carries an array of rates")
        .iter()
        .map(|row| {
            row["effectiveDate"]
                .as_str()
                .expect("each row carries an effective date")
        })
        .collect();
    assert_eq!(served.first(), Some(&"2026-09-14"));
    assert_eq!(served.last(), Some(&"2026-08-31"));

    let events = connector.decode(&payload, &Cursor::beginning())?;
    assert_eq!(events.len(), 10);
    let decoded: Vec<Timestamp> = events.iter().map(|event| event.event_time).collect();
    let mut ascending = decoded.clone();
    ascending.sort_unstable();
    assert_eq!(
        decoded, ascending,
        "the decoded rates are not in ascending order of effective date"
    );
    Ok(())
}

#[test]
fn an_effective_rate_is_withheld_until_the_new_york_fed_would_have_published_the_day_it_covers()
-> Result<()> {
    // The point-in-time property, on the feed whose figures reach a
    // reconciliation tolerance for the desk's own dollars. A rate read on the
    // day its transactions took place is a rate read a full business day
    // before the New York Fed had computed it, and a tolerance derived from
    // one judges a book against something nobody could have known.
    let (mut connector, mut emulator) = connector()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let too_early = runtime.poll(&mut connector, transport, at("2026-09-01T09:00:00Z"))?;
    assert!(
        too_early.admitted.is_empty(),
        "a rate was released before its dissemination delay elapsed: {:?}",
        too_early.admitted
    );
    assert_eq!(too_early.withheld, 10);

    let knowable = runtime.poll(&mut connector, transport, horizon())?;
    assert_eq!(
        knowable.admitted.len(),
        9,
        "the withheld rates never arrived on a later poll"
    );
    for envelope in &knowable.admitted {
        assert_eq!(
            envelope.knowable_at(),
            envelope.event_time().saturating_add(Duration::from_days(5))
        );
        assert!(
            envelope.knowable_at() <= horizon(),
            "a rate was released before it was knowable"
        );
    }
    Ok(())
}

#[test]
fn the_same_response_served_twice_releases_its_rates_once() -> Result<()> {
    // The endpoint serves the same ten rows for as long as the business day
    // lasts, and this connector polls hourly. Without dedup on the stable
    // fingerprint the same nine rates would be published a dozen times a day,
    // and the capital fabric's own guard against a superseded rate would never
    // see a difference to refuse.
    let (mut connector, mut emulator) = connector()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let first = runtime.poll(&mut connector, transport, horizon())?;
    // Premise: the first poll released something, so an empty second poll is
    // dedup and not a broken fixture.
    assert_eq!(first.admitted.len(), 9);

    let second = runtime.poll(
        &mut connector,
        transport,
        horizon().saturating_add(Duration::from_hours(1)),
    )?;
    assert!(
        second.admitted.is_empty(),
        "the identical response was published a second time: {:?}",
        second.admitted
    );
    assert_eq!(second.duplicates, 9);
    Ok(())
}

#[test]
fn a_row_about_another_reference_rate_is_refused_rather_than_filed_under_the_effr_series_id()
-> Result<()> {
    // The narrow half of this connector's defence, and the reason it is
    // written down. The response carries no currency field, so the identity of
    // the rate is the only thing standing between a number about some other
    // series and `POLICY_RATE.US.EFFR` — the id
    // `qip_capital_fabric::tolerance` keys its own refusal on. Nothing on this
    // hop is authenticated, so a row claiming to be SOFR is refused by name
    // rather than believed.
    let (connector, _) = connector()?;
    let payload = recorded()?;
    // Premise: the recording decodes, so the refusal below is about the edit
    // and not about the fixture.
    assert_eq!(connector.decode(&payload, &Cursor::beginning())?.len(), 10);

    let mut swapped = payload;
    swapped["refRates"]
        .as_array_mut()
        .expect("the recording carries an array of rates")[3]["type"] = serde_json::json!("SOFR");
    let refused = connector
        .decode(&swapped, &Cursor::beginning())
        .expect_err("a row about another reference rate was filed under the EFFR series id");
    assert!(
        refused.message().contains("SOFR"),
        "the refusal does not name what arrived: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_response_carrying_more_rows_than_the_path_asked_for_is_refused_before_anything_is_allocated()
-> Result<()> {
    // A well-formed large answer is exactly what a bound exists to refuse: the
    // path asks for the last ten, so an answer carrying more is either a
    // different query or a vendor change, and both are findings rather than
    // rows to decode.
    let (connector, _) = connector()?;
    let payload = recorded()?;
    let rows = payload["refRates"]
        .as_array()
        .expect("the recording carries an array of rates")
        .clone();
    // Premise: the recording is exactly at the bound, so the refusal below is
    // about the eleventh row.
    assert_eq!(rows.len(), 10);
    assert_eq!(connector.decode(&payload, &Cursor::beginning())?.len(), 10);

    let mut oversized = payload;
    let mut extra = rows[0].clone();
    extra["effectiveDate"] = serde_json::json!("2026-09-15");
    oversized["refRates"]
        .as_array_mut()
        .expect("the recording carries an array of rates")
        .insert(0, extra);
    let refused = connector
        .decode(&oversized, &Cursor::beginning())
        .expect_err("a response larger than the question was decoded");
    assert!(
        refused.message().contains("11 row(s)"),
        "the refusal does not say how large the answer was: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_row_the_vendor_flagged_as_revised_reaches_the_record_as_a_revision() -> Result<()> {
    // The New York Fed's Terms of Use reserve the right to alter "rate
    // revision practices ... at any time without prior notice", and the
    // response carries a `revisionIndicator` for exactly that. A record that
    // declared every row final would make a republished figure
    // indistinguishable from the original one, which is the difference between
    // a tolerance derived from a corrected rate and one derived from a rate
    // that was never corrected.
    let (connector, _) = connector()?;
    let payload = recorded()?;
    // Premise: nothing in the recording is flagged, so the assertion below is
    // about the flag and not about a default.
    let unflagged = connector.decode(&payload, &Cursor::beginning())?;
    assert_eq!(unflagged.len(), 10);
    for event in &unflagged {
        let record = connector.map(event, horizon())?;
        let SensedRecord::Macro(observation) = record else {
            panic!("a rate decoded into something other than a macro observation");
        };
        assert!(!observation.is_revision);
    }

    let mut revised = payload;
    revised["refRates"]
        .as_array_mut()
        .expect("the recording carries an array of rates")[9]["revisionIndicator"] =
        serde_json::json!("R");
    let events = connector.decode(&revised, &Cursor::beginning())?;
    // The revised row is 2026-08-31, the vendor's last and this connector's
    // first, so it is the one to read.
    let record = connector.map(&events[0], horizon())?;
    let SensedRecord::Macro(observation) = record else {
        panic!("a rate decoded into something other than a macro observation");
    };
    assert_eq!(observation.reference_date.to_date_string(), "2026-08-31");
    assert!(
        observation.is_revision,
        "a row the vendor flagged as revised was recorded as an original"
    );
    Ok(())
}

#[test]
fn a_reference_rate_source_that_declares_no_dissemination_delay_is_refused() -> Result<()> {
    // Premise: the shipped manifest declares one, so the refusal below is
    // about the edit.
    let shipped = NyFedEffrConnector::shipped_manifest()?;
    assert!(shipped.publication_delay_ms > 0);

    let mut instant = shipped;
    instant.publication_delay_ms = 0;
    let refused = NyFedEffrConnector::new(instant)
        .expect_err("a delayed publication was configured as an instantaneous one");
    assert!(
        refused.message().contains("no instant saying when"),
        "the refusal does not say why a delay is needed here: {}",
        refused.message()
    );
    Ok(())
}
#[test]
fn a_level_outside_the_band_an_effective_rate_can_occupy_is_refused_rather_than_clamped()
-> Result<()> {
    // A tolerance decides whether the books balance, so a mis-scaled figure
    // reaching it is a halt computed from a number nobody published. Refused,
    // never corrected: nobody knows what `1e300` should have been, and a value
    // silently clamped is a vendor bug that survives into a backtest.
    let (connector, _) = connector()?;
    let payload = recorded()?;
    // Premise: the recording's own level decodes, so the refusals below are
    // about the edits and not about the band being closed to everything.
    assert_eq!(connector.decode(&payload, &Cursor::beginning())?.len(), 10);

    for (level, what) in [
        // A rate quoted in basis points rather than percent — the mis-scaling
        // that actually happens when a vendor changes a unit.
        (serde_json::json!(363.0), "a basis-point quote"),
        // Finite, positive, and infinite the moment anything squares it.
        (serde_json::json!(1e300), "an absurd magnitude"),
        // Below the floor that admits every negative policy any major central
        // bank has set.
        (serde_json::json!(-20.0), "a level below the floor"),
    ] {
        let mut edited = payload.clone();
        edited["refRates"]
            .as_array_mut()
            .expect("the recording carries an array of rates")[0]["percentRate"] = level.clone();
        let decoded = connector
            .decode(&edited, &Cursor::beginning())
            .unwrap_or_default();
        assert!(
            decoded.is_empty(),
            "{what} ({level}) was published as an effective federal funds rate"
        );
    }

    // And the band admits the real extremes, which is what distinguishes a
    // working bound from one that refuses everything: 22.36 is the highest
    // daily level on record and 0.04 the lowest.
    for level in [serde_json::json!(22.36), serde_json::json!(0.04)] {
        let mut edited = payload.clone();
        edited["refRates"]
            .as_array_mut()
            .expect("the recording carries an array of rates")[0]["percentRate"] = level.clone();
        assert_eq!(
            connector.decode(&edited, &Cursor::beginning())?.len(),
            10,
            "a level the New York Fed has actually published ({level}) was refused"
        );
    }
    Ok(())
}
