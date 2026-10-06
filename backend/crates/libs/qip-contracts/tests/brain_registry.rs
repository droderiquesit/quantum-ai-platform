use qip_contracts::BrainSpec;

#[test]
fn a_brain_registry_entry_requires_all_six_attributes() {
    let spec = BrainSpec {
        id: "analyst-001".to_string(),
        name: "Equity Analyst".to_string(),
        competency_domains: vec!["equities".to_string(), "fundamentals".to_string()],
        tools: vec!["edgar-connector".to_string(), "valuation-model".to_string()],
        source_permissions: vec!["sec-filings".to_string()],
        memory_scope: "session-local".to_string(),
        model_stack: vec!["analyst-v1".to_string()],
        evaluations: vec!["accuracy-benchmark".to_string()],
        abstention_rules: vec!["outside-competency".to_string()],
        authority_envelope: "shadow".to_string(),
        calibration: 0.82,
        version: 1,
        parent_version_id: None,
    };

    assert!(spec.validate().is_ok());

    // Encode and decode
    let bytes = serde_json::to_vec(&spec).unwrap();
    let decoded = BrainSpec::decode(&bytes).unwrap();

    assert_eq!(decoded.id, "analyst-001");
    assert_eq!(decoded.name, "Equity Analyst");
    assert_eq!(decoded.competency_domains.len(), 2);
    assert_eq!(decoded.tools.len(), 2);
    assert_eq!(decoded.source_permissions.len(), 1);
    assert_eq!(decoded.memory_scope, "session-local");
    assert_eq!(decoded.model_stack.len(), 1);
    assert_eq!(decoded.evaluations.len(), 1);
    assert_eq!(decoded.abstention_rules.len(), 1);
    assert_eq!(decoded.authority_envelope, "shadow");
    assert!((decoded.calibration - 0.82).abs() < 0.001);
    assert_eq!(decoded.version, 1);
    assert_eq!(decoded.parent_version_id, None);
}

#[test]
fn a_new_specialist_version_records_its_parent() {
    let v1 = BrainSpec {
        id: "analyst-001".to_string(),
        name: "Equity Analyst".to_string(),
        competency_domains: vec!["equities".to_string()],
        tools: vec!["valuation-model".to_string()],
        source_permissions: vec!["sec-filings".to_string()],
        memory_scope: "session-local".to_string(),
        model_stack: vec!["analyst-v1".to_string()],
        evaluations: vec!["accuracy-benchmark".to_string()],
        abstention_rules: vec!["outside-competency".to_string()],
        authority_envelope: "shadow".to_string(),
        calibration: 0.82,
        version: 1,
        parent_version_id: None,
    };

    assert!(v1.validate().is_ok());

    // Create version 2 with parent reference
    let v2 = BrainSpec {
        id: "analyst-001".to_string(),
        name: "Equity Analyst".to_string(),
        competency_domains: vec!["equities".to_string(), "fixed-income".to_string()],
        tools: vec!["valuation-model".to_string(), "bond-pricer".to_string()],
        source_permissions: vec!["sec-filings".to_string(), "bond-data".to_string()],
        memory_scope: "session-local".to_string(),
        model_stack: vec!["analyst-v2".to_string()],
        evaluations: vec![
            "accuracy-benchmark".to_string(),
            "robustness-test".to_string(),
        ],
        abstention_rules: vec!["outside-competency".to_string()],
        authority_envelope: "shadow".to_string(),
        calibration: 0.87,
        version: 2,
        parent_version_id: Some("analyst-001-v1".to_string()),
    };

    assert!(v2.validate().is_ok());
    assert_eq!(v2.parent_version_id, Some("analyst-001-v1".to_string()));
    assert_eq!(v2.version, 2);
    assert!(v2.calibration > v1.calibration);
}

#[test]
fn a_brain_spec_missing_a_field_is_refused() {
    let mut spec = BrainSpec {
        id: "analyst-001".to_string(),
        name: "Equity Analyst".to_string(),
        competency_domains: vec!["equities".to_string()],
        tools: vec!["valuation-model".to_string()],
        source_permissions: vec!["sec-filings".to_string()],
        memory_scope: "session-local".to_string(),
        model_stack: vec!["analyst-v1".to_string()],
        evaluations: vec!["accuracy-benchmark".to_string()],
        abstention_rules: vec!["outside-competency".to_string()],
        authority_envelope: "shadow".to_string(),
        calibration: 0.82,
        version: 1,
        parent_version_id: None,
    };

    // Empty competency_domains should fail
    spec.competency_domains.clear();
    assert!(spec.validate().is_err());

    // Restore and test empty tools
    spec.competency_domains = vec!["equities".to_string()];
    spec.tools.clear();
    assert!(spec.validate().is_err());

    // Restore and test invalid calibration (outside [0, 1])
    spec.tools = vec!["valuation-model".to_string()];
    spec.calibration = 1.5;
    assert!(spec.validate().is_err());

    // Zero version should fail
    spec.calibration = 0.82;
    spec.version = 0;
    assert!(spec.validate().is_err());
}

#[test]
fn a_brain_spec_can_be_encoded_and_decoded_exactly() {
    let original = BrainSpec {
        id: "specialist-42".to_string(),
        name: "Market Microstructure Expert".to_string(),
        competency_domains: vec!["microstructure".to_string(), "order-flow".to_string()],
        tools: vec!["tick-analyzer".to_string()],
        source_permissions: vec!["order-book".to_string(), "execution-data".to_string()],
        memory_scope: "regional".to_string(),
        model_stack: vec!["micro-v3".to_string(), "micro-v2".to_string()],
        evaluations: vec!["backtest".to_string()],
        abstention_rules: vec!["crypto".to_string(), "derivatives".to_string()],
        authority_envelope: "paper".to_string(),
        calibration: 0.91,
        version: 3,
        parent_version_id: Some("specialist-42-v2".to_string()),
    };

    let encoded = serde_json::to_vec(&original).unwrap();
    let decoded = BrainSpec::decode(&encoded).unwrap();

    assert_eq!(decoded, original);
}
