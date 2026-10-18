# ADR 0123: Non-fill ledger operations reach qip-ledgerd as a P1 topic on the outcomes stream, booked once by operation id

- **Status**: **Accepted**, 2026-10-17, under the owner's standing instruction
  in the session to make routine engineering decisions and record them
  ("Make the decision, record the rationale, and continue").
- **Date**: 2026-10-17
- **Extends**: ADR 0100 §5 (the P1 lane) and §8 (the ledger reads outcomes);
  ADR 0105 (the ledger consumer).
- **Keeps**: ADR 0003 (paper only), ADR 0007 (attribution), ADR 0109 (no new
  dependency).

## Context

`qip_contracts::ledger_operation::LedgerOperation` names every financial
fact the ledger is the authority for that is not a fill: treasury
allocation, capital-grant funding, custody, inventory reservation, physical
inventory, collateral, margin, transfers, tax lots, commitments, tax
charges and mandate subscriptions. `LedgerStore::record_operation` books
one, refuses a second digest under one id, and is tested
(`qip-ledgerd/tests/history.rs`). Nothing outside tests calls it: the
consumer decodes `reflex.outcome_recorded`, `reflex.chain_span` and
`event_fabric.gap`, and passes every other topic over, so in every real
binary the only financial facts that reach the authoritative ledger are
fills. R12-LEDGER-10179 ("every financial outcome is eventually committed
exactly once") is held PARTIAL on that clause alone, and the subledger rows
(LEDGER-5005/5006/5008-5010/5016-5021) wait on the same path.

## Decision

1. **One topic, `ledger.operation_recorded`, on the existing P1 outcomes
   stream** (`reflex-outcomes.local`). P1 because an operation is as
   irreplaceable as a fill: a funding lost on the way is cash the ledger
   never holds. The same stream, so the grants, the consumer group, the
   archive acknowledgement and the replay all already exist, and a desk
   that may produce outcomes to its own partition may produce operations
   there too. No new stream, identity or grant.
2. **The body is the `LedgerOperation` itself**, validated on decode exactly
   as on construction. Its idempotency key is its `operation_id`, which is
   unique across origins by construction (`<origin>:<kind>:<n>`), and the
   envelope's event id is derived from it, so a resend carries the id the
   first send did.
3. **The ledger books an operation and moves the partition's cursor in one
   commit** (`LedgerStore::apply_operation`), for the reason fills do: an
   offset durable before the postings it stands for is a crash that
   silently drops the operation. A repeat with the same digest is a
   duplicate and moves only the cursor.
4. **A refused operation does not stall the partition.** A conflicting digest
   under one id, an operation that would take a quantity-holding account
   below zero, or one whose event does not balance is journaled as a note
   naming the operation and the reason, counted, and the cursor advances.
   Stalling would hold every fill behind it; parking by cell does not apply,
   because an operation is not on a cell's chain. The refusal moves nothing
   and is on the record for an operator.
5. **The first producer is the desk.** Each desk (`desk`, `desk-api`,
   `desk-deep`, the per-process chains `PlatformConfig::desk_chain` names)
   records the treasury funding of its book —
   the platform's initial equity, Dr the desk's trading account, Cr the
   central treasury — as `TreasuryAllocation` operation
   `<chain>:treasury-funding:0`, queued at assembly and shipped by the same
   `qip-desk-ship` shipper after the chain records. Re-queued on every
   start and deduplicated by id, so no watermark is needed; a changed
   initial equity under the same id is refused by the ledger as a conflict,
   which is the correct answer to a desk that says its starting capital was
   two different things.
6. **The second producer is the capital grant (2026-10-18).** When two
   operators issue a grant through `Platform::issue_capital`, the kernel
   funds the grant's book — Dr `trading:<cell>/<strategy>`, Cr the central
   treasury, for the grant's deployable capital — as `CapitalGrantFunded`
   operation `<chain>:capital-grant-funded:<strategy>@<cell>@<granted-at>`,
   on the same desk operation path and the same shipper. The operation
   itself is journalled (`CapitalGrantFunded`, origin
   `kernel/capital-grant-funding`) and replayed as written on every start,
   because the plane's envelopes do not survive a restart and a funding
   rebuilt at the restart's clock would be a second digest under the same
   id. The §24.9/§27 `CapitalGrant` the operation carries is stated from the
   issued `CapitalEnvelope`: `Platform::fund_capital_grant`'s documentation
   names which fields the envelope states and how the three it does not
   (reserve, drawdown stop, ADV share) are written rather than guessed.

## Consequences

- The ledger's view of a desk's cash is its funding plus its fills, not its
  fills alone against an unfunded book.
- Every other operation kind now has a path; each needs a producer that
  observes the fact it records. Those are named per requirement row and
  are not implied by this record.
- A new topic is a contract change: `qip_events::Topic` gains a variant, the
  catalogue lists it on the outcomes stream, and the consumer's binding set
  grows by one.

## What it costs

- One more topic on the outcomes stream shares its P1 budget with the
  fills; a burst of operations can delay fills behind it on the same
  partition. Today the desk sends one record per start and one per
  capital grant issued.
- The schema lock samples one variant of `OperationKind`
  (`TreasuryAllocation`), and CONTRACT-047 allows exactly one row per topic
  and version, so a second variant cannot be locked beside it: the
  `CapitalGrantFunded` shape the grant producer sends (decision 6) travels
  unlocked, proven instead by `qip-contracts`' one-of-every-kind test and
  by the acceptance test that books it at a real `qip-ledgerd`. Locking
  every variant needs a lock keyed on more than the topic, which this
  record does not decide.
- A grant was funded when it was issued and never recalled, until
  Amendment 1 below (2026-10-18): the operation had no unwind, and nothing
  observed a grant's expiry as a fact to record, so a renewal of the same
  book was a second funding and the ledger's view of that book was every
  grant it was ever issued. The amendment is the recall; its own costs are
  listed with it.
- A refused operation is noted and passed, not halted on: a conflicting
  funding leaves the book unfunded at the ledger and is visible only as an
  `operation_refused` note, not as a stopped consumer.
- Each desk start re-sends its funding and every grant funding and recall
  its log holds; the ledger answers each with a duplicate, which is one
  wasted record per start plus one or two per grant ever issued on that
  log.

## What would make this wrong

- If operations needed an ordering the stream does not give them relative
  to fills (for example a transfer that must post before a fill it funds),
  a per-origin sequence check would be needed at the ledger. Today the only
  producer's operation precedes its fills on the same partition.
- If an operation refusal ever needed to stop the books rather than be
  noted, decision 4 would be the wrong trade.

## Amendment 1 (2026-10-18): a grant's funding is recalled when the grant stops being in force

**Status**: accepted under the same standing instruction as the record;
an edit to an accepted ADR, for the owner's review.

### Context

Decision 6 funded a grant's book and the cost note above said what it did
not do: nothing recalled the funding when the grant's window ended or the
centre withdrew it, and a book renewed hourly was funded again each hour.
The ledger's view of a renewed book was the sum of every grant it had ever
held, which is not a number the ledger is the authority for. The contract
had no way to say "recall the deployable capital of grant X":
`TreasuryAllocation`'s `Phase::Unwind` recalls an amount from a book
without naming the grant, and `CapitalGrantFunded` is admissible only
inside the grant's window, where a recall at expiry is recorded at or
after it.

### Decision

1. **One more kind, `OperationKind::CapitalGrantRecalled`**, in
   `qip_contracts::ledger_operation`: the funding's accounts swapped — Dr
   the treasury, Cr `trading:<cell>/<strategy>` — for the carried grant's
   own `deployable()`, so a funding and its recall net the book to zero by
   construction and no producer restates the amount. It carries a
   `CapitalRecallCause` of `expired`, `revoked` or `superseded`, and the
   contract refuses a cause the grant's own window contradicts: an expiry
   recorded while the grant is live, a revocation or supersession recorded
   at or after the expiry, or any recall before the grant was made. Not a
   `Phase` on the funding, for the window reason above.
2. **Three seams, one recall per grant, in the kernel.** The cycle that
   observes a funded grant's window has ended recalls it in the ACT stage,
   beside the darkness review and for the same reason (no report carries
   the fact; the passage of time makes it). A recall order the plane
   issues on a cell report (`CellIngestion::recalls`, a concentration
   finding) recalls the grant's funding on the same ingestion. A renewal
   through `Platform::issue_capital` recalls the funding the plane's
   envelope just replaced before it funds the new window, so the log reads
   recall then funding and the book's deployable capital at the ledger is
   the live grant's at every point. A recall order or a renewal that
   reaches a grant whose window has already ended records an expiry, which
   is what it is.
3. **The id is the grant's identity and nothing else**:
   `<chain>:capital-grant-recalled:<strategy>@<cell>@<granted-at>`. One
   grant has one recall whatever prompted it. The kernel holds the
   fundings no recall has answered (`capital_grant_fundings`), keyed by
   grant id; a funding leaves that set only once its recall is in the
   log, so a second expiry cycle, a second recall order and a restart all
   find nothing to recall again, and a recall the journal refused is
   attempted again rather than lost — at the next order, or at the
   expiry, which is the backstop the recall model already relies on. A
   journal failure at the revocation seam is a capture problem, not a
   refusal of the report, because the ingestion has already happened.
4. **Journalled whole and replayed as written** (`CapitalGrantRecalled`,
   origin `kernel/capital-grant-recall`), for the reason decision 6
   journals the funding: a restart holds no envelope and no clock that
   could reproduce the instant the recall was observed at, and a recall
   rebuilt at the restart's clock would be a second digest under the same
   id, which the ledger refuses. One walk of the log at assembly re-queues
   every funding and recall in the order written and rebuilds the
   outstanding set, so a grant that lapsed while the process was down is
   recalled on the first cycle that observes it, at that cycle's instant.
5. **The chain's `sequence` runs over fundings and recalls alike**, one
   counter, so two operations on one origin never share a sequence.

### What it costs

- A recall at expiry is recorded at the instant the cycle observed it, not
  at the expiry instant: a process that was down across an expiry records
  the recall when it next cycles. The ledger books when the centre knew,
  which is the honest instant; the grant's own `expires_at` is on the
  record beside it.
- A revocation is the centre's decision, recorded when the plane issues
  the recall order, not when the cell acknowledges it. The cell's residual
  exposure is bounded by the envelope it holds, as `qip_capital::recall`
  documents; the ledger's book reads the capital as withdrawn from the
  instant the centre withdrew it.
- The schema lock still samples one variant of `OperationKind`
  (CONTRACT-047, one row per topic and version), so `CapitalGrantRecalled`
  travels unlocked like `CapitalGrantFunded`, proven by `qip-contracts`'
  one-of-every-kind test, by the kernel's four recall tests in
  `tests/central.rs`, and by the acceptance test that books a funding and
  its recall at a real `qip-ledgerd` across a restart
  (`vertical_slice_ship::a_capital_grants_recall_returns_its_book_to_the_treasury_at_expiry_across_restarts`).
- One more record per grant on the P1 stream, and one more resend per
  grant per start.

### What would make this wrong

- If a grant could be partly recalled — a narrowing of a live grant
  without a renewal — the recall would need an amount of its own and the
  net-to-zero property would need a different proof. Today the plane
  narrows only at issue, and a narrower grant is a renewal.
- If the cell's acknowledgement, not the centre's order, were the fact the
  ledger should book for a revocation, the revocation seam would move to
  `RecallRegister::acknowledge` and carry the residual gross.
