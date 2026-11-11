# ADR 0398: A repo is lent only under a recorded permission; its near leg is the fill, its far leg is its own ledger operation at maturity, and the borrowing arm is declined

- **Status**: **Accepted 2026-11-11 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Session l6-d3-01 writes
  it. The limits of that delegation hold:
  - it sets no figure, limit or threshold: the rate, the maturity and the
    counterparty are the instrument's own record, and the entity and
    jurisdiction a desk finances as are an operator's to state;
  - it touches no paper-trading layer and adds no order path: it adds two
    refusals in front of the existing one;
  - it adds no dependency and no crate edge (`qip-kernel` already depends
    on `qip-capital`);
  - it reverses no accepted refusal: ADR 0153 decisions 3 and 7 stand and
    their three declined kinds stay declined.
- **Date**: 2026-11-11
- **Extends**: ADR 0123 (the ledger operation contract) with one kind. It
  amends no clause of an accepted ADR.
- **Row**: R12-COVERAGE-5100.

## Context

A repo was an instrument that assembled, was valued per unit of its
collateral, and filled at the simulated venue like any priced object. Three
things were missing, and the row stopped on the second.

1. Nothing asked for a permission. `FinancingFunction::Repo` was a permission
   with a counterparty and a jurisdiction that no order ever met.
2. Nothing settled the far leg. The fill paid cash for the units and the
   units stayed in the book and in the ledger for ever; the cash never came
   back. The row asked which operation kind books the legs, and whether a
   repo's own stated haircut lifts ADR 0153's decline of a collateral pledge.
3. `RepoDetails::is_reverse` said which way the desk faced, and nothing read
   it when an order was placed.

Two facts in the tree decide the shape.

- The simulated venue fills a repo as it fills anything: a buy pays the fill
  price and receives the unit. That is a lender's near leg exactly, and it is
  already booked, by the fill, in the ledger and in the book.
- `LedgerObligations` reads a credit on any `accrual:` account as cash the
  platform owes and withholds it from deployable capital. Interest income
  credited to an accrual would be withheld as a debt.

## Decision

1. **A repo order needs the recorded permission.** Before an order object
   exists, and again at the one point every submission passes, an order in a
   repo is refused unless an operator has declared the entity and
   jurisdiction the desk finances as (`Platform::authorise_financing`) and
   that entity holds `FinancingFunction::Repo` for the repo's own
   counterparty in that jurisdiction. Nothing is declared at assembly, so
   every repo order is refused until one is. No entity or jurisdiction is
   assumed: neither is stated anywhere in the tree.
2. **The fill is the near leg and is not restated.** One unit of a repo
   instrument is the lender's claim on one unit of its collateral. The cash
   lent is what the fill paid; the claim is the unit the book holds. No
   second operation books the opening, because the fill already has and two
   claims about one fact disagree.
3. **The far leg is its own operation kind, `OperationKind::RepoMaturity`.**
   At or after the repo's stated maturity the kernel repays every unit the
   book still holds: the units leave the book for the venue they were bought
   at, and the counterparty repays principal and interest in the desk's quote
   unit, as two postings so the money lent and the money earned stay apart.
   The principal is the book's own cost of the units. The interest is
   `principal × repo_rate × days / 365.25` over the whole days from the
   latest purchase to maturity, on the convention the valuation already
   accrues on. Counting from the latest purchase never credits a day the
   cash was not shown lent.
4. **Neither contra is an accrual, and nothing accrues in between.** Both
   cash postings are against `venue:<counterparty>`. Interest is booked when
   cash moves and not before, so no accrual balance exists for
   `LedgerObligations` to read either way.
5. **The whole operation is journalled before the book moves** (ADR 0153
   decision 1). The record carries the operation and the per-unit repurchase
   price; the book is the fold of it as it is of a fill, and a restart
   retires the units from the record and re-sends the operation as written.
   A record whose price does not reproduce its principal and interest, or
   whose units the rebuilt book does not hold, stops assembly.
6. **The borrowing arm is declined, with its reason.** An order in a repo
   whose record says the desk borrows (`is_reverse` false) is refused, and so
   is a sale of more units than the book holds. A borrowing delivers
   collateral, and ADR 0153 decision 3 books a pledge only from a venue
   statement that states it; no simulated venue states one. The repo's own
   haircut does not lift that: a haircut is a term of the contract, not a
   venue's statement that collateral moved. `OperationKind::CollateralPledge`
   keeps no producer.
7. **Collateral received under a lend is not booked.** It is the
   counterparty's asset held as security. Booked to the trading account it
   would be units the desk could sell without owning them.
8. **A repo that cannot be repaid stays held and is said on every cycle.** A
   rate that is not a number or is negative, and a repo already matured, are
   refused at the order; a repayment the contract or the log refuses is
   reported as a problem until it can be made.

## What changes and what does not

- `qip-contracts` gains one `OperationKind` variant with its postings and
  refusals. It is data and its own validation, as every other kind there is.
- `qip-kernel` gains the gate, the settlement at SENSE and the resume arm.
- The existing test that buys a repo now states a reverse repo and records
  the permission first. It asserts what it asserted before.
- The valuation (`FinancingRegister`) is unchanged. It still reports the cash
  a haircut implies and nothing sizes on it.

## What it costs

- No composition root calls `authorise_financing`, so no deployed binary
  will place a repo order. A permission is a legal fact with a sign-off; it
  is an operator's to record and is not invented here.
- The haircut does not bound what is lent. The principal is the fill's cash,
  and no check refuses a fill above the collateral's value less the haircut.
- Interest counts from the latest purchase, so a desk that adds to a repo
  under-earns on the earlier units. It never over-earns.
- `RepoDetails` states no day count. Interest uses the 365.25-day year the
  valuation uses.

## What would make this wrong

- A simulated venue that states a pledge. Then decision 6 is revisited under
  ADR 0153 decision 3, with a producer for that statement.
- A repo whose units are bought at more than one instant and must earn from
  each. Then the term is per lot and the book needs lots.
- A reader that treats `venue:<counterparty>` as a trading venue. The
  account is the outside party the cash is exchanged with, as a fill's is.

## Evidence

- `qip-contracts/tests/ledger_operation.rs::a_repo_maturity_retires_the_units_and_repays_principal_and_interest_apart_and_never_through_an_accrual`
  and `every_operation_kind_posts_to_its_own_subledgers_and_closes_in_every_unit`.
- `qip-kernel/tests/financing.rs::a_repo_is_lent_only_under_a_recorded_permission_and_is_repaid_once_at_maturity_across_a_restart`.
- `qip-kernel/tests/asset_classes.rs::a_repo_an_index_and_a_basket_assemble_trade_on_paper_and_are_refused_without_their_class`.
- `qip-acceptance/tests/ledger_declined_kinds.rs`, unchanged and passing.
