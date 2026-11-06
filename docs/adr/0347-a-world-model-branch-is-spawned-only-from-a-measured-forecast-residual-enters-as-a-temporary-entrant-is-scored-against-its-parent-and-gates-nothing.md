# ADR 0347: A world-model branch is spawned only from a measured forecast residual, enters as a temporary entrant, is scored against its parent and gates nothing (amends ADR 0149 decision 12's residual clause)

- **Status**: **Accepted**, 2026-10-31. Decided by lane L4-quant under the
  parent's 2026-10-31 delegation
  (`docs/blueprint/owner-decisions-2026-10-31.md`), on the ruling for
  R12-EXPAND-6043 as its skeptic corrected it
  (`docs/blueprint/lanes/wave1031-handoff-2-2026-10-31.json`, lane "L4b",
  `skeptic_governs: true`). The number is from L4b's block (decision 15).
  The title is shorter than the claimed one: the claim named the
  federation's residual and the ADR 0149 triple, and the governing skeptic
  removed both, for the reasons in Context. The delegation does not lift
  four limits:
  - no live-order path;
  - no real capital or money movement;
  - no weakened test or limit;
  - no new dependency.

  This decision needs none of them. It adds a record and no new caller.
- **Date**: 2026-10-31
- **Amends**: ADR 0149 decision 12, the clause "the production caller of
  `WorldModelSpawner` is not built until a producer of the residual exists
  in the kernel (the spawner's input is an unexplained residual of P&L)".
- **Keeps**: ADR 0149 decision 12's entrant triple, unamended (see
  decision 4).

## Context

ADR 0149 decision 12 said a world-model branch may be spawned only from a
measured residual, and that the residual is one of P&L, which nothing
computes. The ruling for R12-EXPAND-6043 proposed a second spawn caller,
fed from the federation's resolutions through R12-EXPAND-6067's
persistence tracker.

The ruling's skeptic found a caller already built. `qip-kernel`
`model_tournament.rs` `observe_outcome`, reached from the LEARN stage's
resolution loop, calls `WorldModelSpawner::propose_branch` with the
standing population as parent when the tournament aggregate's residual
exceeds `SPAWN_RESIDUAL` (0.6). The residual is the probability the
aggregate left on the outcome that did not happen. It was landed under
R12-MODEL-7198 and runs in both brains' cycles. It departs from decision 12
on three points no ADR ratified:
- a forecast residual rather than one of P&L;
- the id `temporary:<subject>` rather than `branch:<branch id>/<family>`;
- an outcome-frequency model rather than a lattice `ModelFamily` member.

Nothing measured whether a spawned branch forecast better than the
population it branched from.

## Decision

1. **The tournament aggregate's residual is a measured residual, and is
   the only spawn trigger.** It is taken at the question's opening and read
   against the outcome the question resolved to, so nobody chose it after
   the fact. This amends the "residual of P&L" clause. The threshold stays
   `SPAWN_RESIDUAL`.
2. **No second spawn caller is added.** The federation's discrepancies
   (ADR 0346) raise research items, and do not spawn branches.
3. **Each spawned branch is scored against its parent.** For every question
   on the branch's subject that its temporary specialist forecast, and so
   that opened after the spawn, the tournament records the specialist's
   Brier score and the aggregate's, both as they stood at opening.
   `TournamentReview.branch_measurements` journals each live branch's mean
   of both, cumulative since its spawn, on every cycle that scored one. The
   tally lives on the specialist and leaves with it at expiry.
4. **The record gates nothing.** No promotion, weight, size or retirement
   reads it. It is the benchmark a later decision would need before letting
   a branch outlive its expiry or move a weight. The entrant triple of
   decision 12 is not amended here: the built specialist keeps its
   `temporary:` id and its outcome-frequency model, and both remain
   departures from decision 12 that R12-EXPAND-6043 records as open.
5. **Synthetic-venue resolutions.** The tournament resolves on bars, and no
   bar carries a mark saying it came from a synthetic venue. This decision
   cannot exclude them on this path, and says so rather than filtering on a
   venue-name guess.

## Consequences

- `model_tournament.rs`
  `a_subject_the_population_missed_spawns_a_temporary_specialist_that_competes_until_it_expires`
  now also asserts that the branch's next resolved question journals its
  Brier score beside its parent's, and that the two differ.
- The register states the spawn caller as built and measured, and the
  entrant triple as open.

## What it costs

- One record per live branch per scoring cycle on the tournament review.
  The live temporaries bound it, and they are bounded by
  `MAX_FORECASTERS`.
- A trigger fixed at 0.6 that no measurement chose. This decision ratifies
  the residual's kind, not its threshold; the threshold is still a
  constant.

## What would make this wrong

- Branch measurements showing that spawned specialists forecast no better
  than their parent across many branches. Then the spawn costs a forecaster
  slot for nothing, and the caller should be removed, as the skeptic's
  second option said.
- A producer of a P&L residual landing in the kernel. Then decision 12's
  original clause can be met, and which residual spawns needs deciding
  again.
