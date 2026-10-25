# ADR 0181: The simulated depth dataset permits raw retention, raw frames reach the lake off the decision path, and lake segments are sized at the seal

- **Status**: **Accepted**, 2026-10-25, decided under the owner's delegation
  of 2026-10-24, recorded in
  [`docs/blueprint/owner-decisions-2026-10-24.md`](../blueprint/owner-decisions-2026-10-24.md).
  Nothing below enables a live venue, buys a dataset or adds billed
  infrastructure.
- **Date**: 2026-10-25
- **Acts on**: [ADR 0151](0151-what-enters-understand-each-tick-plane-and-federation-input-has-a-named-producer-and-consumer-or-is-not-built-and-an-outside-prior-may-only-narrow-reason.md)
  decision 8, which builds the raw-frame writer only where a licence permits
  raw retention. **Amends**
  [ADR 0156](0156-platform-plumbing-is-restated-to-the-accepted-records-schema-descriptors-archive-format-storage-classes-and-the-telemetry-drain-have-one-home-each.md)
  §2.3 for the lake.
- **Rows**: R12-TICK-2042, R12-DATA-9187.

## Context

- **TICK-2042** asks that every raw tick stay available for exact replay,
  subject to entitlement, proven by re-materialising a captured session byte
  for byte from the lake. The edge node's lake keeps decoded events only
  (`LakeSink::deliver`: "there is no raw payload to keep here"), and
  `default_retention` grants the simulated dataset `raw: false`. ADR 0151
  decision 8 waits for a licence that permits raw retention. No licensed
  venue source exists, but one dataset's terms are entirely the platform's
  own: `qip-simulated-depth`, the simulated venue's feed, which no third
  party wrote and no licence restricts.
- **DATA-9187** asks that lake objects be partitioned, immutable and
  reasonably sized, with tiny objects compacted asynchronously. ADR 0156 §2.3
  kept `Compactor` for "the lake's small objects only" with no caller. Read
  against the lake, a merge is wrong for the same reason it is wrong on the
  archive: a lake segment is sealed write-once and read by key through a
  pinned manifest, and a merged object would have no place in a manifest
  that names the originals.

## Decision

1. **The simulated depth dataset permits raw retention (TICK-2042).** The
   licence decision ADR 0151 decision 8 waits on is taken for
   `qip-simulated-depth` alone: its terms are the platform's, so
   `default_retention` grants it `raw: true`. Every licensed dataset still
   keeps raw bytes only where its own retention right says so, as today.
2. **Raw frames reach the lake off the decision path.** The simulated feed
   already records, on every captured event, the SHA-256 of the exact line
   it put on the wire (`MarketEvent::native_hash`). `LakeSink`, on the drain
   thread, re-encodes each line with the feed's own encoder
   (`qip_edge_node::feed::native_line`) and keeps it only when it hashes to
   that recorded value, so the bytes it keeps are the published bytes by
   construction and nothing raw travels the spool twice. It seals them in
   the segment's raw slot (`Lake::write_segment`'s `raw` argument, honoured
   only where the partition's entitlement permits raw retention), so one
   sealed segment holds the decoded lines and the bytes they were decoded
   from. A partition with any line that does not re-encode is sealed without
   raw bytes and counted (`LakeSink::raw_unverified`): a raw slot is the
   whole session or absent, never a session with a hole. A venue whose
   encoder is not in this tree cannot be re-encoded; its raw bytes would
   need a raw-frame record on the spool, decided when such a venue exists.
3. **Lake segments are sized at the seal, never merged (DATA-9187; amends
   ADR 0156 §2.3).** No compaction pass merges sealed lake segments. Fewer,
   larger segments come from where they are cut: `LakeSink` accumulates a
   partition's lines across drained batches and seals when a size or age
   bound is reached, rather than one segment per spool offset. The age bound
   keeps a quiet partition from holding lines unsealed indefinitely.
   `Compactor` keeps its archive refusal and gets no lake caller; its
   `min_object_size` semantics are corrected to compact objects *below* the
   threshold, so the tool matches its documentation if a non-lake prefix
   ever needs it.

## Consequences

- TICK-2042 becomes closable by code: the verified re-encoding, the raw
  slot, and a test that re-materialises each published frame byte for byte
  and fails when one line is dropped.
- DATA-9187 becomes closable by code: seal-time batching with a size and age
  bound, a test that many small batches seal as few segments that read back
  the same ticks, and the `min_object_size` fix.

## What it costs

- The drain re-encodes each line once more than it did. The simulated
  feed's lines are short, and the drain thread is off the decision path.
- Batching across offsets means a segment's id names a range of spool
  offsets instead of one, and a crash before the seal re-drains the batch.
  The write-once refusal (`denied` on an existing key) keeps a re-drain from
  duplicating a segment.

## What would make this wrong

- If a licensed dataset's terms forbade even transient raw bytes on the
  spool, decision 2 would need the frame withheld before the spool rather
  than at the seal.
- If the age bound let a partition's unsealed lines grow large under a slow
  drain, the bound would need to become a line-count bound as well.
