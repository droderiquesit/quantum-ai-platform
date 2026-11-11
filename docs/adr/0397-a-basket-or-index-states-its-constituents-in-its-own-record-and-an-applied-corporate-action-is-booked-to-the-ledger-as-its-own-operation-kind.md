# ADR 0397: A basket or index states its constituents and their units in its own record and is valued from them; an applied corporate action is booked to the ledger as its own operation kind, against the venue, from the action's own figures

- **Status**: **Accepted 2026-11-11 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Batch l6-d2-01 writes
  it. The limits of that delegation hold:
  - it sets no figure, limit or threshold;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency and no crate edge;
  - the new ledger kind carries a fact (a split ratio, a dividend per
    share), never a modelled figure, so ADR 0153 decisions 3 and 7 are
    unaffected and their three declined kinds stay declined.
- **Date**: 2026-11-11
- **Extends**: ADR 0123 (the ledger operation contract). It amends no clause
  of any accepted ADR.
- **Rows**: R12-COVERAGE-5099.
- **Correction to the claim**: the index row was claimed saying "against the
  accrual account". The body decides the venue account, for the reason in
  decision 4, and the index row is corrected in the same commit.

## Context

R12-COVERAGE-5099 stopped on two gaps.

1. An index held a constituent count and a divisor, and a basket held
   underlying ids with no weights. Nothing could value either from what it
   holds, or say what one unit is exposed to.
2. `Platform::apply_corporate_action` adjusted the lot and journalled the
   action in the event log. No ledger operation was queued, so the
   double-entry ledger of record never saw a split's unit change or a
   dividend's cash.

## Decision

1. **Constituents are data, stated in units.** `qip-financial` gains
   `Constituent { object_id, units }`. `IndexDetails` and
   `StructuredProductDetails` each gain `constituents`, empty by default so
   existing records still decode. Units, not percentages: a percentage needs
   the price it was struck at, and that price is not in the record.
2. **Valued, reported, sized on by nothing.**
   `qip_kernel::financing::FinancingRegister` values each `Index` as
   `Σ(units × price) / divisor` and each `SyntheticBasket` as
   `Σ(units × price)`, and holds the per-constituent contribution as the
   look-through exposure of one unit. The composite's own quote stays its
   mark. The UNDERSTAND stage reports the count and raises each unvalued
   composite as a problem.
3. **Refused, not guessed.** A record stating no constituents, a constituent
   named twice, non-positive units, a member that is absent, unpriced or in
   another currency, and a non-positive divisor are each a refusal naming
   the record to fix.
4. **A corporate action is its own ledger kind.**
   `OperationKind::CorporateAction { book, venue, action, unit, amount,
   phase }` posts Dr trading, Cr venue on Establish and the reverse on
   Unwind. The contra is the venue, as a fill's is. It is not an accrual:
   `LedgerObligations` reads a credit balance on an accrual account as cash
   owed, and a dividend received would have been withheld from survival
   sizing as a debt.
5. **What the kernel books.** A cash dividend books `quantity × amount` of
   the quote unit. A split books the units it adds, `quantity × factor −
   quantity`, in the instrument's own unit; a reverse split books the units
   it removes as an Unwind. Cost is not moved: the book holds it unchanged.
6. **Once, across restarts.** The operation is journalled as
   `CorporateActionBooked` under its own producer before it is queued, and
   re-queued from the log at resume. Its id is
   `<chain>:corporate-action:<the action's idempotency key>`, so a resend
   is a duplicate at the ledger. The re-fed tape does not book it again,
   because the book's own `actions_taken` already refuses the action.
7. **A refusal does not undo the book.** The lot has moved and the action is
   journalled before the operation is built. An operation the contract or
   the log will not take is a problem on the cycle and is not queued.

## What changes and what does not

- `qip-contracts`: one operation kind, its posting pair and its check. The
  crate still holds data types and their own validation only.
- `qip-financial`: one struct, two fields.
- `qip-kernel`: `value_composite`, `book_corporate_action`,
  `requeue_corporate_actions`, one record type.
- `qip-ledgerd` is unchanged: it books any admitted operation's event.
- No limit, gate, or paper-trading layer changes.

## What it costs

A composite's stated units go stale at a rebalance, and nothing here reads a
rebalance. The valuation is then of the old composition, which is why it is
reported beside the quote and decides nothing.

## What would make this wrong

- A sizing or hedging path starts to read the look-through exposure. That
  needs a rebalance feed first, and its own decision.
- A dividend is paid in a currency other than the desk's quote unit. The
  kernel credits cash in the quote unit today; the operation follows it.

## Evidence

- `backend/crates/libs/qip-contracts/src/ledger_operation.rs`
  (`OperationKind::CorporateAction`);
  `backend/crates/libs/qip-contracts/tests/ledger_operation.rs`
  `every_operation_kind_posts_to_its_own_subledgers_and_closes_in_every_unit`.
- `backend/crates/libs/qip-financial/src/extensions.rs` (`Constituent`).
- `backend/crates/runtime/qip-kernel/src/financing.rs` (`value_composite`).
- `backend/crates/runtime/qip-kernel/src/platform.rs`
  (`Platform::book_corporate_action`, `requeue_corporate_actions`).
- `backend/crates/runtime/qip-kernel/tests/financing.rs`
  `an_index_and_a_basket_are_valued_from_their_stated_constituents_and_one_that_states_none_is_refused`.
- `backend/crates/runtime/qip-kernel/tests/ledger.rs`
  `an_applied_split_and_dividend_are_queued_for_the_ledger_once_and_survive_a_restart`.
