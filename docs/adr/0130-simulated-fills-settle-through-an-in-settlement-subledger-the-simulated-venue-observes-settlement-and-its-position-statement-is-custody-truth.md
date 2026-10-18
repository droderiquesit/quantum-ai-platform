# ADR 0130: Simulated fills settle through an in-settlement subledger, the simulated venue observes settlement, and its position statement is custody truth

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). Paper only: every new record is
  produced by the simulated venue or the ledger, no live variant is added,
  and the ledger's paper fences are unchanged.
- **Date**: 2026-10-18
- **Extends**: ADR 0105 (the ledger deduplicates fills by chain continuity), ADR 0123 (non-fill operations
  on the P1 outcomes stream), ADR 0129 decision 5 (the chain join).
- **Keeps**: ADR 0003, ADR 0007 (attribution reconciles exactly), ADR 0021
  (live capital is refused), ADR 0109.

## Context

`qip-ledgerd` posts a simulated fill straight to cash. The `Decision::Filled`
arm of `LedgerStore`'s apply path (`qip-ledgerd/src/store.rs`) refuses a live
fill as live, then books the fill's units and cash against
`trading:<cell>/<strategy>` and `venue:<venue>` at once. Settlement is
modelled only in the cell: `qip-edge/src/settlement.rs` holds
`SettlementConvention` (`T0`, `T1`, `T2`) and `SettlementTerms` per venue and
refuses a leg funded by proceeds that will not have settled. So the ledger
says cash is available on the trade date while the cell's own gate says it is
not, and five rows wait on the difference:

- R12-LEDGER-5011: a fill carries its settlement state through to settlement.
- R12-LEDGER-5014: intent → order → fill → settlement → position/cash,
  walkable in both directions. `qip_contracts::ledger::SourceEvent` names
  cell, session, journal sequence and digest, and order id; it has no intent
  and there is no settlement record.
- R12-LEDGER-1197: fills flow to ledger, treasury, settlement and inventory.
- R12-LEDGER-10185 and 5017: reconciliation against venue or custody
  statements, and custody subledger postings from a production producer.
  `Account::Custody` and `OperationKind::CustodyMovement` exist; no producer
  posts them, because a paper platform has no custodian.

What the paper platform does have is a venue that keeps exact books. The
node's simulated gateway implements `qip_brokers::adapter::VenueAdapter`,
which reports `PositionSnapshot`s, and `qip_brokers::ledger` keeps positions,
cash and margin "exactly" on `qip_portfolio::Portfolio`'s accounting
identity.

Locate them with
`grep -n 'Decision::Filled' backend/crates/apps/qip-ledgerd/src/store.rs`,
`grep -n 'pub enum SettlementConvention\|pub struct SettlementTerms' backend/crates/edge/qip-edge/src/settlement.rs`,
`grep -n 'pub struct SourceEvent' backend/crates/libs/qip-contracts/src/ledger.rs` and
`grep -n 'use qip_brokers::adapter' backend/crates/apps/qip-edge-node/src/gateway.rs`.

## Decision

1. **A simulated fill posts into an in-settlement subledger, not to cash.**
   The fill's cash leg is booked against a new `in_settlement:<cell>/<strategy>`
   account (Dr or Cr by side), with the settlement date computed from the
   venue's `SettlementTerms` at the fill's instant and carried on the
   posting. Units move to the trading account at the trade date as today:
   the position is the strategy's from execution, and only the cash is
   pending. A venue whose terms are absent posts to cash as today and the
   posting says so, matching the cell's own refusal to read missing terms as
   instant or as T+2.
2. **Settlement is observed by the simulated venue, not computed by the
   ledger.** The node's simulated gateway, which already owns the fill, emits
   a `Decision::Settled { order_id, settlement_date, amount }` outcome when its
   simulated settlement clock passes a fill's date, on the P1 outcomes stream
   beside the fill. The ledger books it — Dr `trading`, Cr `in_settlement`,
   or the reverse — once, keyed on the fill's order id and the settlement
   date. A ledger that computed settlement itself would book a fact nobody
   observed; the venue is the only party that can say a settlement happened,
   even simulated.
3. **The walk is exact in both directions** (R12-LEDGER-5014).
   `SourceEvent` gains the cell's `ChainOpened` journal sequence (the intent:
   the signals that opened the chain, ADR 0129), and the settlement posting
   names the fill's ledger event. Intent → order → fill → settlement → cash is
   then four field joins, and the reverse walk uses the same fields.
4. **Custody truth is the simulated venue's position statement**
   (R12-LEDGER-10185, 5017). The node's gateway publishes its
   `PositionSnapshot`s as a periodic `custody_statement` record on the P1
   stream; the ledger posts the venue-held units to `custody:<venue>` as
   `CustodyMovement` operations (ADR 0123's path) when the statement moves,
   and compares the statement with its own trading balances on each one. A
   disagreement is a reconciliation break, journaled and counted under a
   bounded `direction` label, never an adjustment. The record names it
   plainly: on a paper platform the venue is also the custodian, and
   pretending to a separate custodian would be inventing a party.
5. **Inventory stays where its only observer is** (R12-LEDGER-1197's last
   word). `commerce_research.rs::CommerceResearch::plane_apply` under
   `QIP_DEEPBRAIN_BREADTH_PLAN` is the one producer that observes inventory;
   it posts through ADR 0123's operation path when it lands, and this record
   adds no second source.
6. **Every new record is paper by construction.** `Decision::Settled` and the
   custody statement carry the same `simulated` flag the fill does, and the
   ledger refuses either as live exactly as it refuses a live fill.

## Consequences

- The ledger's available cash agrees with the cell's settlement gate, and a
  strategy's book shows pending cash as pending.
- Code slices left: the `in_settlement` account and the fill posting change
  with a seeded test that cash is unavailable before the date (LEDGER-5011);
  `Decision::Settled` from the gateway and its booking, with an end-to-end
  test that a T+2 fill reaches cash on the second simulated settlement day
  (LEDGER-1197, 5011); the `SourceEvent` field and the acceptance walk in
  `qip-acceptance/tests/vertical_slice_correlation.rs` (LEDGER-5014); the
  custody statement, its postings and a seeded-break test in
  `qip-ledgerd/tests/store.rs` (LEDGER-10185, 5017).

## What it costs

- Every fill becomes two postings separated in time, and the ledger's trial
  balance carries an in-settlement balance that must reach zero for every
  settled fill; a lost `Settled` record leaves cash pending for ever, which
  is visible but is a new failure mode.
- The simulated venue grows a settlement clock and a statement publisher; the
  venue simulation is more code to keep honest.
- `SourceEvent` changes shape on a persisted, hash-chained record; the change
  is additive and old events read the field as absent, which every reader
  must honour.

## What would make this wrong

- If a live-capital record (ADR 0108) were ever accepted, the custodian is a
  real party and decision 4's "the venue is the custodian" is false on day
  one; custody truth would then be the custodian's statement and this record
  would need replacing, not extending.
- If settlement failures (a fail-to-deliver) needed modelling, a
  `Settled`-only lifecycle cannot express them; the gateway would need a
  `SettlementFailed` arm and the ledger a reversal path.

## Rows

R12-LEDGER-5011, R12-LEDGER-5014, R12-LEDGER-1197, R12-LEDGER-10185,
R12-LEDGER-5017. Each stays PARTIAL until its slice lands with a mutated test.
