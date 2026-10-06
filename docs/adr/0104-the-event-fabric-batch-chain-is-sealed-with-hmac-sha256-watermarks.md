# ADR 0104: The event fabric batch chain is sealed with HMAC-SHA256 watermarks

- **Status**: Accepted, 2026-10-06, as part of M5 implementation (ADR 0100 §4).
- **Date**: 2026-10-06
- **Relates to**: ADR 0100 (decision §4, "Ordering, idempotency and fencing"), ADR 0043 (allowed in-tree cryptography), ADR 0002 (two-dependency posture).

## Context

Each partition's batch chain grows as the broker seals segments. The reflex journal, the P1 outcomes chain, and the P2 market-event chain are all produced and consumed by different processes. A consumer must be able to verify that:

1. Every segment it reads is in the order it was sealed (not reordered by a network partition or a corrupted log).
2. No record has been inserted into a sealed segment (the segment is immutable once archived).
3. The chain from one consumer to the next is continuous (no gaps, no spliced history).

ADR 0043 authorises in-tree SHA-256 and HMAC as the only cryptographic primitives, for the exact reason this record exists: the platform needs a chain digest to hold history accountable, without taking on a hand-rolled TLS or BLAKE3 dependency.

## Decision

**Every sealed segment batch carries an HMAC-SHA256 watermark.** The watermark is computed as:

```
watermark = HMAC-SHA256(
  key = master_key(partition, epoch),
  message = canonical_json(previous_watermark || batch_metadata || records)
)
```

**Watermark semantics:**

1. **Master key derivation.** The key is keyed from the partition id and broker epoch using HMAC-KDF, derived from the platform's secret key stored in Secret Manager.
2. **Canonical form.** The message is SHA-256 over `serde_json::to_vec()` with a consistent field order. The platform uses only `serde_json`, which preserves order if `preserve_order` is enabled, and the codec enforces it.
3. **Chaining.** Each batch's watermark includes the hash of the previous batch's watermark and payload. This creates an immutable linked list: changing any byte in any sealed batch changes its watermark, which breaks all downstream watermarks.
4. **Verification.** A reader verifies the chain by:
   - Reading the `previous_hash` field from each batch.
   - Computing the watermark of the previous batch.
   - Comparing to the stored value.
5. **Trusted bootstrap.** The first batch in a partition names `previous_hash = "bootstrap:<epoch>"`, and its watermark is signed at seal time (see ADR 0105).

**Watermark is NOT encryption.** HMAC proves integrity and chaining, not confidentiality. The batch content and all metadata are plaintext in the segment file. The watermark is a hash — it is stored, but it does not lock records. A publisher with the master key can verify the chain; a reader with only the partition digest can verify nothing except that it matches what it should.

## Rationale

**Immutability proof.** A copy of a segment file on the network, a storage layer with a corruption bug, or an operator hand-editing the log are all detectable because the watermark will not verify. This is the same technique used in blockchain ledgers and the platform's own event log hash chain (ADR 0007, exact attribution).

**No distribution of signing keys.** The HMAC master key never leaves Secret Manager or reaches a broker instance until needed; it is not embedded in the segment. A consumer verifies the chain using the same key material in memory, never against a public key carried in the batch. This avoids the "authenticating a document that names its own auth token" problem.

**Per-epoch fencing.** A new broker epoch changes the HMAC key, so replaying a segment from a stale epoch will fail verification. This prevents a failed broker from serving old segments as if they were new.

**Determinism for replay.** Replay of the slice (ADR 0100 §8, test 7) uses the same HMAC key, so byte-for-byte chain equality is provable: if the input is the same, the watermarks are the same. This is the property that makes the test meaningful.

## Cost

**Computational.** HMAC-SHA256 is fast; a million-record batch adds nanoseconds of digest time, not milliseconds. The broker seals segments at a much lower rate than the drain produces batches. Not a bottleneck.

**Keystore operations.** Reading the master key from Secret Manager happens once at broker startup and is cached in memory. Keyring rotation requires a re-derive, which happens at the epoch boundary (usually after an operator restart). Not a runtime hotspot.

## Alternatives rejected

- **No chain digest at all.** Leaves the platform unable to prove the broker did not silently corrupt a sealed segment or splice in a record. Paper trading requires visibility into what happened, not faith in the storage layer.
- **Timestamped hashes without chaining.** A timestamp and a hash of the segment payload is independent proof that the segment exists, but not proof that the segments are in order. A reordering attack would succeed.
- **BLAKE3 instead of SHA-256.** Requires a new dependency (ADR 0002, ADR 0009). BLAKE3 has not been audited for the same class of use as SHA-256; using a known standard is the right choice here.
- **Asymmetric signing (RSA/ECDSA).** This is the third gap ADR 0043 names as unmet. The platform has neither the infrastructure to manage signing keys nor the key distribution mechanism to make public-key infrastructure work at this scale. Symmetric HMAC suffices and stays within the allowed primitives.

## What would make this wrong

- **The watermark stored incorrectly or not at all.** A batch exists in the log with no watermark field, or a watermark that was computed from different input than the one readers see. Test: every sealed batch has a watermark, and it is serialised as part of the batch metadata.
- **The HMAC key embedded in the batch or in plaintext anywhere in code or configuration.** The key reaches only the broker's memory via Secret Manager, and the consumer's memory via the same provider. Test: `secrets.rs` audit and the acceptance suite `no_secret_this_repository_deploys_reaches_a_process_as_an_environment_value`.
- **A watermark computation that ignores previous content or uses a non-canonical form.** This breaks chaining semantics. Test: mutation the batch's `previous_hash`, confirm the verify fails.
- **Replay verification passing when it should fail, or failing when it should pass.** Test: the replay test (test 7) asserts byte-for-byte equality of the chain.

Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01PQgAtHf7btZ8uy61HoLEvQ
