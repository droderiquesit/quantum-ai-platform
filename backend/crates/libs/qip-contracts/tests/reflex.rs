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
