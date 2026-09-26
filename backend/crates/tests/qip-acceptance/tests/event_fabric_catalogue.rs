//! The committed local stream catalogue and its grants (ADR 0100 §5 and §7;
//! FABRIC-045/046/047/057/110, CONTRACT-037).
//!
//! `infrastructure/event-fabric/streams.local.json` is the ACL for the P0
//! control stream — capital grants, `PolicyDistributed`, `KillSwitchEngaged`
//! — and the one place seal cadence and peak byte rate are declared for the
//! first working slice. ADR 0100 §5: "declared in a committed stream
//! catalogue; nothing rests on a broker default". This suite reads the file
//! a deployment would read, through the same [`Catalogue::parse`] the broker
//! and the node use, so a catalogue that the parser refuses fails here
//! before any process refuses to start on it.
//!
//! What it holds that the parser alone does not:
//!
//! * **No key the parser ignores.** `Catalogue::parse` deserialises into a
//!   struct without `deny_unknown_fields`, so a misspelt `lag_limt` beside a
//!   correct `lag_limit`, or a declaration like `archive_required: true` that
//!   no type carries, parses cleanly and is read by nothing. A declaration
//!   nobody reads is the broker default FABRIC-057 refuses, wearing a
//!   committed file's clothes.
//! * **Exactly the grants the slice uses.** The parser refuses a malformed
//!   grant; it cannot refuse a well-formed grant nobody needed, or notice a
//!   needed one is missing. An earlier grant list omitted the node's consume
//!   grant on its own control key, so an enforcing broker would never have
//!   delivered the fixture capital grant to the cell it was for.
//! * **Archive-required is the retention class, not a separate flag.** The
//!   catalogue schema has no archive field, and this suite does not add one
//!   the parser would drop. A stream is archive-required exactly when
//!   `StreamPolicy::archive_required` says so for its declared retention
//!   class (§22.1's `Never` or `InMemoryFixed` are the two that are not): the
//!   log may roll a replaceable record by age, and may delete nothing else
//!   before it is archived. P2 is archive-required too, or a spool mixing P1
//!   and P2 never returns to baseline (ADR 0100 §8 test 8). SLICE-38's broker
//!   answers the identical question off the identical method, so the two
//!   never drift into disagreement about the same stream.

use std::collections::{BTreeMap, BTreeSet};

use qip_acceptance::read;
use qip_events::event_fabric::catalogue::{Catalogue, KeyScope, Permission};
use qip_events::event_fabric::policy::{Mirroring, OverloadPolicy, QosClass};

const CATALOGUE: &str = "infrastructure/event-fabric/streams.local.json";
const SCHEMA_LOCK: &str = "backend/crates/libs/qip-events/schemas.lock.json";

/// The one cell the local slice runs. `reflex:<cell>` is its identity, and
/// its node must be started with this cell id for its own-key grants to
/// match the partition it writes.
const CELL: &str = "reflex:cell-local";

/// Every key a stream declaration carries, in `RawStream`'s own terms.
/// Fifteen policy fields, plus the stream's name and its admitted topics.
const STREAM_KEYS: [&str; 17] = [
    "name",
    "qos_class",
    "partition_key",
    "ordering",
    "retention",
    "replication_factor",
    "mirroring",
    "overload_policy",
    "ack_profile",
    "byte_quota_per_producer",
    "message_quota_per_producer",
    "lag_limit",
    "entitlement_dataset",
    "entitlement_usage",
    "seal_age_ms",
    "peak_bytes_per_second",
    "topics",
];

/// The longest seal age an archive-required stream may declare in the
/// local slice. ADR 0100 §8 test 8 waits for the spool to return to
/// baseline after archive, and archive only follows a seal; a seal age of
/// minutes would make that test either slow or, with a shorter timeout,
/// silently unable to observe the release it exists to prove.
const MAX_ARCHIVE_REQUIRED_SEAL_AGE_MS: u64 = 2_000;

fn catalogue_text() -> String {
    read(CATALOGUE)
}

fn catalogue() -> Catalogue {
    match Catalogue::parse(catalogue_text().as_bytes()) {
        Ok(catalogue) => catalogue,
        Err(error) => panic!("{CATALOGUE} is refused by Catalogue::parse: {error}"),
    }
}

/// The four streams of the slice and their classes, as ADR 0100 §5 and the
/// slice plan name them.
fn expected_streams() -> BTreeMap<&'static str, QosClass> {
    BTreeMap::from([
        ("control.local", QosClass::P0Control),
        ("reflex-outcomes.local", QosClass::P1Outcomes),
        ("reflex-journal.local", QosClass::P2MarketJournal),
        ("telemetry.local", QosClass::P4Telemetry),
    ])
}

#[test]
fn the_committed_catalogue_validates_and_declares_every_field_of_every_stream() {
    let text = catalogue_text();
    let raw: serde_json::Value = serde_json::from_str(&text).expect("the catalogue is JSON");
    let raw_streams = raw["streams"].as_array().expect("a `streams` array");
    assert!(!raw_streams.is_empty(), "the catalogue declares no streams");

    // Every declared key, present and known. A missing key is refused by the
    // parser below too, but only this check refuses a key the parser would
    // silently drop.
    let known: BTreeSet<&str> = STREAM_KEYS.iter().copied().collect();
    for stream in raw_streams {
        let object = stream.as_object().expect("a stream is an object");
        let name = object
            .get("name")
            .and_then(|name| name.as_str())
            .unwrap_or("<unnamed>");
        let present: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        let missing: Vec<&&str> = known.difference(&present).collect();
        let unknown: Vec<&&str> = present.difference(&known).collect();
        assert!(
            missing.is_empty(),
            "stream '{name}' does not declare {missing:?}; FABRIC-057 refuses a broker default"
        );
        assert!(
            unknown.is_empty(),
            "stream '{name}' carries {unknown:?}, which Catalogue::parse ignores — a declaration \
             nothing reads"
        );
    }
    let top: BTreeSet<&str> = raw
        .as_object()
        .expect("the catalogue is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        top,
        BTreeSet::from(["grants", "streams"]),
        "the catalogue holds a top-level key Catalogue::parse ignores"
    );
    // The same for a grant: a `key_scpoe` beside no `key_scope` is refused by
    // the parser, but a stray `expires` or `note` would be read as a
    // restriction by a person and as nothing by the broker.
    let grant_keys = BTreeSet::from(["identity", "key_scope", "permission", "stream"]);
    for grant in raw["grants"].as_array().expect("a `grants` array") {
        let present: BTreeSet<&str> = grant
            .as_object()
            .expect("a grant is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            present, grant_keys,
            "grant {grant} carries a key Catalogue::parse ignores, or lacks one"
        );
    }

    let catalogue = catalogue();
    let declared: BTreeMap<&str, QosClass> = catalogue
        .streams()
        .iter()
        .map(|(name, stream)| (name.as_str(), stream.policy.qos_class()))
        .collect();
    assert_eq!(
        declared,
        expected_streams(),
        "the catalogue must declare exactly the slice's four streams, each in its class"
    );
    assert_eq!(
        raw_streams.len(),
        declared.len(),
        "a stream is declared twice, and the parser kept only the last"
    );

    for (name, stream) in catalogue.streams() {
        let policy = &stream.policy;
        assert_eq!(
            policy.partition_key(),
            "cell",
            "stream '{name}' must be keyed by cell, so a gap or break parks one cell (ADR 0100 §4)"
        );
        for (field, value) in [
            ("byte_quota_per_producer", policy.byte_quota_per_producer()),
            (
                "message_quota_per_producer",
                policy.message_quota_per_producer(),
            ),
            ("lag_limit", policy.lag_limit()),
            ("seal_age_ms", policy.seal_age_ms()),
            ("peak_bytes_per_second", policy.peak_bytes_per_second()),
        ] {
            // Zero is present and well-typed, and would refuse every
            // producer, never seal, or size the node's spool for nothing.
            assert!(value > 0, "stream '{name}' declares {field} = 0");
        }
        // A paper-trading platform's own streams license nothing that could
        // base a live order on them or show them outside the desk.
        assert!(
            !matches!(policy.entitlement().usage(), "trade" | "redistribute"),
            "stream '{name}' entitles '{}', which this platform never needs",
            policy.entitlement().usage()
        );
    }
}

#[test]
fn every_schema_id_a_stream_admits_is_in_the_lock() {
    let lock: Vec<serde_json::Value> =
        serde_json::from_str(&read(SCHEMA_LOCK)).expect("the schema lock is a JSON array");
    let mut locked: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in &lock {
        let topic = row["topic"].as_str().expect("a lock row names its topic");
        let id = row["schema_id"].as_str().expect("a lock row carries an id");
        locked
            .entry(topic.to_string())
            .or_default()
            .push(id.to_string());
    }
    assert!(!locked.is_empty(), "the schema lock is empty");

    // What each stream admits, from the slice plan. Written here rather than
    // read from the catalogue, so a topic moved between streams is caught
    // even though both streams' ids would still be locked.
    let expected: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::from([
        (
            "control.local",
            BTreeSet::from([
                "risk.approved",
                "policy.distributed",
                "system.kill_switch_engaged",
            ]),
        ),
        (
            "reflex-outcomes.local",
            BTreeSet::from([
                "reflex.outcome_recorded",
                "reflex.chain_span",
                "reflex.fabric_gap",
            ]),
        ),
        (
            "reflex-journal.local",
            BTreeSet::from([
                "reflex.journal_recorded",
                "reflex.pass_marked",
                "reflex.market_event_applied",
                "reflex.fabric_gap",
            ]),
        ),
        ("telemetry.local", BTreeSet::new()),
    ]);

    let catalogue = catalogue();
    let mut admitted_anywhere = BTreeSet::new();
    let mut checked = 0usize;
    for (name, stream) in catalogue.streams() {
        let admitted: BTreeSet<&str> = stream.topics.iter().map(|topic| topic.name()).collect();
        assert_eq!(
            admitted.len(),
            stream.topics.len(),
            "stream '{name}' admits a topic twice"
        );
        assert_eq!(
            Some(&admitted),
            expected.get(name.as_str()),
            "stream '{name}' admits a different set of topics than the slice plans"
        );
        for topic in admitted {
            let ids = locked.get(topic).map(Vec::as_slice).unwrap_or_default();
            assert_eq!(
                ids.len(),
                1,
                "stream '{name}' admits '{topic}', which has {} rows in {SCHEMA_LOCK} — a body \
                 whose shape is unlocked can retype underneath the stream unnoticed",
                ids.len()
            );
            let id = &ids[0];
            assert!(
                id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "'{topic}' is locked under '{id}', which is not a SHA-256 hex digest"
            );
            admitted_anywhere.insert(topic.to_string());
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no stream admitted any topic; nothing was checked"
    );

    // And the other direction: a locked body no slice stream admits is a lock
    // row guarding nothing the slice carries.
    let locked_topics: BTreeSet<String> = locked.keys().cloned().collect();
    assert_eq!(
        admitted_anywhere, locked_topics,
        "the slice streams and the schema lock disagree about which bodies the slice carries"
    );
}

#[test]
fn no_slice_stream_is_replicated_mirrored_or_named_for_autonomy() {
    let catalogue = catalogue();
    assert!(
        !catalogue.streams().is_empty(),
        "no streams were declared, so nothing was checked"
    );
    for (name, stream) in catalogue.streams() {
        assert_eq!(
            stream.policy.replication_factor(),
            1,
            "stream '{name}' is replicated; C2's consensus record does not exist"
        );
        assert_eq!(
            stream.policy.mirroring(),
            Mirroring::None,
            "stream '{name}' is mirrored; fabric-mirror is BLOCKED(C8)"
        );
        // Every dot-delimited segment, not only the prefix the parser checks:
        // `reflex.live.x` is as easily read as the live toggle as `live.x`.
        for segment in name.split('.') {
            assert!(
                !matches!(segment, "autonomy" | "live" | "ceiling"),
                "stream '{name}' carries the reserved segment '{segment}'"
            );
        }
    }
}

/// The committed file's own word for a permission. `Permission` has no
/// `Ord`, and the table below is a set; an exhaustive match rather than a
/// `Debug` string, so a fourth permission is a compile error here rather
/// than a word this suite has never compared.
fn permission_word(permission: Permission) -> &'static str {
    match permission {
        Permission::Produce => "produce",
        Permission::Consume => "consume",
        Permission::Admin => "admin",
    }
}

/// The committed file's own word for a key scope; see [`permission_word`].
fn key_scope_word(key_scope: KeyScope) -> &'static str {
    match key_scope {
        KeyScope::OwnKey => "own_key",
        KeyScope::Any => "any",
    }
}

type GrantRow = (&'static str, &'static str, &'static str);

#[test]
fn every_process_identity_in_the_slice_holds_exactly_the_grants_the_slice_uses() {
    // The whole ACL of the slice, identity by identity, as (stream,
    // permission, key scope). The cell writes its outcomes and journal and
    // reads its own control key (SLICE-57); the fixture grant command is the
    // only P0 producer (SLICE-34); the ledger reads outcomes (ADR 0100 §8);
    // the operator can lag, isolate and release every stream and write none
    // (SLICE-37). Nothing produces to telemetry.local in this slice.
    //
    // `release-controller` and `ledger` hold `any` because neither is a cell:
    // the controller writes to the key of the cell a grant is for, and the
    // ledger reads every cell's outcomes. The parser refuses `any` for a
    // `reflex:` identity, and this table refuses it for anyone else not
    // listed here.
    let expected: BTreeMap<&str, BTreeSet<GrantRow>> = BTreeMap::from([
        (
            CELL,
            BTreeSet::from([
                ("reflex-outcomes.local", "produce", "own_key"),
                ("reflex-journal.local", "produce", "own_key"),
                ("control.local", "consume", "own_key"),
            ]),
        ),
        (
            "release-controller",
            BTreeSet::from([("control.local", "produce", "any")]),
        ),
        (
            "ledger",
            BTreeSet::from([("reflex-outcomes.local", "consume", "any")]),
        ),
        (
            "operator",
            BTreeSet::from([
                ("control.local", "admin", "any"),
                ("reflex-outcomes.local", "admin", "any"),
                ("reflex-journal.local", "admin", "any"),
                ("telemetry.local", "admin", "any"),
            ]),
        ),
    ]);
    assert!(
        expected.values().all(|rows| !rows.is_empty()),
        "the expected grant table has an identity with no grants"
    );

    let catalogue = catalogue();
    assert!(
        !catalogue.grants().is_empty(),
        "the catalogue grants nothing, so an enforcing broker would deliver nothing"
    );
    let mut held: BTreeMap<String, BTreeSet<(String, &str, &str)>> = BTreeMap::new();
    for grant in catalogue.grants() {
        assert!(
            !grant.identity.contains('*'),
            "'{}' is a wildcard identity",
            grant.identity
        );
        let fresh = held.entry(grant.identity.clone()).or_default().insert((
            grant.stream.clone(),
            permission_word(grant.permission),
            key_scope_word(grant.key_scope),
        ));
        assert!(
            fresh,
            "'{}' is granted {} on '{}' twice",
            grant.identity,
            permission_word(grant.permission),
            grant.stream
        );
    }

    let identities: BTreeSet<String> = expected
        .keys()
        .map(|identity| identity.to_string())
        .chain(held.keys().cloned())
        .collect();
    let mut wrong = Vec::new();
    for identity in &identities {
        let want: BTreeSet<(String, &str, &str)> = expected
            .get(identity.as_str())
            .into_iter()
            .flatten()
            .map(|(stream, permission, scope)| (stream.to_string(), *permission, *scope))
            .collect();
        let have = held.get(identity).cloned().unwrap_or_default();
        if want != have {
            wrong.push(format!(
                "{identity}: missing {:?}, unneeded {:?}",
                want.difference(&have).collect::<Vec<_>>(),
                have.difference(&want).collect::<Vec<_>>()
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the committed grants are not the slice's grants:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn p1_and_p2_are_archive_required_and_p4_is_the_only_sheddable_class() {
    let catalogue = catalogue();
    // Every stream's expected answer, so a fifth stream added without one is
    // refused rather than skipped.
    let expected_archive: BTreeMap<&str, bool> = BTreeMap::from([
        ("control.local", true),
        ("reflex-outcomes.local", true),
        ("reflex-journal.local", true),
        ("telemetry.local", false),
    ]);
    let names: BTreeSet<&str> = catalogue.streams().keys().map(String::as_str).collect();
    assert_eq!(
        names,
        expected_archive.keys().copied().collect::<BTreeSet<_>>(),
        "every declared stream needs an archive answer here"
    );

    let mut sheddable: BTreeSet<&str> = BTreeSet::new();
    for (name, stream) in catalogue.streams() {
        let policy = &stream.policy;
        let required = policy.archive_required();
        assert_eq!(
            Some(&required),
            expected_archive.get(name.as_str()),
            "stream '{name}' declares retention '{}', so archive-required = {required}",
            policy.retention().as_str()
        );
        if required {
            assert!(
                policy.seal_age_ms() <= MAX_ARCHIVE_REQUIRED_SEAL_AGE_MS,
                "archive-required stream '{name}' seals every {} ms; the archive path cannot \
                 run inside a test",
                policy.seal_age_ms()
            );
        }
        if policy.overload_policy() == OverloadPolicy::SampleOrShed {
            sheddable.insert(policy.qos_class().as_str());
            assert!(
                !required,
                "stream '{name}' is sheddable and archive-required; a shed record is never \
                 archived, so its spool never releases"
            );
        }
    }
    assert_eq!(
        sheddable,
        BTreeSet::from([QosClass::P4Telemetry.as_str()]),
        "P4 telemetry is the one class whose records may be sampled or shed without a Gap"
    );
}
