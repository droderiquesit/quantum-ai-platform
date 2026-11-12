# ADR 0398: A repo is lent only under the treasury's recorded permission; its near leg is the fill, its far leg is its own ledger operation at maturity, and the borrowing arm is declined

- **Status**: **Accepted 2026-11-12 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Session fx-a-01 writes
  it. The limits of that delegation hold:
  - it sets no figure, limit or threshold: the rate, the maturity and the
    counterparty are the instrument's own record, and the permission is a
    person's to declare;
  - it touches no paper-trading layer and adds no order path: it adds
    refusals in front of the existing one, and the repo still fills only at
    the simulated venue;
  - it adds no dependency and no crate edge (`qip-kernel` already depends on
    `qip-treasury` and `qip-contracts`);
  - it reverses no accepted refusal: ADR 0153 decisions 3 and 7 stand, and
    their declined kinds stay declined.
- **Date**: 2026-11-12
- **Extends**: ADR 0123 (the ledger operation contract) with one kind, and
  ADR 0388 (the kernel treasury) with one question it answers. It amends no
  clause of an accepted ADR.
- **Row**: R12-COVERAGE-5100.

## Context

A repo was an instrument that assembled, was valued per unit of its
collateral, and filled at the simulated venue like any priced object.
Nothing asked for a permission, nothing repaid the cash lent, and
`RepoDetails::is_reverse` was read by nothing when an order was placed.

A first draft of this ADR (session l6-d3-01, branch
`ccr-116bbb27-osu8n8-l6-d3-01`, head `da1f2194`) built the gate and the two
legs and was reverted from integration before it reached `main`. It failed
`qip-acceptance`'s
`financing_permissions::no_production_source_outside_the_gate_names_a_financing_source_or_function`
with "a financing seam outside qip_treasury::funding::plan_funding:
[crates/runtime/qip-kernel/src/platform.rs names FinancingFunction::]". That
guard (R12-GOV-4077) holds that regulated financing is asked in one place.
The draft had two defects, and the second was the one that mattered:

1. **It named a financing function in the kernel.** `platform.rs` spelled
   `FinancingFunction::Repo` to ask the registry, which is the shape of seam
   the guard exists to stop: a place that reaches the regulated functions
   without going through the files that own the gate.
2. **It held a second permission registry.** `Platform` gained its own
   `FinancingPermissions`, and its own declared entity and jurisdiction,
   beside the one the kernel's `Treasury` already holds (ADR 0388). Two
   registries answer "may the platform finance with this counterparty"
   twice. An operator who records a permission in one leaves the other
   answering the old question, and DECIDE's funding and a repo order would
   disagree about the same counterparty with nothing to say which is right.

Making the guard stop seeing `platform.rs` would have hidden both. It is not
done.

## Decision

1. **One registry.** The repo permission is the kernel `Treasury`'s, the
   same `FinancingPermissions` its `fund` hands `plan_funding`.
   `Treasury::require_repo(counterparty)` asks it under the treasury's own
   entity and jurisdiction. `Platform::declare_treasury` replaces the
   treasury's declaration whole (floor, leverage, permissions), through
   `Treasury::declared`, so the funding plan and every repo order read the
   same declaration. Nothing in the tree declares one, so every repo order is
   refused until a person does.
2. **The function is named only inside the gate.**
   `FinancingPermissions::require_repo(entity, counterparty, jurisdiction)`
   lives in `qip-treasury/src/treasury.rs`, beside the registry, one of the
   guard's two `GATE_FILES`. No production file outside them names
   `FinancingFunction::` or `FundingSource::Financing`.
3. **Lending draws on no financing source, so it does not go through
   `plan_funding`.** A reverse repo spends the desk's own book, which the
   central treasury already funded (`queue_treasury_funding`). The guard's
   seam is where a financing source is drawn on; a lend asks the permission
   and draws on none. A repo the desk would *borrow* under is a financing
   source (`FundingSource::Financing` with the repo function at the
   counterparty), and when one is ever built it is drawn on through
   `plan_funding` and nowhere else.
4. **A repo order is refused, before an order object exists and again at the
   one point every submission passes, when**: the treasury holds no repo
   permission for the repo's own counterparty; the record says the desk
   borrows (`is_reverse` false); the rate is negative or not a number; the
   repo is at or past its maturity; or the order sells more units than the
   book holds.
5. **The fill is the near leg and is not restated.** One unit of a repo is
   the lender's claim on one unit of its collateral. The cash lent is what
   the fill paid and the claim is the unit the book holds.
6. **The far leg is its own operation kind, `OperationKind::RepoMaturity`.**
   At or after the stated maturity the kernel repays every unit the book
   holds: the units leave the book for the venue they were bought at, and the
   counterparty repays principal and interest in the desk's quote unit as two
   postings. The principal is the book's own cost of the units. The interest
   is `principal × repo_rate × days / 365.25` over the whole days from the
   latest purchase to maturity, the convention the valuation already uses.
   Both cash postings are against `venue:<counterparty>`, never an accrual,
   because `LedgerObligations` reads a credit on an accrual as cash owed.
7. **The whole operation is journalled before the book moves** (ADR 0153
   decision 1). A restart retires the units from the record and re-queues
   the operation as written; a record whose price does not reproduce its
   principal and interest, or whose units the rebuilt book does not hold,
   stops assembly.
8. **The borrowing arm is declined, with its reason.** A borrowing delivers
   collateral, and ADR 0153 decision 3 books a pledge only from a venue
   statement; no simulated venue states one. A haircut is a term of the
   contract, not a venue's statement that collateral moved, so it does not
   lift that decline. `OperationKind::CollateralPledge` keeps no producer.
   Collateral received under a lend is the counterparty's asset held as
   security and is not booked.

## What it costs

- The repo permission is read under the central treasury's entity
  (`central`) and its jurisdiction (`home`), which ADR 0388 chose for the
  treasury and which are the platform's own names, not a legal entity or a
  licensed jurisdiction. A desk that finances as another entity needs that
  entity declared on the treasury, not a second registry beside it.
- `declare_treasury` replaces the whole declaration, so declaring a repo
  permission without restating a floor or a ceiling clears them. A partial
  update would be a second way to read the declaration.
- No composition root calls `declare_treasury`, so no deployed binary places
  a repo order.
- The haircut does not bound what is lent. Interest counts from the latest
  purchase, so a desk that adds to a repo under-earns on the earlier units
  and never over-earns. `RepoDetails` states no day count.
- The resume refusals of decision 7 are reached only by a log the book is not
  the fold of; no test tampers a hash-chained log to reach them.

## What would make this wrong

- A simulated venue that states a pledge: decision 8 is revisited under
  ADR 0153 decision 3, and the borrowing arm draws through `plan_funding`.
- A second registry anywhere: the property is one registry, and
  `financing_permissions` sees only the naming half of it.

## Evidence

- `qip-kernel/tests/financing.rs`:
  `a_repo_order_is_refused_unless_the_treasury_holds_the_repo_permission_for_its_own_counterparty`,
  `a_repo_the_desk_would_borrow_under_is_refused_even_with_the_permission`,
  `selling_more_of_a_repo_than_the_book_holds_is_refused_as_a_borrowing`,
  `a_repo_whose_rate_is_negative_or_not_a_number_is_refused_an_order`,
  `a_repo_at_or_past_its_maturity_is_refused_an_order`,
  `a_permitted_reverse_repo_fills_as_its_near_leg_and_nothing_is_repaid_before_maturity`,
  `a_held_repo_is_repaid_once_at_maturity_with_principal_and_interest_and_not_again_after_a_restart`.
- `qip-contracts/tests/ledger_operation.rs::a_repo_maturity_retires_the_units_and_repays_principal_and_interest_apart_and_never_through_an_accrual`.
- `qip-acceptance/tests/financing_permissions.rs` and
  `ledger_declined_kinds.rs`, unchanged and passing.
