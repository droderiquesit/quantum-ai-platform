//! The evidence-check stage and the capture gate of the connector runtime
//! (R12-DATA-2012, R12-EVID-2031, R12-DATA-1034, R12-TICK-2049, 2087).
//!
//! Each test drives a real `ConnectorRuntime` over a recorded exchange and
//! asserts on what the poll report carries, premise first.

#![allow(clippy::panic_in_result_fn)]

mod connector_common;

use connector_common::{TestConnector, at, body, manifest, manifest_json};
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::connector::emulator::{RecordedAnswer, RecordedExchange, SourceEmulator};
use qip_market_ingestion::connector::runtime::ConnectorRuntime;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector::{RuntimeConfig, SourceManifest};
use qip_world_model::EvidenceFabric;
use std::sync::{Arc, Mutex};

fn now() -> Timestamp {
    at("2026-08-24T15:00:00Z")
}

fn serving(body: &str) -> SourceEmulator {
    SourceEmulator::new(vec![
        RecordedExchange::always("/v1/health", RecordedAnswer::json(200, "{}")),
        RecordedExchange::always("/v1/events", RecordedAnswer::json(200, body)),
    ])
}

fn runtime(
    manifest: SourceManifest,
    fabric: Option<Arc<Mutex<EvidenceFabric>>>,
) -> Result<ConnectorRuntime> {
    let mut config = RuntimeConfig::seeded(7);
    if let Some(fabric) = fabric {
        config = config.with_evidence_fabric(fabric);
    }
    ConnectorRuntime::new(manifest, config)
}

#[test]
fn every_admitted_record_carries_checked_evidence_naming_its_parser_licence_and_document()
-> Result<()> {
    let text = body(&[
        ("e1", "2026-08-24T14:59:00Z", "101.25"),
        ("e2", "2026-08-24T14:59:30Z", "101.50"),
    ]);
    let mut emulator = serving(&text);
    let mut runtime = runtime(manifest(), None)?;
    let mut connector = TestConnector::new(manifest());
    let transport: &mut dyn SourceTransport = &mut emulator;
    let report = runtime.poll(&mut connector, transport, now())?;

    assert_eq!(
        report.admitted.len(),
        2,
        "premise: both events were admitted"
    );
    assert_eq!(
        report.evidence.len(),
        2,
        "one evidence object per admitted record"
    );
    let document = qip_core::sha256_hex(text.as_bytes());
    for checked in &report.evidence {
        assert_eq!(checked.document_hash, document);
        assert!(
            checked
                .record
                .extraction_method
                .starts_with("qip-market-ingestion::connector::test-source@"),
            "{}",
            checked.record.extraction_method
        );
        assert_eq!(checked.record.license, "public");
        assert!(checked.verdict.admissible);
        assert_eq!(
            checked.verdict.authenticity.credited.len(),
            1,
            "the hash signal names the fetched body and is credited: {:?}",
            checked.verdict.authenticity
        );
        assert!(checked.verdict.belief_weight > 0.0);
    }
    assert_eq!(report.knowledge_delta.admitted, 2);
    assert_eq!(
        runtime
            .evidence_fabric()
            .lock()
            .map(|f| f.len())
            .unwrap_or(0),
        2
    );
    Ok(())
}

#[test]
fn a_central_tick_is_stamped_with_the_poll_instant_and_a_per_feed_sequence_that_continues()
-> Result<()> {
    let mut runtime = runtime(manifest(), None)?;
    let mut connector = TestConnector::new(manifest());

    let mut first = serving(&body(&[
        ("e1", "2026-08-24T14:59:00Z", "101.25"),
        ("e2", "2026-08-24T14:59:30Z", "101.50"),
    ]));
    let report = runtime.poll(&mut connector, &mut first, now())?;
    assert_eq!(report.admitted.len(), 2, "premise: both admitted");
    let stamps: Vec<(Timestamp, Option<u64>)> = report
        .admitted
        .iter()
        .map(|envelope| match envelope.record() {
            SensedRecord::Tick(tick) => (tick.capture_time, tick.sequence),
            other => panic!("the test connector maps ticks, not {other:?}"),
        })
        .collect();
    // The connector stamped the event instant; the gate overwrote it with
    // the instant it received the record.
    assert_eq!(stamps, vec![(now(), Some(1)), (now(), Some(2))]);

    let later = at("2026-08-24T15:01:00Z");
    let mut second = serving(&body(&[("e3", "2026-08-24T15:00:30Z", "102.00")]));
    let report = runtime.poll(&mut connector, &mut second, later)?;
    let SensedRecord::Tick(tick) = report.admitted[0].record() else {
        panic!("not a tick");
    };
    assert_eq!(
        tick.sequence,
        Some(3),
        "the sequence continues across polls"
    );
    assert_eq!(tick.capture_time, later);
    Ok(())
}

#[test]
fn two_sources_disagreeing_about_one_print_through_a_shared_fabric_are_kept_as_a_contest()
-> Result<()> {
    let fabric = Arc::new(Mutex::new(EvidenceFabric::default()));
    let other = SourceManifest::from_json(&manifest_json("other-source", 0, "1.0"))?;
    let mut a = runtime(manifest(), Some(Arc::clone(&fabric)))?;
    let mut b = runtime(other.clone(), Some(Arc::clone(&fabric)))?;
    let mut ca = TestConnector::new(manifest());
    let mut cb = TestConnector::new(other);

    let first = a.poll(
        &mut ca,
        &mut serving(&body(&[("a1", "2026-08-24T14:59:00Z", "101.25")])),
        now(),
    )?;
    assert_eq!(
        first.knowledge_delta.new_keys.len(),
        1,
        "premise: the print is a new key"
    );
    let second = b.poll(
        &mut cb,
        &mut serving(&body(&[("b1", "2026-08-24T14:59:00Z", "999.00")])),
        now(),
    )?;
    assert_eq!(second.evidence.len(), 1);
    assert_eq!(
        second.evidence[0].verdict.contradicts.len(),
        1,
        "{:?}",
        second.evidence[0].verdict
    );
    assert_eq!(second.knowledge_delta.contested.len(), 1);
    let held = fabric
        .lock()
        .map(|f| f.contests().values().filter(|c| c.is_contested()).count())
        .unwrap_or(0);
    assert_eq!(held, 1, "both values are kept, neither deleted");
    Ok(())
}

#[test]
fn a_re_fetch_after_an_unwind_reuses_its_verdict_rather_than_corroborating_itself() -> Result<()> {
    let text = body(&[("e1", "2026-08-24T14:59:00Z", "101.25")]);
    let mut runtime = runtime(manifest(), None)?;
    let mut connector = TestConnector::new(manifest());

    let before = runtime.snapshot();
    let first = runtime.poll(&mut connector, &mut serving(&text), now())?;
    assert_eq!(
        first.evidence.len(),
        1,
        "premise: the first poll checked one claim"
    );
    runtime.unwind(before);

    let again = runtime.poll(&mut connector, &mut serving(&text), now())?;
    assert_eq!(
        again.admitted.len(),
        1,
        "the unwound record is delivered again"
    );
    assert_eq!(
        again.quarantined, 0,
        "the re-fetch was not refused by the fabric"
    );
    assert_eq!(again.evidence[0].verdict, first.evidence[0].verdict);
    assert!(again.evidence[0].verdict.corroborates.is_empty());
    Ok(())
}

#[test]
fn a_poll_that_changes_knowledge_carries_it_as_a_measured_pack_holding_keys_not_text() -> Result<()>
{
    let text = body(&[
        ("e1", "2026-08-24T14:59:00Z", "101.25"),
        ("e2", "2026-08-24T14:59:30Z", "101.50"),
    ]);
    let mut runtime = runtime(manifest(), None)?;
    let mut connector = TestConnector::new(manifest());
    let report = runtime.poll(&mut connector, &mut serving(&text), now())?;
    assert_eq!(
        report.knowledge_delta.new_keys.len(),
        2,
        "premise: two new keys"
    );

    let Some(pack) = report.knowledge_pack.clone() else {
        panic!("a delta that changed knowledge produced no pack");
    };
    let decoded = qip_contracts::knowledge_pack::KnowledgePack::decode(&pack.encode()?)?;
    assert_eq!(decoded, pack, "the pack round-trips through its own schema");
    let qip_contracts::knowledge_pack::PackBody::WorldModelDelta {
        from_version,
        to_version,
        added,
        ..
    } = &pack.body
    else {
        panic!("not a world-model delta: {:?}", pack.body);
    };
    assert_eq!((*from_version, *to_version), (0, 2));
    assert_eq!(added.len(), 2);
    assert!(added.iter().all(|key| key.starts_with("asserted:")));
    let encoded = String::from_utf8(pack.encode()?).unwrap_or_default();
    assert!(
        !encoded.contains("reports"),
        "a claim's statement text leaked into the pack: {encoded}"
    );
    assert_eq!(
        report.knowledge_footprint.map(|(_, body)| body),
        Some(text.len() as u64)
    );

    // A re-served page changes nothing and packs nothing.
    let again = runtime.poll(&mut connector, &mut serving(&text), now())?;
    assert!(again.knowledge_pack.is_none());
    Ok(())
}

#[test]
fn a_digest_carries_the_sources_etag_its_parser_and_the_cursor_the_poll_left() -> Result<()> {
    let text = body(&[("e1", "2026-08-24T14:59:00Z", "101.25")]);
    let mut emulator = SourceEmulator::new(vec![
        RecordedExchange::always("/v1/health", RecordedAnswer::json(200, "{}")),
        RecordedExchange::always(
            "/v1/events",
            RecordedAnswer::json(200, text.as_str()).with_etag("\"rev-7\""),
        ),
    ]);
    let mut runtime = runtime(manifest(), None)?;
    let mut connector = TestConnector::new(manifest());
    let report = runtime.poll(&mut connector, &mut emulator, now())?;
    let Some(digest) = report.digest else {
        panic!("premise: the poll took a digest");
    };

    assert_eq!(digest.etag(), Some("\"rev-7\""));
    assert_eq!(
        digest.parser(),
        &qip_market_ingestion::connector::evidence::ParserLineage::of(&manifest())
    );
    let expected = format!("event_time_ns:{}", at("2026-08-24T14:59:00Z").as_nanos());
    assert_eq!(digest.cursor(), Some(expected.as_str()));
    Ok(())
}
