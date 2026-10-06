//! The reflex journal's v1 chain digest, pinned against the value the
//! pre-move code computed.
//!
//! SLICE-07 moved `Decision`, `JournalEntry` and the v1 digest out of
//! `qip-edge::journal` and into `qip-contracts::reflex` verbatim, because the
//! ledger and the API have to read a cell's journal without depending on
//! `qip-edge` (ADR 0100 §1) and a move that quietly reordered a field or
//! changed a `skip_serializing_if` would make every digest a cell has already
//! sealed fail to verify. This test is the check that the move held: it
//! recomputes the digest through the moved `chain_digest_v1` and compares it
//! against a literal computed independently, from a standalone reproduction
//! of the pre-move `Decision::Filled` and `chain_digest` read out of
//! `qip-edge/src/journal.rs` before this packet touched it — not from the
//! code this test exercises, which would make the comparison tautological.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::reflex::{Decision, chain_digest_v1};
use qip_core::Timestamp;

#[test]
fn a_journal_entry_digest_is_unchanged_by_the_move_to_the_contract_layer() {
    // `Filled` because it is the variant the fourth paper fence reads
    // (`simulated`), and every field is given a distinct value so a
    // reordering anywhere in the variant — not only at its first or last
    // field — would change the serialized body and so the digest.
    let at = Timestamp::from_secs(1_700_000_000);
    let decision = Decision::Filled {
        order_id: "ord-1".to_string(),
        venue: "SIM".to_string(),
        object: "obj-1".to_string(),
        quantity: "10".to_string(),
        price: "101.5".to_string(),
        simulated: true,
        shares: vec![("alpha".to_string(), "10".to_string())],
        side: None,
        quote_unit: None,
        fee: None,
    };

    // Assert the premise before the pinned value: the fixture really is the
    // `Filled` arm the literal below was computed against, and not some
    // other arm of the enum's twenty-six.
    assert_eq!(decision.kind(), "filled");

    let digest = chain_digest_v1("genesis", 0, at, &decision);

    // Computed by a standalone program outside this crate, using a
    // reproduction of `Decision::Filled` and `chain_digest` with the field
    // order, types and serde attributes `qip-edge/src/journal.rs` held
    // immediately before this move — see this packet's report for the
    // reproduction. Any reordering of `Filled`'s fields, any added or removed
    // `#[serde(...)]` attribute, or a change to what `chain_digest_v1` hashes
    // moves this value.
    assert_eq!(
        digest, "9f27d4aac9d56ec26c8844c7757e32252e1980a8a5d37d6378f4e6ae93706f85",
        "the v1 chain digest changed across the move to qip_contracts::reflex; \
         every journal a cell has already sealed would fail to verify"
    );
}

#[test]
fn a_latency_episode_is_recorded_with_region_and_cell_identity() {
    let episode = Decision::LatencyEpisode {
        region: "us-east".to_string(),
        cell: "cell-001".to_string(),
        venue: "NYSE".to_string(),
        object: "AAPL".to_string(),
        expected_ms: 50,
        observed_ms: 125,
    };

    assert_eq!(episode.kind(), "latency_episode");
    let serialized = serde_json::to_string(&episode).expect("latency episode serialises");
    let deserialized: Decision =
        serde_json::from_str(&serialized).expect("latency episode deserialises");
    assert_eq!(episode, deserialized);
}

#[test]
fn a_slippage_episode_is_recorded_with_price_and_basis_points() {
    let episode = Decision::SlippageEpisode {
        region: "us-west".to_string(),
        cell: "cell-002".to_string(),
        venue: "NASDAQ".to_string(),
        object: "MSFT".to_string(),
        expected_price: "380.50".to_string(),
        realized_price: "381.25".to_string(),
        slippage_bps: 20,
    };

    assert_eq!(episode.kind(), "slippage_episode");
    let serialized = serde_json::to_string(&episode).expect("slippage episode serialises");
    let deserialized: Decision =
        serde_json::from_str(&serialized).expect("slippage episode deserialises");
    assert_eq!(episode, deserialized);
}

#[test]
fn a_microstructure_episode_is_recorded_with_pattern_kind_and_evidence() {
    let episode = Decision::MicrostructureEpisode {
        region: "us-central".to_string(),
        cell: "cell-003".to_string(),
        pattern_kind: "lead_lag".to_string(),
        evidence: 85,
    };

    assert_eq!(episode.kind(), "microstructure_episode");
    let serialized = serde_json::to_string(&episode).expect("microstructure episode serialises");
    let deserialized: Decision =
        serde_json::from_str(&serialized).expect("microstructure episode deserialises");
    assert_eq!(episode, deserialized);
}

#[test]
fn a_venue_behavior_episode_is_recorded_with_severity() {
    let episode = Decision::VenueBehaviorEpisode {
        region: "eu-west".to_string(),
        cell: "cell-004".to_string(),
        venue: "LSE".to_string(),
        behavior_kind: "latency_profile".to_string(),
        severity: 75,
    };

    assert_eq!(episode.kind(), "venue_behavior_episode");
    let serialized = serde_json::to_string(&episode).expect("venue behavior episode serialises");
    let deserialized: Decision =
        serde_json::from_str(&serialized).expect("venue behavior episode deserialises");
    assert_eq!(episode, deserialized);
}

#[test]
fn episodes_chain_correctly_with_v2_digests() {
    use qip_contracts::reflex::seal_v2;

    let at = Timestamp::from_secs(1_700_000_000);
    let latency_ep = Decision::LatencyEpisode {
        region: "us-east".to_string(),
        cell: "cell-001".to_string(),
        venue: "NYSE".to_string(),
        object: "AAPL".to_string(),
        expected_ms: 50,
        observed_ms: 125,
    };

    let (recorded_decision, digest) = seal_v2("genesis", 1, at, latency_ep.clone());

    // The decision must be the same after sealing (no canonical form errors)
    assert_eq!(recorded_decision, latency_ep);

    // The digest must be non-empty
    assert!(!digest.is_empty());
    assert_eq!(digest.len(), 64); // SHA-256 hex is 64 chars
}

#[test]
fn a_session_with_multiple_episode_types_records_all_kinds() {
    // This test verifies that all four episode types can coexist in a decision stream
    let episodes = vec![
        Decision::LatencyEpisode {
            region: "us-east".to_string(),
            cell: "cell-001".to_string(),
            venue: "NYSE".to_string(),
            object: "AAPL".to_string(),
            expected_ms: 50,
            observed_ms: 125,
        },
        Decision::SlippageEpisode {
            region: "us-west".to_string(),
            cell: "cell-002".to_string(),
            venue: "NASDAQ".to_string(),
            object: "MSFT".to_string(),
            expected_price: "380.50".to_string(),
            realized_price: "381.25".to_string(),
            slippage_bps: 20,
        },
        Decision::MicrostructureEpisode {
            region: "us-central".to_string(),
            cell: "cell-003".to_string(),
            pattern_kind: "lead_lag".to_string(),
            evidence: 85,
        },
        Decision::VenueBehaviorEpisode {
            region: "eu-west".to_string(),
            cell: "cell-004".to_string(),
            venue: "LSE".to_string(),
            behavior_kind: "latency_profile".to_string(),
            severity: 75,
        },
    ];

    let kinds: Vec<&str> = episodes.iter().map(|e| e.kind()).collect();
    assert_eq!(kinds.len(), 4);
    assert!(kinds.contains(&"latency_episode"));
    assert!(kinds.contains(&"slippage_episode"));
    assert!(kinds.contains(&"microstructure_episode"));
    assert!(kinds.contains(&"venue_behavior_episode"));

    // Verify all can be serialized and deserialized
    for episode in episodes {
        let json = serde_json::to_string(&episode).expect("serialises");
        let _back: Decision = serde_json::from_str(&json).expect("deserialises");
    }
}
