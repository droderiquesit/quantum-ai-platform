# ADR 0208: A cell's open positions ride its mesh delta as a bounded, sequenced statement the centre reconciles against its settled books and never treats as a balance

- **Status**: **Accepted**, 2026-10-31, decided under the owner's 2026-10-31
  delegation (decision 2: a lane may decide a row that waited only on a
  product decision, conservative, paper-only, fail-closed; the parent lane
  assigned this one to L5b-mesh). The delegation authorises the wire and
  store shape below. It does not authorise a live order, a financial limit
  or a weakened guard, and none is made: every control here refuses more
  than it did before, and nothing here moves a balance.
- **Date**: 2026-10-31
- **Extends**: ADR 0207 (a cell's episodes ride the delta as a view, bounded,
  defaulted), ADR 0116 (the mesh delta is the cell-to-centre wire), ADR 0149
  decision 17 (an added field is defaulted so a sealed record replays).
- **Keeps**: ADR 0003 (paper trading), ADR 0109 (no dependency is added),
  `.claude/rules/architecture/00-boundaries.md`'s "no second source of truth
  for a fact the event log already holds", the delta as a *view* of the cell,
  and `CentralPlane::settle` as the one place a cell's fills become the
  centre's books.

## Context

R12-MESH-7143's second slice asks that a leg left over after a cycle break
be proven unwound and hedged end to end. The lane's first reading of the
gap said a cell's positions never reach the kernel's risk state because
they are not on the mesh delta. That reading was wrong and this record
corrects it before building on it: the delta carries the cell's **fills**,
`CentralPlane::settle` books each one into the per-cell, per-strategy
books, and `Platform::charge_cell_fills` charges every settled fill into
the risk aggregate the Hedge Brain plans against. A leg that filled and was
not offset is therefore already an exposure the centre sees, through the
fills it was sent. What the centre did **not** have was the cell's own
statement of its book to hold the settled book against: a fill lost on the
wire, a cell restarted from an older chain, or a sender that is not the
cell would leave the centre's books and the cell's disagreeing, and nothing
would say so.

Two properties of the existing wire shape the decision, as they did for
ADR 0207: a delta is a view (`Cell::state_delta` takes `&self`; the
transport is at-least-once), and every list on it is bounded with its
omitted count beside it.

## Decision

### 1. The delta carries the cell's book as a statement, sequenced and bounded

`CellStateDelta` (edge), `WireDelta`/`CellStanding` (`qip-mesh`) and
`CellReport` (kernel) gain `positions: Vec<AdvisoryPosition>` — object,
venue and signed quantity, every non-zero entry of the cell's venue-facing
book as its confirmed fills built it, in the book's own venue-then-instrument
order — `positions_omitted: u32` past
`qip_contracts::wire::MAX_POSITIONS_PER_DELTA` (64), and
`positions_as_of: u64`, the last sequence on the cell's journal when the
statement was taken. All three are `#[serde(default)]`, so a record sealed
before them replays as a cell that stated none at sequence zero, which the
centre treats as no statement. **No price travels**: the centre holds its
own marks, and a price the cell chose would be a second claim about a fact
the centre already has.

### 2. The centre holds the statement, replaces it by sequence, and refuses one that went backwards

`CentralPlane::ingest` keeps one `AdvisoryBook` per cell — the positions, the
sequence they are as of, the report instant — and the newest statement
replaces the last. A report whose `positions_as_of` is non-zero and **below**
the sequence the centre already holds for that cell is refused whole, as an
error, before anything in the report is written: a cell's chain only grows,
so an older statement arriving as new is a cell that restarted from an
earlier chain or a sender that is not the cell, and the API's drain treats
an ingest error as a condition to stop on rather than skip past. A report
stating more than `MAX_POSITIONS_PER_DELTA` positions is refused as one that
did not come off the wire. A replayed report (same instant and content as
one already settled) restates nothing, as it settles nothing.

### 3. The statement is reconciled against the settled book and is never a balance

After the report's fills have settled, the plane compares the statement with
its own books for that cell, per instrument: the cell's quantity summed over
its venues against the strategies' lots summed over strategies. Every
instrument on which the two differ — including one present on only one
side — is a disagreement, recorded on the `AdvisoryBook`, returned on
`CellIngestion::advisory_disagreements`, and written by
`Platform::ingest_cell_report` to the capture problems. **Neither side is
corrected by the other.** The settled book stays the truth for every balance
and every limit; the statement is the cell's claim, and a claim the centre
cannot reproduce from the fills it was sent is what a reconciler reads
next. Nothing that decides reads the statement: not the risk aggregate, not
the Hedge Brain, not a gate.

### 4. What is deliberately not done

- **No overlay into the Hedge Brain's state.** The lane first wrote one and
  removed it: settled fills already charge the aggregate, so an overlay
  would count the same exposure twice, and a position the centre never
  settled would enter a plan on the cell's word alone.
- **No price on the wire** (above).
- **No cursor on the cell**, for ADR 0207's reason.
- **No automatic correction, halt or withdrawal** on a disagreement. A
  disagreement is recorded; what to do about it is an operator's reading,
  and a control keyed on the cell's own claim would be a control the cell
  can drive.

## Consequences

- The centre can now say, per cell and per instrument, whether the cell's
  own book and the centre's settled book agree, as of a journal sequence,
  and refuses a statement that went backwards.
- A delta grows by up to 64 positions; the frame bound and the inbox bound
  are unchanged.
- R12-MESH-7143's end-to-end suite — a broken two-leg cycle whose first leg
  filled and whose offset did not, leaving a position the cell states and
  the centre cannot settle — runs on the production types in
  `qip-acceptance`, and the row's gap is corrected twice over: the fills
  were always the path to the risk aggregate, and the statement is the
  check on them. The suite also records what it found rather than what it
  was asked to prove: a refused leg fails `Cell::work`, `qip-edge-node`'s
  scheduler keeps no `WorkReport` for a failed pass and resets on a halted
  one, so the orders sent before the break never ride the delta, the fill
  that follows is an `UnsentFill` break, nothing settles and nothing is
  charged. The statement makes that visible as a disagreement of exactly the
  leftover leg. "Hedged end to end" is therefore not asserted; carrying a
  failed pass's orders on the wire is the next decision, not this one.
- The row stays PARTIAL on bar part 2 for the same reasons as ADR 0207:
  the node publishes over a mesh the API binds no port for on Cloud Run,
  and no node is deployed.

## What it costs

- Up to 64 `AdvisoryPosition` values on every delta while the cell holds
  that many non-zero positions — an object id, a venue and a decimal each,
  of the order of a few kilobytes against the 64 fills the delta already
  bounds. The frame bound and the inbox bound do not move.
- At the centre, one `AdvisoryBook` per cell that has ever stated one, at
  most 64 positions each, and one pass over that cell's strategy books per
  statement, on the drain thread, not on any order path.
- Three defaulted fields on three records, one field on `CellIngestion`,
  two accessors on `CentralPlane`, and a capture-problem line per
  disagreement.
- No dependency, no journal topic, no deployment change.

## Verification

- `qip-kernel` `central::plane::tests::a_cells_stated_book_is_reconciled_replaced_by_a_newer_one_and_refused_when_it_goes_backwards`:
  a statement the centre settled nothing of raises one disagreement; a newer
  flat statement replaces it and raises none; a statement that went
  backwards is refused whole and writes nothing, not even `last_heard`.
- `qip-mesh` `delta::tests::a_delta_written_before_positions_existed_decodes_as_having_stated_none`.
- `qip-acceptance/tests/cycle_unwind_hedge.rs::a_broken_cycle_is_unwound_and_hedged_end_to_end`:
  a two-leg cycle on a catalogue instrument whose second leg the venue
  refuses, whose first leg filled and whose offset was sent and did not
  fill; the cell journals the offset as `Decision::Hedged` and halts; the
  leftover leg rides the cell's delta as a statement with a non-zero
  sequence, through the mesh codec into the platform; the centre records
  an `UnsentFill` break, settles nothing, raises one disagreement naming
  the stated quantity against a settled book of zero, and refuses a
  statement at an older sequence. Mutations, each restored byte for byte:
  `state_delta` shipping no positions fails at the delta assertion
  (`delta.positions.len() == 1`); the comparison reading the stated side
  twice fails at "the centre did not learn the leg the cell holds".

## What would make this wrong

A reader that *acts* on the statement — a hedge sized from it, a limit
charged with it, a venue withdrawn on it — would make a cell's own claim an
input to a control, which is the second source of truth the boundary rule
forbids. If the centre ever needs to act on a cell's book rather than on
the fills it was sent, the fills are what must be made reliable, not the
statement promoted.
