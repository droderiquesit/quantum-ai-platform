# 0080 — An orphaned position is a derived state, owned by its retired strategy, and unwound reduce-only by its cell

**Status:** *proposed*, 2026-09-19. Decided by the architecture lane; awaiting
implementation across the crates named under "The change, crate by crate".

**Relates to:** blueprint §35.1 (the `Orphaned` row), §35.2 (question one),
§35.3 (unwind ordering), §27.1 and §27.2 (an unwind passes the netting seam),
§36.2 (the payload); ADR 0007 (exact attribution), ADR 0008 (no order waits
on the centre), ADR 0062 and ADR 0073 (the policy wire may subtract and never
add), ADR 0075 (`issue` refuses a strategy at a non-capital-holding rung).

**Does not touch:** any risk limit, any capital envelope's bounds, any
autonomy ceiling, `AutonomyController`, and the paper-trading boundary in all
three layers. The one order this record lets a cell send is one that
*reduces* a lot the platform already holds, and the sign check that makes it
so is the decision.

---

## Context

`docs/DELIVERY-STATUS.md` scores §35.1 `PARTIAL` on one of six states.
`Orphaned` "still has no non-test writer, and deliberately so", because the
only fact that produces it — a retirement — lives at the centre, whose
position book is `StrategyLot` and not `qip_portfolio::Position`. The row's
last sentence is this record's brief: "closing it means giving the centre a
position book or carrying the disposition to a cell, and that is an ADR rather
than a wire."

Every premise of that row was re-verified. `StrategyLot` is average-cost and
keyed `(cell, strategy, instrument)`
(`grep -n 'pub struct StrategyLot\|pub fn strategy_books' backend/crates/runtime/qip-kernel/src/central/plane.rs`).
`disposition_for` and `scheduled_unwinds` derive a retired strategy's
remaining lots from the ledger and the books on every call, refuse when a
cell's reported book disagrees with the attribution, and journal a
`RetirementDisposition` with an idempotency key of one per strategy
(`grep -n 'fn disposition_for\|pub fn scheduled_unwinds\|struct RetirementDisposition\|struct DispositionRefused' backend/crates/runtime/qip-kernel/src/central/learning.rs`).
`DispositionInstruction` has one arm, `Unwind { flatten_by }`, and its module
doc says why handover is absent: the centre records no thesis shared between
two strategies. `qip_portfolio::PositionLifecycle::Orphaned` exists, is
reachable only from `Flagged` and `Unwinding`, exits only to `Closed`, and
its doc says "found in a reconciliation break rather than opened
deliberately" — a different meaning from §35.1's "its strategy retired but the
position remains"
(`grep -n 'Orphaned' backend/crates/libs/qip-portfolio/src/lifecycle.rs`).
`qip-edge` does not depend on `qip-portfolio`
(`grep -n 'qip-portfolio' backend/crates/edge/qip-edge/Cargo.toml` prints
nothing).

Two findings were not in the brief. **First, nothing carries the disposition
to the cell.** The record is journaled at the centre and the module doc calls
it "a flatten intent for the owning cell's own DECIDE/ACT path", but no
payload slot, grant, recall or topic delivers it, so §35.2's "never orphaned
silently" is met at the record and unmet at the book: the lot is listed and
stays open. **Second, no cell consumes a recall at all.** `RecallOrder`
already carries `RecallReason::StrategyDemoted` and
`requires_immediate_flatten`
(`grep -n 'pub enum RecallReason' -A 12 backend/crates/services/qip-capital/src/recall.rs`),
which is precisely the shape "flatten on the centre's instruction", and
`grep -rn 'Recall' --include=*.rs backend/crates/edge/ backend/crates/apps/qip-edge-node/`
prints nothing. A flattening recall is therefore a control that cannot fire
at the cell for *any* reason today, not only retirement. This record does not
close that; it names it so the carrier chosen below is not mistaken for the
recall path being fixed.

§35.1 and §35.2 say two things about an orphan that read as one and are two:
the lifecycle table lists `Orphaned` as a state, and §35.2 says "a position
with no owner is a reconciliation break, not a normal state". Both are kept,
and the seam between them is what this record decides.

## Decision

**One. At the centre, `Orphaned` is a derived state and never a stored one.**

An orphaned position is a `StrategyLot` with non-zero quantity whose strategy
stands at `GateStage::Retired`. `scheduled_unwinds` *is* that definition, read
from the ledger and the books on every call, and nothing stores an "orphaned"
flag beside them — the stored facts are the retirement in the ledger and the
lot in the books, both replayed from the event log, and the
`RetirementDisposition` event is the record of what was instructed at the
instant of retirement. The two blueprint sentences are reconciled by which
record exists: an orphan with a named disposition is a *listed* state, and an
orphan whose disposition the centre could not name is `DispositionRefused`,
which is §35.2's reconciliation break. A lot in neither list is not an orphan;
it is a lot.

**Two. Three kinds of ownership, kept apart by name.**

- **Attribution owner: the retired strategy, permanently.** The lot stays
  keyed by the strategy that opened it, and every fill that unwinds it is
  attributed to that strategy through the same contributor vector as every
  other fill. A "house" or "orphan" bucket is refused: it is unattributed P&L
  wearing a name, the thing ADR 0007 exists to forbid, and a retired strategy's
  record is not complete until its last lot is flat.
- **Execution owner: the cell whose book holds the lot**, through its own
  DECIDE/ACT path and sized against its own ladder (§35.1: "sized and ordered
  against the liquidity ladder"; ADR 0008: nothing on the hot path waits on
  the centre). The centre never submits the order and has no ladder to size
  one on.
- **Record owner: the centre**, which journals the disposition and lists the
  orphan until the books say it is flat.

**No re-parenting.** `DispositionInstruction` keeps its one arm. A `Reassign`
arm is admissible only once the ledger records a shared-thesis fact between
two strategies — the condition `learning.rs`'s module doc already states —
and even then a lot is re-keyed only by a journaled event naming both
strategies and the thesis, never by the successor's first fill. An owner
chosen on anything less is an owner picked to make the record look complete.

**Three. The disposition reaches the cell as a thirteenth slot on the signed,
sequenced policy payload.**

`PolicyPayload` gains `dispositions: Slot<Dispositions>`, where `Dispositions`
is `BTreeMap<StrategyId, BTreeMap<String, Decimal>>` — strategy, instrument,
signed `flatten_by` — which is exactly `scheduled_unwinds`' shape filtered to
the one cell the payload is for. It is produced in `qip_api::mesh::pending_policy`
from `CentralPlane::scheduled_unwinds`, beside `grant_manifests`
(`grep -n 'pub fn pending_policy\|grant_manifests(' backend/crates/apps/qip-api/src/mesh.rs`),
and it follows `withdrawn_venues`' wire discipline to the letter: absent from
the wire when unproduced or empty, so every payload signed before the field
keeps its digest and its signature, and present only when there is something
to say — with the same deploy-order consequence, **cells upgrade before the
centre**, because `PolicyPayload` is `deny_unknown_fields` and an old cell
refuses the whole payload the first time the slot is present. `Slot<T>`
serialises `{value, produced_at}` under `deny_unknown_fields` of its own, so
"absent when unproduced" is a `skip_serializing_if` on the payload field and
not a change to `Slot`. A `PolicyItem` arm is added for it so `narrowing`
enumerates thirteen; its capability mapping is `None`, because a stale
instruction to reduce is still an instruction to reduce and narrows nothing
that was not already narrowed.

This is a stated deviation from §36.2's twelve-row table: one row added. The
register's §36.2 row is not moved by this record; the lane that adds the slot
amends it.

**Four. The cell applies a disposition reduce-only, against its own book, and
refuses any other reading.**

On each pass, for each `(strategy, instrument, flatten_by)` in the applied
slot, the cell reads the lot *it* holds for that strategy in that instrument.
It refuses — under one bounded gate literal, with the refusal riding the delta
so the centre's two claims are seen to disagree — when it holds nothing, or
when `flatten_by`'s sign would increase the lot or carry it through flat. That
is the same discipline `disposition_for` applies at the centre when a reported
book disagrees with the attribution, applied at the other end of the wire.
Otherwise it builds a directional `Intent` for the retired strategy, sized to
the smaller of `|flatten_by|` and `|lot|` and then to what the ladder says is
reachable this pass, and takes it through the same gates every intent passes
— book presence, staleness, venue status, price, pricing policy — with two
steps deliberately skipped: the capital envelope and the region hold. It is
the one intent in the cell that passes with no envelope, and it is safe only
because the sign check makes it structurally reduce-only: it commits no new
notional and can only lower gross.

The intent enters `net()` as `Nettable`, so it can cross internally against a
peer strategy's opposite intent under §27.1's cap — the cheapest exit there is
— and its fill is attributed to the retired strategy like any other.

**Five. Why no envelope, and why the wire may carry this at all.** ADR 0075:
`issue` refuses a strategy at any rung that does not hold capital, so a
retired strategy can never again receive an envelope, and the only order the
platform can ever again send under its name is one that reduces its lot. The
policy wire's rule (ADR 0062, ADR 0073) is that it may subtract and never add,
and this slot keeps it: the worst a forged or replayed payload can do with a
disposition is close a position the platform holds — a loss of edge, never a
widening of risk — and a replayed one at or below the applied sequence is
refused by `apply_policy` before it is read.

**Six. Completion is the books, and replay re-derives everything.** There is
no completion event. The fills ride the delta, `settle` moves the lot, the
lot leaves `scheduled_unwinds` by the arithmetic that closed it, and the next
payload no longer names it. A replay of the event log — the retirement, the
disposition, the payloads, the fills — re-derives the orphan's appearance and
disappearance without a second record to drift from.

**Seven. `qip_portfolio::PositionLifecycle::Orphaned` is pinned to §35.1's
meaning, and its writer is not invented.** Its doc is corrected to "its
strategy retired while the position remained open". The transition table
stands as written and is now argued rather than inherited: `Held → Flagged`
at the demotion the monitor makes before any retirement — the ledger retires
only a strategy "pushed off capital and still decaying", so no retirement
lands on a `Held` position; `Flagged → Orphaned` at the retirement;
`Unwinding → Orphaned` for a retirement landing mid-unwind; `Orphaned →
Closed` at flat. There is no `Orphaned → Unwinding` because the orphan's
unwind is the disposition itself. Its production writer is the seam that
holds a `Portfolio` of `Position`s beside a retirement; none exists, this
record does not invent one, and the register's row keeps saying so. Should
`qip-edge` ever hold `Position`s, the only admissible writer is the slot's
application in Decision four.

## The change, crate by crate

- **`qip-contracts` (lib):** `Dispositions`, `PolicyPayload::dispositions`,
  the `PolicyItem` arm. No new dependency.
- **`qip-kernel` (runtime):** nothing new to derive — `scheduled_unwinds`
  exists; at most a per-cell view of it for the producer.
- **`qip-api` (app):** `pending_policy` produces the slot.
- **`qip-edge` (edge):** `apply_policy` stores the applied slot; `Cell::work`
  builds reduce-only intents from it before the strategy loop and after the
  halt gates; one refusal literal; a `WorkReport` line per disposition acted
  on or refused.
- **`qip-portfolio` (lib):** the doc correction in Decision seven, and
  nothing else.

**Dependency direction.** `qip-contracts` ← `qip-kernel` ← `qip-api`;
`qip-contracts` ← `qip-edge`. No lib gains a dependency on a service; no
service depends on the runtime; nothing depends on an app. `qip-edge` does not
gain `qip-portfolio`.

## What it costs

- **Thirteen slots, not twelve**, and the cells-before-centre skew that every
  addition to this payload carries.
- **The unwind runs at the cell's pace**, bounded by its ladder, so a large
  orphan takes passes to flatten and the centre lists it throughout. That is
  the listing working, not the unwind failing.
- **A lot the cell's book does not show is refused, not traded.** Where the
  centre's attribution and the cell's book disagree, nothing moves and both
  ends say so. That is deliberate and it is the same rule at both ends.
- **No handover.** A position whose thesis is genuinely shared with a funded
  strategy is flattened rather than re-parented, until the ledger can say
  the thesis is shared.
- **A retired strategy's record stays open** until its last lot is flat,
  and its realised P&L keeps moving after retirement. That is attribution
  being exact, and a reader expecting a retired strategy's number to freeze
  should find this sentence.
- **The recall path stays unconsumed at the cell.** This record chose a
  carrier that exists end to end rather than fixing the one that does not,
  and says so.

## What would make this wrong

- A second writer of the flatten intent — any path building one from
  anything but the applied slot. Check
  `grep -rn 'flatten_by' --include=*.rs backend/crates/edge/` and read each
  hit for its source.
- The sign check removed or weakened: an intent from this path that can
  increase a lot or carry it through flat.
- Any other intent admitted with no envelope on the strength of this one:
  the exemption is for a structurally reduce-only intent and for nothing
  else.
- A strategy id meaning "house", "orphan" or "unattributed" anywhere in the
  books.
- `Orphaned` stored as a flag rather than derived from stage and lot.
- A `Reassign` arm with no ledger-recorded shared thesis behind it.

## Alternatives considered

**The recall channel, as a `RecallReason::StrategyDemoted` recall.** The
closest existing shape, and rejected on three facts: a recall names an
`envelope_signature` and a retired strategy holds no live envelope; `recall_for`
targets live envelopes only; and no cell consumes a recall. Building the
disposition on it would have meant building the recall consumer first and
then finding it had nothing to name. If a cell gains a recall consumer that
flattens on `requires_immediate_flatten`, the disposition may move to it —
and what must survive the move is Decision four's sign check and Decision
two's attribution, not the slot.

**Slot 7, `GrantManifest`.** Rejected by its own doc: "a manifest, not a
delivery path", kept free of grants themselves so as not to be a second
source of truth.

**A new signed topic on the downlink, beside grants and recalls.** Rejected:
a second sequence, replay and freshness discipline to re-derive, when the
payload already has all three and `apply_policy` already enforces them
atomically.

**The centre submits the flatten order.** Rejected by ADR 0008 — no order
from the centre, and the centre has no ladder to size one against.

**Give the centre a `qip_portfolio` position book.** Rejected: the centre's
book is `StrategyLot`, keyed by strategy because attribution is keyed by
strategy, and a second book beside it would be two claims about one fact.

**Leave the disposition as a journal record only.** Rejected: an orphan
nobody unwinds is §35.2's "never left dangling" unmet for ever, listed
honestly and acted on never.

**Write the lib's `Orphaned` from the backtester.** Rejected in the
register's own words: a trigger nobody can defend, which reads as a working
lifecycle and is not one.

## For the implementer

Tests worth writing first, each mutation-verified, each asserting its
premise before its property:

- A retired strategy with a long lot at one cell; the payload names the
  disposition; the cell emits one sell intent for that strategy of at most
  the lot; the fill comes back on the delta and the lot leaves
  `scheduled_unwinds`. Mutation: flip the sign in the cell's check and the
  test must fail on the *direction* of the intent, not on its absence.
- The same, with the cell holding nothing: one refusal under the literal, no
  intent, and the refusal on the delta. Mutation: remove the "holds nothing"
  arm.
- The same, with `flatten_by` larger than the lot: an intent of the lot's
  size and not the instruction's.
- A directional intent from any live strategy still refused without an
  envelope; the exemption reaches only the disposition path.
- A payload at or below the applied sequence naming a disposition: refused
  before it is read, as today.
