//! The ECB's key interest rates through the connector runtime, with no
//! network.
//!
//! The recorded fixture is the body `data-api.ecb.europa.eu` served on
//! 2026-09-15 at 21:10 UTC, byte for byte. What these tests hold is the part a
//! contract harness cannot: that the three levels come out stamped with both
//! instants and withheld until the later of them, that a level is attributed
//! to the rate the message says it is rather than to whichever series key
//! happened to be first, and that the response is held to the **currency** the
//! manifest asked for — which is the fact `qip-capital-fabric` refuses on, and
//! therefore the fact a hostile answer on an unauthenticated hop would most
//! want to change.

// The workspace denies `panic_in_result_fn` for production code. In a test the
// assertion is the deliverable, and `?` keeps the fixtures readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::connector::emulator::SourceEmulator;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector::{ConnectorRuntime, RuntimeConfig, SourceConnector};
use qip_market_ingestion::connectors::{EcbKeyRatesConnector, ecb_key_rates};
use qip_transport::RecordingSleeper;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a fixture timestamp is valid RFC 3339")
}

/// The observation date the recorded message carries, plus the manifest's
/// sixteen-hour dissemination delay and a margin.
fn horizon() -> Timestamp {
    at("2026-09-16T09:00:00Z")
}

fn connector() -> Result<(EcbKeyRatesConnector, SourceEmulator)> {
    let connector = EcbKeyRatesConnector::new(EcbKeyRatesConnector::shipped_manifest()?)?;
    Ok((
        connector,
        SourceEmulator::from_json(ecb_key_rates::FIXTURE)?,
    ))
}

fn runtime_for(connector: &dyn SourceConnector) -> Result<ConnectorRuntime> {
    let config = RuntimeConfig::seeded(11).with_sleeper(Arc::new(RecordingSleeper::new()));
    ConnectorRuntime::new(connector.manifest().clone(), config)
}

/// The three levels the recording carries, by series id.
fn levels(records: &[SensedRecord]) -> BTreeMap<String, f64> {
    records
        .iter()
        .filter_map(|record| match record {
            SensedRecord::Macro(observation) => {
                Some((observation.series_id.clone(), observation.value))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_recorded_message_decodes_into_one_level_per_key_rate_with_the_ecb_s_own_figures()
-> Result<()> {
    let (mut connector, mut emulator) = connector()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;
    let outcome = runtime.poll(&mut connector, transport, horizon())?;

    // Premise: something was admitted at all, so the assertions below are
    // about the contents and not about an empty list.
    assert_eq!(
        outcome.admitted.len(),
        3,
        "the recorded message produced {:?}",
        outcome.admitted
    );
    let records: Vec<SensedRecord> = outcome
        .admitted
        .iter()
        .map(|envelope| envelope.record().clone())
        .collect();
    let by_series = levels(&records);
    // The figures the ECB's own portal served on 2026-09-15. The deposit
    // facility is the one §38.3's fiat row can take an interval from; the
    // other two are absorbed and govern no tolerance.
    assert_eq!(
        by_series,
        BTreeMap::from([
            ("POLICY_RATE.EA.DFR".to_string(), 2.25),
            ("POLICY_RATE.EA.MLFR".to_string(), 2.65),
            ("POLICY_RATE.EA.MRR_FR".to_string(), 2.4),
        ])
    );
    // The ordering of the deposit facility beneath the main refinancing rate
    // beneath the marginal lending rate is the corridor the ECB operates. A
    // decode that mis-indexed the series keys would still produce three
    // numbers and would produce them in the wrong order, which no count
    // catches.
    assert!(
        by_series["POLICY_RATE.EA.DFR"] < by_series["POLICY_RATE.EA.MRR_FR"]
            && by_series["POLICY_RATE.EA.MRR_FR"] < by_series["POLICY_RATE.EA.MLFR"],
        "the levels are not in the ECB's own corridor order: {by_series:?}"
    );

    for record in &records {
        let SensedRecord::Macro(observation) = record else {
            panic!("a key interest rate decoded into {record:?}");
        };
        assert_eq!(observation.region, "EA");
        assert_eq!(observation.unit, "percent per annum, EUR");
        assert_eq!(observation.reference_date, at("2026-09-15T00:00:00Z"));
        assert_eq!(observation.provenance.source, "ecb-key-interest-rates");
        // Neither invented: the feed publishes no consensus and no prior
        // level, and a surprise built from one nobody forecast is a signal
        // out of nothing.
        assert_eq!(observation.consensus, None);
        assert_eq!(observation.previous, None);
    }
    Ok(())
}

#[test]
fn a_key_rate_is_withheld_until_the_ecb_would_have_published_the_day_it_applies_to() -> Result<()> {
    // The point-in-time property, on the feed whose figures reach a
    // reconciliation tolerance. A level read at midnight on the date it
    // applies to is a level read before the vendor wrote the row, and a
    // tolerance derived from it judges a book against something nobody could
    // have known.
    let (mut connector, mut emulator) = connector()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let too_early = runtime.poll(&mut connector, transport, at("2026-09-15T09:00:00Z"))?;
    assert!(
        too_early.admitted.is_empty(),
        "a level stamped 2026-09-15 was released at 09:00 on 2026-09-15, seven hours before the \
         manifest says it became readable"
    );
    assert_eq!(too_early.withheld, 3);

    let knowable = runtime.poll(&mut connector, transport, horizon())?;
    assert_eq!(
        knowable.admitted.len(),
        3,
        "the withheld levels never arrived on a later poll"
    );
    for envelope in &knowable.admitted {
        assert_eq!(
            envelope.knowable_at(),
            envelope
                .event_time()
                .saturating_add(Duration::from_hours(16))
        );
        assert!(
            envelope.knowable_at() <= horizon(),
            "a level was released before it was knowable"
        );
    }
    Ok(())
}

#[test]
fn the_same_message_served_twice_releases_its_levels_once() -> Result<()> {
    // The ECB serves the same daily row for as long as the day lasts, and this
    // connector polls hourly. Without dedup on the stable fingerprint the same
    // three levels would be published twelve times a day, and the capital
    // fabric's own guard against a superseded rate would never see a
    // difference to refuse.
    let (mut connector, mut emulator) = connector()?;
    let mut runtime = runtime_for(&connector)?;
    let transport: &mut dyn SourceTransport = &mut emulator;

    let first = runtime.poll(&mut connector, transport, horizon())?;
    // Premise: the first poll released something, so an empty second poll is
    // dedup and not a broken fixture.
    assert_eq!(first.admitted.len(), 3);

    let second = runtime.poll(
        &mut connector,
        transport,
        horizon().saturating_add(Duration::from_hours(1)),
    )?;
    assert!(
        second.admitted.is_empty(),
        "the identical message was published a second time: {:?}",
        second.admitted
    );
    assert_eq!(second.duplicates, 3);
    Ok(())
}

#[test]
fn a_message_about_another_currency_is_refused_rather_than_published_under_a_euro_series_id()
-> Result<()> {
    // The failure this prevents is the one the whole source exists to avoid.
    // `qip_capital_fabric::tolerance` refuses to let a euro rate judge a book
    // in another currency, and it can only do that if `POLICY_RATE.EA.DFR`
    // really is a euro rate. Nothing on this hop is authenticated — the
    // transport speaks plaintext HTTP/1.1 behind an egress proxy — so a
    // response claiming another currency is refused by name rather than
    // believed.
    let (connector, _) = connector()?;
    let recorded: serde_json::Value = {
        let script: serde_json::Value = serde_json::from_str(ecb_key_rates::FIXTURE)?;
        let body = script["exchanges"][0]["answers"][0]["body"]
            .as_str()
            .expect("the fixture records a body");
        serde_json::from_str(body)?
    };
    // Premise: the recorded message decodes, so the refusal below is about the
    // edit and not about the fixture.
    let cursor = qip_market_ingestion::connector::checkpoint::Cursor::beginning();
    assert_eq!(connector.decode(&recorded, &cursor)?.len(), 3);

    let mut swapped = recorded;
    for dimension in swapped["structure"]["dimensions"]["series"]
        .as_array_mut()
        .expect("the message declares series dimensions")
    {
        if dimension["id"] == serde_json::json!("CURRENCY") {
            dimension["values"] = serde_json::json!([{ "id": "USD", "name": "US dollar" }]);
        }
    }
    let refused = connector
        .decode(&swapped, &cursor)
        .expect_err("a message about another currency was decoded into euro-area series");
    assert!(
        refused.message().contains("`CURRENCY` dimension"),
        "the refusal is not about the currency: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_message_carrying_a_rate_nobody_asked_for_is_refused_rather_than_partly_published() -> Result<()>
{
    // A source answering a question this connector did not ask has either
    // changed or been answered by someone else, and both are findings. Each
    // unrequested identifier would otherwise mint a permanent feature series
    // and a permanent event-log key from a value the vendor chose.
    let (connector, _) = connector()?;
    let cursor = qip_market_ingestion::connector::checkpoint::Cursor::beginning();
    let script: serde_json::Value = serde_json::from_str(ecb_key_rates::FIXTURE)?;
    let body = script["exchanges"][0]["answers"][0]["body"]
        .as_str()
        .expect("the fixture records a body");
    let recorded: serde_json::Value = serde_json::from_str(body)?;
    // Premise: the three requested identifiers are what the recording carries.
    let asked: BTreeSet<&str> = EcbKeyRatesConnector::PUBLISHED_RATES.into_iter().collect();
    assert_eq!(asked.len(), 3);
    assert_eq!(connector.decode(&recorded, &cursor)?.len(), 3);

    let mut extended = recorded;
    for dimension in extended["structure"]["dimensions"]["series"]
        .as_array_mut()
        .expect("the message declares series dimensions")
    {
        if dimension["id"] == serde_json::json!("PROVIDER_FM_ID") {
            dimension["values"]
                .as_array_mut()
                .expect("the dimension declares values")
                .push(serde_json::json!({ "id": "SOMETHING_ELSE", "name": "not asked for" }));
        }
    }
    let refused = connector
        .decode(&extended, &cursor)
        .expect_err("a message carrying an unrequested rate identifier was decoded");
    assert!(
        refused.message().contains("SOMETHING_ELSE"),
        "the refusal does not name what arrived: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_key_rate_source_that_declares_no_dissemination_delay_is_refused() -> Result<()> {
    // Premise: the shipped manifest declares one, so the refusal below is
    // about the edit.
    let shipped = EcbKeyRatesConnector::shipped_manifest()?;
    assert!(shipped.publication_delay_ms > 0);

    let mut instant = shipped;
    instant.publication_delay_ms = 0;
    let refused = EcbKeyRatesConnector::new(instant)
        .expect_err("a delayed publication was configured as an instantaneous one");
    assert!(
        refused.message().contains("no instant saying when"),
        "the refusal does not say why a delay is needed here: {}",
        refused.message()
    );
    Ok(())
}
