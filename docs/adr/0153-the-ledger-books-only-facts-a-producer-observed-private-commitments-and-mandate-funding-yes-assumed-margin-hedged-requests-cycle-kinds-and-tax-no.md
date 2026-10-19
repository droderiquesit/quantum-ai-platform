# ADR 0153: The ledger books only facts a producer observed — private commitments and mandate funding yes; assumed margin, hedged requests, cycle kinds and tax no

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). It decides which events book, and which never will; it admits
  no crate, creates no billed resource, sets no limit value and touches no
  protected path.
- **Date**: 2026-10-19
- **Extends**: ADR 0123 (non-fill operations reach `qip-ledgerd` as a P1
  topic, booked once by operation id), ADR 0130 (settlement and custody
  truth), ADR 0100 §5 (the P1 outcome row).
- **Keeps**: ADR 0003 (paper only), ADR 0007 (attribution), ADR 0100 §5's six
  outcome kinds — this record declines to widen them.

## Context

`qip_contracts::ledger_operation::OperationKind` can book a treasury
allocation, a capital grant, custody, reservations, physical inventory,
collateral, margin, transfers, commitments, tax and a mandate subscription.
Of these, ADR 0123 gave the kernel two producers: `DeskTreasuryFunded` and
`CapitalGrantFunded`, each a journalled record on
`Topic::LedgerOperationRecorded`, queued in `Platform::desk_operations` and
shipped by `qip-desk-ship`. Eleven open rows ask for more producers. They are
not eleven wiring jobs. Each asks *which event is the source of truth for the
fact*, and for several the honest answer is that no event is.

The retriage was wrong about one cost. It said a commitment producer needs a
new `qip-events` topic. It does not: `DeskTreasuryFunded` and
`CapitalGrantFunded` already ride `Topic::LedgerOperationRecorded`, and a new
record body on that topic needs no topic, retention class or stream.

The standing hazard is the one this repository names in its first principles:
a ledger that books an *assumption* presents it as a *fact* to every reader
that trusts the ledger. `review_cross_margin` runs `MarginModel::default`,
whose own comment records that the repository has never been shown a margin
agreement; its figures are a model's, and posting them as a `MarginRequirement`
would turn a guess into a booked obligation that `qip-capital`'s
`LedgerObligations` and the survival governor then read back as truth.

Locate the pieces with
`grep -n 'fn private_holdings_of\|struct DeskTreasuryFunded\|struct CapitalGrantFunded\|fn review_cross_margin\|pub fn fund_user' backend/crates/runtime/qip-kernel/src/platform.rs`,
`grep -n 'pub enum OperationKind' backend/crates/libs/qip-contracts/src/ledger_operation.rs` and
`grep -n 'fn is_outcome' backend/crates/apps/qip-edge-node/src/event_fabric/writer.rs`.

## Decision

1. **A fact books only when something observed it, and the producer journals
   the whole `LedgerOperation`.** Every new producer follows
   `CapitalGrantFunded`: the operation itself is journalled before it is
   queued, not the facts it was built from, so a restart re-sends the bytes the
   first process sent and the ledger never sees a second digest under one id.
2. **Private commitments book at assembly, `Commit` stage only.** The
   observer is the universe record: `private_holdings_of` already builds a
   `Commitment` from each `PrivateAsset` extension. For each, a new
   once-only record (`PrivateCommitmentMade`, wrapping the operation, on
   `Topic::LedgerOperationRecorded`) is journalled and queued as
   `OperationKind::Commitment { stage: Commit }`, operation id
   `<origin>:commitment:<commitment id>`, counterparty the fund's object id.
   `recorded_at` is the instant of the first assembly that journalled it,
   read back on restart, not the universe record's `updated_at` (which
   moves). A universe record whose amount later differs from the journalled
   one is **not rebooked**: the kernel reports it as a problem on the pass
   naming the commitment, and the original booking stands. `Draw` and
   `Release` are not produced: no capital-call observer exists in the tree,
   and a draw inferred from a cash-flow schedule is a forecast, not an event.
3. **Margin and collateral post only from a venue-stated requirement.** The
   figures `MarginModel::default` produces are advice to a desk and are never
   posted. A `MarginRequirement` or `CollateralPledge` operation is produced
   only from a venue statement line that states it. No simulated venue states
   one today, so no producer is written now; the rule is recorded so that the
   first venue that does gets a producer and not a model-derived stand-in.
   (This is why LEDGER-5009 stays PARTIAL on a stated reason.)
4. **A user's mandate funding is a `MandateSubscription`, not a transfer.**
   `Platform::fund_user` queues one `MandateSubscription { Establish }`
   (treasury ← the user's mandate) as a journalled operation on the first
   funding of a mandate, once, keyed by mandate. The treasury → book leg is
   the grant that already exists. `TransferEnd` gains no mandate arm: a
   transfer is a movement between books, venues and custodies, and the real
   ones originate in `qip-capital-fabric`, not in the kernel.
5. **A `Hedged` outcome books nothing.** `Decision::Hedged` states that a
   hedge was placed; the hedge leg's cash and position arrive as their own
   `Decision::Filled`. Posting both double-books the leg, so
   `LedgerStore::apply` keeps admitting `Filled` only for fills, and the
   register's clause on "ledger posting for hedges" is met by the leg's fill.
6. **The P1 outcome row is not widened for cycle kinds.** ADR 0100 §5 keeps
   its six kinds; `is_outcome` keeps returning `false` for `CycleCommitted`.
   "Every leg of a cycle filled" is a property of the booked fills, proven by
   an acceptance test over them (the cycle's cash nets; dropping one leg's
   posting breaks it), not a ledger record the cell would have to tally. The
   cell keeps no per-cycle fill tally and gains none.
7. **No `TaxCharge` is produced.** Tax method, rate and jurisdiction are legal
   facts this repository has never been shown, and a paper platform realises
   nothing taxable. An operator-declared tax policy would be an invented
   input until a person with the facts declares it. The operation kind stays
   bookable and tested; it has no producer, and the register says so.
8. **Commerce inventory reconciles against the marketplace, not a warehouse.**
   No warehouse keeps a record in the tree and none is built. An open
   `ReconciliationBreak` on a unit **holds the unit**: `CommercePlane` refuses
   to list or sell it until the break is cleared by a recorded correction on
   one side, so a break is acted on by refusing, not by guessing. The venue is
   not held. Chargeback risk stays `Absent` with its reason: the plan terms
   carry no settlement term to derive a rate from.
9. **Physical inventory books only where the commerce plane runs.** A paper
   platform holds none. `PhysicalInventory` is produced, as a journalled
   operation, only by the commerce research plane's receipt and sale, and only
   when that plane is installed; enabling it in a deployment is a tfvars
   switch (PROTECTED, below), not a code change here.

## Consequences

- The ledger's non-fill content becomes: treasury funding, grants, mandate
  subscriptions and private commitments. Everything else is bookable and
  has no producer *by decision*, and a reader can tell "not built because no
  event exists" from "not built yet".
- A restated commitment amount is a visible problem rather than a silent
  second booking or a silent overwrite.
- `R12-LEDGER-1164` closes only on three things this record settles
  separately: commitments (decision 2), margin/collateral (decision 3: rule
  only, no venue states one) and tax lots/tax (decision 7: not built). It
  stays PARTIAL with those named, which is more honest than its current
  "depends on three rows".

## What it costs

- Margin and collateral stay absent from the ledger until a venue states
  them. Survival's collateral read keeps reading nothing, which is true.
- A commitment draw must be recorded by hand when a capital call is real;
  nothing infers it.
- Cycle completion is a property proven in a test, not a record an operator
  can query. If operators need the latter, it is a read model over fills.
- Holding a unit on an open break stops a commerce experiment until the break
  is corrected; that is the intended price.

## What would make this wrong

- If a venue starts stating margin (a real or simulated statement carrying a
  requirement line), decision 3 is exercised, not wrong; if instead someone
  proposes posting `MarginModel` output "temporarily", the harm in the
  Context is the reason to refuse.
- If a capital-call notice becomes a universe record, decision 2's "no draw"
  becomes a missing producer and a follow-on record should add it.
- If operators need a queryable "cycle completed" fact, decision 6 is
  superseded by a read model, not by widening P1.

## Dissent

*Post the default margin figures; a figure the desk can see beats a blank.*
Strongest form: the survival governor and `LedgerObligations` currently read
no collateral at all, and an approximate obligation is closer to the truth
than zero. It lost because the ledger is the platform's authority and a
modelled number there is read as booked; a blank is reported as a blank, and
`review_cross_margin` already shows the desk the modelled figure *as a model*.
It would be revived by a margin agreement actually shown to the repository.

## Code it constrains

- `private_holdings_of` and `Platform::desk_operations` (a new
  `PrivateCommitmentMade` body beside `DeskTreasuryFunded`).
- `Platform::fund_user` (queues the `MandateSubscription` once).
- `Platform::review_cross_margin` (stays advisory; posts nothing).
- `LedgerStore::apply` in `qip-ledgerd/src/store.rs` (keeps `Filled` only).
- `is_outcome` in `qip-edge-node/src/event_fabric/writer.rs` (unchanged,
  pinned by a test).
- `CommercePlane::reconcile` and `CommerceResearch::simulate` (hold on break).

## Rows unblocked

- R12-LEDGER-5006, R12-LEDGER-5020 — decision 2: the commitment producer,
  stage `Commit`, no new topic.
- R12-LEDGER-5009 — decision 3: venue-stated only; no producer today, wording
  restated.
- R12-LEDGER-5010 — decision 4: `MandateSubscription` from `fund_user`.
- R12-LEDGER-10114 — decision 5: `Hedged` books nothing; the leg's fill does.
- R12-MESH-5066 — decision 6: P1 not widened; cycle net-zero is a test.
- R12-LEDGER-5019 — decision 7: no tax producer; wording restated.
- R12-LEDGER-5026, R12-COMMERCE-5090 — decision 8: hold the unit on a break;
  chargeback stays `Absent`.
- R12-COVERAGE-7164 — decisions 2, 3, 4, 9: the four named facts each have a
  stated source or a stated absence.
- R12-LEDGER-1164 — restated blockers, stays PARTIAL.

## Not decided here

- **PROTECTED** — enabling `QIP_DEEPBRAIN_BREADTH_PLAN` in a deployment
  (`infrastructure/environments/*/terraform.tfvars`), which decision 9 needs
  for any physical-inventory booking to run (R12-LEDGER-5026,
  R12-COVERAGE-7164).
- Nothing here is BILLED, RISK or paper-boundary: no limit value, no live
  path, no resource.
