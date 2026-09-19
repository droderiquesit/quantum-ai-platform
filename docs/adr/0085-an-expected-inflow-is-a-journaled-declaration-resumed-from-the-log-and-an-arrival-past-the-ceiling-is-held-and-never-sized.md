# ADR 0085: An expected inflow is a journalled declaration resumed from the log, and an arrival past the ceiling is held and never sized

- **Status**: Accepted, under the authority the owner delegated on 2026-09-19
  to the lane deciding §40.12's "Add capital" writer and §43.2's capital-call
  writer. The safe half — declaration, cancellation, resume, the held bucket
  and the operator routes — is built by the same lane in the commits this
  record travels with. The posting half is **refused**, with its reversal
  condition named in §2.
- **Date**: 2026-09-19
- **Supersedes**: nothing. Corrects three priors the lane was briefed with:
  that a declaration needs an idempotency key on its reference (§1 says why it
  must not have one); that only an ineligible user's declaration may be
  refused (§3 refuses two more, and says why the declaration is where a
  refusal belongs); and that a §43.2 capital call is "a declared expected
  inflow with a consequence of failure" (§5: it is a demand *on* the desk,
  the opposite direction, and its own record family).
- **Related**: ADR 0021 (capital leaves this platform by no path), ADR 0065
  (a standing token attests nobody's presence), ADR 0075 (a capital route is
  authorised in shape and refused in fact until a per-person credential
  exists), ADR 0076 (what that credential is), ADR 0007 (the attribution the
  ledger books from).

## Context

Blueprint §40.12's "Add capital" flow runs `funding source → destination →
amount → review → instruction → expected inflow → detected → settled →
available`. The register's row for it was re-scored on 2026-09-19 and found
the tree holding the *read* side in production — `ExpectedInflowView` renders
every declared inflow on every balance — and no writer: `UserLedger::
expect_inflow` was reached by nothing outside its own crate's tests,
`UserLedger` had no `cancel_inflow` at all, and the lane sent to wire an
operator intake stopped, because `post_inflow` was a capital-arrival path that
asked none of the ceilings `fund` asks. `cash.rs`'s own header names five ways
a number may enter a user's book and says each asks "the mandate registry, the
eligibility registry, the investable ceiling or the exact-split rule";
`post_inflow` asked the registry for a currency and added the whole amount to
`settled`, the figure positions are sized against. An intake wired to it would
have been a second, weaker door beside `fund`. The row named three questions
this record answers, and one more from §43.2.

Every premise in that row was re-run in the tree this record was written
against, and each held, with one refinement worth stating: `grep -n 'fn
resume_' backend/crates/runtime/qip-kernel/src/platform.rs` returns three
functions and none is the ledger, exactly as the row says — but there is a
fourth resume seam in `qip-kernel/src/references.rs` (`resume_references`,
called from the constructor), so "three" is a count of one file and not of the
kernel. Two further facts the design turns on, both verified rather than
assumed: `Principal::authentication_instant` in `qip-api/src/auth.rs` refuses
every credential the API accepts, so every mutating operator route today is
refused in fact (`api_boundary.rs` walks the table and requires each to be
either presence-gated or named exempt); and `Platform::new` applies the
configuration's committed eligibilities *after* the struct is built, which is
the ordering §1's resume must respect and the first cut of it did not — a
replay placed before that loop ran against an empty registry and refused every
record it existed to keep. The restart test found it.

## Decision

### 1. A declaration lives in the event log and nowhere else, and the ledger is resumed from it

`LedgerEntry` — the kernel's journalled per-user ledger record, already
carrying `Funded`, `Booked` and `FundingRefused` on `Topic::AttributionCompleted`
under the producer `kernel/ledger` — gains `InflowExpected` and
`InflowCancelled`. Each carries the user, the strategy, the reference, the
amount, the instant and the operator's subject, taken from the
`OperatorIdentity` and never from a body. `Platform::expect_inflow` and
`Platform::cancel_inflow` keep the order `apply_eligibility` keeps: the ledger
is asked on a scratch copy, the record is journalled, the live ledger is
moved. The log therefore has the record before the state exists and never a
record of a state that did not.

`Platform::resume_ledger` replays both variants in log order into the ledger
the enrolments and the committed eligibilities have just built, through the
same `UserLedger` gates a live declaration passes. It is called *after* the
committed eligibilities are applied, for the reason the Context gives. A
record this boot's ledger refuses — a user the configuration no longer enrols,
a mandate that has shrunk under the declaration, an eligibility this
configuration does not grant — **stops assembly**, naming the record's
sequence and the remedy (restore the enrolment the declaration was admitted
under, or archive the log and start a new one). Skipping the record would be
the erasure this seam exists to end; resuming it past the gate would be a
document minting ledger state, which `cash.rs` closed by removing
`Deserialize` and which this record does not reopen.

**No idempotency key is put on the reference, and the prior that asked for one
was wrong.** The log's `holds_idempotent` guard is keyed on a fact that must
never be recorded twice. A declaration is not that: a reference declared,
cancelled and declared again for the wire that did come is three legitimate
records under one reference, and a key on the reference would refuse the third
— the exact re-declaration `cancel_inflow` exists to permit. The guard against
a duplicate is the ledger's own rule, asked on the scratch copy before anything
is journalled, and the replay reproduces the sequence rather than a set.

**What is deliberately not resumed, and the cost of that.** `Funded`, `Booked`
and `FundingRefused` are passed over. A funding re-run through `fund` at boot
would be re-decided against an eligibility registry and a product catalogue
that this boot builds from the configuration alone — neither is resumed from
its own log records, and `replay_eligibility` and `replay_products` are proofs
a caller compares against the live state, not resumes — so replaying a funding
is a fresh decision under different evidence and not a check of the record.
The books a funding rebuilds are read by the attribution's pro-rata split and
by the exploration budget, and their lifetime is §43.2's finding about
*derived state* in general, which is not this record's to settle. A
declaration is safe to resume where a funding is not, because it is never in
`available()`: a resumed declaration can loosen nothing. The asymmetry is
real and this record names it rather than hiding it: a book resumes with its
declarations and without its fundings, which is what it did before, minus the
declarations. Closing the rest needs eligibility, products and fundings
resumed together, in log order, before the ledger is read by anything — one
lane, one ADR, not this one.

### 2. Nothing in this tree may post an inflow, and the refusal names its reversal

`UserLedger::post_inflow` keeps no production caller. `cash.rs` says an inflow
is posted "by whatever reconciled them", and nothing here reconciles a user's
deposit: the one custodian statement the platform observes
(`Platform::observe_statement`, fed from the file the composition root mounts)
is the **desk's own wallet** at its broker, and it names venues, assets and
observed balances, never a user or a wire reference. An operator route that
posted an inflow would be an operator asserting that money arrived, which is
the bank's fact and not a person's; a "statement" invented to carry it would
be a number this repository made up and then quoted back as though it had
measured it, the shape ADR 0050 and ADR 0072 refuse in their own domains.

So the flow stops at `expected inflow`, and the surface says so rather than
implying otherwise: `/ledger/users` and both route answers carry
`inflow_posting`, a constant sentence that no declared inflow is ever posted
by this build, that an expected inflow stays expected until an operator
cancels it, and that `uninvestable` is zero on every balance until a
reconciled statement exists. A page rendering an expected inflow without that
sentence would be promising `detected → settled → available`.

**Reversal condition.** A custodian statement line for a *user's* account
that names the wire reference and the amount, observed through the same
mounted-file path the wallet statement takes and matched by the ledger against
the outstanding declaration under that reference, is the evidence that posts
an inflow. When such a source exists, `post_inflow` gains its caller in the
statement's observation path — not in a route, because an arrival is not an
operator's act — `INFLOW_POSTING` is deleted, and the test that pins the
sentence fires so the deletion is deliberate. Until then, `post_inflow` is
reached from tests, and §3 is what it does when reached.

### 3. An arrival is never refused; investability is a ceiling asked at the arrival, and the declaration asks what it can

You cannot un-receive a wire, and a ledger that refuses one disagrees with the
bank. But the mandate's ceilings are not asked of the bank; they are asked of
the book. So `CashBalance` gains a fifth figure, `uninvestable`: cash that
arrived and did not fit under the mandate when it was posted. It is received,
reported on every balance, and never in `available()` — never reserved
against, never debited, never sized against. `UserLedger::post_inflow`
computes the room under both of `fund`'s ceilings by name — the investable
capital less what the user has settled, and the capital under management less
what they have contributed — and admits the smaller to `settled`, recording
the invested part as a `TaxLot` so the contribution ceiling counts it against
the next funding exactly as it counts a `fund`; the rest is held. A room at
or below zero (realised gains can carry `settled` above the investable
figure) holds the whole arrival; that is not a clamp of an input but the
statement that a book already at its ceiling has none. Eligibility is not
re-asked at the arrival: it was asked at the declaration, and a lapse since
does not un-receive the wire.

**The declaration is where a refusal can still reach the person**, before the
wire is sent, and so it asks what it honestly can, mirroring `fund`'s
refusals by name: a user with no mandate; a user the eligibility registry
does not admit at the declaration instant, by the `Ineligible` reason named; a
blank reference or a non-positive amount; a reference outstanding at *any* of
the user's books, not only this one, because a wire reference names one wire
and a second book claiming it is a split reconciliation could never resolve;
and an amount that, with what the user has contributed and every declaration
still outstanding, would pass the capital the mandate places under
management. The last departs from the lane's prior, which refused only an
ineligible user's declaration and let the bucket absorb the rest. It departs
because a desk that accepts a declaration it already knows it cannot invest
tells the user to send money it will only hold, and `fund`'s own refusal
already says what to do instead — record a new mandate under the change that
supersedes this one. The investable ceiling is deliberately *not* asked at
the declaration: it moves with realised profit and loss between now and the
arrival, so it is asked where the money lands, and what it refuses there is
held rather than refused.

### 4. The routes are operator writes on the existing authentication path, authorised in shape and refused in fact

`POST /ledger/users/:user/expected-inflows` and `DELETE
/ledger/users/:user/expected-inflows/:reference`, at `Role::Operator`, are the
tenth and eleventh mutating rows of `qip-api`'s table. Each does exactly what
the eligibility route does and nothing it does not: screens a body of three
keys and refuses any other by position without echoing it (a caller writing
against the blueprint will send `source`, `destination` or `settled`, and none
is read); resolves the user against the mandate registry and answers 404 for a
stranger; dates the operator by `Principal::authentication_instant`, which
refuses every standing bearer token; builds the `OperatorIdentity` from
`principal.subject` and nothing a body carries; raises the typed kernel
intent; and answers with the user's `/ledger/users` row read back from the
ledger, beside `inflow_posting`.

They are **not** exempted from the presence gate. `api_boundary.rs` admits an
exemption only where somebody writes down why, and the only one written is the
kill switch's engage direction: a halt gated on a control that always refuses
is a platform nobody can stop. A declaration has no such urgency, and a record
that names an operator with no attested person behind it is precisely the
audit fact ADR 0076 refuses to fabricate. So the routes are authorised in
shape and refused in fact, as ADR 0075's capital route is, and they act on the
day a per-person credential exists without a line here changing. The
production call path exists — the route reaches `Platform::expect_inflow`,
which reaches `UserLedger::expect_inflow`, which the register asked for — and
it reaches no deployed process's *act* until Gate A closes. Both halves are
stated in the register row rather than one.

### 5. A capital call is its own record family, not an inflow, and its writer is not built here

The brief's prior read §43.2's `CapitalCall` as "a declared expected inflow
with a consequence of failure", to be carried in the same event family. **The
direction is wrong.** `qip_financial::cashflow::CapitalCall` is "a drawdown
demand: a date, an amount, and what failing it costs" — a fund calling the
desk's committed capital *in*. From this platform's side it is an obligation
the desk must pay, the opposite of a user's deposit, and `Commitment::
obligation` adds it to the reserve `unfunded_total` subtracts from free
capital before anything is sized. Filing one under `LedgerEntry` on a user's
book would put a demand on the desk into the record of what a user has
promised, and the resume in §1 would replay it into a `CashBalance` that has
no arm for it.

What the two *share* is the discipline, and that is the decision: a capital
call notice is a journalled declaration with a reference the type itself
refuses to duplicate (`Commitment::record_call`), carrying its
`CallConsequence` on the record, written only by an authenticated operator
through the same presence-gated path as §4, and resumed at boot by a seam
beside `resume_ledger` that replays notices into the commitment book
`private_holdings_of` has just built — refusing closed, as §1 does, when a
notice names a commitment this boot's universe no longer holds. The record
type is its own (`CapitalCallEntry` on `Platform`, alongside the commitment
book, not inside `LedgerEntry`), because its subject is a commitment and not
a user's book and its reader is the reserve and not the attribution. That
writer is **not built by this record**: it needs the same per-person identity
§4 waits on, the commitment book's own resume, and a route on the table, and
each is a reviewed change. §43.2 stays `PARTIAL` with this decision on it,
so the lane that builds it inherits a design and not a question.

## What it costs

- **A `UserLedger` clone per declaration and per cancellation**, for the
  scratch check — the same cost `apply_eligibility` already pays, on a
  structure bounded by the enrolled users and their books.
- **Assembly can now be refused by a log this boot's configuration will not
  take.** That is the fail-closed cost of a real resume: a deployment that
  drops a user's mandate while a declaration stands for them must cancel the
  declaration first or archive the log, and the refusal says so. Silently
  booting without the declaration was the defect.
- **The asymmetry §1 names**: declarations resume and fundings do not. Nothing
  is looser than before; the remainder is named and owned elsewhere.
- **Two mutating routes that refuse in fact**, and the register must say both
  halves. The acceptance suite's mutating-route set and typed-intent allowlist
  each grew by two, with `post_inflow` deliberately absent from the allowlist
  and reached by no route.
- **A declaration the mandate cannot take in is refused rather than held.**
  A user whose mandate is being superseded must have the new mandate recorded
  before declaring against it. That is `fund`'s existing rule applied one
  step earlier, and it is the step at which the person can still act on it.

## What would make this wrong

- **A custodian statement source for user accounts lands.** Then §2's
  refusal reverses on its own condition: `post_inflow` gains its caller in the
  statement path, `INFLOW_POSTING` is deleted, and §3's bucket is exercised
  in production for the first time. The tests on the bucket are already
  written against that day.
- **A per-person operator credential lands (ADR 0076).** The routes act.
  Nothing here changes, and that is the test of §4's design.
- **Eligibility, products and fundings are resumed from the log.** Then §1's
  ordering — eligibility before ledger — must hold in the new seam, and the
  restart test here is what would fail if it did not.
- **`CashBalance::available` ever reads `uninvestable`.** That would be the
  second weaker door again, and `an_arrival_past_the_contribution_ceiling_is_
  held_uninvestable_and_available_does_not_move` is the test that fires.

## Consequences

- Register row §40.12 moves within `PARTIAL`: the writer's safe half is built
  with a production call path that refuses in fact; posting is refused by this
  record. Row §43.2 stays `PARTIAL` with §5's decision recorded on it.
- The paper-trading boundary is untouched at all three layers.
  `infrastructure/terraform/variables.tf` still refuses `supervised_live`,
  `limited_autonomous_live` and `autonomous_live` at plan time;
  `AutonomyLevel::deployable` still refuses the same three at start-up in
  `qip-api`, `qip-fastbrain` and `qip-deepbrain`; `qip-edge`'s `Cell` still
  has no constructor taking a ceiling other than paper trading and
  `qip-cost-router`'s `Determinism::Required` arm still returns a type that
  cannot name a model rung. An inflow declaration admits no instrument and no
  order: it is a reference and an amount held outside `available`, and the
  only thing that can happen to it in this build is that an operator cancels
  it.
- No dependency is added. The workspace still holds eleven third-party
  packages.
