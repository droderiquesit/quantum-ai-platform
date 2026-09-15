# ADR 0075: The capital-issuance route is authorised in shape and refused in fact, and a family may never revise a share

- **Status**: Proposed
- **Date**: 2026-09-15
- **Supersedes**: nothing
- **Related**: ADR 0003 (paper trading by default), ADR 0008 (edge cells decide
  alone, on capital granted in advance), ADR 0039 (a region's grant is shared
  across its cells), ADR 0043 (the three cryptographic gaps no in-tree code may
  close), ADR 0055 (a counterfactual result narrows sizing and only narrows
  it), ADR 0062 (a venue is reinstated only by two signatures), ADR 0063 (a cap
  floors at the minimum position and the shortfall is recorded, not
  reallocated), ADR 0064 (a family's funding standing is measured and no weight
  is revised), ADR 0065 (a standing secret has no authentication instant),
  ADR 0066 (a regime narrows an allocation and never favours it)

This record exists because ADR 0064 said it must. Its first reversal condition
reads: the day `CentralPlane::issue` gets a production caller, "the two family
notions in this platform — provenance and correlation — become confusable in a
way they are not today. That needs its own record before either is wired to a
weight." The condition has not fired. This is the record taken in advance of it,
which is the only order in which it is worth anything.

## Context

### One missing caller holds four things dead

`CentralPlane::issue` is the only writer of `self.envelopes`, and it has no
caller in any `src/` file in the workspace.

```
grep -rn --include=*.rs 'envelopes\.insert' backend/crates
grep -rn --include=*.rs '\.issue(' backend/crates | grep -v '/tests/' | grep -v ':[0-9]*: *//'
```

The second command carries a filter that is not decoration. `family_review.rs`
quotes the unfiltered form of it inside a file the command searches, so the
unfiltered form reads itself and reports a caller that is a sentence. This
repository has already been burned by a recount command that matched its own
citation — `.claude/rules/domains/observability.md` records the day the
comma-less form of a `FEASIBILITY_REFUSALS` count printed four and then five,
the fifth hit being a doc comment quoting the command. Run the filtered form,
and if you paste either into a `.rs` file, expect it to find you.

What is dead behind the missing caller, each stated by its own consequence
rather than by a list of symbols:

- `retain_grants` iterates `self.envelopes` and retains nothing, so the realised
  calendar has no day in it, so `CentralPlane::family_structure` returns `None`
  on every cycle of every deployment, so blueprint §23.1 LEVEL 1's family
  measurement records nothing however much the cells settle. Pinned by
  `the_learn_stage_measures_no_family_structure_on_a_corpus_the_centre_never_granted`
  in `qip-kernel/tests/central.rs`.
- §23.1 LEVEL 2 — allocation across families — has no subject to allocate over.
- `cycle_whitelist_for` answers `NoLiveGrant` for every cell.
- `recall_for` has no live grant to recall when the exposure aggregate finds a
  concentration.

`issue`'s own doc names what the caller must be, and this record does not
improve on it: an operator-authenticated route on `qip-api`, carrying an
`Approval` naming two humans neither of whom is the requester, one
`OperatorCredential` per name minted by the authentication middleware, and
`Platform::drawdown` for the drawdown argument so that the grant and the
ADR 0039 share are one number from one source. The doc is equally explicit that
a cycle stage must not be that caller: it would have to manufacture the approval
and the credentials, which is forging control 4, and it would defeat
`MAXIMUM_ENVELOPE_VALIDITY` — the only revocation there is for a cell the centre
cannot reach — because an expiry a process renews for itself every cycle is not
an expiry.

### The route is not the binding constraint, and that is the finding

A lane commissioned to wire §23.1's family allocation stopped and reported the
missing caller. That is correct as far as it goes, and it does not go far
enough: **building the route today would call nothing.**

`issue`'s first act is to refuse unless the factory says the strategy stands at
a rung that holds capital.

```
grep -n 'holds_capital()' backend/crates/runtime/qip-kernel/src/central/plane.rs
grep -n 'fn holds_capital' -A 6 backend/crates/libs/qip-contracts/src/gate.rs
```

`GateStage::holds_capital` is `Pilot | Scaled`, and the only production path to
either is `approve_promotion`, which needs two signatures from two distinct
people. `qip-api` mints the operator credential per **role**, not per person:

```
grep -n '@env' backend/crates/apps/qip-api/src/main.rs
```

Both holders of `QIP_TOKEN_OPERATOR` present the subject `operator@env`, so
every countersignature in this platform is refused as one person signing twice
(ADR 0062 Amendment B). And since ADR 0065 the question does not even reach that
comparison: `Principal::authentication_instant` refuses every signature-gated
route, because a standing bearer token carries no instant at which anybody was
present.

```
grep -n 'fn authentication_instant' -A 20 backend/crates/apps/qip-api/src/auth.rs
```

So no strategy in any deployment can reach `Pilot`. A capital route built today
would refuse on its own authentication gate, and if that gate were somehow
satisfied the kernel would refuse on the stage. It would be a route whose entire
purpose is to give `issue` a caller and which cannot call it.

There is a third link in the same chain, and it is worth naming because it fails
quietly rather than loudly. `retain_grants` does not retain every live grant; it
retains the live grants of strategies the factory holds a baseline for.

```
grep -n 'fn retain_grants' -A 22 backend/crates/runtime/qip-kernel/src/central/plane.rs
grep -rn --include=*.rs 'set_baseline' backend/crates
```

The second command prints the definition and nothing else — `set_baseline` has
no caller anywhere in the workspace, not even a test. The only writer of
`self.baselines` with a caller is `baseline_for`, invoked from inside
`approve_promotion`:

```
grep -n 'baseline_for\|baselines\.insert' backend/crates/runtime/qip-kernel/src/central/factory.rs
```

The same act that admits a strategy to a capital-holding rung is the act that
seeds the baseline `retain_grants` filters on. That is good design — the two
facts have one origin — and it means the realised calendar is gated on
`approve_promotion` twice over, once through the stage and once through the
baseline. Both go through the signature nobody can complete.

**The order is therefore fixed, and it is not the order the work was
commissioned in.** Gate A is a per-person operator identity carrying an
authentication instant. Gate B is a strategy standing at `Pilot` in a
deployment, which Gate A alone unblocks. Gate C is the capital route. §23.1
LEVEL 1 has a subject only after C, and C is worth nothing before A.

### Two facts about the allocator that decide the rest of this record

**Shares are normalised.**

```
grep -n 'let share = ' -B 6 backend/crates/services/qip-capital/src/allocation.rs
grep -n 'fn risk_adjusted_edge' -A 12 backend/crates/services/qip-capital/src/allocation.rs
```

A strategy's share is `score / total_score`, where the score is
`expected_sharpe - penalty * standard_error` floored at zero. Lowering any one
strategy's score raises every other strategy's share of the same budget. There
is no such thing as a local narrowing here.

**The approval does not bind an amount.**

```
sed -n '/impl CapitalRequest/,/^}/p' backend/crates/libs/qip-compliance/src/approval.rs
```

`CapitalRequest::subject()` is `capital:{strategy}@{cell}`, and
`ApprovalChain::check` refuses an `Approval` whose `subject` differs from it.
The gross limit appears in the freshness-and-approver checks only as the
threshold test for whether a second approver is required. So two humans sign for
a strategy at a cell, not for a number. The same `Approval` value validates at
any size the allocator subsequently produces.

Neither fact is a defect today, because nothing issues and nothing is funded.
Both become defects on the day Gate C closes, which is why they are decided here
rather than discovered in the implementing lane.

## Decision

### 1. An operator-authenticated capital route may exist, and is not built yet

**The route is authorised in shape and refused in fact until Gate A is closed.**

`POST /strategies/:strategy/capital-grants` is the correct fifth mutating row of
`qip-api`'s route table. It is not a live-order path and nothing about it is one:
it fixes a ceiling on what a *paper* simulator may commit. It is not built in
this record and must not be built before a per-person credential class exists,
for the reason the preceding section gives — it would be a route that cannot
reach the function it exists to reach, and the register would then read "capital
issuance exists".

This platform keeps a list of controls that read as protection and could never
fire, and the list is long enough to be a design rule: `MaxExpectedShortfall`
shipping in every default limit set with an always-empty tail-risk map; the
`liquidity-read` sign-off; the fifteen-minute freshness window measuring pod
uptime; `StrategyFactory::set_baseline` with no caller at all; the family cap
ADR 0064 declined to build. A capital route landing before Gate A is the same
shape, and it would be the first instance built by a lane that had been shown
the list.

**The paper-trading boundary is untouched, at all three layers, and stays
untouched by everything in this record.** Terraform's `variables.tf` refuses
`supervised_live`, `limited_autonomous_live` and `autonomous_live` at plan time,
so no live ceiling reaches a configuration. `AutonomyLevel::deployable` refuses
the same three at start-up in `qip-api`, `qip-fastbrain` and `qip-deepbrain`, and
stops the process rather than lowering the value. The type system holds the
third: `qip-edge`'s `Cell` has no constructor taking a ceiling other than paper
trading, and `qip-cost-router`'s `Determinism::Required` arm returns a type that
cannot name a model rung. An envelope is a bound, not a permission — it says how
much a cell may commit, never to what. `Cell::send` still records
`qip_edge_refusals_total{gate="live_venue"}` and refuses an order bound for a
live-class venue whatever envelope the cell holds, and ADR 0039's
`region_share` suite already asserts that a *funded* cell is assembled
paper-only. Nothing here adds an autonomy path, and issuing capital is not
raising autonomy: `AutonomyController::request_change` is a different act with a
different gate and this route may never reach it.

### 2. The route originates the proposal, and nothing else may

`CentralPlane::set_proposal` is the only writer of an allocator proposal, and
its only `src/` caller is `central::learning::resize`, which begins by requiring
a proposal that already exists. So the platform can revise a proposal and cannot
originate one, and `issue` refuses a strategy with no proposal to size on.

The obvious repair is to let a cycle stage originate the first proposal — the
machine has the evidence, a human still signs the envelope. **Rejected.** Two
arguments, and the second is decisive.

First, the approval binds no amount. A machine-originated proposal decides the
size that two humans then sign for without the size appearing anywhere in what
they signed.

Second, **origination is not a local act in a normalised allocator**. Writing a
proposal for one strategy changes the denominator, and therefore changes the
allocation of every other funded strategy on the same write. A cycle stage
originating a proposal is a cycle stage reducing the capital of every strategy a
human already approved. There is no version of that which is "the machine
proposed and the human decided".

So: **the proposal arrives on the request, and is written only inside the act
that issues.** A request that is refused for any reason writes no proposal. This
is not a nicety about transaction shape; it is the difference between a refused
request and a refused request that silently resized the whole book. It needs its
own test and the test is named in the design below, because it is the one an
implementing lane will not think to write.

### 3. Three family notions, no conversions, and only one may ever bound anything

This platform has **three** distinct objects called a family, not two. ADR 0064
named two; ADR 0066 added the third and its module doc already says which of the
three a reader is holding.

| Notion | Where | What it is |
|---|---|---|
| Provenance | `qip_lifecycle::StrategyFamily`, `StrategyCandidate::family` | The sweep a candidate was enrolled under. Fixed at enrolment, never recomputed. |
| Correlation | `qip_optimization_engine::families` | A cluster recomputed every cycle from realised returns inside a stated stress window. §23.1 LEVEL 1. |
| Alpha taxonomy | ADR 0066's §19 table | A classification of what kind of edge is being harvested. |

Four rules, and each is a rule because the alternative is a confusion that
outlives the code that made it.

- **No conversion exists between any two of them.** No `From`, no `Into`, no
  `as_str()` handed to the other's constructor. A conversion is the confusion,
  compiled, and it will be written by somebody who needs a key and has the wrong
  one to hand.
- **Only the correlation family may ever bound an allocation.** It is the only
  one of the three that is a claim about *risk*: two strategies out of one sweep
  may be genuinely uncorrelated, and two out of different sweeps may be one bet
  in a drawdown, which is the entire reason `families` refuses to cluster on the
  full sample. A cap keyed on provenance is a cap on a filing system.
- **The provenance family may never bound anything.** ADR 0064 already says no
  weight moves, but its reason was circumstantial — there was no reachable
  weight. This record replaces it with a positive reason that survives Gate C:
  provenance is not risk.
- **Neither may name the other's key in a journaled record.** A
  `MisallocationFinding` carrying a correlation `FamilyId`, or a
  `FamilyStructureJournal` carrying a `StrategyFamily`, puts the confusion in
  the permanent log, where it is read long after the code that wrote it is gone.

The structural half of this is that the two crates do not depend on each other,
and the absence is deliberate and already argued in the tree rather than being
an accident this record is relying on:

```
grep -n 'qip-lifecycle\|qip-optimization-engine' backend/crates/services/qip-lifecycle/Cargo.toml backend/crates/services/qip-optimization-engine/Cargo.toml
```

The only hit outside the two `name =` lines is a comment in `qip-lifecycle`
beginning "Deliberately NOT a dependency: qip-optimization-engine", which
records that the edge was taken once, that
`nothing_that_vetoes_executes_or_moves_money_can_reach_a_quantum_solver` caught
it, and that the reconciliation now goes through a port the kernel composes. So
neither family type can name the other without a new dependency line a reviewer
sees, and adding that line fails an existing acceptance test for a second and
unrelated reason. The kernel is the only place both are in scope, which is what
`.claude/rules/architecture/00-boundaries.md` says the kernel is for, and it is
therefore the only place the rules above can be broken. That is where the check
belongs.

### 4. A family-level revision may lower a ceiling and may never move a share

ADR 0064 left this open and said it would need its own record. It is taken here.

**A family finding may not revise a proposal, at any time, in any direction.**
Because shares are normalised, lowering one family's score raises every other
family's share of the same budget on no evidence about those families. ADR 0066
settled that a regime narrows and never favours, and `Stance::multiplier` has no
arm above 1.0 to enforce it. In a normalised allocator that discipline cannot be
obtained by lowering a term at all — every narrowing is somebody else's raise.

**The only admissible family-level consequence is one that lowers a ceiling and
leaves the residual unallocated.** `AllocationLimits::total_budget`, or a
per-family cap that subtracts from the budget rather than redistributing it, so
that what the cap took is recorded as a shortfall and not handed to whoever came
second. That is the discipline ADR 0063's `construct_capped` already follows for
a per-instrument bound, and it is the shape any future family cap must take. It
is not built here and nothing in this record authorises building it; what this
record fixes is that the *other* shape — a family finding lowering a share — is
refused, so the implementing lane does not reach for the easy one.

**What a revision may do to a strategy holding its own approvals: nothing to the
envelope, and nothing to the proposal while a request is pending.** A live
envelope two humans signed is revoked by expiry
(`MAXIMUM_ENVELOPE_VALIDITY`, twelve hours — `grep -n 'MAXIMUM_ENVELOPE_VALIDITY'
backend/crates/services/qip-capital/src/envelope.rs`) or by `recall_for`, and by
nothing else. That is ADR 0008 kept whole: a partitioned cell keeps spending
within its last grant, and a grant a central finding could shrink underneath it
would make the cell's own accounting a second claim about the same fact.

The narrower half matters more and is easy to miss. `ApprovalChain::check`
requires every approver's credential to be fresh at the instant of the grant, so
the two signatures of a dual-approval grant must land inside one fifteen-minute
window. **That window is exactly the interval in which a machine revision could
change what the two humans are signing for**, because the approval names no
amount and the allocator would re-size on the second signature. So while a
capital request is pending, the proposal book is frozen for that strategy:
`resize`, and any future family finding, refuse to write and journal the
refusal rather than swallowing it. A refusal nobody records is a LEARN stage
that silently stopped learning.

### 5. Nothing here needs a dependency

No new crate. The workspace stays at `serde` and `serde_json` (ADR 0002,
ADR 0009). Gate A may need one — an asymmetric primitive is one of ADR 0043's
three gaps that no in-tree code may close — and that is Gate A's record to
argue, not this one's. Nothing in this record may be read as authorising it.

## The design handed to the implementing lane

Buildable only after Gate A. Written now so that the lane that closes Gate A can
see what it is closing it for, and so that the shape is reviewed on its own
merits rather than inside a commit that was making a red test green.

### Where it sits

`backend/crates/apps/qip-api/src/routes.rs`, as new arms in the same `match` on
`(Method, path)` that holds `POST /venues/:venue/reinstatements` and
`POST /registrations/:source/approve`. Rendering and body parsing go in a new
`capital_views.rs` beside `venue_views.rs` and `ledger_views.rs`; the kernel
seam is a new `Platform` method; no logic lives in `routes.rs` beyond
authentication, parsing and status mapping, which is what every other mutating
row there does.

Dependency direction: `apps → runtime → services → libs`, unchanged. The route
is in an app, the seam is in the runtime, the approval chain is a lib and the
allocator is a service. Nothing in `qip-capital` or `qip-compliance` learns that
an HTTP route exists, nothing in `qip-kernel` reads an environment variable, and
no lib acquires I/O. The one new piece of kernel state — the pending-request
book — sits beside `CentralPlane`'s existing books, in the crate that already
composes the factory, the allocator and the approval chain, because it is a fact
about all three and belongs where they meet.

### Authentication path

Exactly the existing one. No new mechanism, no new credential class invented by
this lane.

1. The middleware authenticates the bearer token and produces a `Principal`.
2. `principal.require(Role::Operator)` or refuse 403.
3. `principal.authentication_instant("granting capital to a strategy")` or
   refuse 403 with the refusal's own message. Today this always refuses; after
   Gate A it returns the instant a person actually authenticated.
4. `OperatorCredential::verified(principal.subject.clone(), "api-bearer-token",
   authenticated_at)` — the subject from the principal and never from the body,
   which is the rule that makes this an approval rather than a claim to have
   been approved.

### Shape: three requests, because two humans cannot be in one

`ApprovalChain::grant` takes both credentials in one call, and one HTTP request
carries one principal. So the act is accumulated the way ADR 0062's
reinstatement accumulates:

- `POST /strategies/:strategy/capital-grants` — the **request**. Body carries the
  cell, the venue, `expected_sharpe`, `sharpe_standard_error`, the capacity model
  and `capacity_uncertainty` (the fields of `StrategyProposal`, which already
  derives `Deserialize`), plus the rationale. The requester is the principal.
  The kernel sizes the plan **against a book that includes this proposal without
  writing it**, and returns 202 with the request identifier, the subject string
  `capital:{strategy}@{cell}`, and the gross, order and loss limits the grant
  would carry. That quotation is the point of the step: it is the only place the
  number the signatures buy is ever shown to the people signing.
- `POST /strategies/:strategy/capital-grants/:request/signatures` — one
  signature per request, one principal each. The kernel holds the accumulated
  credentials.
- On the signature that completes the requirement — one approver below
  `requires_dual_approval(gross_limit)`, two above — the kernel builds the
  `Approval`, calls `CentralPlane::issue` with `Platform::drawdown()`, writes the
  proposal and the envelope together, and journals the grant with the sized
  limits so the signatures can be reconciled afterwards against the number that
  was quoted.

**The pending window is derived from `qip_compliance::approval::MAXIMUM_CREDENTIAL_AGE`
and never restated.** A pending request that outlives it is expired and refused.
Choosing any longer window — ADR 0062's day, for instance — would create a
pending record that can never complete, because `check` requires the *first*
approver's credential to still be fresh when the last signature lands. A pending
state that cannot be left is the same defect as a limit that cannot fire, and it
would pass every test that did not advance a clock.

### What it must refuse, and what proves each refusal fires

Every row needs both halves. A gate proven only to refuse may be refusing
everything, which is what `modules/network`'s prefix check was doing when
ADR 0069 found it.

| # | Refusal | Fires when | Admits when |
|---|---|---|---|
| R1 | 403, role | An analyst token requests a grant | An operator token passes this step |
| R2 | 403, no authentication instant | `Presence` has only the unattested variant | A credential class carrying an instant passes |
| R3 | 409, stage | The strategy stands at `Candidate`, and the message names the stage | A strategy at `Pilot` passes |
| R4 | 400, mismatch | The body's strategy differs from the path segment | They agree |
| R5 | 403, approver | Approver is the requester; second is the first; second is the requester | Two distinct people, neither the requester |
| R6 | 403, dual threshold | A gross limit above the threshold with one approver | The same limit with two |
| R7 | 403, stale | A credential one second past the window | A credential exactly at the window |
| R8 | 409, quote drift | The recomputed limits differ from the ones quoted at step 1 | An unchanged book completes |
| R9 | 409, validity | Requested validity above `MAXIMUM_ENVELOPE_VALIDITY` — refused, not clamped | Twelve hours exactly |
| R10 | 409, venue | The allocation names a live-class venue | A simulated venue |
| R11 | 403, rationale | Under ten characters | A reviewable sentence |
| R12 | 410, expired | A pending request older than `MAXIMUM_CREDENTIAL_AGE` | One inside it |

Three tests beyond the table, and they are the ones that matter:

- `a_refused_capital_request_writes_no_proposal_and_moves_no_other_strategys_allocation`
  — build a book with two funded strategies, record the plan, submit a request
  that fails at each of R3, R5, R6 and R7 in turn, and assert after each that the
  proposal book and both existing allocations are unchanged. This is the test for
  §2's decision, and without it the normalisation hazard ships.
- `two_signatures_buy_the_number_that_was_quoted_and_a_book_that_moved_refuses`
  — quote at step 1, mutate another strategy's proposal, sign, assert the refusal
  names the drift; restore, sign, assert completion. R8 is the whole defence of
  an approval that names no amount, and it is the one an implementer will read
  as belt-and-braces.
- `a_strategy_with_a_pending_capital_request_cannot_be_resized_and_the_refusal_is_journaled`
  — drive LEARN's `resize` against a strategy with a request open and assert both
  that it refuses and that the refusal reached the log.

Mutation-verify all of them, as `.claude/rules/architecture/01-testing-strategy.md`
requires. R5 in particular: `contains("approver")` is true of almost every
refusal message this chain produces, and matching a substring of a neighbour is
the class of failure that rule exists for.

Cross-cutting assertions — the paper boundary, and the ordering constraint below
— belong in `backend/crates/tests/qip-acceptance/tests/`, not in `qip-api`'s own
suite, because nothing there can see the other side of the seam.

### One ordering test, and an honest statement of what it is worth

An acceptance assertion that a capital route and a single-variant `Presence`
cannot both exist: if `routes.rs` names a `capital-grants` path, then
`auth::Presence` has more than one variant. It fails closed and it makes the
gate order a thing a test holds rather than a thing this document asserts.

It is a text scan, and ADR 0064's fourth round is the record of what a text scan
is worth: three successive versions of a similar scan were each defeated by
ordinary code an independent review compiled into the tree and ran. This one is
narrower than those — it reads two literals rather than guessing at dataflow —
but it can be walked past by a route registered under another spelling, and
saying so is the point. A reader who takes it for a mechanical guarantee stops
looking.

## What it costs

**§23.1 LEVEL 1 stays a measurement that measures nothing, for longer.** The
delivery register scores it `REACHED` on the ground that `stage_learn` calls
`family_structure` every cycle, which is true of the code and false of any
deployed process: the call returns `None` every time, by construction, and will
until Gate C. This record does not move the verdict — the bar the register
states is met — and it names the gap so that the row can carry a caveat rather
than a correction.

**A decision recorded years before the code it governs.** Every clause here is
written against a tree in which nothing is funded, and a clause that turns out
to be wrong will be wrong in a lane that has already started. The mitigation is
that each decision is derived from an invariant that is checkable today — shares
are normalised, the approval names no amount, the credential is per-role — and
each names the command that checks it.

**A shape authorised and not built reads as permission.** Somebody will find
this record, read the design section, and build the route. That is what the
ordering test is for, and the ordering test is a text scan.

**New kernel state that can be stale.** The pending-request book is bounded by
the number of strategies at a capital-holding rung — zero today, small forever,
set by the desk rather than by traffic — and entries expire at
`MAXIMUM_CREDENTIAL_AGE`. It is still a book somebody has to reason about during
an incident.

**LEARN can now refuse.** Freezing a strategy's proposal while a request is
pending means `resize` sometimes does not resize, and the standard-error ratchet
that only turns one way skips a turn. Journaled rather than swallowed, so the
skip is visible; but a cycle that recorded a refusal where it used to record a
revision is a line an operator has to learn to read.

**Nothing is bought today.** No row moves, no gap closes, no binary changes. What
is bought is that the day the gaps close, the four questions this record answers
are already answered, rather than being answered inside the commit that needed
them answered.

## What would make this wrong

- **Gate A closes with a per-request proof of recency rather than an interactive
  sign-in.** This is the likelier of ADR 0065's two remedies for a
  machine-to-machine surface, and it closes the *freshness* half while leaving
  the subject per-role. The dual signature would still be impossible, so the
  route would be buildable for the single-approver case alone and the ordering
  above needs amending rather than following. Say which half closed before
  building anything.
- **`CapitalRequest::subject()` starts binding the amount.** That is the honest
  fix for the amount-free approval, it lives in `qip-compliance`, it changes a
  string every existing test asserts, and it deserves its own lane. If it lands,
  the freeze in §4 and the quote-drift refusal R8 both become narrower than they
  need to be, and this record should be amended rather than worked around.
- **R8 fires on ordinary traffic.** If every second signature is refused because
  LEARN resized in between, the freeze is in the wrong place — it should cover
  the whole pending window rather than being detected at the end — and the answer
  is to widen the freeze, never to drop the comparison. Dropping it would leave
  two signatures buying an unknown number.
- **A fourth family notion appears.** The table above is an enumeration, and this
  repository's own history says an enumeration in a document goes stale silently
  while every reader believes it. Three was two until ADR 0066. Check the crates
  rather than the table.
- **Somebody argues that a family finding lowering one share is a narrowing.**
  It is arithmetically a raise for everything else, and the normalisation is one
  `grep` away. If the allocator ever stops normalising, this clause is void and
  needs re-deriving rather than re-reading.
- **`execution_nodes` stops being `{}`.** Everything here about what an envelope
  bounds is in-process reasoning today. The day a cell runs in a project, a
  granted envelope is an authority a process holds across a partition, and the
  twelve-hour validity stops being a constant in a test and starts being the
  actual blast radius of a grant nobody can recall.
- **The ordering test is cited as the guarantee.** ADR 0064 removed the sentence
  "the guarantee is the absence of a code path" because it was false and had
  taught three implementations to trust a scan. The same sentence will be
  tempting here. What is true is smaller: no route named `capital-grants` exists
  while `Presence` has one variant.

## Consequences

- No verdict in the delivery register moves. §23.1 stays `PARTIAL` with LEVEL 2
  `ABSENT`; §12.3 stays `PARTIAL` and keeps ADR 0055's `absent` clause verbatim.
  The shape table is unchanged.
- `CentralPlane::issue` still has no production caller. Its doc gains a pointer
  to this record as the decision that authorises the route and names the gate
  that blocks it, so the next lane to read it finds the answer rather than the
  question.
- ADR 0064's first reversal condition is answered in advance. It does not fire;
  it is discharged.
- ADR 0065 gains a named consumer for its "real fix". Gate A is not only about
  seven routes that refuse — it is the single prerequisite for capital allocation
  existing at all in this platform, and for §23.1 LEVEL 1 ever measuring
  anything.
- `StrategyFactory::set_baseline` is recorded here as having no caller of any
  kind. It is not deleted: it is what a desk re-baselining a pilot from realised
  returns would call, and its own doc explains why the cycle must not. A future
  lane that finds it dead should read that doc before removing it.
- No new dependency. No infrastructure change. The paper-trading boundary is
  untouched at all three layers, and no type named in this record can name a
  live venue.
