//! Event fabric P-class classification: every topic has a declared QoS class.

#[test]
fn every_event_fabric_topic_declares_a_pclass() {
    use qip_events::Topic;
    use qip_events::event_fabric::policy::QosClass;

    // FABRIC-104: warm services must publish events with P0/P1/P3 classification
    // so the broker can enforce durability: P1 (fills, settlement) never dropped,
    // P2 (journal) throttled with explicit gaps, P3 (research) allows backlog.
    // This test verifies every fabric topic carries a declared class.

    let fabric_topics = [
        // P0Control: policy announcements, never dropped
        (Topic::PolicyDistributed, QosClass::P0Control),
        // P1Outcomes: fills, settlement, never dropped
        (Topic::OrderFilled, QosClass::P1Outcomes),
        (Topic::SettlementRecorded, QosClass::P1Outcomes),
        (Topic::ReflexOutcomeRecorded, QosClass::P1Outcomes),
        (Topic::ReflexChainSpan, QosClass::P1Outcomes),
        // P2MarketJournal: reflex journal entries, throttled
        (Topic::ReflexJournalRecorded, QosClass::P2MarketJournal),
        (Topic::MarketEventApplied, QosClass::P2MarketJournal),
        (Topic::EventFabricGap, QosClass::P2MarketJournal),
        // P3Research: world model, research/knowledge, backlog allowed
        (Topic::WorldModelSnapshot, QosClass::P3Research),
        (Topic::OutcomeObserved, QosClass::P3Research),
        (Topic::HypothesisScored, QosClass::P3Research),
        (Topic::ModelEvaluated, QosClass::P3Research),
        (Topic::LearningCompleted, QosClass::P3Research),
    ];

    for (topic, expected_class) in fabric_topics.iter() {
        let actual_class = topic.pclass();
        assert_eq!(
            actual_class,
            Some(*expected_class),
            "{topic} should be classified as {expected_class:?}, got {actual_class:?}"
        );
    }
}

#[test]
fn non_fabric_topics_have_no_pclass() {
    use qip_events::Topic;

    // Topics not published on the event fabric have pclass() == None.
    // This includes market data (replaceable), discovery, reasoning, simulation
    // topics that are internal to the platform.

    let non_fabric_topics = [
        Topic::MarketTick,
        Topic::MarketQuote,
        Topic::MarketTrade,
        Topic::FeatureComputed,
        Topic::SignalGenerated,
        Topic::HypothesisCreated,
        Topic::SimulationStarted,
        Topic::OrderProposed,
        Topic::PositionUpdated,
        Topic::SystemAlert,
        Topic::BudgetExhausted,
    ];

    for topic in non_fabric_topics.iter() {
        assert_eq!(
            topic.pclass(),
            None,
            "{topic} should not be on the event fabric"
        );
    }
}

#[test]
fn pclass_assignments_match_retention_and_group() {
    use qip_events::Topic;
    use qip_events::event_fabric::policy::QosClass;
    use qip_events::retention::RetentionClass;

    // P-class choices align with retention class and group semantics:
    // - P0/P1 (never dropped) -> Irreplaceable retention
    // - P2 (throttled, gap if shed) -> EventAnchored retention
    // - P3 (backlog allowed) -> Episodic retention (research)
    // - P4 (sampled/shed) -> Episodic or lower

    for topic in Topic::ALL.iter() {
        if let Some(pclass) = topic.pclass() {
            let retention = topic.retention_class();
            match pclass {
                QosClass::P0Control | QosClass::P1Outcomes => {
                    assert_eq!(
                        retention,
                        RetentionClass::Irreplaceable,
                        "{topic} has {pclass:?} but {retention:?} retention"
                    );
                }
                QosClass::P2MarketJournal => {
                    assert!(
                        matches!(retention, RetentionClass::EventAnchored),
                        "{topic} has P2MarketJournal but {retention:?} retention"
                    );
                }
                QosClass::P3Research => {
                    assert!(
                        matches!(
                            retention,
                            RetentionClass::Episodic | RetentionClass::DerivedState
                        ),
                        "{topic} has P3Research but {retention:?} retention"
                    );
                }
                QosClass::P4Telemetry => {
                    // P4 can be Episodic or lower (Transient, DerivedState)
                    assert!(
                        !matches!(retention, RetentionClass::Irreplaceable),
                        "{topic} has P4Telemetry but {retention:?} retention"
                    );
                }
            }
        }
    }
}
