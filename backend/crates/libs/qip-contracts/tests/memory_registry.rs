use qip_contracts::{MemoryEntry, MemoryKind, RetentionPolicy, ValuePolicy};

#[test]
fn a_memory_entry_with_all_required_fields_is_accepted() {
    let entry = MemoryEntry {
        id: "memory-001".to_string(),
        kind: MemoryKind::Episodic,
        content: "observed pattern in market microstructure".to_string(),
        retention_policy: RetentionPolicy {
            max_age_ms: 86400000, // 24 hours
            description: "keep for 24 hours; daily patterns expire".to_string(),
        },
        value_policy: ValuePolicy {
            importance_threshold: 0.6,
            rationale: "only keep episodes that were actionable".to_string(),
        },
        created_at_ms: 1000000,
        importance: 0.75,
        tags: vec!["market".to_string(), "pattern".to_string()],
    };

    assert!(entry.validate().is_ok());

    // Encode and decode
    let bytes = serde_json::to_vec(&entry).unwrap();
    let decoded = MemoryEntry::decode(&bytes).unwrap();

    assert_eq!(decoded.id, "memory-001");
    assert_eq!(decoded.kind, MemoryKind::Episodic);
    assert_eq!(decoded.content, "observed pattern in market microstructure");
    assert_eq!(decoded.retention_policy.max_age_ms, 86400000);
    assert!((decoded.value_policy.importance_threshold - 0.6).abs() < 0.001);
    assert_eq!(decoded.created_at_ms, 1000000);
    assert!((decoded.importance - 0.75).abs() < 0.001);
    assert_eq!(decoded.tags.len(), 2);
}

#[test]
fn all_memory_kinds_are_accepted() {
    let kinds = vec![
        MemoryKind::Episodic,
        MemoryKind::Semantic,
        MemoryKind::Procedural,
        MemoryKind::Failure,
        MemoryKind::Simulation,
        MemoryKind::CompressedAbstraction,
    ];

    for kind in kinds {
        let entry = MemoryEntry {
            id: format!("memory-{:?}", kind),
            kind,
            content: "test content".to_string(),
            retention_policy: RetentionPolicy {
                max_age_ms: 3600000,
                description: "test policy".to_string(),
            },
            value_policy: ValuePolicy {
                importance_threshold: 0.5,
                rationale: "test rationale".to_string(),
            },
            created_at_ms: 1000000,
            importance: 0.7,
            tags: vec!["test".to_string()],
        };

        assert!(entry.validate().is_ok());
        let bytes = serde_json::to_vec(&entry).unwrap();
        let decoded = MemoryEntry::decode(&bytes).unwrap();
        assert_eq!(decoded.kind, kind);
    }
}

#[test]
fn a_memory_entry_missing_retention_or_value_policy_is_refused() {
    let mut valid_entry = MemoryEntry {
        id: "memory-001".to_string(),
        kind: MemoryKind::Semantic,
        content: "semantic knowledge".to_string(),
        retention_policy: RetentionPolicy {
            max_age_ms: 604800000, // 7 days
            description: "keep for one week".to_string(),
        },
        value_policy: ValuePolicy {
            importance_threshold: 0.5,
            rationale: "retain moderate importance".to_string(),
        },
        created_at_ms: 1000000,
        importance: 0.65,
        tags: vec!["semantic".to_string()],
    };

    // Zero retention time should fail
    valid_entry.retention_policy.max_age_ms = 0;
    assert!(valid_entry.validate().is_err());

    // Restore and test invalid importance_threshold
    valid_entry.retention_policy.max_age_ms = 604800000;
    valid_entry.value_policy.importance_threshold = 1.5;
    assert!(valid_entry.validate().is_err());

    // Restore and test zero created_at
    valid_entry.value_policy.importance_threshold = 0.5;
    valid_entry.created_at_ms = 0;
    assert!(valid_entry.validate().is_err());

    // Restore and test invalid importance
    valid_entry.created_at_ms = 1000000;
    valid_entry.importance = -0.1;
    assert!(valid_entry.validate().is_err());
}

#[test]
fn retention_policy_without_valid_max_age_or_description_is_refused() {
    let mut policy = RetentionPolicy {
        max_age_ms: 3600000,
        description: "one hour retention".to_string(),
    };

    // Zero max_age_ms should fail
    policy.max_age_ms = 0;
    assert!(policy.validate("test").is_err());

    // Restore and test blank description
    policy.max_age_ms = 3600000;
    policy.description = "   ".to_string();
    assert!(policy.validate("test").is_err());
}

#[test]
fn value_policy_with_out_of_range_importance_or_blank_rationale_is_refused() {
    let mut policy = ValuePolicy {
        importance_threshold: 0.7,
        rationale: "keep high-value episodes".to_string(),
    };

    // Importance above 1.0 should fail
    policy.importance_threshold = 1.5;
    assert!(policy.validate("test").is_err());

    // Restore and test negative importance
    policy.importance_threshold = -0.1;
    assert!(policy.validate("test").is_err());

    // Restore and test blank rationale
    policy.importance_threshold = 0.7;
    policy.rationale = "".to_string();
    assert!(policy.validate("test").is_err());
}

#[test]
fn entries_with_all_six_memory_kinds_persist_their_kind_exactly() {
    let mut entries = vec![];

    let kinds = vec![
        (MemoryKind::Episodic, "market episode at 10:00 UTC"),
        (
            MemoryKind::Semantic,
            "liquidity at SPX usually rises by 14:30",
        ),
        (MemoryKind::Procedural, "steps to execute a two-leg cross"),
        (MemoryKind::Failure, "order rejected due to capital breach"),
        (MemoryKind::Simulation, "outcome if we had entered here"),
        (MemoryKind::CompressedAbstraction, "weekly pattern summary"),
    ];

    for (kind, content) in kinds {
        let entry = MemoryEntry {
            id: format!("entry-for-{:?}", kind),
            kind,
            content: content.to_string(),
            retention_policy: RetentionPolicy {
                max_age_ms: 2592000000, // 30 days
                description: "keep for 30 days".to_string(),
            },
            value_policy: ValuePolicy {
                importance_threshold: 0.4,
                rationale: "retain anything above 40% confidence".to_string(),
            },
            created_at_ms: 1609459200000, // 2021-01-01
            importance: 0.72,
            tags: vec!["regression-test".to_string()],
        };

        entries.push(entry);
    }

    // Verify all entries can be encoded and decoded exactly
    for entry in &entries {
        let bytes = serde_json::to_vec(&entry).unwrap();
        let decoded = MemoryEntry::decode(&bytes).unwrap();
        assert_eq!(decoded, *entry);
    }
}
