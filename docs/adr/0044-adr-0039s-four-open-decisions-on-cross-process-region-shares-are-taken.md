# ADR 0044 — ADR 0039's four open decisions on cross-process region shares are taken, and three of them are ratifications of what a commit already decided

- **Status:** **proposed**
- **Date:** 2026-09-06
- **Amends:** ADR 0039 ("A region's grant is shared across its cells over the
  mesh"), section *"What this does not decide, and what the owner must
  decide"*, items 1 to 4. ADR 0039's decision — option (a), disjoint per-cell
  shares — is **not** reopened, superseded or weakened. This record answers
  the four questions that record left standing.
- **Decides:** PHASE-B24's blocking condition, which the register states as
  "Owner must (1) accept/amend/decline, (2) choose unfunded-at-boot
  (recommended) or ceiling-at-boot, (3) say where cell membership and the
  region's grant are configured — nothing at the centre names a region today,
  (4) set the payload cadence"
  (`docs/plan/PROJECT-PLAN.md:222`).
- **Relates to:** ADR 0008 (cells decide alone, on capital granted in advance
  — the standard every answer below is measured against), ADR 0035 (the one
  shadow node, the only place any of this would be observed), ADR 0045
  (*proposed*, concurrent: the *ceiling* for that node — a different number
  from the share, and decision 2 below is what makes them different), ADR 0024
  (one cell per process, which is why this is a cross-process problem at all),
  ADR 0002 and 0009 (no crate is added by any answer below).
- **Does not touch:** the paper-trading boundary. `Cell::new` remains the only
  cell constructor and takes no ceiling; a share is checked after the
  envelope, after autonomy and after every other gate in `Cell::work`. No
  answer below creates, eases or implies an order path.
- **Applies nothing.** Decisions 1 to 3 ratify code that is already in the
  tree, so accepting them changes no file. Decision 4 changes no file either,
  because the thing it decides does not exist to be changed — see it.

## Why a record rather than four sentences in the plan

ADR 0039 corrected itself on 2026-09-05 for a specific failure, and this
record exists to avoid the twin of it. That record was marked *proposed* while
three slices of work sat applied beneath it, and its own correction says why
that mattered:

> a record marked *proposed* with three slices of applied work under it is
> drift of the kind this repository exists to prevent, because a reader who
> stops at the status line concludes the double-spend window is still open
> when it has been closed in the tree.

The residue of that episode is the four items below. ADR 0039's own preamble
describes what happened to three of them:

> Three of the four owner decisions under "What this does not decide" have
> been taken by the implementation and are recorded there as taken; the
> fourth (payload cadence) is untaken and is the only thing still waiting on
> the owner.

And its item 3 says so in the sharpest possible terms:

> **Answered in code, not by an owner: `QIP_MESH_REGIONS`, read at the API
> root.**

A decision taken in code and annotated in the record it was supposed to be
taken by is not a decision that has been *made*; it is a decision that has
been *observed*. The difference is that nobody has argued the rejected
alternative, so nobody can tell later whether the shape was chosen or fell
out. Three of the four answers below therefore ratify what the tree does —
and each states the alternative that was not taken and why, which is the part
the commits did not supply. The fourth is genuinely open, and the answer it
gets is not the number the question asked for.

## Decision 1 — Option (a) is accepted as applied, and the explicit `RegionShare` field is declined rather than deferred

**ADR 0039's item 1, quoted in full:**

> 1. **Accepted, as option (a) with the by-reference deviation.** Declining
>    leaves F6's cross-process half as operator discipline, which is the
>    status quo the matrix records honestly; it should then be recorded as a
>    deliberate absence beside `execution_nodes = {}` rather than left as a
>    gap.

**Decided: accept, and close the deviation as permanent.**

The applied shape carries the share *by reference*: `qip-contracts` was not
edited, the `capital_grants` slot carries the manifest of grant signatures it
already carried, and the cell's share is the sum of `gross_limit` over the
verified, still-live envelopes the manifest names. The centre withholds a
manifest from any cell whose live grants already sum past `plan.for_cell(cell)`
rather than naming them anyway. ADR 0039 records this as a deviation from the
option it recommended, and leaves the door open:

> The owner may still add the explicit field under its own commit;
> `RegionAllocation::rebase` takes an amount and would not change.

**That door is closed here, and closed rather than left ajar**, because
leaving it open is itself a hazard the same record names. Its own reversal
conditions include:

> **A second signed path to the cell's bound appears** — a re-balance
> message, a per-strategy envelope that also carries a region amount, an
> operator override over the mesh. Two claims about one bound will disagree
> and the wider one will win by accident.

An explicit `RegionShare` field beside a manifest whose envelope gross the
centre has already checked against the same share **is** a second claim about
one bound. The two would be produced from the same `AllocationPlan` on the
day they were written and would drift the first time a renewal path moved one
and not the other, and the drift would be silent because both are signed and
both verify. By-reference is not a lesser version of the field; it is the
version with one source, which is `CLAUDE.md` principle 6 applied to a bound
rather than to a bill.

**What declining the field costs, stated rather than minimised.** ADR 0039
already names it:

> the cell's share can never be *narrower* than its envelopes' sum by the
> centre's say-so alone; narrowing below that needs the envelopes renewed
> smaller, which is the allocator's existing path.

So the centre has two narrowing moves — renew the envelopes smaller, or
withhold the manifest entirely, which takes the cell to zero — and no third
one in between. There is no way to say "you may keep these grants but only
spend half of them". That is a real loss of expressiveness and it is
acceptable because the missing move is one the allocator can already make by
its own route, and because the alternative buys expressiveness with a second
signed claim about the same number.

**Rejected alternatives.**

- *Add the explicit field for symmetry with option (a) as written.* Rejected
  above: it is the second-claim hazard, and symmetry with a record's own
  first draft is not a reason.
- *Decline option (a) altogether and record F6's cross-process half as a
  deliberate absence.* This was ADR 0039's stated fallback and it is no
  longer available on its own terms: the mechanism is applied, tested across
  five acceptance tests in
  `backend/crates/tests/qip-acceptance/tests/region_share.rs`, and declining
  now would mean deleting working code to restore a gap. The fallback was
  written when nothing was applied and it expired when the first slice
  landed.
- *Adopt the field but make the manifest advisory.* Rejected: it inverts
  which of the two is load-bearing without removing either, and leaves the
  cell verifying a bound whose envelopes might not support it.

**What would falsify this.** If a case arises where the centre must narrow a
cell below its live envelopes' sum *faster than a renewal cycle* — a risk
event, an operator's hand on the ceiling — then the two narrowing moves are
insufficient, the withhold-to-zero move is too blunt to use, and the explicit
field becomes necessary. The correct response is then a superseding record,
not a patch, because the field's arrival makes the manifest's meaning change
again.

## Decision 2 — A node opens unfunded, and the operator's amount is a ceiling that never funds

**ADR 0039's item 2, quoted in full:**

> 2. **Answered: unfunded.** `qip_edge_node::assemble` calls
>    `Cell::with_unfunded_region(ceiling)`, and the node's health body carries
>    a `region_share` block saying why a node that places nothing places
>    nothing. The original argument, kept: unfunded is "capital granted in
>    advance" read strictly and changes what the ADR 0035 probe does until the
>    API ships it a share; at-ceiling keeps today's behaviour and keeps
>    today's double-spend window open until the first payload lands. The
>    record recommends unfunded and says what it costs.

**Decided: unfunded, ratified as a decision.**

The argument that carries it is the one about *when* the alternative fails,
not about which is more conservative in general. Ceiling-at-boot leaves the
region's grant unbounded across its cells for exactly the window between a
node starting and its first payload arriving — and that window is not a rare
condition. It is every deployment, every restart, every crash loop, and every
partition that outlives an envelope. It is also, precisely, the window in
which ADR 0035's shadow probe would first be observed. A control that holds
except during first boot is a control that does not hold during the only
period anybody has yet planned to watch.

`QIP_REGION_ALLOCATION` is therefore a **ceiling and never a funding**: the
most this process will ever accept as a share, capable only of narrowing what
the centre sends. ADR 0045 (*proposed*, concurrent) chooses the number for the
first node; this decision is what makes that number a bound on a share rather
than a balance a node may spend. The two records must be read together, and if
0045 is accepted while this is declined, the node would spend that number
without the centre ever having partitioned it — which is the double-spend the
whole of ADR 0039 exists to close.

**The cost, restated because it is a change in what an operator sees.** A
fresh node places nothing and its refusal count under `region_reservation`
climbs until the API ships it a share. From the order count alone that is
indistinguishable from a quiet market, and the mitigation is already applied:
the health body's `region_share` block carries `funded`, `bound`, `free`,
`ceiling`, `sequence` and `why`, and the banner prints `region_ceiling` and
`region_bound` as two facts. This decision depends on that block existing.
Removing it would leave the node's silence unexplained, and unexplained
silence is how "unfunded" becomes "broken" in an operator's reading.

**Rejected alternatives.**

- *Ceiling-at-boot.* Rejected above: it fails exactly during first boot,
  restart and partition-recovery.
- *A small bootstrap share — open at some fraction of the ceiling until the
  first payload.* Rejected because a bootstrap fraction is an amount nobody
  computed. It is the same defect as a sentinel netting ratio or a permanent
  `unavailable` capability reading: a number that looks like a measurement
  and is a guess. And it does not even remove the failure — it bounds the
  first-boot double-spend at *n* × fraction instead of at *n* × ceiling.
- *Refuse to boot without a share.* Superficially the strictest option and
  wrong: it makes a node's start depend on the centre being reachable, which
  is ADR 0008 consequence 2 inverted. A cell must be able to run, halt,
  journal and report while partitioned. Unfunded-and-running is exactly that
  state, and it is why the block says *why*.

**What would falsify this.** If the interval between a node's start and its
first share proves long enough that operators routinely see a node in the
unfunded state and route around it — restarting it, raising the ceiling,
setting `QIP_MESH_REGIONS` off — then unfunded-at-boot has produced a workaround
culture, which is worse than the window it closed. The fix would be a faster
first payload (decision 4), not a funded boot.

## Decision 3 — Membership is `QIP_MESH_REGIONS`, read at the API's composition root, and `CentralConfig` is declined

**ADR 0039's item 3, quoted in full:**

> 3. **Answered in code, not by an owner: `QIP_MESH_REGIONS`, read at the API
>    root.** Membership is an argument to `region_shares`, not a
>    `CentralConfig` field, and the root parses it beside `QIP_MESH_CELLS` and
>    refuses a served cell the declaration does not file. This is the one
>    answer an owner may still want to move, and moving it is a one-line change
>    at the root. The original recommendation, kept: `CentralConfig`,
>    operator-set and committed, as the arbitrage policy is. The alternative —
>    deriving *G* from `qip-capital-fabric`'s location balances — ties a grant
>    to treasury on hand, which is a different number with a different owner,
>    and is not recommended without its own record.

**Decided: `QIP_MESH_REGIONS` stays, and this is the right answer rather than
the one that happened.** ADR 0039's own recommendation — `CentralConfig` —
is overruled here, and the reason it was wrong is a fact that did not exist
when it was written.

The membership is half of a two-halves invariant: **every served cell must be
filed under exactly one region.** The other half is the served cell set, which
is `QIP_MESH_CELLS`, an environment value read at the API's composition root.
`RegionMembership::covering` enforces the invariant by name, and the root
calls it against `QIP_MESH_CELLS` **before the backbone opens**, so a served
cell filed nowhere stops the process naming the cell.

If membership were a `CentralConfig` field, the two halves would live in two
places with two owners, and `covering` could not run at start-up against a set
the config does not know — the root would have to reach into the kernel's
configuration to check its own environment, or the check would move after the
port is bound. `.claude/rules/architecture/00-boundaries.md` decides this
directly:

> `backend/crates/apps/` — composition roots. Configuration is read **here and
> only here**

and

> Bind ports and prove storage writable **before** reporting healthy.

A membership that cannot be checked against the served cells before the
backbone opens is a configuration error that becomes a runtime refusal, and
the refusal lands as a withheld share on a cell the centre is already talking
to. Refuse at start, not at the seam.

The second reason is fail-closed direction. Left unset, `QIP_MESH_REGIONS`
does not default a share: every live grant ships to every cell — the
one-cell-per-region shape — and the cycle says so beside the payload, naming
the variable. That is the honest degradation for a platform that has not yet
declared its regions, and it is visible in the response and on stderr rather
than silent.

**Rejected alternatives.**

- *`CentralConfig::regions`, as ADR 0039 recommended.* Rejected above: it
  separates the two halves of an invariant that must be checked together at
  the root, and it puts configuration in a runtime crate.
- *Deriving *G* from `qip-capital-fabric`'s `LocationBalance::on_hand`.*
  Rejected, and ADR 0039 already declined it: treasury on hand per (region,
  currency, venue) is a different number with a different owner. A grant is a
  permission; a balance is a fact about custody. Tying the first to the second
  makes a capital permission move when a settlement moves, which nobody asked
  for.
- *Deriving membership from what cells claim about themselves* — a cell's
  delta already carries its `region` (`qip-mesh/src/delta.rs`). Rejected, and
  this is the one that would be actively unsafe: ADR 0039 lists it as a
  condition that would make the whole record wrong ("If membership must be
  discovered from what cells claim about themselves, a cell's share would rest
  on the cell's own word, and the share is no longer capital granted in
  advance. Stop there."). Recorded here so that a future reader finds it
  refused rather than unconsidered.
- *A Terraform-rendered file mount rather than an environment value.*
  Considered and not taken now: the membership is not credential material, it
  sits beside `QIP_MESH_CELLS`, and splitting the two halves across two
  mechanisms is the very thing decision 3 is avoiding. If `QIP_MESH_CELLS`
  ever becomes a file, this follows it in the same commit.

**What this changes in a deployment: nothing, today.** `QIP_MESH_REGIONS` is
argued unset for every Cloud Run deployment in the manifest-wiring suite for
the reason `QIP_MESH_CELLS` is — the mesh has no port there — and
`execution_nodes = {}` in every environment, so there is no cell to file.
Ratifying this decision does not set a variable anywhere, and this record does
not authorise setting one.

**What would falsify this.** If the API is ever deployed in a shape where the
served cell set is not known at start-up — cells registering themselves, or a
mesh discovered rather than declared — then `covering` cannot run at the root
and the argument above collapses along with the shape it defends. That is a
larger change than a configuration move and would supersede this decision
rather than amend it.

## Decision 4 — The payload cadence is not a free number; it is already pinned at sixty seconds by the fastest slot the cell reads, and the missing thing is a driver, not a value

**ADR 0039's item 4, quoted in full:**

> 4. **Still open, and the only one: the payload cadence** the region's
>    efficiency depends on. A number, from the owner, once the probe has shown
>    what idle share looks like — which needs a deployed node, and
>    `execution_nodes` is empty in every environment, so the probe has shown
>    nothing yet.

**Decided: the payload cadence is sixty seconds or faster, the share has no
cadence of its own and gets none, and the honest open item is that nothing
schedules the cycle at all.**

The question as ADR 0039 asked it presumes the cadence is a free parameter
whose only constraint is capital efficiency. It is not. Three facts in the
tree bound it before efficiency is reached.

**First: the share rides the payload, deliberately.** ADR 0039 lists among its
costs:

> **A share coupled to the payload's sequence** arrives only with a whole
> payload. A share cannot be re-issued alone, which is deliberate (one source
> of truth) and means the payload cadence is the share cadence.

So there is one cadence to choose, not two, and choosing a share cadence
separately would be ADR 0039 option (c) returning under a new name — a second
signed message with its own sequence discipline interleaved with the
payload's, which that record rejected as a second source of truth for the
cell's bound.

**Second: the payload's cadence is pinned from below by its fastest-expiring
slot the cell actually reads.** The slots `qip-edge` reads from a payload are
`cycle_whitelist`, `compiled_plan`, `capital_grants` and
`feasibility_constraints`
(`grep -n 'payload()\.' backend/crates/edge/qip-edge/src/cell.rs`). Their
times to live are 60 s, 86,400 s, 3,600 s and 86,400 s respectively
(`qip-contracts/src/policy.rs:113-135`). The whitelist is the binding one, and
`Cell::cycle_whitelist` reads it fresh-only — stale reads as `None`
(`cell.rs:715-733`), with the reason stated there:

> a desk built from a whitelist the centre has stopped republishing would
> price a graph the centre may since have withdrawn

A payload cadence slower than sixty seconds therefore delivers shares to a
cell that has no fresh permission to use them. The idle-share inefficiency
ADR 0039 worried about would be the second-order effect of a first-order
outage: the cell would hold a bound and be unable to trade under it. **Sixty
seconds is not a target chosen for efficiency; it is the point below which
the payload stops working as a payload**, and the share's cadence question is
answered by a clock that was already there.

**Third, and this is the finding that changes the shape of the answer:
nothing schedules a cycle.** The payload is built inside the handler for
`POST /cycle` (`qip-api/src/routes.rs:1167`, `pending_policy` called at
`:1263`, under the platform lock), and that route requires `Role::Analyst`
(`:141-147`). Nothing in the repository calls it periodically:
`grep -rn 'cloud_scheduler\|google_cloud_scheduler' infrastructure/` returns
**no matches**. The node half is the same shape — `qip-edge-node`'s serve loop
runs one mesh exchange per accepted connection on its health port, and its own
comment says why that is a compromise (`qip-edge-node/src/main.rs:749-754`:
"this node has no scheduler, so the liveness probe is the only periodic event
it has… tying capital renewal to how often something asks whether the cell is
alive is a compromise, not a design").

So writing "the cadence is sixty seconds" into a record today would be a claim
about the system that nothing in the system makes true. What this decision
therefore says, in two halves:

- **The bound: a payload cadence slower than sixty seconds is refused.** Not
  as a runtime check — there is nothing to check it in — but as the standard
  any scheduler proposal is measured against. A cadence at the
  `capital_grants` slot's own hourly TTL, which is the number a reader would
  reach for because it is the *share's* slot, is specifically wrong: the share
  would be fresh inside a payload whose whitelist had been stale for
  fifty-nine minutes.
- **The open item is a driver, and it is not the number ADR 0039 asked for.**
  Until something periodic drives `POST /cycle`, this platform has no payload
  cadence in the sense the question means, and the correct answer to "what is
  the cadence" is "there isn't one". Naming a number while the driver is an
  analyst's manual request would be the class of statement this repository
  treats as false about the system.

**Rejected alternatives.**

- *A separate, faster cadence for the share alone.* Rejected: ADR 0039 option
  (c), a second signed path to the cell's bound, which that record's own
  reversal conditions name as grounds for superseding it.
- *The `capital_grants` TTL — one hour — as the cadence.* Rejected above: the
  slowest slot the cell reads cannot set the payload's clock, because the
  payload is one signed object and its fastest slot governs.
- *Defer, as ADR 0039 did, until the ADR 0035 probe shows what idle share
  looks like.* Rejected as the answer to the *whole* question, and kept as the
  answer to half of it. It is rejected as a whole because the upper bound is
  derivable from the payload's own clocks without any probe, and because the
  probe cannot run: `execution_nodes = {}` in every environment, so deferring
  to it defers indefinitely. It is kept for the remaining half: whether sixty
  seconds is *fast enough* — whether idle share between payloads costs enough
  to justify going faster — is genuinely an empirical question that needs a
  deployed node and the `free` figure at each cell. This record fixes the
  ceiling on the interval and leaves the floor to evidence.
- *Have the cell request a payload when it is starved.* Rejected: a cell
  blocking on the centre is ADR 0008 consequence 2, and the non-blocking
  version of it is the delta the cell already sends, carrying its refusals
  under `region_reservation` — which is the input the centre re-partitions
  from, and needs no new message.

**What would falsify this.**

- **If a slot the cell reads gains a TTL shorter than sixty seconds**, the
  bound moves with it, and this decision's number was never the point — the
  rule ("the payload's cadence is its fastest-read slot's TTL") is.
- **If `qip-edge` ever begins reading the risk-envelope slot**, whose TTL is
  thirty seconds (`policy.rs:130`), the bound halves. Today
  `grep -rn risk_envelope backend/crates/edge` returns nothing, so that slot
  is produced by the API and read by no cell, and it does not bind. This is
  stated because it is the most likely way the number here goes stale.
- **If the observed idle share at sixty seconds is large**, the floor rather
  than the ceiling becomes the binding question, and the record that answers
  it needs the probe. That is the half this decision deliberately does not
  take.
- **If a scheduler arrives that cannot hold sixty seconds** — a Cloud
  Scheduler minimum, a rate limit at the API, a cycle whose own duration
  exceeds its period — then the platform cannot meet the bound its own
  payload defines, and the answer is to lengthen the whitelist's TTL in a
  record that argues for it, not to ship stale whitelists quietly.

## What changes, by crate, if accepted

| Crate / path | Layer | Change |
|---|---|---|
| `qip-contracts` | lib | **None, and permanently.** Decision 1 declines the `RegionShare` field; the `capital_grants` slot keeps the meaning it has |
| `qip-kernel` | runtime | None. `central/regions.rs` — `RegionMembership`, `RegionShare`, `partition`, `grant_manifests` — is what decisions 1 and 3 ratify |
| `qip-edge` | edge | None. `RegionAllocation::unfunded`/`rebase`/`rederive` and the `region_share` gate are what decision 2 ratifies |
| `qip-edge-node` | app | None. `assemble` already calls `Cell::with_unfunded_region(ceiling)`, and decision 2 fixes `QIP_REGION_ALLOCATION` as a ceiling |
| `qip-api` | app | None. `QIP_MESH_REGIONS`, `RegionMembership::parse` and the `covering` check before the backbone opens are what decision 3 ratifies |
| `docs/plan/PROJECT-PLAN.md` | register | PHASE-B24's blocking condition and DEC-D3's neighbour rows are re-scored by whoever applies this, with the applying commit, not before |
| `docs/architecture/algorik-blueprint-traceability.md` | matrix | The F6 row's cross-process half is re-scored from "operator discipline, not a structural guarantee" only once a node exists to be disciplined; not by this record |
| infrastructure | infra | **Nothing, and this record authorises nothing.** No tfvars are edited; `execution_nodes = {}` stays `{}` |

## What is still not true after this record

Said plainly, because three ratifications in a row read like progress and
this one moves no deployed byte.

- **Nothing is deployed.** `execution_nodes = {}` in every environment
  (`grep -rn execution_nodes infrastructure/environments/*/terraform.tfvars`),
  so no cell of any funding state runs in any project, and every sentence in
  ADR 0039 and in this record about what a cell does is a sentence about what
  `cargo test` observed.
- **`QIP_MESH_REGIONS` is set nowhere.** Every Cloud Run deployment is argued
  to have it unset, so the shipped shape is still "every live grant to every
  cell" and the cycle says so beside each payload.
- **There is no payload cadence**, in the sense decision 4 establishes, until
  something periodic drives `POST /cycle`.
- **Decision 4 is half-taken by design.** The ceiling on the interval is
  decided; whether to go faster is not, and cannot be until a node runs.

## Dependency-direction argument

No decision here moves an edge, and three of them move nothing at all. The
graph, restated from ADR 0039 and re-checked against it:

`qip-contracts` (lib) ← `qip-mesh` (service) ← `qip-kernel` (runtime) ←
`qip-api` (app); `qip-contracts` ← `qip-edge` (edge) ← `qip-edge-node` (app);
`qip-capital` (service) ← `qip-kernel`, and *not* ← `qip-edge`, which
`qip-acceptance/tests/architecture.rs::no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy`
enforces.

- **Decision 1** declines a field in `qip-contracts`. A lib that gains nothing
  gains no edge, and declining removes the only crate change ADR 0039's option
  (a) would have made to a lib.
- **Decision 2** ratifies `Cell::with_unfunded_region`, a builder on
  `qip-edge`'s own type taking a `Decimal` from `qip-core`. The ceiling is
  read in `qip-edge-node`, an app, which is the only layer permitted
  `std::env`. `qip-edge` reaches for nothing; it is handed a value, the same
  shape as `Cell::with_metrics` being handed a registry.
- **Decision 3** keeps membership in an app's environment rather than moving
  it into `CentralConfig` in the runtime. This is the decision that most
  actively *protects* the direction: it keeps configuration at the composition
  root, as `.claude/rules/architecture/00-boundaries.md` requires, and it
  keeps the runtime free of an environment-shaped input. `RegionMembership`
  itself lives in `qip-kernel` and is passed *in* as an argument to
  `region_shares` and `grant_manifests`, never read by it.
- **Decision 4** touches no crate. Its subject is a scheduler that does not
  exist; when one arrives it will call an HTTP route from outside the
  workspace, which is not a dependency in the sense this section means.

No lib comes to depend on a service, no service on the runtime, no crate on an
app. `qip-edge`'s dependency set is unchanged and in particular still excludes
`qip-capital` and `qip-mesh`; the two ends of the wire still agree on nothing
but the vocabulary in `qip-contracts`.

## What it costs

Collected here because the acceptance suite requires it in one place, and
because four decisions taken together cost more than any one of them does
alone. Each cost below is argued in the decision it belongs to; none is new.

**Decision 1 — declining the explicit `RegionShare` field** costs exactly what
ADR 0039 already named: the cell's share can never be *narrower* than its
envelopes' sum by the centre's say-so alone. Narrowing below that needs the
envelopes renewed smaller, which is the allocator's existing path and is
slower than a field would be. The price of one source of truth is that the
centre cannot tighten a cell in one message.

**Decision 2 — a node opening unfunded** costs an operator's certainty at
exactly the moment they are least able to spare it. A fresh node places
nothing and its `region_reservation` refusals climb until the API ships a
share, and from the order count alone that is indistinguishable from a quiet
market. The mitigation is already applied and this decision *depends* on it:
the health body's `region_share` block carries `funded`, `bound`, `free`,
`ceiling`, `sequence` and `why`, and the banner prints `region_ceiling` and
`region_bound` as two facts. Remove that block and the node's silence becomes
unexplained — which is how "unfunded" is read as "broken".

**Decision 3 — membership at the API's composition root** costs a second place
a deployment must be got right, and couples the share's correctness to an
environment value rather than to a signed artifact. It buys the thing that
makes it worth paying: `covering` can check membership against
`QIP_MESH_CELLS` before the backbone opens, which `CentralConfig` could not.

**Decision 4 — a sixty-second cadence** costs capital efficiency at the margin:
a cell's share is at most sixty seconds stale, so the centre cannot reallocate
faster than that even when it would be right to. The cost is bounded by the
same slot TTL that sets it, and the real outstanding cost is not this number
at all — there is no driver. `grep -rn 'cloud_scheduler' infrastructure/`
returns nothing and `POST /cycle` requires an analyst, so today the cadence is
whatever a person's hand supplies.

**The cost the four share.** Every one of them ratifies a mechanism that has
never run outside `cargo test`, against `execution_nodes = {}` in all four
environments. Accepting this record makes the arrangement *decided*, not
*proven*, and a reader who takes it for the second has been misled by nothing
in it.

## What would make this wrong

- **A slot the cell reads gains a TTL below sixty seconds.** Decision 4's
  cadence is pinned by the fastest slot, so a faster one re-opens it.
  `risk_envelope` already carries thirty seconds — it is produced and read by
  no cell today (`grep -rn risk_envelope backend/crates/edge` returns
  nothing), and the day a cell reads it, this record's number is stale.
- **The centre must narrow a cell below its envelopes' sum faster than a
  renewal allows.** That is decision 1's declined field returning as a real
  requirement rather than a convenience, and it should be reconsidered as one.
- **Membership must be discovered from what cells claim about themselves.**
  ADR 0039 names this as a condition that voids the whole arrangement, because
  a share resting on a cell's own word is no longer capital granted in
  advance. Stop there rather than adapting this record.
- **`QIP_MESH_CELLS` becomes a file mount.** Decision 3's argument is that the
  two halves of membership belong in one mechanism; if one half moves, this
  follows it in the same commit.
- **A node's time from start to first share stops being short.** Decision 2
  trades a window of unfunded operation for partition tolerance. If that
  window is long in practice rather than in principle, the trade is worse than
  it looks here and the bootstrap alternatives deserve re-reading.

## What was not run

No cargo gate ran for this record, and none applies: it changes no code, no
`Cargo.toml`, no test, no Terraform and no tfvars. `cargo fmt`,
`cargo clippy`, `cargo test`, `terraform fmt -check` and `terraform validate`
were **not run**, and are named here as not run rather than omitted. The
existing evidence for the mechanism this record ratifies is ADR 0039's, not
re-run here: five tests in
`backend/crates/tests/qip-acceptance/tests/region_share.rs`, whose count that
record establishes with `grep -c '^#\[test\]'` and whose names it lists.
That count was **not re-run in this session** and is quoted from ADR 0039
rather than verified.

`./scripts/check-secrets.sh` was run because this record adds a file to the
repository; its output is quoted in the session report.
