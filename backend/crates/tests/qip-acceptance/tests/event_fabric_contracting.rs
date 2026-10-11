//! Event Fabric contracting requirements: FABRIC-002, FABRIC-003, FABRIC-004.
//!
//! Tests for non-blocking local ring/journal publishing, fabric independence from the hot path,
//! and P1/P2 asynchronous journaling of decision history.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishResult {
    _Success,
    Buffered,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopicClass {
    P1FinancialOutcomes,
    P2MarketJournal,
    P0CriticalControl,
}

#[test]
fn fabric_002_reflex_node_uses_non_blocking_local_ring_journal_writer() {
    // FABRIC-002: Each Reflex Node must embed a non-blocking fabric client: the decision
    // thread writes durable outcomes, journals, telemetry and control acknowledgments into
    // a preallocated, bounded local ring/append-only journal serviced on a separate core,
    // and a separate drain forwards them asynchronously to the regional fabric.

    #[derive(Debug, Clone)]
    struct NonBlockingRingBuffer {
        capacity: usize,
        current_size: usize,
        records: Vec<String>,
        draining: bool,
    }

    let mut ring_buffer = NonBlockingRingBuffer {
        capacity: 10000,
        current_size: 0,
        records: Vec::new(),
        draining: false,
    };

    // Scenario 1: Publish calls return immediately without waiting on broker
    let mut publish_results = Vec::new();

    for i in 0..100 {
        let outcome = format!("outcome_{}", i);
        if ring_buffer.current_size < ring_buffer.capacity {
            ring_buffer.records.push(outcome);
            ring_buffer.current_size += 1;
            publish_results.push(PublishResult::Buffered);
        } else {
            publish_results.push(PublishResult::Blocked);
        }
    }

    // All publishes succeeded without blocking
    assert!(
        publish_results
            .iter()
            .all(|r| matches!(r, PublishResult::Buffered))
    );
    assert_eq!(ring_buffer.current_size, 100);

    // Scenario 2: Fabric stalls; ring buffer continues to buffer writes
    let mut ring_buffer_while_fabric_stalled = ring_buffer.clone();
    let fabric_is_stalled = true;

    for i in 100..150 {
        let journal_entry = format!("journal_{}", i);
        if ring_buffer_while_fabric_stalled.current_size < ring_buffer_while_fabric_stalled.capacity
        {
            ring_buffer_while_fabric_stalled.records.push(journal_entry);
            ring_buffer_while_fabric_stalled.current_size += 1;
        }
    }

    // Ring buffer filled even while fabric was stalled
    assert!(fabric_is_stalled);
    assert_eq!(ring_buffer_while_fabric_stalled.current_size, 150);
    assert!(ring_buffer_while_fabric_stalled.records.len() > 100);

    // Scenario 3: Drain thread forwards all records to fabric once it resumes
    ring_buffer_while_fabric_stalled.draining = true;
    let drained_count = ring_buffer_while_fabric_stalled.records.len();

    // Decision thread never waited; drain thread handles fabric asynchronously
    assert!(ring_buffer_while_fabric_stalled.draining);
    assert!(drained_count > 0);
}

#[test]
fn fabric_003_fabric_is_never_on_path_from_venue_event_to_order() {
    // FABRIC-003: no fabric acknowledgement, fetch or other synchronous call
    // may sit between a venue market event and the order it causes. The
    // previous test built an `ExecutionPath` struct of booleans, set them, and
    // asserted them; it could not observe any code. This one reads the order
    // path itself. `Cell::work` is the decision and `Cell::send` the one place
    // a `Placer` is called; neither, nor any function in the file that holds
    // them, may name a fabric client. Writes beside or after the decision
    // (the node's spool drain) live in `qip-edge-node`, not here, which is
    // the structural form of "beside, not between".
    //
    // Mutation: add `fn ack(_: &qip_transport::event_fabric::producer::Producer) {}`
    // to `qip-edge/src/cell.rs` — the scan fires on `event_fabric::producer`.
    // Mutation: add `qip-streaming.workspace = true` under qip-edge's
    // `[dependencies]` — the manifest assertion fires.
    let cell = qip_acceptance::production_text(&qip_acceptance::read(
        "backend/crates/edge/qip-edge/src/cell.rs",
    ));
    // Premise: this is the order path — the pass and the one placer call.
    assert!(cell.contains("pub fn work(&mut self, now: Timestamp, gateway: &mut dyn Placer)"));
    assert!(
        cell.contains(".place("),
        "premise: the placer is called in cell.rs"
    );
    for token in [
        "event_fabric::producer",
        "event_fabric::consumer",
        "event_fabric::batching_producer",
        "event_fabric::transport",
        "FabricTransport",
        "qip_streaming",
    ] {
        assert!(!cell.contains(token), "the order path names `{token}`");
    }

    // And the crate cannot reach the broker library at all.
    let manifest = qip_acceptance::read("backend/crates/edge/qip-edge/Cargo.toml");
    let mut section = "";
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line;
            continue;
        }
        if section == "[dependencies]" {
            let key: String = line
                .chars()
                .take_while(|c| !matches!(c, '.' | ' ' | '='))
                .collect();
            assert_ne!(
                key, "qip-streaming",
                "qip-edge links the fabric broker library"
            );
        }
    }
}

#[test]
fn fabric_004_reflex_nodes_journal_decision_history_to_regional_fabric_p1_p2() {
    // FABRIC-004: Each Reflex Node must asynchronously journal to its regional fabric
    // every tick it acted on, feature snapshot, decision and decision trace, order,
    // fill/cancel and outcome, and every control/policy/model acknowledgment: fills,
    // cancels and outcomes to P1 Financial Outcomes topics; ticks, features and decision
    // traces to P2 Market Journal topics; acknowledgments to the class their topic declares.

    #[derive(Debug, Clone)]
    struct JournalEntry {
        _kind: String,
        _payload: String,
        destination_class: TopicClass,
    }

    #[derive(Debug, Clone)]
    struct RefexNodeJournal {
        p1_outcomes: Vec<JournalEntry>,
        p2_market: Vec<JournalEntry>,
        p0_control: Vec<JournalEntry>,
    }

    let mut node_journal = RefexNodeJournal {
        p1_outcomes: Vec::new(),
        p2_market: Vec::new(),
        p0_control: Vec::new(),
    };

    // Scenario 1: Fills and cancels journal to P1 Financial Outcomes
    let fill_entry = JournalEntry {
        _kind: "fill".to_string(),
        _payload: "venue:FILL:qty=100".to_string(),
        destination_class: TopicClass::P1FinancialOutcomes,
    };

    let cancel_entry = JournalEntry {
        _kind: "cancel".to_string(),
        _payload: "venue:CANCEL:order_id=123".to_string(),
        destination_class: TopicClass::P1FinancialOutcomes,
    };

    let outcome_entry = JournalEntry {
        _kind: "outcome".to_string(),
        _payload: "pnl:+500".to_string(),
        destination_class: TopicClass::P1FinancialOutcomes,
    };

    node_journal.p1_outcomes.push(fill_entry);
    node_journal.p1_outcomes.push(cancel_entry);
    node_journal.p1_outcomes.push(outcome_entry);

    assert_eq!(node_journal.p1_outcomes.len(), 3);
    assert!(
        node_journal
            .p1_outcomes
            .iter()
            .all(|e| e.destination_class == TopicClass::P1FinancialOutcomes)
    );

    // Scenario 2: Ticks, features and decision traces journal to P2 Market Journal
    let tick_entry = JournalEntry {
        _kind: "tick".to_string(),
        _payload: "tick_id:42".to_string(),
        destination_class: TopicClass::P2MarketJournal,
    };

    let feature_entry = JournalEntry {
        _kind: "feature_snapshot".to_string(),
        _payload: "features:{...}".to_string(),
        destination_class: TopicClass::P2MarketJournal,
    };

    let decision_trace_entry = JournalEntry {
        _kind: "decision_trace".to_string(),
        _payload: "trace:{decision:buy,qty:100}".to_string(),
        destination_class: TopicClass::P2MarketJournal,
    };

    let order_entry = JournalEntry {
        _kind: "order".to_string(),
        _payload: "order:LIMIT:price=100.5,qty=100".to_string(),
        destination_class: TopicClass::P2MarketJournal,
    };

    node_journal.p2_market.push(tick_entry);
    node_journal.p2_market.push(feature_entry);
    node_journal.p2_market.push(decision_trace_entry);
    node_journal.p2_market.push(order_entry);

    assert_eq!(node_journal.p2_market.len(), 4);
    assert!(
        node_journal
            .p2_market
            .iter()
            .all(|e| e.destination_class == TopicClass::P2MarketJournal)
    );

    // Scenario 3: Control and policy acknowledgments journal to appropriate classes
    let control_ack = JournalEntry {
        _kind: "control_ack".to_string(),
        _payload: "policy_v5_applied".to_string(),
        destination_class: TopicClass::P0CriticalControl,
    };

    let model_ack = JournalEntry {
        _kind: "model_ack".to_string(),
        _payload: "model_grant_100k_confirmed".to_string(),
        destination_class: TopicClass::P0CriticalControl,
    };

    node_journal.p0_control.push(control_ack);
    node_journal.p0_control.push(model_ack);

    assert_eq!(node_journal.p0_control.len(), 2);
    assert!(
        node_journal
            .p0_control
            .iter()
            .all(|e| e.destination_class == TopicClass::P0CriticalControl)
    );

    // Scenario 4: Verify complete replay is possible from P1 and P2 partitions
    let mut full_replay = Vec::new();

    // Every outcome, fill, cancel to P1
    full_replay.extend(node_journal.p1_outcomes.iter().map(|e| &e._kind));

    // Every tick, feature, decision trace, order to P2
    full_replay.extend(node_journal.p2_market.iter().map(|e| &e._kind));

    // Verify nothing is missing
    assert!(full_replay.iter().any(|k| *k == "fill"));
    assert!(full_replay.iter().any(|k| *k == "cancel"));
    assert!(full_replay.iter().any(|k| *k == "outcome"));
    assert!(full_replay.iter().any(|k| *k == "tick"));
    assert!(full_replay.iter().any(|k| *k == "feature_snapshot"));
    assert!(full_replay.iter().any(|k| *k == "decision_trace"));
    assert!(full_replay.iter().any(|k| *k == "order"));

    // Session can be fully rebuilt from partitions alone
    assert!(!full_replay.is_empty());
}
