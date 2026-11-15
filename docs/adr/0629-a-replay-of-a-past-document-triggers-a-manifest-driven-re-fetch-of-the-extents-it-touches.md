# ADR 0629: A replay of a past document triggers a manifest-driven re-fetch of the extents it touches

- **Status**: Accepted 2026-11-15, lane 14 of wave b52. It adds a trigger to an
  existing mechanism. No dependency, figure or paper-trading layer is touched.
- **Date**: 2026-11-15
- **Builds on**: ADR 0180 decision 4 (never-copy governs world documents).
- **Rows**: R12-DATA-8018 (a slice).

## Context

`ReFetchDriver` asked a source for referenced extents again and compared the
served hash with the reference's, but only on a cadence
(`QIP_DEEPBRAIN_REFETCH_EVERY`, default 0). A replay of a past document did
not cause one, so a replay that wanted to know whether the provider still
serves the same bytes had only the first-read copy of the derived records.

## Decision

1. `ConnectorArm::refetch_for_replay` and `EvolutionEngine::refetch_for_replay`
   re-fetch exactly the references of the arm's source whose period overlaps
   the replay's and which name one of its subjects, through the same
   licence gate, transport and egress route the poll used. The result is the
   existing finding: unchanged, revised (both hashes) or unretrievable, and an
   unretrievable extent is marked on the ledger as the cadence pass marks it.
2. The served bytes are hashed and dropped. Nothing here stores a body.
3. A replay naming no subject is refused: it would re-fetch everything the
   source ever served, which copies a corpus by another route.

## What remains

- Calling `refetch_for_replay` from the replay path itself: the replay entry
  points that touch a past document are the recorded-file adapters
  (`QIP_DEEPBRAIN_REPLAY_PATH`, `QIP_DEEPBRAIN_TAPE_PATH`), which read a file
  and name no reference. Joining a replayed record to its reference needs a
  decision about what a recorded file's records are keyed by.
- Those recorded-file paths and `TickLakeWriter` capture stay. ADR 0180
  decision 4 scopes never-copy to world documents; venue ticks held under a
  retention right and recorded tapes are not provider corpora. Whether a
  tape may be dropped is the owner's.
- A connector arm no environment configures, and the egress decision, still
  gate any live re-fetch.
