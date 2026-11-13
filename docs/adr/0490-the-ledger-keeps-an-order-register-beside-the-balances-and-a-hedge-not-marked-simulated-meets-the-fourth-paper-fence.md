# ADR 0490: The ledger keeps an order register beside the balances, opened by every intent and hedge on P1 and moved by its fills and withdrawals in the batch that books them; an intent or hedge not marked simulated meets the fourth paper fence

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**
  (in-repository design decisions), session t50-32 (wave580). It tightens:
  it adds a refusal and a memo, removes none, and weakens no paper-trading
  layer. Nothing here reaches a venue.
- **Date**: 2026-11-13
- **Reads**: ADR 0100 §4 and §5 (the ledger consumer and the P1 outcomes
  row), ADR 0003 (paper trading), ADR 0440 (the pending-settlement row this
  mirrors), ADR 0454 (a fill's decision sequence).
- **Rows**: R12-LEDGER-10114 (stays PARTIAL; the gap is named below).

## Context

R12-LEDGER-10114 asks that every intent, fill, hedge and outcome of a
multi-region cycle be journaled to Fabric and ledgered centrally, and that
replay evaluate missed or failed cycles. Its rationale is one sentence:
"Unledgered hedges are positions the books do not know."

Before this ADR the journaling half held and the ledgering half held for
fills only. `qip-edge-node`'s `is_outcome` already puts `OrderSent`,
`Hedged`, `Filled`, `OrderExpired`, `MassCancelled` and `VenueWithdrawn` on
P1, and `qip-ledgerd`'s `LedgerStore::apply` booked `Filled` and `Settled`
and advanced past everything else writing nothing but the chain record. A
hedge the cell had sent and the venue had not yet filled therefore existed
nowhere in the central books — exactly the rationale's failure. And the
fourth paper fence (a record not marked simulated parks its cell) applied
to fills and settlements only: a `Hedged` or `OrderSent` record carrying
`simulated: false` was advanced past like noise.

## Decision

1. **An order register beside the balances.** `LedgerStore` keeps one
   `OrderRow` per order under `order/<cell>/<session>/<order>`. `OrderSent`
   opens a row of kind `intent`; `Hedged` opens a row of kind `hedge`,
   carrying the instrument. A `Filled` naming the order adds its quantity
   to `filled`, and the row becomes `filled` once `filled` reaches the
   quantity sent. `OrderExpired`, `MassCancelled` and `VenueWithdrawn`
   close an open row as `withdrawn` with the venue's answer. The session is
   in the key because the cell numbers orders from one at every start.
2. **Moved in the batch that books the record.** The row is written in the
   same `LedgerBatch` as the balances, chain tail, chain record and cursor,
   so an order and the fill that closes it are never committed apart, and
   a duplicate or a gap — which return before the batch — moves no row.
3. **A memo, not a posting.** The register moves no balance. A hedge's
   economic effect is its fills, which `post` already books; booking the
   request itself would invent a price the venue never gave. What the
   register adds is that the request is *known*: an open hedge row is a
   position the books know is being taken before any of it fills.
4. **Refused, never invented.** A quantity that is not a positive decimal
   parks the cell as an integrity break. An order id opened twice in one
   session parks the cell and keeps the first row, because overwriting it
   would forget fills already counted. A fill or withdrawal naming an order
   with no row writes no row: the fill is still booked to the balances, and
   a fill on an order the centre never saw sent is the central plane's
   `unsent_fill` finding, not a row for this register to make up.
5. **The fourth fence covers intents.** An `OrderSent` or `Hedged` record
   with `simulated: false` parks its cell under `ParkReason::LiveFill`,
   counts `qip_ledger_live_fill_refused_total`, and opens no row — the same
   treatment a live fill already received. This is a tightening of the
   paper boundary, not a fourth layer replacing any of the three named in
   `.claude/rules/01-security-and-safety.md`.

## Consequences

- `LedgerStore::orders()` answers "what did the cell send, what of it has
  filled, and what is still open", per cell and session, from the ledger
  alone.
- The register grows by one row per order, as the chain grows by one record
  per outcome; it is no new unbounded buffer, and the event log remains
  the record a refold can rebuild it from.
- A stream that previously advanced past a live intent now parks that
  cell. No producer in the tree should emit one: `Cell::send` refuses an
  order bound for a live-class venue under `GATE_LIVE_VENUE` before any
  intent or hedge is journaled.

## What this does not do (the gap, dated 2026-11-13)

- **No cycle identity.** P1 records carry no cycle id, so the register can
  say which orders and hedges a cell sent but not which multi-region cycle
  each belonged to. Grouping a cycle's legs across regions needs a cycle
  id stamped by the producer on `OrderSent` and `Hedged`, which is a
  journal-contract change for its own ADR.
- **No replay of missed or failed cycles.** Nothing evaluates a cycle the
  ledger saw only part of.
- **No node → broker → ledgerd test of a two-region cycle with a hedge.**
  The register is proven at the store's contract
  (`qip-ledgerd/tests/store.rs`), not end to end.

## What would make this wrong

A producer that legitimately reuses an order id within one session — a
replace that keeps its id, say. `OrderReplaced` is not read by the register
today; if a replace is ever journaled as a second `OrderSent` under the same
id, decision 4 parks the cell and this ADR must be revisited rather than
the check removed.
