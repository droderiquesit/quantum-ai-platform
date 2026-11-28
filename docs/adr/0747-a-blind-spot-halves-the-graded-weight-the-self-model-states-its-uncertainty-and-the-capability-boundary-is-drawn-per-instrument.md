# ADR 0747: A blind spot halves the graded weight, the self-model states its uncertainty, and the capability boundary is drawn per instrument

- **Status**: Proposed 2026-11-28, for the owner
- **Date**: 2026-11-28
- **Rows**: R12-WORLD-3036

## Context

R12-WORLD-3036 asks for a self-model of known weaknesses, blind spots,
uncertainty, stale areas, drift, operational health and capability
boundaries, which REASON scales evidence by. On 2026-11-28 the register
left three things open:

- **(a)** A blind spot, a (component, sector) cell with no graded resolved
  thesis, capped evidence at `min(UNGRADED_WEIGHT, SelfModel::factor)`. That
  is the weight the origin carries anyway, so the cap changed no weight in
  any configuration. It was a label on `blind_origins` and nothing else.
- **(b)** The shrinkage toward one half by `PSEUDO_COUNTS`, and the refusal
  below `MINIMUM_SAMPLE`, were applied but never stated.
- **(c)** The capability boundary was drawn by asset class.
  `data/datasets/universe.json` holds six common stocks in one class, so the
  boundary fired only when the whole feed had stopped.

Decision 2 of `docs/blueprint/owner-decisions-2026-11-28.md` settles the
figures and the shape. This record states how the platform carries them,
and the one journal change they need.

## Decision

1. **Blind-spot discount.** `SelfModel::blind_spot` returns
   `BLIND_SPOT_DISCOUNT × min(UNGRADED_WEIGHT, factor)`, with
   `BLIND_SPOT_DISCOUNT = 0.5` from decision 2. `Platform::synthesise` applies
   it as a cap, which is a minimum with the factor held, so it can only lower
   a weight. The REASON detail line names both figures: "where grading gives
   G; weighed at W".
2. **Uncertainty reading.** `SelfModel::uncertainty` returns an
   `UncertaintyReading` for each component the model holds. It carries
   `resolved` (the window the estimate is computed over),
   `prior_share = PSEUDO_COUNTS / (PSEUDO_COUNTS + resolved)` and
   `below_minimum_sample`. `PSEUDO_COUNTS` is made public so the reading uses
   the same figure as the estimate. `GET /cognition/self-model` serves it on
   each component row.
3. **Journaled as a change record.** `CycleJournalEntry` gains an optional
   `self_model: SelfModelJournal`, which carries:
   - the components whose reading moved since the last entry, with `null` for
     a component the model evicted;
   - the operational-health reading, only when its halted flag or its
     degraded rows changed.

   `SelfModelJournal::replay` folds the journal back into the table, and
   `replay_health` returns the newest health reading. World-model members are
   graded on nearly every cycle, so a full table in every entry would grow the
   fast brain's in-memory log by the whole table on each cycle. The field is
   skipped when absent, so the version-3 shape and older entries decode
   unchanged. No `SCHEMA_VERSION` bump is needed, which is the precedent set
   by `experience` and `risk_discovery`.
4. **Boundary per instrument.** On every REASON pass,
   `Platform::observe_served_boundary` declares the instruments the feed
   serves with fresh bars: the universe's instruments whose own newest bar
   closed within `FEED_HORIZON`. `synthesise` refuses a thesis on a universe
   instrument outside that set and takes the opportunity off the queue, as the
   class rule does. The class boundary stays as it was. A series the universe
   does not hold is never put inside.

## What it costs

- A blind-spot cell now weighs half. On the shipped review that lowers some
  confidences. `qip-fastbrain` `tests/tape.rs` is the gate that the review
  still admits theses.
- Before this change, a universe instrument whose bars stopped had its
  evidence withheld at weight 0 and still formed a thesis. That thesis is now
  refused. The stale-input withholding arm in `SelfModel::shared_withholding`
  therefore can no longer be reached through the cycle: an unlisted series
  raises no opportunity, and a listed one is refused first. The arm is kept,
  because it only ever restricts, but no cycle test guards it now. The kernel
  test that held it was renamed to the stronger outcome.
- The kernel holds one more bounded map: the last-journaled reading, at most
  `MAX_COMPONENTS` rows.

## Not decided here

- The cycle report does not carry the health reading. The journal does.
- Sizing still reads the degradation state directly. It does not read the
  self-model's health, and nothing here moves sizing.
- Data drift without a worsening Brier score is still not modelled.

## What would make this wrong

- A measurement on the shipped review showing that half weight starves it as
  weight 0 did. The discount would then need a fresh owner figure, not a
  silent change.
- A deployment in which the feed serves an instrument on a cadence slower
  than `FEED_HORIZON`. That instrument would read outside between bars.
- A consumer that replays the journal by taking the newest `self_model`
  entry as the whole table. Entries are changes, and only the fold is the
  table.
