//! The Data Truth Loop: source event to state, in seven stages.
//!
//! The architecture's first loop is the one everything else stands on. A fact
//! enters at a venue, is captured with both of its times, is checked for
//! quality and for the right to use it, is put onto canonical identity,
//! is connected to what else the platform knows, is preserved so the answer
//! given today can be reproduced next year, and only then updates state and
//! the graph. Every later loop — discovery, reasoning, allocation, execution —
//! reads what this one wrote, so a defect here is not a wrong number in one
//! report but a wrong number everywhere at once.
//!
//! Each stage but one is owned by a crate with its own tests, and those tests
//! pass whether or not the stages are wired in this order. The exception is
//! stage 4: since ADR 0029 no crate canonicalises identity, and the rewrite
//! there is this file's own — see `CanonicalIdentity`. What is checked here is
//! the composition: that the record leaving one stage is the record the next
//! one accepts, that both timestamps are still attached at the end, and above
//! all that the two arrows out of the quality gate go where the architecture
//! says they go — pass to the world model, fail to quarantine.
//!
//! The properties are stated as refusals wherever possible. That a good record
//! arrives is visible in any happy-path test; that a bad one is stopped, that
//! a research-only licence cannot be spent on a trade, and that a reader as of
//! last March cannot see what arrived in April are only visible if something
//! deliberately tries.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_compliance::licensing::{EntitlementRegistry, LicensedData};
use qip_compliance::pit::{LeakageDetector, PointInTime};
use qip_contracts::governance::{Entitlement, Usage};
use qip_contracts::time::Stamped;
use qip_core::error::{Error, Result};
use qip_core::{Context, Decimal, Duration, EntityId, ObjectId, Timestamp};
use qip_entity_resolution::entity::{Entity, EntityKind, EntityRecord};
use qip_entity_resolution::resolver::{Decision, Resolver};
use qip_financial::identifiers::{IdentifierKind, Identifiers};
use qip_financial::quality::{
    DECISION_QUALITY_FLOOR, DataQuality, LicensingClass, Provenance as RecordProvenance,
};
use qip_market::bar::Bar;
use qip_market_ingestion::adapter::{SensedRecord, quality_failure};
use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};
use qip_mesh::catalog::{Catalog, DatasetRegistration, QualityState};
use qip_mesh::ports::{Edge, EvidenceReceipt, EvidenceStore, GraphStore, Lakehouse, TableVersion};
use qip_mesh::provider::MeshPort;
use qip_mesh::{MemoryEvidence, MemoryGraph, MemoryLakehouse};
use qip_world_model::WorldModel;
use qip_world_model::features::FeatureValue;
use qip_world_model::graph::{Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use serde::Serialize;
use std::collections::BTreeMap;

// --- the world the walk happens in ------------------------------------------

/// The adapter the record comes from, and the provenance source it is
/// attributed to for the rest of its life.
const FEED: &str = "synthetic-exchange";

/// The symbol and venue *as the provider writes them*. Neither is canonical,
/// which is what gives stage 4 something to do.
const PROVIDER_SYMBOL: &str = "NWSC.O";
const PROVIDER_VENUE: &str = "NYSE";

/// The same instrument on canonical identity: the MIC for the venue and the
/// bare ticker for the symbol.
const CANONICAL_SYMBOL: &str = "NWSC";
const CANONICAL_VENUE: &str = "XNYS";

/// What the provider's spelling of an instrument maps to.
///
/// **Test-owned, and deliberately so.** ADR 0029 deleted `qip-normalization`,
/// the central normaliser that used to carry stage 4 here, because nothing in
/// a deployed process ever constructed it and four documents were citing it as
/// a control that ran. Nothing in the workspace now canonicalises a venue
/// alias or a provider symbol, and this type does not pretend otherwise: it is
/// the smallest rewrite that hands stages 5 to 7 a record on canonical
/// identity, so that what those stages preserve — both timestamps, above all —
/// is still checked across a real composition. It proves nothing about the
/// platform's ability to normalise, because the platform has none.
#[derive(Debug, Clone, PartialEq)]
struct CanonicalIdentity {
    canonical_symbol: String,
    canonical_venue: String,
}

/// The test's own mapping from a provider's spelling to canonical identity.
///
/// Keyed on the provider *and* its symbol, because the same string means
/// different instruments at different providers, and a table keyed on the
/// symbol alone is the phantom the deleted crate's `drop_unmapped` was meant
/// to stop and never did. A provider symbol that is not in the table is
/// refused by the walk rather than passed through: an unmapped symbol is an
/// unknown instrument, and guessing produces a record on nobody's identity.
fn canonical_identities() -> BTreeMap<(String, String), CanonicalIdentity> {
    BTreeMap::from([(
        (FEED.to_string(), PROVIDER_SYMBOL.to_string()),
        CanonicalIdentity {
            canonical_symbol: CANONICAL_SYMBOL.to_string(),
            canonical_venue: CANONICAL_VENUE.to_string(),
        },
    )])
}

const DATASET: &str = "xnys.bars.minute";
const OBJECT: &str = "OBJ00000000000000000NWSC";
const ENTITY: &str = "ent-northwind";

/// Twenty alphanumerics, which is what `IdentifierKind::Lei` considers
/// well-formed. Resolution is meant to turn on the identifier rather than on
/// the name, so the identifier has to be a real one.
const LEI: &str = "NWSC00000000000000AB";

const EVIDENCE_KEY: &str = "bars/xnys/NWSC/minute/first";
const TABLE: &str = "market.bars.minute";

fn origin() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// How long after a bar closes the platform could first have acted on it.
///
/// Non-zero on purpose. A loop tested with zero latency is a loop where
/// valid-time and known-time are the same number, and every place that
/// confuses the two passes.
fn feed_latency() -> Duration {
    Duration::from_millis(250)
}

fn licence_expiry() -> Timestamp {
    origin().saturating_add(Duration::from_days(365))
}

/// The issuer, as the master data already knows it.
fn northwind() -> Entity {
    Entity::new(
        EntityId::from_string(ENTITY),
        EntityKind::Company,
        "Northwind Semiconductor Corporation",
        origin(),
    )
    .with_identifiers(Identifiers::new().with(IdentifierKind::Lei, LEI))
    .with_country("US")
}

/// One minute bar from the platform's own synthetic venue.
///
/// Taken from the seeded environment rather than assembled here, because
/// stage 1 is supposed to be a *source* event: a bar written by hand to suit
/// the stages downstream would only prove that those stages agree with the
/// fixture. The venue is then rewritten to the provider's spelling, because
/// the synthetic adapter already emits canonical MIC codes and a normalisation
/// stage handed canonical input is a stage that proves nothing.
fn source_event() -> Result<Bar> {
    let mut environment = SyntheticEnvironment::demo(
        origin(),
        EnvironmentConfig {
            seed: 20_260_822,
            ..EnvironmentConfig::default()
        },
    );
    let object = ObjectId::from_string(OBJECT);
    let mut bar = environment
        .run_until(origin().saturating_add(Duration::from_mins(10)))
        .into_iter()
        .find_map(|record| match record {
            SensedRecord::Bar(bar) if bar.object_id == object => Some(*bar),
            _ => None,
        })
        .ok_or_else(|| Error::not_found("the synthetic venue produced no bar in ten minutes"))?;
    bar.venue = PROVIDER_VENUE.to_string();
    Ok(bar)
}

// --- the walk ---------------------------------------------------------------

/// Everything one journey through the loop produced.
///
/// Kept whole rather than reduced to a verdict so the end-to-end test can
/// assert stage by stage while the determinism test compares two journeys —
/// two tests over one walk, rather than two walks that could drift apart.
#[derive(Debug)]
struct Journey {
    /// Stage 2: the record as captured, with both of its times.
    captured: Stamped<SensedRecord>,
    provenance: RecordProvenance,
    /// Stage 3.
    quality: DataQuality,
    catalog: Catalog,
    registry: EntitlementRegistry,
    /// Stage 4 — the test's own rewrite, not the platform's; see
    /// `CanonicalIdentity`.
    mapping: CanonicalIdentity,
    normalised: Stamped<SensedRecord>,
    /// Stage 5.
    decision: Decision,
    entity_id: EntityId,
    graph: MemoryGraph,
    /// Stage 6.
    receipt: EvidenceReceipt,
    version: TableVersion,
    evidence: MemoryEvidence,
    lakehouse: MemoryLakehouse,
    /// Stage 7.
    world: WorldModel,
    bar: Bar,
}

impl Journey {
    fn valid_at(&self) -> Timestamp {
        self.captured.valid_at()
    }

    fn known_at(&self) -> Timestamp {
        self.captured.known_at()
    }

    /// The journey reduced to values two runs can be compared on.
    fn outcome(&self) -> Result<Outcome> {
        let state = self.world.state_at(self.valid_at(), self.known_at());
        let neighbours = self
            .graph
            .neighbours(ENTITY, None, self.known_at())?
            .into_value()
            .into_iter()
            .map(|edge| format!("{}-{}->{}", edge.from, edge.kind, edge.to))
            .collect();
        Ok(Outcome {
            valid_at: self.valid_at().as_nanos(),
            known_at: self.known_at().as_nanos(),
            close: self.bar.close.to_string(),
            venue: self.bar.venue.clone(),
            entity: self.entity_id.as_str().to_string(),
            evidence_digest: self.receipt.digest.clone(),
            table_version: self.version.version,
            table_digest: self.version.digest.clone(),
            neighbours,
            relationships: state.relationship_count,
            features: state.features,
        })
    }
}

/// A journey, flattened. Every field is exact or an integer, so two of these
/// compare byte for byte rather than approximately.
#[derive(Debug, PartialEq, Serialize)]
struct Outcome {
    valid_at: i64,
    known_at: i64,
    close: String,
    venue: String,
    entity: String,
    evidence_digest: String,
    table_version: u64,
    table_digest: String,
    neighbours: Vec<String>,
    relationships: usize,
    features: BTreeMap<String, f64>,
}

/// One market event, walked through all seven stages.
fn walk() -> Result<Journey> {
    // --- 1. SOURCE EVENT ------------------------------------------------
    let raw = source_event()?;

    // --- 2. CAPTURE AND TIMESTAMP ---------------------------------------
    // Both times are established here and nowhere else. `valid_at` is when
    // the fact was true in the market — for a bar, when its bucket closed —
    // and `known_at` is when this platform could first have acted on it.
    // Setting them at capture is what makes every later stage able to carry
    // them rather than invent them.
    let valid_at = raw.close_time();
    let known_at = valid_at.saturating_add(feed_latency());
    let provenance = RecordProvenance::new(FEED, valid_at, known_at)
        .with_licensing(LicensingClass::Synthetic)
        .with_upstream_id(format!("{PROVIDER_SYMBOL}@{}", valid_at.as_nanos()));
    let captured = Stamped::new(SensedRecord::Bar(Box::new(raw)), valid_at, known_at);

    // --- 3. VERIFY QUALITY AND RIGHTS -----------------------------------
    // Two different questions, asked in this order. Quality is whether the
    // record is true; rights are whether the platform may act on it. A
    // correct record nobody licensed and a licensed record that is wrong are
    // both refusals, and conflating them produces a refusal nobody can fix.
    let issues = captured.value().validate();
    let quality = issues.iter().fold(DataQuality::clean(), |quality, issue| {
        quality.with_issue(issue)
    });

    let mut catalog = Catalog::new();
    catalog.register(
        DatasetRegistration::new(
            DATASET,
            "market-data-engineering",
            MeshPort::Lakehouse,
            origin(),
        )?
        .licensed(Entitlement::Granted {
            dataset: DATASET.to_string(),
            usage: Usage::Research,
            expires_at: licence_expiry(),
        })
        .licensed(Entitlement::Granted {
            dataset: DATASET.to_string(),
            usage: Usage::Derive,
            expires_at: licence_expiry(),
        })
        .with_quality(QualityState::Verified {
            at: known_at,
            checks: vec!["structural".to_string(), "continuity".to_string()],
        }),
    )?;
    catalog.usable_for(DATASET, Usage::Derive, known_at)?;

    let mut registry = EntitlementRegistry::new();
    registry.grant(DATASET, Usage::Research, licence_expiry(), known_at)?;
    registry.grant(DATASET, Usage::Derive, licence_expiry(), known_at)?;

    // Wrapping the record is what enforces the licence from here on: the
    // value is private, and the only ways to it record the usage they were
    // reached under.
    let licensed = LicensedData::from_dataset(DATASET, captured.clone());
    let admitted = licensed.into_inner(&mut registry, Usage::Derive, known_at)?;

    // --- 4. NORMALISE IDENTITY ------------------------------------------
    // This stage is the test's, not the platform's (ADR 0029): the mapping
    // below is owned by this file, and no production code runs here. What
    // is still the platform's is how the rewrite is carried. `Stamped::map`
    // rewrites the value and has no way to touch either timestamp, so
    // bitemporality survives this stage by construction rather than by
    // remembering to re-attach it — and that is what the later assertions
    // on `normalised` check.
    let mapping = canonical_identities()
        .remove(&(FEED.to_string(), PROVIDER_SYMBOL.to_string()))
        .ok_or_else(|| Error::not_found(format!("{PROVIDER_SYMBOL} has no canonical mapping")))?;
    let normalised = admitted.map(|record| match record {
        SensedRecord::Bar(mut bar) => {
            bar.venue = mapping.canonical_venue.clone();
            SensedRecord::Bar(bar)
        }
        other => other,
    });

    // --- 5. ENRICH AND CONNECT ------------------------------------------
    // The bar is a claim on an instrument, the instrument is issued by a
    // company, and the company arrives from four providers under four names.
    // Resolving on the identifier rather than the name is what stops one
    // company's news being attributed to another's securities.
    let (context, _clock) = Context::deterministic(known_at, 20_260_822);
    let mut resolver = Resolver::default();
    resolver.insert(northwind());
    let observed = EntityRecord::new(
        format!("{FEED}:{PROVIDER_SYMBOL}"),
        EntityKind::Company,
        "Northwind Semi",
        FEED,
        valid_at,
    )
    .with_identifiers(Identifiers::new().with(IdentifierKind::Lei, LEI))
    .with_country("US");
    let (decision, _event) = resolver.resolve(&observed, &context);
    let entity_id = decision
        .entity_id()
        .cloned()
        .ok_or_else(|| Error::not_found("the observation resolved to no entity"))?;

    let graph = MemoryGraph::new();
    graph.add_edge(Stamped::new(
        Edge::new(entity_id.as_str(), OBJECT, "issues"),
        valid_at,
        known_at,
    ))?;

    // --- 6. PRESERVE POINT-IN-TIME TRUTH --------------------------------
    // Evidence first, because it is the copy that cannot be rebuilt, then
    // the lakehouse version that everything else is derived from. Both take
    // the known-time, which is what makes a read as of an earlier moment
    // return nothing rather than today's answer.
    let evidence = MemoryEvidence::new();
    let receipt = evidence.put(
        EVIDENCE_KEY,
        serde_json::to_vec(normalised.value())?,
        known_at,
    )?;

    let bar = match normalised.value() {
        SensedRecord::Bar(bar) => bar.as_ref().clone(),
        other => {
            return Err(Error::invalid(format!(
                "the walk lost its bar somewhere: {}",
                other.subject()
            )));
        }
    };
    let lakehouse = MemoryLakehouse::new();
    let version = lakehouse.append(
        TABLE,
        vec![Stamped::new(
            serde_json::json!({
                "object_id": OBJECT,
                "venue": bar.venue,
                "entity_id": entity_id.as_str(),
                "close": bar.close.to_string(),
                "volume": bar.volume.to_string(),
                "source": provenance.source,
            }),
            valid_at,
            known_at,
        )],
        known_at,
    )?;

    // --- 7. UPDATE STATE AND GRAPH --------------------------------------
    let mut world = WorldModel::new();
    world.add_entity(northwind());
    world.graph_mut().add_node(Node::new(
        OBJECT,
        NodeKind::FinancialObject,
        &mapping.canonical_symbol,
        known_at,
    ));
    world.relate(
        Relationship::new(
            entity_id.as_str(),
            OBJECT,
            RelationshipKind::Issues,
            1.0,
            FEED,
        ),
        valid_at,
        known_at,
        0.99,
    );
    // The known-time stage 2 established, not the bar's close: this bar came
    // over a wire and the platform learned it a quarter of a second later.
    world.absorb_bar(&bar, known_at);
    for (feature, value) in [
        ("close", bar.close.to_f64()),
        ("volume", bar.volume.to_f64()),
    ] {
        world.features_mut().record(
            feature,
            OBJECT,
            FeatureValue::new(value, valid_at, known_at),
        );
    }

    Ok(Journey {
        captured,
        provenance,
        quality,
        catalog,
        registry,
        mapping,
        normalised,
        decision,
        entity_id,
        graph,
        receipt,
        version,
        evidence,
        lakehouse,
        world,
        bar,
    })
}

// --- the end-to-end walk ----------------------------------------------------

#[test]
fn one_market_event_travels_all_seven_stages_of_the_truth_loop() -> Result<()> {
    let journey = walk()?;
    let valid_at = journey.valid_at();
    let known_at = journey.known_at();

    // 1. A source event, from a venue rather than from a fixture.
    assert!(journey.bar.is_coherent(), "the venue produced a broken bar");
    assert!(journey.bar.close.is_positive());

    // 2. Captured with both times, and they are different times.
    assert_eq!(journey.captured.valid_at(), valid_at);
    assert_eq!(journey.captured.known_at(), known_at);
    assert_eq!(journey.captured.latency(), feed_latency());
    assert_eq!(journey.provenance.ingestion_latency(), feed_latency());
    assert!(
        !journey.provenance.licensing.allows_production_decisions(),
        "a synthetic feed must never read as fit to drive real capital"
    );

    // 3. Quality and rights both answered, and both recorded.
    assert_eq!(journey.quality.validation_failures, 0);
    assert!(journey.quality.meets(DECISION_QUALITY_FLOOR));
    assert!(
        journey
            .catalog
            .usable_for(DATASET, Usage::Derive, known_at)
            .is_ok()
    );
    assert_eq!(
        journey.registry.refusals().len(),
        0,
        "a clean walk should not have been refused anything"
    );
    assert_eq!(
        journey.registry.checks().len(),
        1,
        "reaching the value is what records the check, so there is exactly one"
    );

    // 4. Identity is canonical. The assertions on the normaliser's own
    // report — records processed, venues canonicalised, timestamps
    // corrected, scale warnings — went with the crate they tested (ADR
    // 0029); they were claims about `qip-normalization`'s counters, not
    // about the loop. What remains is the loop's claim: the record the
    // later stages received, and the bar that reached the world model, are
    // on canonical identity and not on the provider's spelling.
    assert_ne!(PROVIDER_VENUE, CANONICAL_VENUE, "stage 4 had nothing to do");
    assert_eq!(journey.bar.venue, CANONICAL_VENUE);
    assert_eq!(journey.mapping.canonical_symbol, CANONICAL_SYMBOL);
    match journey.normalised.value() {
        SensedRecord::Bar(bar) => assert_eq!(bar.venue, CANONICAL_VENUE),
        other => panic!("stage 4 handed on something other than a bar: {other:?}"),
    }

    // 5. Connected to what the platform already knew, on evidence.
    match &journey.decision {
        Decision::Linked {
            entity_id,
            score,
            evidence,
        } => {
            assert_eq!(entity_id.as_str(), ENTITY);
            assert!(*score >= 0.9, "linked on weak evidence at {score}");
            assert!(
                evidence.iter().any(|item| item.signal == "identifier"),
                "the link was made on something other than the identifier: {evidence:?}"
            );
        }
        other => panic!("the observation did not reach its issuer: {other:?}"),
    }
    assert_eq!(
        journey
            .graph
            .neighbours(ENTITY, Some("issues"), known_at)?
            .value()
            .len(),
        1,
        "the issuer is not connected to the instrument"
    );

    // 6. Preserved, with a receipt that names what was written when.
    assert_eq!(journey.receipt.key, EVIDENCE_KEY);
    assert_eq!(journey.receipt.written_at, known_at);
    assert!(journey.receipt.size_bytes > 0);
    assert_eq!(journey.version.version, 1, "version 0 must not exist");
    assert_eq!(journey.version.rows, 1);
    assert_eq!(journey.version.committed_at, known_at);

    // 7. State and graph updated, and the state reads back bitemporally.
    let state = journey.world.state_at(valid_at, known_at);
    assert_eq!(state.entity_count, 1);
    assert_eq!(state.object_count, 1);
    assert_eq!(
        state.relationship_count, 1,
        "the issuer edge is not in force"
    );
    assert!(
        state.features.contains_key(&format!("close/{OBJECT}")),
        "the price never became a feature: {:?}",
        state.features.keys().collect::<Vec<_>>()
    );

    // And the whole thing is still one fact with two times at the end, which
    // is the property every stage above exists to preserve.
    assert_eq!(journey.normalised.valid_at(), valid_at);
    assert_eq!(journey.normalised.known_at(), known_at);
    Ok(())
}

// --- the loop's claims ------------------------------------------------------

#[test]
fn both_timestamps_survive_every_stage_of_the_loop() -> Result<()> {
    // The single property the loop is for. A stage that dropped known-time
    // would leave a fact that reads as having been available the instant it
    // was true, and every backtest downstream would quietly read the future.
    // Each stage is checked at its own output rather than only at the end,
    // because a pair that is restored at the end tells you nothing about
    // where it was lost.
    let journey = walk()?;
    let valid_at = journey.valid_at();
    let known_at = journey.known_at();
    assert!(valid_at < known_at, "the fixture has no latency to lose");

    // Capture and normalisation.
    for stamped in [&journey.captured, &journey.normalised] {
        assert_eq!(stamped.valid_at(), valid_at);
        assert_eq!(stamped.known_at(), known_at);
        assert!(stamped.was_known_by(known_at));
        assert!(!stamped.was_known_by(valid_at));
    }

    // Provenance carries the same pair under different names, because the
    // record's own metadata has to agree with its envelope.
    assert_eq!(journey.provenance.event_time, valid_at);
    assert_eq!(journey.provenance.ingestion_time, known_at);

    // The mesh keeps it: a read as of the valid time sees nothing, and the
    // same read a quarter of a second later sees everything.
    assert!(journey.lakehouse.snapshot(TABLE, valid_at).is_err());
    assert_eq!(
        journey.lakehouse.snapshot(TABLE, known_at)?.value().len(),
        1
    );
    assert!(
        journey
            .evidence
            .get(EVIDENCE_KEY, valid_at)?
            .value()
            .is_none()
    );
    assert!(
        journey
            .evidence
            .get(EVIDENCE_KEY, known_at)?
            .value()
            .is_some()
    );
    assert!(
        journey
            .graph
            .neighbours(ENTITY, None, valid_at)?
            .value()
            .is_empty()
    );

    // And the world model, which is the one that a strategy reads.
    let feature = journey
        .world
        .features()
        .value_as_of("close", OBJECT, valid_at, known_at)
        .ok_or_else(|| Error::not_found("the close feature is not readable at its known-time"))?;
    assert_eq!(feature.valid_at, valid_at);
    assert_eq!(feature.available_at, known_at);
    assert_eq!(feature.availability_lag(), feed_latency());
    Ok(())
}

/// The gate stage 3 puts in front of stage 7.
///
/// Composed here rather than taken from a crate because no single crate owns
/// the composition: ingestion validates structure, the catalogue decides
/// quality and rights, and the world model absorbs. The loop's claim is that
/// they run in that order and that the first two can stop the third, which is
/// the thing under test.
fn admit_to_world_model(
    world: &mut WorldModel,
    catalog: &Catalog,
    record: &SensedRecord,
    at: Timestamp,
) -> Result<()> {
    let issues = record.validate();
    if !issues.is_empty() {
        return Err(Error::guard(format!(
            "{} failed its quality gate: {}",
            record.subject(),
            issues.join("; ")
        )));
    }
    catalog.usable_for(DATASET, Usage::Derive, at)?;
    if let SensedRecord::Bar(bar) = record {
        world.absorb_bar(bar, at);
    }
    Ok(())
}

#[test]
fn a_record_that_fails_its_quality_gate_is_refused_by_name_and_never_reaches_the_world_model()
-> Result<()> {
    // The FAIL arrow out of the gate. Two things have to be true for it to be
    // worth anything: the refusal has to say what is wrong in terms somebody
    // can act on, and the record must genuinely not arrive downstream — a
    // refusal that logs and then absorbs anyway is the worst of both, because
    // the alert makes it look handled.
    let journey = walk()?;
    let known_at = journey.known_at();

    // A bar whose high is below its low. Not a value judgement — no market
    // produces one, so it is a defect in the feed or the parser.
    let mut broken = journey.bar.clone();
    broken.high = broken.low - Decimal::from_int(1);
    let record = SensedRecord::Bar(Box::new(broken));

    let issues = record.validate();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        issues[0].contains("incoherent bar"),
        "the refusal does not name what is wrong: {issues:?}"
    );

    // One structural failure is already enough to put the record under the
    // floor a decision needs, which is the arithmetic that makes the gate a
    // gate rather than a warning.
    let quality = DataQuality::clean().with_issue(&issues[0]);
    assert!(
        !quality.meets(DECISION_QUALITY_FLOOR),
        "a bar with a broken high still scores {}",
        quality.score()
    );

    // The failure is published rather than dropped, and it names the subject,
    // the source and the topic the record would have reached.
    let failure = quality_failure(&record, FEED, issues.clone(), known_at);
    assert!(failure.rejected);
    assert_eq!(failure.source, FEED);
    assert_eq!(failure.subject_id.as_deref(), Some(OBJECT));
    assert_eq!(failure.issues, issues);

    // And it does not reach the world model.
    let mut world = WorldModel::new();
    let before = world.features().value_count();
    let error = admit_to_world_model(&mut world, &journey.catalog, &record, known_at)
        .expect_err("a broken bar was absorbed");
    assert!(error.message().contains("incoherent bar"), "{error}");
    assert_eq!(
        world.features().value_count(),
        before,
        "the world model absorbed a record that failed its gate"
    );

    // The other half of the arrow: once a dataset is quarantined, the gate
    // refuses even a well-formed record, and the refusal names the reason so
    // an operator knows which feed to go and fix.
    let mut catalog = Catalog::new();
    catalog.register(
        DatasetRegistration::new(
            DATASET,
            "market-data-engineering",
            MeshPort::Lakehouse,
            origin(),
        )?
        .licensed(Entitlement::Granted {
            dataset: DATASET.to_string(),
            usage: Usage::Derive,
            expires_at: licence_expiry(),
        }),
    )?;
    catalog.register(
        DatasetRegistration::new(
            "factors.momentum",
            "research",
            MeshPort::Analytical,
            origin(),
        )?
        .produced_from(vec![DATASET.to_string()]),
    )?;
    catalog.quarantine(DATASET, "the venue replayed a corrupt session", known_at)?;

    let refused = admit_to_world_model(&mut world, &catalog, journey.captured.value(), known_at)
        .expect_err("a quarantined dataset was read");
    assert!(refused.message().contains("quarantined"), "{refused}");
    assert!(refused.message().contains("corrupt session"), "{refused}");
    assert_eq!(
        world.features().value_count(),
        before,
        "a quarantined dataset still reached the world model"
    );

    // "Block affected trading" is a question about the graph, not the record:
    // everything computed from the bad feed is now suspect too.
    assert_eq!(
        catalog.impacted_by(DATASET),
        vec!["factors.momentum".to_string()],
        "the quarantine did not propagate to what was derived from it"
    );
    Ok(())
}

#[test]
fn a_dataset_licensed_for_research_cannot_be_spent_on_a_trade() -> Result<()> {
    // The most common licence in the building and the most common breach: a
    // feed that may be researched and derived from but never used to base a
    // live order on, promoted by someone who read "we have the data" as "we
    // may use the data". Both the catalogue and the enforcing registry are
    // checked, because they are two crates holding one `Entitlement` type and
    // the whole point of sharing the type is that they cannot disagree.
    const RESEARCH_ONLY: &str = "vendor.sentiment.v3";
    let now = origin();

    let mut registry = EntitlementRegistry::new();
    registry.grant(RESEARCH_ONLY, Usage::Research, licence_expiry(), now)?;

    let data = LicensedData::from_dataset(RESEARCH_ONLY, 0.42_f64);
    assert!(data.is_available_for(&registry, Usage::Research, now));
    assert!(!data.is_available_for(&registry, Usage::Trade, now));
    data.open(&mut registry, Usage::Research, now)?;

    let refusal = data
        .open(&mut registry, Usage::Trade, now)
        .expect_err("a research-only feed was opened for trading");
    assert!(refusal.message().contains(RESEARCH_ONLY), "{refusal}");
    assert!(refusal.message().contains("trade"), "{refusal}");
    assert!(
        refusal.message().contains("not as permission"),
        "an unrecorded licence must not read as a granted one: {refusal}"
    );

    // An explicit denial is different from silence, and says which clause to
    // go and read.
    registry.deny(RESEARCH_ONLY, Usage::Trade, "research licence, clause 4.2")?;
    let stated = data
        .open(&mut registry, Usage::Trade, now)
        .expect_err("an explicitly denied usage was opened");
    assert!(stated.message().contains("clause 4.2"), "{stated}");

    // Both refusals are on the record. Code repeatedly asking whether it may
    // trade on a research feed is a finding in its own right.
    assert_eq!(registry.refusals().len(), 2);
    assert_eq!(
        registry.permitted_usages(RESEARCH_ONLY, now),
        vec![Usage::Research]
    );

    // And the catalogue, holding the same type, says the same thing.
    let mut catalog = Catalog::new();
    catalog.register(
        DatasetRegistration::new(RESEARCH_ONLY, "research", MeshPort::Analytical, now)?.licensed(
            Entitlement::Granted {
                dataset: RESEARCH_ONLY.to_string(),
                usage: Usage::Research,
                expires_at: licence_expiry(),
            },
        ),
    )?;
    catalog.usable_for(RESEARCH_ONLY, Usage::Research, now)?;
    let catalogued = catalog
        .usable_for(RESEARCH_ONLY, Usage::Trade, now)
        .expect_err("the catalogue permitted a use the licence does not cover");
    assert!(
        catalogued.message().contains("not licensed for trade"),
        "{catalogued}"
    );
    Ok(())
}

#[test]
fn the_evidence_written_by_the_loop_cannot_be_revised_afterwards() -> Result<()> {
    // Stage 6 is the only stage whose output is supposed to be unchangeable.
    // An evidence layer its own operators can correct proves nothing, because
    // the thing it exists to rule out is exactly that correction.
    let journey = walk()?;
    let original = serde_json::to_vec(journey.normalised.value())?;
    let later = journey.known_at().saturating_add(Duration::from_days(30));

    let conflict = journey
        .evidence
        .put(
            EVIDENCE_KEY,
            b"the bar as we would prefer it".to_vec(),
            later,
        )
        .expect_err("the loop's evidence was rewritten");
    assert!(conflict.message().contains(EVIDENCE_KEY), "{conflict}");
    assert!(conflict.message().contains("write-once"), "{conflict}");

    // The original is untouched, byte for byte, and still readable as of the
    // moment it was written rather than the moment of the attempt.
    let stored = journey.evidence.get(EVIDENCE_KEY, later)?;
    assert_eq!(stored.value().as_deref(), Some(original.as_slice()));
    assert_eq!(
        journey
            .evidence
            .receipt(EVIDENCE_KEY, later)?
            .value()
            .as_ref(),
        Some(&journey.receipt)
    );

    // The honest retry still works. If an identical rewrite were an error,
    // every writer would need a read before its write, and the race that
    // opens is worse than the duplicate it prevents.
    let retry = journey.evidence.put(EVIDENCE_KEY, original, later)?;
    assert_eq!(retry, journey.receipt, "a retry changed the record");
    Ok(())
}

#[test]
fn nothing_the_loop_wrote_is_visible_before_the_moment_it_became_known() -> Result<()> {
    // The property the whole loop is built to make true. Every stage above
    // carried a known-time; this is the test that the known-time is load
    // bearing rather than decorative — a reader as of one nanosecond earlier
    // must see nothing, everywhere at once.
    let journey = walk()?;
    let valid_at = journey.valid_at();
    let known_at = journey.known_at();
    let a_moment_before = known_at.saturating_sub(Duration::from_nanos(1));

    // A reader built as of that moment does not hold the fact at all, which
    // is stronger than filtering it out on the way past.
    let reader = PointInTime::as_of(a_moment_before, vec![journey.normalised.clone()]);
    assert!(reader.is_empty());
    assert_eq!(reader.withheld(), 1);
    assert!(reader.require_latest().is_err());
    assert!(reader.in_force_at(valid_at).is_none());

    // A quarter of a second later the same reader sees it, so the emptiness
    // above is about the horizon and not about an empty store.
    let reader = PointInTime::as_of(known_at, vec![journey.normalised.clone()]);
    assert_eq!(reader.len(), 1);
    assert_eq!(reader.worst_latency(), feed_latency());
    assert!(reader.in_force_at(valid_at).is_some());
    // And it cannot be talked into a later horizon once it exists.
    assert!(
        reader
            .restrict_to(known_at.saturating_add(Duration::from_days(1)))
            .is_err()
    );

    // The detector covers what does not come through a reader — a feature
    // vector assembled by hand, a joined column, a covariate somebody added.
    let detector = LeakageDetector::new(a_moment_before);
    let report = detector.audit([("bars.minute.close", &journey.normalised)]);
    assert!(!report.is_clean());
    assert_eq!(report.inspected(), 1);
    let error = report.require_clean().expect_err("the audit passed a leak");
    assert!(error.message().contains("bars.minute.close"), "{error}");
    assert!(
        LeakageDetector::new(known_at)
            .audit([("bars.minute.close", &journey.normalised)])
            .is_clean()
    );

    // And the stores the loop actually wrote to agree with the detector.
    assert!(journey.lakehouse.snapshot(TABLE, a_moment_before).is_err());
    assert!(
        journey
            .evidence
            .get(EVIDENCE_KEY, a_moment_before)?
            .value()
            .is_none()
    );
    assert!(
        journey
            .graph
            .neighbours(ENTITY, None, a_moment_before)?
            .value()
            .is_empty()
    );
    assert!(
        journey
            .world
            .features()
            .value_as_of("close", OBJECT, valid_at, a_moment_before)
            .is_none(),
        "the world model would have answered with a price nobody had yet"
    );
    assert!(
        journey
            .world
            .features()
            .value_as_of("close", OBJECT, valid_at, known_at)
            .is_some()
    );
    Ok(())
}

#[test]
fn two_journeys_through_the_loop_produce_byte_identical_outcomes() -> Result<()> {
    // Replayability is what makes the record worth keeping. Every source of
    // nondeterminism in the walk is injected — the synthetic venue's RNG is
    // seeded, the clock is manual, and every collection along the way is
    // ordered — so two runs that differed would mean one of those escaped.
    //
    // Compared as encoded bytes rather than as values, because that is the
    // form the outcome is stored and reconciled in, and a difference that
    // survives serialisation is one an auditor would see.
    let first = walk()?.outcome()?;
    let second = walk()?.outcome()?;
    assert_eq!(
        serde_json::to_string(&first)?,
        serde_json::to_string(&second)?
    );
    assert_eq!(first, second);

    // The guard against a vacuous comparison: an outcome that had lost its
    // content would still be equal to itself.
    assert!(!first.evidence_digest.is_empty());
    assert!(!first.features.is_empty());
    assert_eq!(first.neighbours.len(), 1);
    Ok(())
}

// --- the log as the only source of truth ------------------------------------
//
// Everything above walks one market fact into state. What follows is the
// claim the walk is worth keeping for: that the state the platform acts on
// can be rebuilt from the event log and nothing else, by a process that took
// none of the decisions.
//
// It belongs here rather than in `qip-kernel/tests/` because it is a claim
// about three registries at once — who may be funded, which venues a named
// person registered with, and where the desk's cash is — rebuilt through
// three different paths that only meet in one log. A crate's own tests can
// see one of those paths; none of them can see that the three agree about the
// same log, or that one altered byte stops all of them.
//
// The imports are here rather than in the file's header because this section
// was appended: the walk above reaches ingestion, the mesh and the world
// model, and this one reaches the kernel, and keeping the two lists apart
// says which test needs what.

use qip_capital::ledger::{
    Eligibility, EligibilityDecision, EligibilityTerms, Jurisdiction, Mandate, MandateId,
    MandateTerms, PermittedFamilies, UserId,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Currency, dec};
use qip_data_finder::registration::{RegistrationRecord, RegistrationRequirement};
use qip_events::log::EventLog;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::universe::Universe;
use qip_kernel::config::{PlatformConfig, UserMandate};
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market_ingestion::connector::manifest::SecretRef;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::OperatorIdentity;

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

/// The instrument the desk's book is opened over — the same object the walk
/// above carries, so the log the registries are rebuilt from also holds the
/// market records of the loop this file is about rather than operator
/// decisions alone.
const DESK_INSTRUMENT: &str = OBJECT;

/// A source the shipped registration table says needs an account, so it is
/// pending — refused, with nobody's name on it — until an operator approves.
const ACCOUNT_SOURCE: &str = "alpaca-daily-bars";
const REGISTRATION_TERMS: &str = "https://alpaca.markets/terms-and-conditions";

/// The *name* of the deployment variable the credential is read under, never
/// a credential. `SecretRef` refuses anything that is not a variable name,
/// which is the point of the type.
const CREDENTIAL_SLOT: &str = "QIP_ALPACA_API_KEY_ID";

const OPERATOR: &str = "ops-dana";
const INVESTOR: &str = "alice";
const STRATEGY: &str = "alpha";
const DESK_VENUE: &str = "simulated-venue";

/// How long the operator's verification of the investor stands.
fn verified_until() -> Timestamp {
    origin().saturating_add(Duration::from_days(365))
}

/// A universe of one instrument, so the platform has a book to open.
fn desk_universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string(DESK_INSTRUMENT),
            CANONICAL_SYMBOL,
            InstrumentType::CommonStock,
            fixture_liquidity(),
        )
        .venue(CANONICAL_VENUE)
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(RecordProvenance::synthetic("truth-loop", origin()))
        .build(origin())?,
    )?;
    Ok(universe)
}

/// Two hours of the synthetic venue's own output for that instrument.
///
/// The same environment `source_event` draws the walk's bar from, and the
/// same seed, so the market half of the log is the loop's own material and
/// not a fixture written to suit the replay.
fn observed_history() -> Vec<SensedRecord> {
    SyntheticEnvironment::demo(
        origin(),
        EnvironmentConfig {
            seed: 20_260_822,
            ..EnvironmentConfig::default()
        },
    )
    .run_until(origin().saturating_add(Duration::from_mins(120)))
}

fn desk_limits() -> LimitSet {
    LimitSet::new("truth-loop-replay").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at twice equity"),
    )
}

/// The configuration both processes assemble under.
///
/// Identical for the two of them on purpose — same seed, same mandate, same
/// file — because the second process must differ from the first in exactly
/// one respect: it has the log and it took none of the decisions in it. A
/// configuration that carried the eligibility or the registration would prove
/// nothing, since the registry would then be the configuration's doing.
fn replay_config(path: &std::path::Path) -> Result<PlatformConfig> {
    Ok(PlatformConfig::default()
        .with_event_log_file(path)
        .with_user_mandates(vec![UserMandate {
            user: UserId::new(INVESTOR)?,
            id: MandateId::new("mandate-truth-loop")?,
            mandate: Mandate::new(MandateTerms {
                capital: dec!("1000"),
                currency: Currency::USD,
                risk_tolerance: Decimal::ONE,
                permitted_families: PermittedFamilies::Any,
                liquidity_floor: Decimal::ZERO,
                exploration_share: Decimal::ZERO,
                jurisdiction: Jurisdiction::new("GB")?,
            })?,
        }]))
}

fn assemble(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(origin(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        desk_universe()?,
        desk_limits(),
    )
}

/// A directory nothing else in this run writes to.
fn log_directory(label: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "qip-truth-loop-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    directory
}

/// Flip one byte of one record, in place, and say which byte was flipped.
///
/// The alteration is deliberately the smallest one a disk fault or a hand
/// edit produces: a single character inside a string the record already
/// holds, leaving the line valid JSON and a loadable `LogRecord`. A tamper
/// that broke the JSON would be caught by the parser at load, which proves
/// nothing about the chain.
fn flip_one_byte(line: &str) -> (String, char, char) {
    // The producer name: content the chain covers, and no part of the log's
    // structure, so the record still loads and still indexes.
    const MARKER: &str = "\"producer\":\"";
    let at = line
        .find(MARKER)
        .unwrap_or_else(|| panic!("no record names its producer: {line}"))
        + MARKER.len();
    let mut bytes = line.as_bytes().to_vec();
    let was = bytes[at];
    assert!(
        was.is_ascii_alphabetic(),
        "the producer does not start with a letter, so the flip below would not be one byte"
    );
    let now = if was == b'a' { b'b' } else { b'a' };
    bytes[at] = now;
    (
        String::from_utf8(bytes).expect("flipping one ASCII byte for another leaves valid UTF-8"),
        char::from(was),
        char::from(now),
    )
}

#[test]
fn every_registry_the_platform_acts_on_rebuilds_from_the_event_log_alone_and_one_flipped_byte_refuses_the_replay()
-> Result<()> {
    // The failure this closes, and it has happened here before: a decision
    // that lived in a registry's memory and nowhere else, so the platform
    // acted on something the log could not account for. The fabric's
    // controls decided exactly that way once — in memory, with nothing in
    // the process writing the decision anywhere — and `/wallet` reported
    // that no wallet existed. Three registries are driven through their
    // operator paths, three cycles are run so the fabric writes, and then
    // all three are rebuilt from the log by a *second* process that took
    // none of the decisions. The last part is the one that matters most: a
    // rebuild is only evidence if a log that has been altered is refused,
    // and refused in a way that names where.
    let directory = log_directory("replay");
    let path = directory.join("events.jsonl");
    assert!(!path.exists(), "the premise: nothing is written there yet");

    let investor = UserId::new(INVESTOR)?;
    let strategy = StrategyId::new(STRATEGY);
    let venue = VenueId::new(DESK_VENUE);
    let mut platform = assemble(replay_config(&path)?)?;
    let initial_equity = platform.config().initial_equity;

    // --- the premise: nothing decided, and nothing to replay ------------
    assert!(
        platform.replay_eligibility()?.records().is_empty(),
        "the log already held an eligibility decision before one was taken"
    );
    assert_eq!(
        platform.registrations().requirement(ACCOUNT_SOURCE),
        Some(RegistrationRequirement::Account),
        "the source under test needs no account, so approving one proves nothing"
    );
    assert!(
        platform.registrations().record(ACCOUNT_SOURCE).is_none(),
        "somebody was already recorded as having registered"
    );
    assert_eq!(
        platform.fabric_records(),
        0,
        "the fabric journal held records before anything was decided"
    );
    assert!(
        platform.fabric_state().wallet().is_none(),
        "a wallet existed before any statement was handed in"
    );
    assert!(
        platform
            .fund_user(&investor, &strategy, dec!("100"), origin())
            .is_err(),
        "the premise: nobody has verified the investor, so the funding below is the decision's doing"
    );

    // --- an operator decides eligibility --------------------------------
    let operator = OperatorIdentity::verified(OPERATOR, "hardware-token", origin());
    let decided = platform.decide_eligibility(
        &investor,
        EligibilityDecision::Granted {
            eligibility: Eligibility::new(EligibilityTerms {
                verified_at: origin(),
                can_invest: true,
                jurisdiction: Jurisdiction::new("GB")?,
                expires_at: verified_until(),
            })?,
        },
        &operator,
        "identity verified against the document on file",
        origin(),
    )?;
    assert_eq!(decided.by.subject(), OPERATOR);

    // --- the same operator approves a venue registration ----------------
    let registration = platform.approve_registration(
        ACCOUNT_SOURCE,
        &operator,
        REGISTRATION_TERMS,
        SecretRef::new(CREDENTIAL_SLOT)?,
        origin(),
    )?;
    assert_eq!(registration.operator(), OPERATOR);

    // --- the investor is funded, and a statement is handed in -----------
    platform.fund_user(&investor, &strategy, dec!("100"), origin())?;
    platform.observe_statement(venue, "USD", initial_equity, dec!("1"), origin())?;
    assert_eq!(platform.holdings_observed().len(), 1);

    // --- the market the cycle reasons over ------------------------------
    let history = observed_history();
    assert!(
        !history.is_empty(),
        "the synthetic venue produced nothing, so the cycle below has no market to sense"
    );
    let absorbed = platform.observe(history);
    assert!(
        absorbed > 0,
        "the platform absorbed none of what it was given"
    );

    // --- three cycles, so the fabric journal writes ---------------------
    // Three rather than one because a replay that kept only the last record
    // per key, or that reordered them, rebuilds a one-cycle log correctly
    // and a three-cycle log wrongly.
    for minutes in [121, 181, 241] {
        let report = platform.run_cycle(origin().saturating_add(Duration::from_mins(minutes)));
        assert!(
            report.stage(Stage::Learn).is_some(),
            "the premise is a cycle whose LEARN ran, since that is where the wallet is \
             assembled: {report:?}"
        );
    }
    assert!(
        platform.fabric_records() >= 6,
        "the cycles wrote {} fabric records; each assembles the wallet and reconciles it",
        platform.fabric_records()
    );
    assert!(
        platform.fabric_state().wallet().is_some(),
        "no wallet was assembled, so the fabric state below would be empty and equal to itself"
    );

    // The premise for every comparison that follows: the log holds more than
    // a handful of records, so an equality between two rebuilt-from-nothing
    // registries cannot be what passes.
    let written = platform.event_log().records().len();
    assert!(
        written > 10,
        "the log holds only {written} records; a replay of almost nothing proves almost nothing"
    );
    if let Err(sequence) = platform.event_log().verify_chain() {
        panic!("the live log's own chain breaks at sequence {sequence}");
    }

    // --- path 1: the eligibility registry -------------------------------
    let replayed_eligibility = platform.replay_eligibility()?;
    assert_eq!(
        replayed_eligibility.records().len(),
        1,
        "the log rebuilt {} standing decisions where one was taken",
        replayed_eligibility.records().len()
    );
    assert!(
        matches!(
            replayed_eligibility
                .record(&investor)
                .expect("the log holds the investor's standing decision")
                .decision,
            EligibilityDecision::Granted { .. }
        ),
        "the log rebuilt a decision other than the grant that was taken"
    );
    assert_eq!(&replayed_eligibility, platform.user_ledger().eligibility());

    // --- path 2: the registration registry ------------------------------
    let replayed_registrations = platform.replay_registrations()?;
    assert_eq!(
        replayed_registrations
            .record(ACCOUNT_SOURCE)
            .map(RegistrationRecord::operator),
        Some(OPERATOR),
        "the log rebuilt no registration for {ACCOUNT_SOURCE}, or one in somebody else's name"
    );
    assert_eq!(&replayed_registrations, platform.registrations());

    // --- path 3: the fabric, through a second process's resume ----------
    // Assembly is the replay: `resume_fabric` decides the log's commands
    // again into a fresh journal and refuses if it does not arrive where the
    // log's own replay says it should.
    let resumed = assemble(replay_config(&path)?)?;
    assert_eq!(
        resumed.fabric_state(),
        platform.fabric_state(),
        "a second process over the same log rebuilt a different fabric state"
    );

    // And the second process rebuilds the other two registries as well —
    // from the log alone, having been configured with neither. This is the
    // property in its strongest form: what the platform acts on came from
    // the record, not from a memory that happened to survive.
    assert!(
        resumed.user_ledger().eligibility().records().is_empty(),
        "the second process was configured with an eligibility, so its rebuild proves nothing"
    );
    assert!(
        resumed.registrations().record(ACCOUNT_SOURCE).is_none(),
        "the second process was configured with the registration, so its rebuild proves nothing"
    );
    assert_eq!(
        &resumed.replay_eligibility()?,
        platform.user_ledger().eligibility()
    );
    assert_eq!(&resumed.replay_registrations()?, platform.registrations());

    // --- one flipped byte -----------------------------------------------
    // A rebuild is evidence only if a log that was altered is refused. One
    // byte of the first record — a letter of its producer's name, still
    // valid JSON, still a loadable record — and the replay must stop rather
    // than rebuild a state that reads as having come from the log.
    let original = std::fs::read_to_string(&path)?;
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    assert!(
        lines.len() > 10,
        "the file holds {} lines; the flip below would be most of the log",
        lines.len()
    );
    let (tampered, was, now) = flip_one_byte(&lines[0]);
    assert_ne!(tampered, lines[0], "the flip changed nothing");
    assert_eq!(
        tampered.len(),
        lines[0].len(),
        "the flip changed the length, so it was not one byte for another"
    );
    lines[0] = tampered;
    std::fs::write(&path, format!("{}\n", lines.join("\n")))?;

    let refused = assemble(replay_config(&path)?)
        .expect_err("a platform assembled over a log whose first record had been altered");
    assert!(
        refused
            .message()
            .contains("record at position 1 (sequence 1)"),
        "the refusal does not name where the log was altered — an auditor cannot act on \
         'somewhere': {refused}"
    );
    assert!(
        refused.message().contains("has been altered"),
        "the refusal does not say what is wrong: {refused}"
    );

    // The log's own chain check, a second implementation of the same rule,
    // agrees about which record it was. Two independent answers, because one
    // of them could be the thing that is broken.
    assert_eq!(
        EventLog::open(&path)?.verify_chain(),
        Err(1),
        "the log's own chain check disagrees with the replay about where the alteration is"
    );

    // And the other half, which is what distinguishes a gate from a refusal
    // of everything: put the byte back and the same log rebuilds the same
    // state again. Byte for byte — the file is restored from what was read
    // before the flip, not re-derived.
    std::fs::write(&path, &original)?;
    assert_eq!(
        std::fs::read_to_string(&path)?,
        original,
        "the restore did not put the log back as it was"
    );
    let restored = assemble(replay_config(&path)?)?;
    assert_eq!(
        restored.fabric_state(),
        platform.fabric_state(),
        "the restored log no longer rebuilds the state, so the refusal above was not the \
         flipped {was} → {now} byte's doing"
    );
    assert_eq!(
        &restored.replay_eligibility()?,
        platform.user_ledger().eligibility()
    );
    assert_eq!(&restored.replay_registrations()?, platform.registrations());

    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}
