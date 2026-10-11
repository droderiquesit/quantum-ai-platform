//! A connector's checked evidence reaches the world model's fabric, and the
//! poisoning detector reads what arrived (R12-EVID-2031, 2035, 2038, 2039).
//!
//! The failure each test prevents is the one the audit found shipped: the
//! world fabric was never fed, so `detect_poisoning` scanned an empty fabric
//! in every binary and reported a clean feed. Each test asserts first that
//! the fabric was empty before the forward, so a test that passes because
//! the fabric was seeded some other way is not possible.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_core::{Context, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::evidence_intake::{
    EVIDENCE_ABSORBED, EVIDENCE_POISONING_FINDINGS, PoisoningClass, PoisoningThresholds,
};
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::connector::evidence::CheckedEvidence;
use qip_observability::Telemetry;
use qip_observability::metrics::labels;
use qip_risk::limits::LimitSet;
use qip_world_model::evidence::{META_ATTRIBUTE, META_SUBJECT, META_VALUE};
use qip_world_model::{EvidenceFabric, EvidenceRecord};

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_800_000_000 + secs)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(at(0), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap()
}

/// One claim as a connector runtime would hand it over: the record, the body
/// hash, and the verdict its *private* fabric reached — a fresh fabric per
/// call, which is exactly the isolation that hid every cross-source fact.
fn checked(id: &str, source: &str, statement: &str, value: &str, hash: &str) -> CheckedEvidence {
    let record = EvidenceRecord::new(
        id.to_string(),
        source.to_string(),
        at(10),
        at(15),
        "connector:test/1.0".to_string(),
        "commercial".to_string(),
        statement.to_string(),
        0.9,
    )
    .unwrap()
    .with_claimed_at(at(0))
    .with_metadata(META_SUBJECT.into(), "EURUSD".into())
    .with_metadata(META_ATTRIBUTE.into(), "rate".into())
    .with_metadata(META_VALUE.into(), value.into());
    let (verdict, _) = EvidenceFabric::default()
        .admit(record.clone(), Some(hash), at(20))
        .unwrap();
    CheckedEvidence {
        record,
        document_hash: hash.to_string(),
        verdict,
    }
}

fn hash(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

/// Two sources disagreeing about one value are each uncontested in their own
/// runtime's fabric; forwarded into the world fabric, the disagreement is a
/// contest. Mutation: make `absorb_evidence` skip `world.absorb_evidence`
/// (count and continue) — `evidence_held` stays 0 and `contested` 0, and the
/// assertions below fail.
#[test]
fn two_sources_disagreeing_are_a_contest_only_once_forwarded_into_the_world_fabric() {
    let mut platform = platform();
    assert_eq!(
        platform.evidence_held(),
        0,
        "premise: an empty world fabric"
    );
    let a = checked("a1", "https://a.example", "rate 1.10", "1.10", &hash('a'));
    let b = checked("b1", "https://b.example", "rate 1.20", "1.20", &hash('b'));
    assert!(
        a.verdict.contradicts.is_empty() && b.verdict.contradicts.is_empty(),
        "premise: each source's private fabric saw no contest"
    );

    let intake = platform.absorb_evidence(vec![a, b], at(30));

    assert_eq!(platform.evidence_held(), 2, "{intake:?}");
    assert_eq!(intake.admitted, 2, "{intake:?}");
    assert_eq!(
        intake.contested, 1,
        "the second claim contradicts the first across sources: {intake:?}"
    );
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(EVIDENCE_ABSORBED, &labels([("outcome", "contested")])),
        1
    );
}

/// A claim forwarded twice is held once and counted as a duplicate rather
/// than corroborating itself. Mutation: delete the `get(..).is_some()` guard —
/// the fabric's own duplicate refusal then lands on `refused`, and the
/// `duplicate == 1` assertion fails.
#[test]
fn a_claim_forwarded_twice_is_held_once_and_counted_as_a_duplicate() {
    let mut platform = platform();
    let first = checked("d1", "https://a.example", "rate 1.10", "1.10", &hash('a'));
    let again = first.clone();
    assert_eq!(platform.absorb_evidence(vec![first], at(30)).admitted, 1);

    let intake = platform.absorb_evidence(vec![again], at(31));

    assert_eq!(intake.duplicate, 1, "{intake:?}");
    assert_eq!(intake.refused, 0, "{intake:?}");
    assert_eq!(platform.evidence_held(), 1);
}

/// Three distinct fetches carrying one statement from three sources is a
/// campaign the detector names — once the claims are in the fabric it reads.
/// The premise half is the audit's finding: before the forward the same
/// detector over the same platform finds nothing. Mutation: return an empty
/// `Vec` from `evidence_poisoning` — the class assertion fails.
#[test]
fn a_coordinated_campaign_forwarded_from_connectors_is_found_by_the_poisoning_detector() {
    let mut platform = platform();
    let thresholds = PoisoningThresholds::default();
    assert!(
        platform.evidence_poisoning(at(40), &thresholds).is_empty(),
        "premise: nothing to find in an empty fabric"
    );
    let campaign = ['a', 'b', 'c']
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            checked(
                &format!("k{i}"),
                &format!("https://site{i}.example"),
                "Acme CEO arrested",
                "x",
                &hash(c),
            )
        })
        .collect();

    platform.absorb_evidence(campaign, at(30));
    let findings = platform.evidence_poisoning(at(40), &thresholds);

    assert_eq!(platform.evidence_held(), 3);
    assert!(
        findings
            .iter()
            .any(|f| f.class == PoisoningClass::CoordinatedManipulation && f.sources.len() == 3),
        "{findings:?}"
    );
    let snapshot = platform.telemetry().metrics.snapshot();
    assert!(
        snapshot.counter(
            EVIDENCE_POISONING_FINDINGS,
            &labels([("class", "coordinated_manipulation")])
        ) >= 1
    );
}

// ---------------------------------------------------------------------------
// The production path, end to end: a real `ConnectorRuntime` polling a
// recorded exchange, its report forwarded with `absorb_evidence`, and the
// world fabric's verdicts and detector read back (R12-EVID-2034, 2036, 2038,
// 2039). The hand-built `checked` above never touches the connector's
// evidence builder, which is where the self-confirming hash lived.
// ---------------------------------------------------------------------------

mod production {
    use super::*;
    use qip_core::Decimal;
    use qip_core::error::{Error, Result};
    use qip_financial::quality::DataQuality;
    use qip_market::quote::Tick;
    use qip_market_ingestion::adapter::SensedRecord;
    use qip_market_ingestion::connector::emulator::{
        RecordedAnswer, RecordedExchange, SourceEmulator,
    };
    use qip_market_ingestion::connector::runtime::{ConnectorRuntime, PollReport};
    use qip_market_ingestion::connector::{
        Cursor, RawEvent, RuntimeConfig, SourceConnector, SourceManifest,
    };
    use qip_world_model::TemporalConsistency;
    use serde_json::Value;

    pub(super) fn now() -> Timestamp {
        Timestamp::parse_rfc3339("2026-08-24T15:00:00Z").unwrap()
    }

    pub(super) fn manifest(source_id: &str) -> SourceManifest {
        SourceManifest::from_json(&format!(
            r#"{{
  "source_id": "{source_id}",
  "provider": "a source that exists to be tested",
  "asset_class": "crypto",
  "region": "global",
  "protocol": "rest",
  "schema": {{
    "version": "1.0",
    "required_fields": [ {{ "path": "events", "kind": "array" }} ],
    "unknown_fields": "ignore"
  }},
  "auth": {{ "scheme": "none" }},
  "endpoint": {{
    "base_url": "http://source.test:8080",
    "path": "/v1/events",
    "health_path": "/v1/health"
  }},
  "rate_limit": {{ "requests": 10, "per_ms": 1000, "burst": 10 }},
  "retry": {{
    "max_attempts": 3,
    "initial_backoff_ms": 100,
    "max_backoff_ms": 2000,
    "multiplier": 4,
    "jitter_basis_points": 2500
  }},
  "poll_interval_ms": 1000,
  "freshness_sla_ms": 60000,
  "publication_delay_ms": 0,
  "licensing": "public",
  "max_events_per_batch": 16
}}"#
        ))
        .unwrap()
    }

    /// `{"events":[{"key","at","price","seq"?}]}` as ticks on venue `TEST`,
    /// carrying the feed's own sequence where the body numbers it.
    #[derive(Debug)]
    pub(super) struct Feed(pub SourceManifest);

    impl SourceConnector for Feed {
        fn manifest(&self) -> &SourceManifest {
            &self.0
        }

        fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
            let items = payload
                .get("events")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::schema("no `events`"))?;
            items
                .iter()
                .map(|item| {
                    let key = item.get("key").and_then(Value::as_str);
                    let at = item
                        .get("at")
                        .and_then(Value::as_str)
                        .and_then(Timestamp::parse_rfc3339);
                    match (key, at) {
                        (Some(key), Some(at)) => Ok(RawEvent::new(key, at, item.clone())),
                        _ => Err(Error::schema("an event without `key` or `at`")),
                    }
                })
                .collect()
        }

        fn map(&self, event: &RawEvent, _ingest: Timestamp) -> Result<SensedRecord> {
            let price = event
                .body
                .get("price")
                .and_then(Value::as_str)
                .and_then(Decimal::parse)
                .ok_or_else(|| Error::schema("no price"))?;
            Ok(SensedRecord::Tick(Tick {
                object_id: qip_core::ObjectId::from_string("OBJ00000000000000000TEST"),
                venue: "TEST".to_string(),
                at: event.event_time,
                capture_time: event.event_time,
                sequence: event.body.get("seq").and_then(Value::as_u64),
                price,
                volume: Decimal::ONE,
                quality: DataQuality::clean(),
            }))
        }
    }

    pub(super) fn events(rows: &[(&str, &str, &str, Option<u64>)]) -> String {
        let items: Vec<String> = rows
            .iter()
            .map(|(key, at, price, seq)| match seq {
                Some(seq) => {
                    format!(r#"{{"key":"{key}","at":"{at}","price":"{price}","seq":{seq}}}"#)
                }
                None => format!(r#"{{"key":"{key}","at":"{at}","price":"{price}"}}"#),
            })
            .collect();
        format!(r#"{{"events":[{}]}}"#, items.join(","))
    }

    /// One poll of `source` serving `answer`.
    pub(super) fn poll(source: &str, answer: RecordedAnswer) -> PollReport {
        let mut emulator = SourceEmulator::new(vec![
            RecordedExchange::always("/v1/health", RecordedAnswer::json(200, "{}")),
            RecordedExchange::always("/v1/events", answer),
        ]);
        let mut runtime =
            ConnectorRuntime::new(manifest(source), RuntimeConfig::seeded(7)).unwrap();
        runtime
            .poll(&mut Feed(manifest(source)), &mut emulator, now())
            .unwrap()
    }

    /// The defect R12-EVID-2034 named: the connector attached a hash of the
    /// fetched body and the fabric compared it with the hash of that same
    /// body, so every production claim was credited. A body arriving exactly
    /// as its source declared it must now earn nothing for that, and a body
    /// that is *not* what its source declared is quarantined and the source
    /// named as spoofed by the world detector.
    ///
    /// Mutations: (1) restore the self-`HashVerified` in `evidence_of` —
    /// `credited.is_empty()` fails on the honest poll; (2) make the runtime
    /// release spoofed evidence (drop the `spoofed` refusal) — `admitted` is
    /// 1 and the quarantine assertion fails; (3) stop forwarding
    /// `refused_evidence` in the feed / here — no `Spoofing` finding.
    #[test]
    fn a_body_that_is_not_what_its_source_declared_is_quarantined_and_found_spoofed() {
        let text = events(&[("e1", "2026-08-24T14:59:00Z", "101.25", None)]);
        let honest = poll(
            "honest-source",
            RecordedAnswer::json(200, text.clone())
                .with_content_digest(qip_core::sha256_hex(text.as_bytes())),
        );
        assert_eq!(
            honest.admitted.len(),
            1,
            "premise: an honest body is released"
        );
        let verdict = &honest.evidence[0].verdict;
        assert!(!verdict.authenticity.spoofed);
        assert!(
            verdict.authenticity.credited.is_empty(),
            "a body matching its own declaration earned authenticity: {:?}",
            verdict.authenticity
        );
        // The score is the constant itself when nothing was credited, so an
        // exact comparison is meant; it is written as a tolerance only because
        // float equality is linted (clippy::float_cmp).
        assert!(
            (verdict.authenticity.score - qip_world_model::fabric::UNSIGNED_AUTHENTICITY).abs()
                < 1e-12,
            "an uncredited body scored {} rather than the unsigned floor",
            verdict.authenticity.score
        );

        // The source declared one body and these bytes arrived.
        let declared = qip_core::sha256_hex(b"what the source actually sent");
        let altered = poll(
            "spoofed-source",
            RecordedAnswer::json(200, text).with_content_digest(declared),
        );
        assert!(altered.admitted.is_empty(), "an altered body was released");
        assert_eq!(altered.quarantined, 1);
        assert_eq!(altered.refused_evidence.len(), 1);
        assert!(altered.refused_evidence[0].verdict.authenticity.spoofed);

        let mut platform = platform();
        let thresholds = PoisoningThresholds::default();
        assert!(
            platform.evidence_poisoning(now(), &thresholds).is_empty(),
            "premise: nothing to find before the forward"
        );
        platform.absorb_evidence(
            honest
                .evidence
                .into_iter()
                .chain(altered.refused_evidence)
                .collect(),
            now(),
        );
        let findings = platform.evidence_poisoning(now(), &thresholds);
        assert!(
            findings.iter().any(
                |f| f.class == PoisoningClass::Spoofing && f.sources.contains("spoofed-source")
            ),
            "{findings:?}"
        );
        assert!(
            !findings.iter().any(|f| f.sources.contains("honest-source")),
            "{findings:?}"
        );
    }

    /// R12-EVID-2038 in production: two venues' runtimes disagree about one
    /// print and, forwarded into the world fabric, open a contest; the
    /// contest resolves only once two *other* sources, fetched separately,
    /// agree with one side — never on a claim's word about itself.
    ///
    /// Mutation: delete the corroboration credit in `EvidenceFabric::admit`
    /// — the fourth source scores the unsigned floor, nothing resolves, and
    /// `resolution.is_some()` fails.
    #[test]
    fn a_contest_between_connectors_resolves_on_genuinely_corroborated_evidence() {
        let print = |source: &str, key: &str, price: &str| {
            let text = events(&[(key, "2026-08-24T14:59:00Z", price, None)]);
            let report = poll(source, RecordedAnswer::json(200, text));
            assert_eq!(report.evidence.len(), 1, "premise: {source} released one");
            report.evidence
        };
        let mut platform = platform();
        let mut batch = print("venue-a", "a1", "101.25");
        batch.extend(print("venue-b", "b1", "999.00"));
        let intake = platform.absorb_evidence(batch, now());
        assert_eq!(intake.contested, 1, "premise: a contest: {intake:?}");

        platform.absorb_evidence(print("venue-c", "c1", "101.25"), now());
        let unresolved = platform
            .world()
            .evidence()
            .contests()
            .values()
            .all(|c| c.resolution.is_none());
        assert!(unresolved, "one corroborating source resolved the contest");

        platform.absorb_evidence(print("venue-d", "d1", "101.25"), now());
        let world = platform.world();
        let fabric = world.evidence();
        let contest = fabric.contests().values().next().unwrap();
        let resolution = contest.resolution.as_ref().expect("resolved");
        assert_eq!(resolution.value, "101.25");
        assert_eq!(contest.hypotheses.len(), 2, "the losing side is kept");
        assert_eq!(fabric.source_history("venue-b").unwrap().contradicted, 1);
    }

    /// R12-EVID-2036: production evidence declared no cause, so the
    /// event-ordering check had nothing to compare. A feed that numbers its
    /// messages declares an order; a message numbered after another and
    /// dated before it now fails the check, in the connector's fabric and
    /// again in the world's.
    ///
    /// Mutation: pass `None` for `cause` in `check_evidence` — the second
    /// claim reads `Consistent` and the assertion fails.
    #[test]
    fn a_feed_message_dated_before_its_numbered_predecessor_violates_event_ordering() {
        let text = events(&[
            ("s10", "2026-08-24T14:59:30Z", "101.25", Some(10)),
            ("s11", "2026-08-24T14:59:00Z", "101.50", Some(11)),
        ]);
        let report = poll("numbered-feed", RecordedAnswer::json(200, text));
        assert_eq!(report.evidence.len(), 2, "premise: both released");
        let first = report.evidence[0].clone();
        let second = report.evidence[1].clone();
        assert!(
            first.record.caused_by.is_empty(),
            "premise: s10 has no predecessor"
        );
        assert_eq!(second.record.caused_by, vec![first.record.id.clone()]);
        assert!(
            matches!(
                &second.verdict.temporal,
                TemporalConsistency::ViolatesEventOrdering { cause_id, .. }
                    if *cause_id == first.record.id
            ),
            "{:?}",
            second.verdict.temporal
        );

        let mut platform = platform();
        platform.absorb_evidence(report.evidence, now());
        let held = platform
            .world()
            .evidence()
            .get(&second.record.id)
            .map(|h| h.verdict.temporal.clone());
        assert!(
            matches!(
                held,
                Some(TemporalConsistency::ViolatesEventOrdering { .. })
            ),
            "{held:?}"
        );

        // In order, the same numbering is consistent: the check judges the
        // ordering, not the presence of a cause.
        let ordered = poll(
            "numbered-feed",
            RecordedAnswer::json(
                200,
                events(&[
                    ("s10", "2026-08-24T14:59:00Z", "101.25", Some(10)),
                    ("s11", "2026-08-24T14:59:30Z", "101.50", Some(11)),
                ]),
            ),
        );
        assert_eq!(ordered.evidence[1].record.caused_by.len(), 1);
        assert_eq!(
            ordered.evidence[1].verdict.temporal,
            TemporalConsistency::Consistent
        );
    }
}
