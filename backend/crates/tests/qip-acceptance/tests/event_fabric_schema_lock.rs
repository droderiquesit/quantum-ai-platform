//! The event-fabric schema lock (ADR 0100 §5, FABRIC-023/024, CONTRACT-046/047).
//!
//! `qip_events::event_fabric::schema_id::SchemaId` hashes a body's full
//! recursive [`Shape`](qip_events::event_fabric::schema_id::Shape) — the
//! fix for `SchemaRegistry`'s own fingerprint, which only sees the top-level
//! field names and so lets a nested field silently retype underneath an
//! unchanged field list. This suite is the control that actually uses that
//! id: it locks the shape of every body a slice stream may admit — the six
//! reflex topics `qip-events`' catalogue binds
//! (`event_fabric::bindings::ALL`) and the three P0 control frames
//! (`RiskApproved`, `PolicyDistributed`, `KillSwitchEngaged`; ADR 0100 §5's
//! `QosClass::P0Control` row: "capital grants, policy, halt") — against a
//! committed row in `qip-events/schemas.lock.json`, and fails whenever a
//! computed shape and its committed row disagree.
//!
//! **Inverted from FABRIC-025's usual direction, and recorded as a deviation
//! pending C2 rather than claimed as codegen** (see `schema_id.rs`'s own doc
//! comment): the lock file is generated from the Rust types below, by a
//! human running this suite and copying the pretty-printed mismatch into
//! `schemas.lock.json`, not the other way round. This file never opens the
//! lock for writing — only [`std::fs::read_to_string`] — so a passing run is
//! never a run that quietly rewrote the thing it was supposed to check.
//!
//! # Why every fixture is built with no field left `None` or empty
//!
//! [`Shape::of`] reads a *sample*, not a type: an `Option<T>` that happens to
//! be `None`, or a `Vec<T>` that happens to be empty, serialises to `null` or
//! `[]` and reads back as [`Shape::Null`] or an unknown-element array, which
//! hides a retype of `T` behind a shape the lock cannot see. Every fixture
//! here is deliberately built with its optional fields `Some(..)` and its
//! collections non-empty — `PassMarker::readings`'s three wire readings,
//! every `Decision::Filled` posting field, every one of
//! [`qip_contracts::policy::PolicyPayload`]'s twelve slots produced rather
//! than left at [`qip_contracts::policy::Slot::unproduced`] — so the shape
//! this lock commits is the fullest one the type can carry today, not the
//! thinnest one a sparse sample happened to produce.
//!
//! One field this cannot reach: [`qip_contracts::reflex::JournalEntry`]'s own
//! `Serialize` is hand-written, not derived, and a v1 entry writes no
//! `version` key at all while a v2 entry writes `at` as an integer and adds
//! `version` as a string (`qip_contracts::reflex`'s own doc comment). The two
//! chain versions are two different wire shapes for one Rust type, and no
//! single sample can carry both. This lock is taken over a v2 sample —
//! "every new entry is sealed under it" per that module's doc comment — and
//! a v1 entry's narrower shape is intentionally not covered: it is a
//! historical wire form nothing produces any more, not a retype this lock
//! failed to notice.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::governance::{Entitlement, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::policy::{
    AdversaryProfiles, BeliefPriors, CausalDigest, CycleWhitelist, Dispositions, EpisodicDigest,
    FeasibilityConstraints, GrantManifest, HaltCommand, InventoryTargets, ModelManifest,
    PlanDigest, PolicyPayload, RegimeState, RiskEnvelopeSnapshot, Slot, WhitelistedConversion,
};
use qip_contracts::reflex::{ChainSpan, ChainVersion, Decision, Gap, JournalEntry, OutcomeRecord};
use qip_contracts::replay::{
    AppliedReadings, ControlPosition, PassMarker, PressureRecord, WireRecord,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueClass, VenueId};
use qip_core::{Duration, EventId, ObjectId, Timestamp, dec};
use qip_events::event_fabric::bindings::{self, TopicBinding};
use qip_events::event_fabric::schema_id::{SchemaId, Shape};
use qip_events::{EventBody, Topic};
use qip_mesh::spine::{CapitalGrantFrame, HaltFrame, PolicyFrame};

/// The trust root fixtures sign with. Not a secret: this file never runs
/// against a deployed process, and a real key is read through
/// `qip_core::secret` there, never here.
const TEST_KEY: &[u8] = b"schema-lock-fixture-key-of-decent-length";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

// --- one committed row ------------------------------------------------------

/// One row of `schemas.lock.json`: the topic and version a body is bound at,
/// the Rust type sampled to compute its shape, and the [`SchemaId`] that
/// shape hashes to.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct LockRow {
    topic: String,
    version: u32,
    type_name: String,
    schema_id: String,
}

fn row_for<T: EventBody>(sample: &T) -> LockRow {
    let shape = Shape::of(sample).expect("an EventBody fixture serialises to compute its shape");
    let schema_id = SchemaId::new(T::TOPIC.name(), T::SCHEMA_VERSION, &shape);
    LockRow {
        topic: T::TOPIC.name().to_string(),
        version: T::SCHEMA_VERSION,
        type_name: std::any::type_name::<T>().to_string(),
        schema_id: schema_id.as_str().to_string(),
    }
}

/// Like [`row_for`], for a reflex body: `qip-contracts` owns these types and
/// deliberately never implements `EventBody` for them (`bindings.rs`'s own
/// doc comment — the binding is a constant table `qip-events` holds about
/// its own closed set, not a dependency edge). So the topic and version come
/// from the binding `qip-events` already commits to, not from the type.
fn row_for_binding<T: serde::Serialize>(binding: TopicBinding, sample: &T) -> LockRow {
    let shape = Shape::of(sample).expect("a reflex fixture serialises to compute its shape");
    let schema_id = SchemaId::new(binding.topic.name(), binding.schema_version, &shape);
    LockRow {
        topic: binding.topic.name().to_string(),
        version: binding.schema_version,
        type_name: std::any::type_name::<T>().to_string(),
        schema_id: schema_id.as_str().to_string(),
    }
}

// --- the six reflex fixtures -------------------------------------------------

fn pass_marker_sample() -> PassMarker {
    PassMarker {
        cell: "cell-nynj".to_string(),
        session: "session-2026-09-26T00".to_string(),
        pass: 7,
        now_ns: 1_760_000_000_000_000_000,
        tape_digest: "tape-digest-abc".to_string(),
        tape_from: 100,
        tape_to: 140,
        control: vec![ControlPosition::new(
            "policy",
            0,
            12,
            EventId::from_string("evt-00000000000000000000000001"),
        )],
        // All three readings `Some`, not `AppliedReadings::default()`'s all
        // `None` — a sample with every reading absent would lock a shape
        // that cannot tell `Option<PressureRecord>` from a retyped field
        // that happens to serialise to `null` too.
        readings: AppliedReadings {
            journal_pressure: Some(PressureRecord::new("nominal", "spool at 12%")),
            halt_flag: Some(WireRecord::new("clear", "no halt flag set")),
            region_wire: Some(WireRecord::new("healthy", "all regions lit")),
        },
        config_digest: "config-digest-1".to_string(),
        plan_digest: "plan-digest-1".to_string(),
        gateway_seed: 42,
        binary_version: "qip-edge-node@sha256:deadbeef".to_string(),
    }
}

/// A v2-sealed entry with every posting field on `Decision::Filled` present
/// (`side`, `quote_unit`, `fee`) — see the module doc comment for why v2 and
/// not v1.
fn journal_entry_sample() -> JournalEntry {
    JournalEntry {
        sequence: 42,
        at: t(5),
        decision: Decision::Filled {
            order_id: "order-1".to_string(),
            venue: "XNYS".to_string(),
            object: "obj-ACME".to_string(),
            quantity: "100".to_string(),
            price: "100.25".to_string(),
            simulated: true,
            shares: vec![("alpha".to_string(), "100".to_string())],
            side: Some(BookSide::Ask),
            quote_unit: Some("USD".to_string()),
            fee: Some("0.50".to_string()),
        },
        digest: "digest-1".to_string(),
        version: ChainVersion::V2,
    }
}

fn outcome_record_sample() -> OutcomeRecord {
    OutcomeRecord {
        cell: "cell-nynj".to_string(),
        session: 3,
        journal_sequence: 42,
        journal_digest: "digest-1".to_string(),
        entry: journal_entry_sample(),
    }
}

fn chain_span_sample() -> ChainSpan {
    ChainSpan {
        cell: "cell-nynj".to_string(),
        session: 3,
        first_seq: 10,
        last_seq: 41,
        tail_digest: "tail-digest-1".to_string(),
    }
}

fn gap_sample() -> Gap {
    Gap {
        stream: "itch-a/0".to_string(),
        from_seq: 100,
        to_seq: 105,
        reason: "spool pressure shed the window".to_string(),
    }
}

fn market_event_sample() -> MarketEvent {
    let origin = Origin::new(VenueId::new("XNYS"), "itch-a", 0, 1);
    let payload = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        origin,
        MessageBody::Trade {
            price: dec!("100.25"),
            quantity: dec!("100"),
            condition: TradeCondition::Regular,
            // `Some`, not `None`: the aggressor is optional on the wire and
            // this fixture must not hide a retype of `BookSide` behind an
            // absent field.
            aggressor: Some(BookSide::Bid),
        },
        t(0),
        t(0),
    );
    let hash = MarketEvent::hash_payload(&payload).expect("the fixture payload hashes");
    MarketEvent::new(
        payload,
        t(0),
        t(1),
        t(2),
        Duration::from_millis(5),
        hash,
        Entitlement::Granted {
            dataset: "xnys-itch".to_string(),
            usage: Usage::Trade,
            expires_at: t(10_000),
        },
    )
    .expect("the market event fixture holds together")
}

// --- the three P0 control frames ---------------------------------------------

fn capital_grant_frame_sample() -> CapitalGrantFrame {
    let envelope = CapitalEnvelope::new(
        StrategyId::new("mean-reversion-1"),
        "cell-nynj",
        dec!("1000000"),
        dec!("100000"),
        dec!("50000"),
        vec![VenueId::new("XNYS")],
        t(0),
        t(3600),
        "alice@example.com",
        "signature-placeholder",
    )
    .expect("the capital envelope fixture holds together");
    CapitalGrantFrame(envelope)
}

/// Every one of [`PolicyPayload`]'s thirteen slots produced, none left at
/// [`Slot::unproduced`] — an unproduced slot serialises absent or `null` and
/// would hide a retype of the slot's payload behind a shape this lock cannot
/// see. Every collection inside a slot is non-empty for the same reason.
fn policy_payload_sample() -> PolicyPayload {
    let mut payload = PolicyPayload::unproduced(9, "cell-nynj", t(0));
    payload.valid_for = Duration::from_secs(300);
    payload.halted = false;
    payload.trained_models = Slot::produced(
        ModelManifest {
            models: BTreeMap::from([("model-a".to_string(), "digest-a".to_string())]),
        },
        t(0),
    );
    payload.compiled_plan = Slot::produced(
        PlanDigest {
            digest: "plan-digest-1".to_string(),
            strategies: 3,
        },
        t(0),
    );
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::from([("AAPL".to_string(), 0.62)]),
        },
        t(0),
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "episodic-digest-1".to_string(),
            episodes: 5,
        },
        t(0),
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: vec!["edge-1".to_string()],
        },
        t(0),
    );
    payload.regime_state = Slot::produced(
        RegimeState {
            regime: "risk_on".to_string(),
            confidence: 0.8,
        },
        t(0),
    );
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec!["sig-1".to_string()],
        },
        t(0),
    );
    payload.cycle_whitelist = Slot::produced(
        CycleWhitelist {
            cycles: BTreeMap::from([("cycle-1".to_string(), "path-3".to_string())]),
            conversions: vec![WhitelistedConversion {
                venue: "XNYS".to_string(),
                venue_class: VenueClass::Exchange,
                market: "obj-1".to_string(),
                from: "USD".to_string(),
                to: "AAPL".to_string(),
                side: BookSide::Ask,
                cost_fraction: dec!("0.001"),
            }],
            start_sizes: BTreeMap::from([("USD".to_string(), dec!("1000"))]),
        },
        t(0),
    );
    payload.risk_envelope = Slot::produced(
        RiskEnvelopeSnapshot {
            limits: serde_json::json!({"max_gross": "1000000"}),
        },
        t(0),
    );
    payload.inventory_targets = Slot::produced(
        InventoryTargets {
            targets: BTreeMap::from([("AAPL".to_string(), dec!("100"))]),
            reference_prices: BTreeMap::from([("AAPL".to_string(), dec!("150.25"))]),
        },
        t(0),
    );
    payload.feasibility_constraints = Slot::produced(
        FeasibilityConstraints {
            minimum_order: BTreeMap::from([("XNYS".to_string(), dec!("1"))]),
            fee_floor: BTreeMap::from([("XNYS".to_string(), dec!("0.001"))]),
            tick: BTreeMap::from([("XNYS".to_string(), dec!("0.01"))]),
            withdrawn_venues: BTreeSet::from(["BADVENUE".to_string()]),
            dark_regions: BTreeSet::from(["eu-west".to_string()]),
        },
        t(0),
    );
    payload.adversary_profiles = Slot::produced(
        AdversaryProfiles {
            venues: BTreeMap::from([(
                "XNYS".to_string(),
                serde_json::json!({"posture": "defensive"}),
            )]),
        },
        t(0),
    );
    payload.dispositions = Slot::produced(
        Dispositions {
            unwinds: BTreeMap::from([(
                StrategyId::new("alpha"),
                BTreeMap::from([("AAPL".to_string(), dec!("-10"))]),
            )]),
        },
        t(0),
    );
    payload
        .signed(TEST_KEY)
        .expect("the policy payload fixture signs")
}

fn policy_frame_sample() -> PolicyFrame {
    PolicyFrame(policy_payload_sample())
}

fn halt_frame_sample() -> HaltFrame {
    let halt = HaltCommand::new("cell-nynj", t(0), "manual test halt")
        .signed(TEST_KEY)
        .expect("the halt command fixture signs");
    HaltFrame(halt)
}

// --- every bound body, and the file that locks it ---------------------------

/// One row per body a slice stream may admit today: the six reflex topics
/// `event_fabric::bindings::ALL` names, plus the three P0 control frames.
///
/// The `match` on `binding.topic` has no wildcard fixture arm — only the
/// catch-all `panic!` below — so a seventh binding added to `ALL` fails this
/// suite loudly instead of silently locking eight of nine bodies.
fn computed_rows() -> Vec<LockRow> {
    let mut rows = Vec::new();
    for binding in bindings::ALL {
        let row = match binding.topic {
            Topic::ReflexPassMarked => row_for_binding(binding, &pass_marker_sample()),
            Topic::ReflexJournalRecorded => row_for_binding(binding, &journal_entry_sample()),
            Topic::MarketEventApplied => row_for_binding(binding, &market_event_sample()),
            Topic::ReflexOutcomeRecorded => row_for_binding(binding, &outcome_record_sample()),
            Topic::ReflexChainSpan => row_for_binding(binding, &chain_span_sample()),
            Topic::EventFabricGap => row_for_binding(binding, &gap_sample()),
            other => panic!(
                "event_fabric::bindings::ALL now binds {other}, which this lock has no fixture \
                 for; add one in event_fabric_schema_lock.rs before this suite can cover it"
            ),
        };
        rows.push(row);
    }
    rows.push(row_for(&capital_grant_frame_sample()));
    rows.push(row_for(&policy_frame_sample()));
    rows.push(row_for(&halt_frame_sample()));
    rows.sort_by(|a, b| (a.topic.as_str(), a.version).cmp(&(b.topic.as_str(), b.version)));
    rows
}

/// `backend/crates/libs/qip-events/schemas.lock.json`, found relative to this
/// crate's own manifest so the suite runs the same wherever the workspace is
/// checked out.
fn lock_file_path() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("qip-acceptance sits two levels below crates/")
        .join("libs")
        .join("qip-events")
        .join("schemas.lock.json")
}

/// The committed lock, read (never written) from disk.
fn committed_rows() -> Vec<LockRow> {
    let path = lock_file_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "{} does not parse as a lock row array: {error}",
            path.display()
        )
    })
}

/// FABRIC-023/024: every bound body's computed [`SchemaId`] must equal its
/// committed row, and a body the lock has no row for at all is named rather
/// than silently treated as compatible.
///
/// Mutation: add a field to `OutcomeRecord` without bumping
/// `event_fabric::bindings::REFLEX_OUTCOME_RECORDED`'s schema version. The
/// computed shape changes, the committed row does not, and the `assert_eq!`
/// below fails naming `qip_contracts::reflex::OutcomeRecord` — CONTRACT-046's
/// "a shape change without a version bump fails CI."
#[test]
fn every_bound_body_has_the_shape_its_lock_row_records() {
    let computed = computed_rows();
    // Premise: there is something to check, so every assertion below is
    // exercised at least once rather than vacuously true over an empty list.
    assert!(
        !computed.is_empty(),
        "no bound body was sampled; the fixture list in this file is empty"
    );

    let committed = committed_rows();
    if committed.len() != computed.len() {
        panic!(
            "schemas.lock.json has {} row(s) but {} bound bodies are computed today; \
             regenerate it with exactly this content:\n{}",
            committed.len(),
            computed.len(),
            serde_json::to_string_pretty(&computed).expect("lock rows serialise")
        );
    }

    for row in &computed {
        let locked = committed
            .iter()
            .find(|existing| existing.topic == row.topic && existing.version == row.version)
            .unwrap_or_else(|| {
                panic!(
                    "schemas.lock.json has no row for topic {} version {} ({}); it was never \
                     locked, or the lock has drifted out from under it",
                    row.topic, row.version, row.type_name
                )
            });
        assert_eq!(
            locked.type_name, row.type_name,
            "topic {} is locked against {} but is sampled today from {}; the topic was \
             reassigned to a different Rust type",
            row.topic, locked.type_name, row.type_name
        );
        assert_eq!(
            locked.schema_id, row.schema_id,
            "{} (topic {}) has changed shape without its schema id in schemas.lock.json moving; \
             bump the schema version if the change is deliberate",
            row.type_name, row.topic
        );
    }
}

/// CONTRACT-047: the lock names exactly one row per bound body and version —
/// no topic is locked twice under two disagreeing rows, which is a control
/// that silently uses whichever a lookup happens to find first.
///
/// Mutation: duplicate one committed row, changing only its `schema_id`
/// (so the duplicate is not a byte-for-byte copy — "under a different id").
/// The two rows still share one `(topic, version)` key, and the `assert!`
/// below fails on the second `insert`.
#[test]
fn the_lock_has_exactly_one_row_per_bound_body_and_version() {
    // Premise: the binding list this lock is meant to cover is non-empty, or
    // "no duplicate" below would hold vacuously over zero rows.
    assert!(
        !bindings::ALL.is_empty(),
        "premise: there is at least one bound reflex topic to lock"
    );

    let committed = committed_rows();
    let mut seen = BTreeSet::new();
    for row in &committed {
        let key = (row.topic.clone(), row.version);
        assert!(
            seen.insert(key.clone()),
            "schemas.lock.json carries topic {} version {} more than once; whichever row a \
             lookup happens to find first silences the other",
            key.0,
            key.1
        );
    }

    assert_eq!(
        committed.len(),
        computed_rows().len(),
        "schemas.lock.json does not have exactly one row per bound body; it has drifted from \
         the topics this build actually binds"
    );
}
