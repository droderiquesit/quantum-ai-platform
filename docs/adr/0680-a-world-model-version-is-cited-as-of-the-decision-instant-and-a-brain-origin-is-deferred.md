# ADR 0680: A world-model version is cited as of the decision instant; a per-brain origin is deferred

Status: Proposed (lane 40, wave b52, 2026-11-15). Provisional.

## Context

ADR 0191 decision 3 journals versioned world-model snapshots. A skeptic found
that no reader cites a version, and that all three brains write under one
origin. Nothing was compiled when this was written (owner instruction of
2026-11-15).

## Decision

1. `world_snapshot::cite(log, domain, at)` returns the highest version whose
   snapshot instant is not after `at`. A decision cannot cite content read
   after it. A domain with no snapshot yet yields `None`, never version 0.
2. A per-brain origin is deferred. It changes the snapshot body or the
   producer string, which changes how `resume` reads the log, and that needs
   the brain's identity at `Platform` construction, which the platform does
   not hold. Provisional: one origin stays; versions of different brains are
   not told apart.
3. Features, entities, contradictions and episodic memory stay unsnapshotted.

## Consequences

A reader can cite "causal graph at version N as of T". Wiring `cite` into a
decision record is not done here.

## Further lane 40 decision: R12-MODEL-6004 (hedged and unwound paths)

This record is lane 40's only home for decisions besides ADR 0681, so the
decision is written here rather than under a third number.

1. The alternative to an **unwind** is holding the closed quantity to the
   horizon; the alternative to a **hedge** is the unhedged hold. Both are
   reported as `regret` = alternative minus chosen, positive meaning the
   alternative was better.
2. The queue is bounded at 256 pending paths (`path_counterfactual::MAX_PENDING`),
   refused beyond that, never dropped. **Provisional**: no figure was given.
3. Not decided and not built: wiring the queue into `Platform` and LEARN. An
   earlier draft did that (unwind queue, twin hold arm) and broke
   `a_closed_position_is_priced_as_the_unhedged_hold_and_a_climbing_tape_is_a_regret`.
   Owner decision 7 also holds that no composition root declares a hedge
   policy, so a hedge counterfactual would score nothing on a default path.
