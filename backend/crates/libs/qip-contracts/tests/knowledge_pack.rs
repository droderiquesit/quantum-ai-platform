//! DATA-013: every pack kind round-trips byte for byte, carries its schema
//! version, and a pack with a field outside its schema is refused.

use qip_contracts::knowledge_pack::{KnowledgePack, PackBody, SCHEMA_VERSION};
use std::collections::BTreeMap;

fn one_of_each() -> Vec<PackBody> {
    vec![
        PackBody::Summary {
            subject: "ECB".into(),
            text: "rates held".into(),
        },
        PackBody::Embedding {
            subject: "ECB".into(),
            values: vec![0.25, -1.5, 3.0],
        },
        PackBody::WorldStateSnapshot {
            as_of_ms: 9,
            facts: BTreeMap::from([("a".to_string(), "b".to_string())]),
        },
        PackBody::WorldModelDelta {
            from_version: 1,
            to_version: 2,
            added: vec!["x".into()],
            removed: vec![],
        },
        PackBody::ModelArtifact {
            name: "m".into(),
            artifact_sha256: "ab".repeat(32),
        },
        PackBody::EpisodicMemory {
            episode: "e1".into(),
            lessons: vec!["l".into()],
        },
    ]
}

#[test]
fn a_pack_of_each_content_type_round_trips_byte_for_byte_and_carries_its_schema_version() {
    let bodies = one_of_each();
    assert_eq!(
        bodies.len(),
        6,
        "premise: all six content types are covered"
    );
    for body in bodies {
        let pack = KnowledgePack::new("p1", 5, body);
        let bytes = pack.encode().expect("encode");
        let text = String::from_utf8(bytes.clone()).expect("utf8");
        assert!(
            text.contains(&format!("\"schema_version\":{SCHEMA_VERSION}")),
            "{text}"
        );
        let back = KnowledgePack::decode(&bytes).expect("decode");
        assert_eq!(back, pack);
        assert_eq!(back.encode().expect("re-encode"), bytes);
    }
}

#[test]
fn a_pack_carrying_a_field_outside_its_schema_is_refused_at_every_level() {
    let pack = KnowledgePack::new(
        "p1",
        5,
        PackBody::Summary {
            subject: "s".into(),
            text: "t".into(),
        },
    );
    let good = String::from_utf8(pack.encode().expect("encode")).expect("utf8");
    assert!(
        KnowledgePack::decode(good.as_bytes()).is_ok(),
        "premise: the unmodified pack decodes"
    );
    let top = good.replacen(
        "{\"schema_version\"",
        "{\"raw_body\":\"<html>\",\"schema_version\"",
        1,
    );
    assert!(
        KnowledgePack::decode(top.as_bytes()).is_err(),
        "extra top-level field"
    );
    let inner = good.replacen("\"text\"", "\"raw_body\":\"<html>\",\"text\"", 1);
    assert!(
        KnowledgePack::decode(inner.as_bytes()).is_err(),
        "extra body field"
    );
    let kind = good.replacen("\"summary\"", "\"web_page\"", 1);
    assert!(
        KnowledgePack::decode(kind.as_bytes()).is_err(),
        "unknown kind"
    );
    let version = good.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
    assert!(
        KnowledgePack::decode(version.as_bytes()).is_err(),
        "other schema version"
    );
}
