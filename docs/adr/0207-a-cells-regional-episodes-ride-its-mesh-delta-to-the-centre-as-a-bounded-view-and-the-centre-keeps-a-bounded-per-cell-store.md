# ADR 0207: A cell's regional episodes ride its mesh delta to the centre as a bounded view, and the centre keeps a bounded per-cell store

- **Status**: **Accepted**, 2026-10-31, decided under the owner's 2026-10-31
  delegation ("complete all sessions ... finish what you can by morning",
  every remaining product decision delegated to the parent lane, which
  assigned this row to lane L5b-mesh). The delegation authorises the wire
  and store shape below. It does not authorise a live order, a financial
  limit or a weakened guard, and none is made: nothing here decides anything,
  and every bound is a refusal to grow.
- **Date**: 2026-10-31
- **Extends**: ADR 0116 (the mesh delta is the cell-to-centre wire; the
  reflex loop reaches the peer wire only through the port), ADR 0149
  decision 17 (an added field is defaulted so a sealed record replays),
  ADR 0079 (the centre's per-cell facts are bounded by the cells that have
  ever reported).
- **Keeps**: ADR 0003 (paper trading), ADR 0109 (no dependency is added),
  the delta as a *view* of the cell rather than a queue drained on send, and
  `Cell::record_episode` private: only the cell classifies what the cell saw.

## Context

R12-REFLEX-6012 asks that the regional episodes a cell classifies — an
execution report it cannot book, a reconciliation break, an order completed
late, a fill at a price other than its limit — reach the centre, so a region
can be asked not only what it holds but what it has seen go wrong. Since
EXPAND-009 `qip_edge::cell::Cell` records these into a bounded ring
(`MAX_RETAINED_EPISODES`, 128) and `Cell::episodes` returns them, and nothing
off the cell read them: no field of `CellStateDelta` carried them, the
centre's `CellReport` had no place for them, and `CentralPlane` had no
store. The row's gap said the carry was a wire change that needed its own
record, and this is it.

Two properties of the existing wire shape the decision:

1. **A delta is a view, not a drain.** `Cell::state_delta(&self, report,
   at)` takes `&self` on purpose: the transport is at-least-once, and a
   delta rebuilt after a failed attempt must be the same fact. So the cell
   cannot remember what it has sent, and whatever rides the delta rides it
   again on the next one.
2. **Every list on the delta is bounded with its omitted count beside it**
   (`refusals`, `fills`, `crosses`), because the far inbox is bounded by
   frames rather than bytes, and a truncation nobody can see is worse than a
   smaller list.

## Decision

### 1. The delta carries the newest of the cell's ring, bounded, with the omitted count

`CellStateDelta` (edge), `WireDelta`/`CellInterval` (`qip-mesh`) and
`CellReport` (kernel) gain `episodes: Vec<RegionalEpisode>` and, on the
wire halves, `episodes_omitted: u32`, each `#[serde(default)]` so a delta or
report sealed before the field replays as a cell that reported none —
decision 17 of ADR 0149, applied to a record with no digest. `state_delta`
ships the ring as it stands and `bound_refusals` keeps the **newest**
`qip_contracts::wire::MAX_EPISODES_PER_DELTA` (32), counting the older ones
it dropped: unlike the other lists, which keep the first and count the rest,
an episode's age is exactly how many deltas the centre has already had to
hear it, so the newest are the ones worth the bytes.

### 2. The centre merges, deduplicates and bounds per cell

`CentralPlane::ingest` merges the report's episodes into a per-cell store
(`CentralPlane::episodes(cell)`, oldest first), appending each one the store
does not already hold and dropping the oldest past
`MAX_EPISODES_PER_CELL` (256, twice the cell's ring, so the centre remembers
what a cell has let go of). Equality is the episode's own — cell, region,
kind, both instants and the detail — which is what makes a view repeated on
every delta, and a redelivered report, idempotent at the centre. The store
is bounded by the cells that have ever reported, like `last_heard`, and is
**read by nothing that decides**: it is a record for an operator and for the
LEARN stage's future readers, not an input to any gate.

### 3. What is deliberately not done

- **No cursor on the cell.** A "since the last delta" filter would make the
  delta depend on publish history and break the view property; the cost of
  the alternative is at most 32 repeated episodes per delta, each a short
  string, against lists already carrying 64 fills.
- **No new recording seam.** `Microstructure` and `VenueBehaviour` stay
  named and unrecorded; the cell's doc at `episodes()` says so and this
  record does not change it. A kind nothing records is not a kind the wire
  invents.
- **No journal topic.** The centre's store is not the journal; the cell's
  own chain already holds every anomaly and break as the entries that
  caused them, and a reader who wants more than 256 reads it there.
- **No counters on the API status page** for episodes yet; the sink carries
  them and counts nothing, so the page says nothing false.

## Consequences

- A cell's anomalies, failures, latency spikes and fill slippage reach the
  centre on the next delta after they are recorded, and the centre can
  answer "what has this region seen go wrong" from memory.
- A delta grows by up to 32 episodes when the ring is full; the frame bound
  and the inbox bound are unchanged.
- The row stays PARTIAL on bar part 2 until an execution node is deployed:
  the cell records on the node's pass and the node publishes over a mesh
  the API binds no port for on Cloud Run (`manifest_wiring.rs`,
  `THE_MESH_HAS_NO_PORT_ON_CLOUD_RUN`). Everything in the tree runs on the
  shipped path once both exist.

## What it costs

- Up to 32 `RegionalEpisode` values on every delta a cell publishes while
  its ring holds that many — each a cell id, a region, a kind, two instants
  and a sentence, so on the order of a few kilobytes against the 64 fills
  and 64 crosses the delta already bounds. The frame bound and the inbox
  bound do not move.
- At the centre, at most 256 episodes per cell that has ever reported, and
  a `contains` scan over that store per episode received: 32 × 256
  comparisons on the worst delta, on the drain thread, not on any order
  path.
- One more field on three records (`CellStateDelta`, `WireDelta`,
  `CellReport`), each defaulted, so nothing sealed before it changes
  meaning; and one more accessor on `CentralPlane` that nothing decides on.
- No dependency, no journal topic, no deployment change.

## Verification

- `qip-acceptance/tests/regional_episodes.rs::an_episode_recorded_at_a_cell_reaches_the_central_store`:
  a cell that cannot book an execution report records an anomaly, its
  `state_delta` carries it, the mesh codec decodes it, and `CentralPlane`
  holds it. Mutations: `state_delta` shipping an empty list fails at "the
  delta is not the cell's view"; `ingest` not storing fails at the end.
- `qip-api/tests/mesh.rs::a_cells_regional_episode_crosses_the_wire_into_the_central_store`:
  the same over a real socket and the drain. Mutation: the sink dropping
  `with_episodes` leaves the store empty.
- `qip-kernel` `central::plane::tests::an_episode_a_cell_reports_is_held_once_and_the_store_is_bounded_per_cell`:
  the dedup and the bound. Mutations: the `contains` check removed stores
  two; the bound at `>=` holds one fewer.
- `qip-mesh` `delta::tests::a_delta_written_before_episodes_existed_decodes_as_having_reported_none`.

## What would make this wrong

A reader of the store that *decides* — a gate, a sizing rule, a withdrawal
— would turn a bounded operator record with a dedup-by-equality into a
control whose inputs a cell's own classification can fill. If that is ever
wanted, the episodes need a signed, sequenced carriage of their own, not
this one.
