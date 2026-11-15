//! A write is checked against the class it was opened for (R12-DATA-1041).

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::knowledge_pack::{KnowledgePack, MAX_PACK_TEXT_BYTES, PackBody};
use qip_core::error::Result;
use qip_storage::blob::{BlobStore, MemoryBlobStore};
use qip_storage::class_checked::ClassCheckedStore;
use qip_storage::classes::{DECLARED, KNOWLEDGE_PACK_NAMESPACE, blob_class};
use std::sync::Arc;

fn store() -> Result<(Arc<MemoryBlobStore>, ClassCheckedStore)> {
    let inner = Arc::new(MemoryBlobStore::new());
    let class = *blob_class(DECLARED, KNOWLEDGE_PACK_NAMESPACE)?;
    Ok((inner.clone(), ClassCheckedStore::new(inner, class)))
}

/// The failure prevented: the pack store accepted any bytes under its
/// namespace, so a document body written there was durable, and the rule
/// "no durable store holds document bodies" held only while every caller
/// behaved. The valid pack in the same test keeps the refusals from passing
/// on a wrapper that refuses everything.
///
/// Mutated by deleting the `KnowledgePack::decode` check in
/// `ClassCheckedStore::check`: the raw-body write succeeds and the first
/// refusal assertion fails; restored.
#[test]
fn a_write_that_is_not_a_compact_pack_in_the_packs_namespace_is_refused() -> Result<()> {
    let (inner, checked) = store()?;
    let pack = KnowledgePack::new(
        "p1",
        1,
        PackBody::Summary {
            subject: "EUR".into(),
            text: "rate rose".into(),
        },
    )
    .encode()?;
    let key = "src/p1";
    checked.put(key, pack)?;
    assert_eq!(inner.list("")?.len(), 1, "premise: a valid pack is kept");

    let body = "x".repeat(MAX_PACK_TEXT_BYTES * 4).into_bytes();
    assert!(
        checked.put("src/doc", body.clone()).is_err(),
        "a document body was kept as a pack"
    );
    let oversized = serde_json::json!({
        "schema_version": 1, "id": "p2", "created_ms": 1,
        "body": {"kind": "summary", "subject": "s", "text": "y".repeat(MAX_PACK_TEXT_BYTES + 1)}
    });
    assert!(
        checked
            .put_if_absent("src/p2", serde_json::to_vec(&oversized)?)
            .is_err()
    );
    assert_eq!(
        inner.list("")?.len(),
        1,
        "a refused write reached the store"
    );
    Ok(())
}
