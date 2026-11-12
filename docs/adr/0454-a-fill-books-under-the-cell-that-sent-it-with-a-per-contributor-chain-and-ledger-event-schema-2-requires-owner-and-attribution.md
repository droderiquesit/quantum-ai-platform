# ADR 0454: A fill books under the cell that sent it with a per-contributor attribution chain the cell journals; LedgerEvent schema 2 requires owner and attribution, and schema-1 events decode as legacy and are never rewritten

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  (decision 14, ADR_THEN_IMPLEMENT, where the skeptic's text governs)**,
  session t50-14 (wave580). It tightens: it adds refusals and a required
  shape, removes none, and touches no paper-trading layer. `Settlement`
  still has one variant, and nothing here reaches a venue.
- **Date**: 2026-11-12
- **Reads**: ADR 0100 §1, §8 and §9 (the posting contract and its paper
  fence), ADR 0153 decision 1 (a fact books only when something observed
  it), ADR 0084 (optional journal fields skipped when absent so sealed
  digests do not move).
- **Rows**: R12-CONTRACT-7069 (stays PARTIAL; the gap is named below).

## Context

R12-CONTRACT-7069 asks for a typed LedgerEvent between brains carrying the
immutable economic event, its debit and credit impacts, ownership,
settlement state and the full attribution chain, and verifies it by "a
LedgerEvent missing any required field is refused; round-trip preserves
every field". Before this ADR the event carried `owner` and `attribution` as
options, `Attribution` named one fill, order, intent and decision id each,
and no producer stated either on a fill.

The owner's ruling (decision 14) was that the chain is the producer's fact,
so the cell states it. The skeptic corrected three readings, and its text
governs:

1. No intent id or decision id exists anywhere in `qip-edge`. A
   `NetIntent` nets several contributors from different strategies, so one
   fill rests on many intents, and a single `intent_id` would pick one —
   the guess `ledger_operation.rs` already refuses for owners.
2. The ledger of record books fills in `qip-ledgerd` (`store.rs`,
   `PaperFill::try_from` then `post`); the kernel's `desk_fabric` only
   pre-checks the desk's own records.
3. `owner = SourceEvent.cell` conflicted with `with_owner`'s doc ("a desk or
   mandate id"), so owner semantics must be decided, not implied.

## Decision

1. **The decision is the cell's `OrderSent` entry; an intent is a
   contributor of it.** Nothing new is minted. `Decision::Filled` gains
   `decision_sequence: Option<u64>`, the sequence of the cell's own
   `OrderSent` entry for the order, on the same session's chain. The cell
   takes it from the entry `record_sent` journals and keeps it on the
   working order, so every fill of that order names it. A contributor's
   intent is identified by that decision and the strategy that stated it —
   `<cell>/<session>/<decision sequence>/<strategy>`, rendered by
   `LedgerEvent::intent_ids` — because that is exactly the record the
   intent was netted into and nothing else names it.

2. **Attribution is a per-contributor list matching the fill's shares.**
   `Attribution` is `{ order_id, decision_sequence, contributors }`, the
   contributors being strategy to share, `BTreeMap` for a stable order. The
   fill itself is the `SourceEvent` (cell, session, journal entry and
   digest), so no separate fill id is carried: a second id for the same
   fact is a second claim that can disagree. `with_attribution` refuses a
   blank or foreign order, a decision that does not precede the fill on the
   chain, no contributor, a blank strategy and a non-positive share.

3. **Owner is the book whose capital the event moved.** For a fill that is
   the cell that sent the order: its trading books are the only books a
   fill's postings move. For a treasury allocation or capital grant it stays
   the treasury, and for a mandate subscription the mandate, as
   `LedgerOperationFields::owner` already states. No strategy-to-mandate map
   is invented: no record assigns strategies to mandates.

4. **LedgerEvent schema 2.** `LedgerEvent` carries a `schema`. Schema 1 is
   every event written before this ADR; it is encoded without the field, so
   a stored schema-1 event decodes as legacy and re-encodes byte for byte
   and is never rewritten — decoding never upgrades. Schema 2 is built only
   by `LedgerEvent::attributed(owner, attribution)`, which beyond the
   refusals above refuses an event that posts to no trading book, a trading
   book of a cell other than the owner, and postings whose strategies are
   not exactly the chain's contributors. Decoding a schema-2 event without
   its owner or its chain is refused, not read as legacy, and an unknown
   schema is refused.

5. **`qip_portfolio::ledger::post` books a fill whose cell stated its
   decision at schema 2**, owned by the source cell with the chain built
   from the fill's own shares, and books one with no stated decision at
   schema 1. It does not rebuild a chain nobody stated.

6. **The journal is not version-bumped.** `decision_sequence` is optional
   and skipped when absent, the ADR 0084 pattern `side`, `quote_unit` and
   `fee` already follow, so every digest a cell sealed before this ADR
   stays where it was. An old entry decodes with none.

## What this does not yet do (the row's gap, dated 2026-11-12)

- **A new unattributed fill is not yet refused at `qip-ledgerd`'s posting
  seam.** The kernel desk's fills (`qip-kernel/src/desk_ledger.rs`) are
  sealed on a desk chain that has no `OrderSent` entry, so there is no
  decision for them to cite; refusing every fill without
  `decision_sequence` would stop the desk's book. The refusal lands when
  the desk journals its own decision entry and states its sequence, or when
  a follow-on ADR decides that chain's decision record.
- **Capital-operation events stay schema 1.** `LedgerOperationFields::to_event`
  states an owner for the four kinds with a single owner; the closure asked
  for an owner on every kind and an operation-shaped attribution (the
  operation id and origin), which this ADR does not decide.

## Consequences

- A fill a cell sends after this change books in `qip-ledgerd` at schema 2
  with every field the row names: the balanced postings (debit and credit
  impacts), the simulated settlement, the source fact, the owning cell, and
  the chain from fill to order to decision to each contributor's share.
- Every event `qip-ledgerd` already holds stays readable and unchanged.

## What it costs

- One more optional field on `Decision::Filled` and on `PaperFill`, and a
  `schema` field on `LedgerEvent`: every reader of a fill entry now meets a
  value it may ignore, and every writer of a fill literal must state it.
- Schema 2 events carry the contributor map twice in substance — once as
  the per-strategy postings and once as the chain — on purpose, so the two
  can be compared and a chain that does not describe its event is refused.
  That is bytes per fill in `qip-ledgerd`'s store.
- Two schemas are read forever: schema 1 is never rewritten, so no
  migration retires it, and every consumer of `LedgerEvent` must accept an
  event with no owner and no chain.
- Until the desk states its decision, the ledger holds attributed and
  unattributed fills side by side, and a report over owners must say which
  fills had none.

## What would make this wrong

A cell whose working orders survive a session restart would journal a fill
in a later session than its `OrderSent`; `decision_sequence` is read on the
source session's chain and would then cite the wrong entry. Today the
working set is in memory and does not cross a restart; if that changes, the
chain needs the decision's session too.
