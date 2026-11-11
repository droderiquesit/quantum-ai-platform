# ADR 0387: What is pledged has one source, the ledger's collateral balance, and the posting book is retired from composition until a pledge record has a producer

- **Status**: **Accepted 2026-11-11 under the owner's 2026-11-11 instruction
  to the orchestrator** that every in-repository design decision be made so
  that no register row waits on one. Batch l1-d1-01 writes it on the
  orchestrator's ruling: one claim on what is pledged; the ledger's posting
  records are the source; `PostingBook` is derived from them or retired. The
  limits of that instruction hold:
  - it sets no figure and no haircut;
  - it touches no paper-trading layer;
  - it adds no producer of a ledger operation, and so reverses no refusal of
    an accepted ADR;
  - it deletes no test and no refusal.
- **Date**: 2026-11-11
- **Extends**: ADR 0153 decisions 3 and 7 (margin and collateral post only
  from a venue-stated requirement; the kinds stay bookable with no
  producer). ADR 0153's file is not edited.
- **Rows**: R12-CAPITAL-4080.

## Context

Two things in the tree can say what is pledged.

- **The ledger.** A `CollateralPledge { Establish }` operation debits the
  collateral account and an `Unwind` credits it. `qip-ledgerd` folds the
  balances into `LedgerObligations`; `qip-api`'s cycle reads that at the
  cycle instant and hands it to `Platform::observe_ledger_obligations`; the
  survival waterfall withholds the collateral figure
  (`unsettled_cash_and_pledged_collateral_on_the_ledger_lower_what_survival_releases`,
  and `qip-api/tests/ledger_obligations.rs` in a binary).
- **`qip_capital::bank::PostingBook`** (CAPITAL-011). Collateral is
  encumbered from the instant a posting is recorded until the venue's
  release confirmation arrives; a recall request frees nothing. Its
  `pledges()` is shaped to feed `CollateralGraph::build`. Nothing constructs
  one outside `qip-capital`'s own tests.

R12-CAPITAL-4080 stayed PARTIAL on the second. Lane L07 declined on
2026-10-18 to give it a kernel-side producer, because a book the kernel fed
beside the ledger would be a second claim about the same collateral.

Deriving the book from the ledger's records was the other option, and it
meets a fact: **the ledger has no pledge records to derive from, by
decision.** ADR 0153 decision 3 says a `CollateralPledge` is produced only
from a venue statement line that states one, that no simulated venue states
one, and so no producer is written. `ledger_declined_kinds.rs` fails on any
production construction of the kind. A derivation would read an empty
stream in every running process, and the only way to give it input would be
the model-derived stand-in ADR 0153 refuses.

## Decision

1. **One source.** What is pledged is the ledger's collateral balance, read
   through `LedgerObligations`. Nothing else may state it.

2. **`PostingBook` is retired from composition.** No production source
   outside `qip-capital/src/bank.rs` may name it. It is not deleted: it is a
   tested statement of the encumbrance rule, deleting it would delete tests
   and refusals this record has no authority to delete, and the rule is the
   one a future producer must keep.

3. **CAPITAL-011's instant-of-posting encumbrance is met by the ledger's
   reading, for every pledge the ledger holds.** A pledge is in the
   collateral account from the instant its `Establish` is recorded until an
   `Unwind` is recorded. The contract has no recall-requested operation, so
   nothing short of a recorded return reduces the balance: a request cannot
   free collateral because a request cannot be booked.

4. **The derivation is declared absent, with its reason.** A `PostingBook`
   folded from ledger pledge records has no input in the tree. When a venue
   statement first states a pledge, ADR 0153 decision 3 gives that venue a
   producer; the derivation belongs to that slice and to a new ADR, which
   must say how a per-posting book is rebuilt from balance-moving records
   (the ledger's `Unwind` may return part of a pledge; `PostingBook` releases
   a posting whole).

5. **The collateral graph the kernel builds each cycle is unchanged.**
   `cross_margin::review` builds it from venue statements and risk
   aggregates. That is what a venue says it holds, not what the ledger says
   was pledged, and the two are reconciled in the survival waterfall, where
   `CombinedObligations` takes both.

## What holds it

`qip-acceptance/tests/pledge_source.rs`, a new suite:

- `no_production_source_outside_the_bank_module_names_the_posting_book`;
- `the_ledgers_obligations_are_still_the_reading_the_kernel_and_the_api_cycle_take`,
  so the first cannot pass on a tree where nothing reads a pledge.

## What it costs

- `PostingBook` remains in the tree as code no process runs. A reader meets
  a type with tests and no caller and must read its module note to learn
  that is deliberate. That is the cost of not deleting tests.
- The per-posting view (which obligation a posting backs, and whether a
  recall is outstanding) exists nowhere in a running process. The ledger
  states an amount per book, counterparty and unit, and that is all a
  running platform knows about a pledge.
- `qip-fastbrain` and `qip-deepbrain` do not read the ledger's obligations;
  only `qip-api`'s cycle does. This record does not change that.

## What would make this wrong

- If an operator needs to see an outstanding recall before the venue
  confirms it, the ledger's contract needs a state it does not have, and
  decision 3's "a request cannot be booked" stops being a virtue.
- If a second reader of pledges appears that cannot use a balance (for
  example a per-obligation margin call), it needs the derivation in decision
  4, not a second book.
