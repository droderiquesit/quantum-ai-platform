# 0039 — A region's grant is shared across its cells over the mesh, as disjoint per-cell shares the centre signs

**Status:** *proposed*. Nothing here is applied. No crate is changed, no
slot is produced, no test named below exists, and no environment is affected.
The owner accepts, amends or declines the recommendation in "Decision to be
taken", and the record moves to *accepted* only with the commit that applies
it.
**Decides:** how one region's capital grant is shared by that region's cells
when each cell runs in its own process, without a cell ever waiting on the
centre or on a sibling.
**Relates to:** ADR 0008 (cells decide alone, on capital granted in advance —
the constraint every option below is measured against), ADR 0024 (one
execution node per region, one cell per process — the deployment shape that
makes this a cross-process problem), ADR 0035 (one node in shadow mode in
`dev` — the only place the change would first be observed), ADR 0002 and
0009 (no crate is added by any option), ADR 0011 (the mesh is a hub between
cells and centre, not a peer network).
**Does not touch:** the paper-trading boundary's three layers. A share of a
grant is checked after all three and cannot reach past them; no option here
creates, eases or implies an order path. `CapitalEnvelope`'s private fields
and no-setter rule (ADR 0008 consequence 1) are unchanged by every option.

## Context, as verified in the working tree on 2026-09-05

Every reference below was read in the tree, not taken from a commit message.
Line numbers are as of this date and move when the file above them grows;
recount before quoting one again.

### What the blueprint asks for

§25.1's decision chain (`algorik-blueprint-v10.1-source.md:2148-2151`) names
capital grants as "bounded, expiring permission per family **per region**,
shipped outward", and closes the table with the sentence every option here
must honour (`:2160`): "A grant expires rather than being revoked — if a
region loses contact, its grants age out and its capacity to deploy shrinks
on its own." §33's gate table (`:2743-2745`) lists "settlement-aware
reservation — reservation table plus projected timeline — veto", and the
capability list at `:302` names the "inventory manager and reservation
table" as one regional component. §26 is the strategy engine's budget and
says nothing about where the table lives; it is cited by the F6 row because
sizing (`:2328`) is the step that consults it. Nowhere does the blueprint ask
two cells to agree with each other; the only agreement it describes is
between a cell and the grant it was handed.

### What the tree holds

- **The per-region table exists and is proven in one process.**
  `backend/crates/edge/qip-edge/src/reservation.rs` holds `RegionAllocation`
  (a ledger: `free`, holds keyed by owning cell, `committed`) and
  `RegionTable`, an `Arc<Mutex<RegionAllocation>>` handle a composition root
  gives to every cell of a region through `Cell::with_region_table`
  (`cell.rs:635`); `Cell::with_region_allocation` (`cell.rs:621`) opens a
  private one. `qip-edge/tests/region_table.rs::two_cells_under_one_region_table_cannot_each_spend_the_whole_grant`
  (`:398`) proves two cells over one table refuse against each other, with
  the contrast that the same two cells over two separate tables both send —
  so it is the sharing, not the amount, that refuses.
- **The deployment is one cell per process, and the table is per process.**
  `qip-edge-node/src/lib.rs::assemble` (`:96-108`) opens a private table
  over `QIP_REGION_ALLOCATION` (`allocation.rs:41`), read once at start and
  refused when absent or non-positive. Two nodes under one regional grant
  therefore hold two tables, and nothing checks that the two operator-given
  amounts sum to the region's grant. The traceability matrix's F6 row
  (`algorik-blueprint-traceability.md:630-639`, re-scored 2026-09-05) calls
  this "operator discipline, not a structural guarantee", and
  `reservation.rs:36-47` says the same in the code: "nothing on any wire the
  cell receives carries a region allocation … making it the centre's number
  needs a signed field on the wire and a producer at the centre, in two
  crates this one may not edit."
- **The centre already computes a per-cell number.** `CentralPlane::allocate`
  (`qip-kernel/src/central/plane.rs:957`) returns an `AllocationPlan` whose
  allocations are keyed by strategy and cell; `AllocationPlan::for_cell`
  (`qip-capital/src/allocation.rs:310`) sums one cell's gross, and
  `is_within_budget` (`:301`) is an exact fixed-point invariant that the
  whole plan does not exceed its budget. The envelopes the centre issues
  (`plane.rs:545`, `envelope()` at `:824`) are keyed `(cell, strategy)` and
  carry a `gross_limit` each. What does not exist: any notion of a *region*
  at the centre. `grep -ri region backend/crates/runtime/qip-kernel/src/central`
  returns nothing. The only region-keyed amount in the workspace is
  `qip-capital-fabric::LocationBalance::on_hand`, per (region, currency,
  venue) — treasury on hand, not a grant. A cell's delta does carry its
  `region` (`qip-mesh/src/delta.rs:161`), but that is the cell's claim, and
  granting on a cell's claim about itself is the thing ADR 0008 exists to
  prevent.
- **The wire has a signed, per-cell payload with a capital slot in it.**
  `qip_contracts::policy::PolicyPayload` (`policy.rs:386-418`) is one signed
  payload per cell with twelve typed slots and `deny_unknown_fields`; its
  signature covers a digest of every serialised slot (`:493-496`). Slot 7,
  `capital_grants: Slot<GrantManifest>` (`:410`), carries the signatures of
  the cell's live grants and is documented as "a manifest, not a delivery
  path" (`:267-274`) because the envelopes have their own verified channel.
  The `CycleWhitelist` slot shows how a field is added to a signed slot
  without breaking a signature taken before it existed:
  `#[serde(default, skip_serializing_if = …)]` (`:332-338`), with the
  stated consequence that a cell built before the field refuses a payload
  carrying it, by `deny_unknown_fields`, "which is the safe direction".
  `PolicyItem::CapitalGrants` has a one-hour time-to-live (`:126`).
- **The producer and the consumer are in the apps, on either side of the
  kernel.** `qip-api/src/mesh.rs::pending_policy` (`:644-684`) builds one
  payload per configured cell and fills three of the twelve slots — the
  grant manifest at `:667` from `central.envelope(cell, strategy)`, the risk
  envelope, the cycle whitelist — leaving the rest unproduced, "the
  fail-closed design, not an omission". `qip-mesh`'s `PolicyFrame`
  (`spine.rs:533`) is `#[serde(transparent)]` over the payload and carries
  whatever the vocabulary defines. At the cell, `qip-edge-node/src/mesh.rs:428`
  calls `Cell::apply_policy` (`cell.rs:1021`), which refuses a payload for
  another cell, refuses a sequence at or below the last applied, and swaps
  the verified payload in whole.
- **A cell may not reach `qip-capital`.** `architecture.rs::no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy`
  (`:681`) asserts it, and `reservation.rs:27-34` explains that the
  reservation discipline is reproduced in `qip-edge` rather than reused for
  that reason. Every option below is measured against that test staying
  exactly as it is.

### What ADR 0008 fixes, and so what every option must satisfy

ADR 0008 is accepted and not reopened here. Its three enforced consequences
translate into a test any design must pass before its efficiency is worth
discussing:

1. **A cell's authority is a data structure it was handed, not a policy it
   follows.** Whatever bounds a cell's spending arrives signed, from the
   centre, and the cell cannot widen it in place.
2. **A partitioned cell keeps working within what it already holds, and its
   authority shrinks on its own.** It never blocks on the centre — and, by
   the same argument, never on a sibling cell, because a sibling is outside
   the process and reachable only over a network that can partition.
3. **Nothing on the hot path consults anything outside the process.** A hold
   is a mutex acquisition and a comparison (`reservation.rs:280-283`); it
   stays that.

The efficiency question — how much of a region's grant is *usable* when its
cells are unevenly busy — is real and is priced below. It is priced second.

## The decision to be taken

When a region's grant is *G* and the region runs *n* cells in *n* processes,
what bounds each cell so that the *n* cells together cannot commit more than
*G*, without any cell waiting on the centre or on another cell?

Secondary, and part of whichever option is taken: where the per-cell bound
travels (it must be signed), what the cell does with its `RegionTable` when
one arrives, and what a cell that has *never* received one may spend.

## Options

### Option (a) — the centre partitions the grant per cell in the signed payload; each cell's envelope is disjoint by construction

The centre computes, for every cell it ships a payload to, a **share**: the
most that cell may hold and commit in total, from the region's grant, with
the invariant that the shares of a region's cells sum to at most *G*. The
share travels in the payload's `capital_grants` slot, signed with everything
else. At the cell, `RegionTable` stops being "one table for the region" and
becomes **the cell's private view of its own disjoint share**: the table is
opened closed, re-based to the share when a payload carries one, and only
ever re-based by a payload with a higher sequence. No cell needs to know
what any other cell holds, because the sum was made safe before anything
was shipped.

*Shape.*

- **Vocabulary (`qip-contracts`, a lib).** `GrantManifest` gains one
  additive field, `region_share: Option<RegionShare>` under
  `#[serde(default, skip_serializing_if = "Option::is_none")]`, exactly the
  `CycleWhitelist::conversions` pattern, so every payload signed before the
  field existed verifies unchanged. `RegionShare { region: String, amount: Decimal }`,
  `deny_unknown_fields`, `Decimal` because it bounds money. The slot's
  documentation changes from "a manifest, not a delivery path" to "a
  manifest of grants that travel their own channel, and the one bound that
  has no other channel" — the reason the manifest refused to carry envelopes
  (a second claim about a fact the envelope channel already carries) does not
  apply to a fact nothing else carries. It is not a thirteenth item: §41.5's
  item 7 is defined by the blueprint as "per family per region", so the
  share is that item's content. `PolicyItem` stays at twelve and its
  exhaustive matches are untouched.
- **Wire (`qip-mesh`, a service).** Nothing. `PolicyFrame` is transparent
  over the payload; the slot digest and signature cover the new field
  because they cover the serialised slot. The task that commissioned this
  record named "a payload slot in `qip-mesh`"; the slot's type lives one
  layer down, in the vocabulary both ends may name, and that is where it
  must go — `qip-edge` may not name a `qip-mesh` type (`spine.rs:70-78`).
- **Partitioner (`qip-kernel`, the runtime).** A pure function on
  `CentralPlane`, of the shape
  `region_shares(&AllocationPlan, &RegionMembership) -> Result<BTreeMap<String, RegionShare>>`,
  where `RegionMembership` is configuration in `CentralConfig` (as
  `ArbitragePolicy` is) mapping each configured cell to its region and each
  region to its grant *G*. Each cell's share is `plan.for_cell(cell)` — the
  same number its envelopes were issued against, so the share and the
  envelopes are one fact from one source, not two claims (`CLAUDE.md`
  principle 6). The function *refuses* — never scales — a plan whose cells'
  sums exceed a region's *G*, and a cell not in any region gets no share.
  `BTreeMap` throughout, so the same plan yields the same shares on every
  machine. A remainder of *G* left over after the shares is left
  unallocated, not rounded onto anyone.
- **Producer (`qip-api/src/mesh.rs::pending_policy`, an app).** Places the
  cell's share into the slot it already fills at `:667`. A refusal from the
  partitioner ships the slot without a share and says why beside the
  whitelist line, the pattern `:676-679` already uses: the cell narrows, and
  never receives a share the centre had to guess at.
- **Cell (`qip-edge`, edge).** `RegionTable` gains `unfunded(ceiling)`,
  which opens with `free = 0` and records the operator's ceiling, and
  `rebase(share)`, which sets the effective bound to `min(share, ceiling)`
  and recomputes `free = effective − held − committed`. `Cell::apply_policy`
  calls `rebase` when the applied payload's `capital_grants` is produced and
  carries a share for the cell's own region; the sequence discipline already
  in `apply_policy` (`cell.rs:1029-1037`) is what makes a share
  un-widenable by replay. Three rules, each stated rather than left to the
  arithmetic:
  1. A share smaller than `held + committed` is a legitimate narrowing, not
     an invalid input: `free` becomes zero, the deficit is journaled as its
     own `Decision` variant and carried in the next delta, and nothing is
     un-sent. That is a stated ledger state, not a clamp — the input was
     valid and the cell cannot undo an order.
  2. A share naming another region is refused under a `region_share` gate,
     and the table re-bases to **zero**: the centre and the node disagree
     about where this cell is, and spending under that disagreement is
     spending a share meant for someone else.
  3. A cell whose slot goes stale (one hour) keeps its last share. It cannot
     widen without a fresh sequence, and its envelopes expire on their own
     clock (ADR 0008 consequence 2); a second expiry on the share would
     double the partition failure mode without bounding anything the
     envelope does not.
- **Node (`qip-edge-node`, an app).** `assemble` opens the table with
  `unfunded(ceiling)` rather than `with_region_allocation(amount)`.
  `QIP_REGION_ALLOCATION` stays, renamed in meaning only: it is the most
  this process will ever accept as a share, a local backstop that can only
  narrow, which is what `allocation.rs:30-34` already says it is.

*What it satisfies.* Every ADR 0008 consequence, by construction: the
share is signed and handed down (1); a partitioned cell spends within the
last share it applied and its envelopes age out (2); a hold is still a
mutex and a comparison (3). Two cells in two processes cannot together
exceed *G* because the centre never shipped shares that could.

*What it costs.* Capital efficiency. A cell with an idle share cannot lend
it to a busy sibling until the next payload re-partitions, and today the
payload is issued per central cycle. In a region whose busiest cell is
consistently starved while its quietest sits idle, *G* is under-used by
exactly the idle shares. A second cost is at first boot: a node opened
`unfunded` sends nothing until the centre's first payload reaches it, which
changes the ADR 0035 node's observed behaviour — refusals under
`region_reservation` climb until the `dev` API has shipped it a share. That
is the honest reading of "capital granted in advance": a cell nobody has
granted to has nothing in advance. It is also a deployment-visible change
and is the owner's second decision below.

*What evidence closes it.* Named in "The tests that would prove it".

*Dependency direction.* `qip-contracts` (lib) gains a field in a type it
owns; `qip-kernel` (runtime) gains a function over two types it already
depends on (`qip-capital`, `qip-contracts`); `qip-edge` (edge) gains two
methods on its own `RegionTable` and one branch in `apply_policy` over a
`qip-contracts` type it already reads; `qip-api` and `qip-edge-node` (apps)
change their composition. No lib gains an edge to a service, no service to
the runtime, nothing to an app, and `qip-edge` does not gain `qip-capital`.

### Option (b) — a per-region ledger over the mesh, holds gossiped between cells, reconciled by the centre

One logical `RegionAllocation` for the region; each cell holds locally and
publishes the hold to its siblings, and the centre reconciles the union
against *G* after the fact.

*What it is, said plainly.* A distributed exclusive resource. A hold is not
a grow-only fact: two cells that each hold the last unit of *G* have not
both held it, one of them has overspent, and which one is a question no
cell can answer without hearing from the other. Under a partition between
two cells that both still reach their venues, they either wait for each
other — a cell blocking on something outside its process, which ADR 0008
consequence 2 forbids — or they both proceed and the region overspends,
which is the defect this record exists to close, reproduced with a message
bus in front of it. Making it safe needs an ordering authority (a leader,
a quorum, or a fencing token), which is a consensus problem. The blueprint
does not ask for one: its only agreement is between a cell and its grant
(`:2160`), and ADR 0011's mesh is a hub between cells and the centre, not a
peer link. `qip-edge` would need a cell-to-cell transport that does not
exist, a peer identity model that does not exist, and a reconciliation at
the centre that ADR 0008's own reversal condition 2 already warns about:
"if aggregate exposure across cells cannot be kept accurate enough to be
worth having, then granting capital in advance is granting it blind."

*What it buys.* The idle-share loss of (a) is recovered, when the cells can
hear each other — which is the case in which it was least needed.

*What it costs.* A new failure class (split-brain over capital) in the
highest-consequence path; a new transport; a hold that is no longer a
comparison under a mutex but a message with a timeout; and a centre that
reconciles three views of one number. It also weakens (a)'s cleanest
property: with (a) a reconciliation break is a cell disagreeing with the
centre, one direction, already charted by
`qip_central_reconciliation_breaks_total`; with (b) it can be two cells
disagreeing with each other and both with the centre.

*Rejected.* Not for effort — for the shape of the guarantee. It trades a
bound that holds by construction for one that holds when the network does.

*Dependency direction.* `qip-edge` would gain a peer transport, and either
`qip-mesh` would learn to route between cells (a service reaching sideways)
or a new crate would appear, which is a dependency decision this record
declines to open.

### Option (c) — disjoint shares by default, with a bounded, signed re-balance the centre issues on request

Option (a)'s shares, plus a path by which a starved cell asks the centre for
more and the centre, if a sibling's share is idle, issues a signed
re-balance that narrows one cell and widens the other.

*What it is, on inspection.* Two halves. The *widening* half is exactly what
(a)'s next payload already does: a fresh `AllocationPlan`, a fresh set of
shares, a higher sequence. The *request* half adds a cell-to-centre ask
that the cell then waits on, or does not. If it waits, it blocks on the
centre (ADR 0008). If it does not wait, the request is a hint the centre
folds into its next allocation — which is a delta, and the cell already
sends one, carrying its refusals (`DeltaRefusal`, `delta.rs:92`), including
refusals under `region_reservation`. The centre can read "this cell was
refused *k* times under its share" from what it already receives and
re-partition on the next cycle. The bounded re-balance is (a) with a
cadence short enough to matter, and the request is a delta the cell already
sends.

*What the "signed re-balance" would add that (a) lacks.* A narrowing that
lands on the idle sibling *between* payloads. Under (a) the sibling narrows
at the next payload too; the difference is latency, and the latency is the
centre's cycle period.

*What it costs.* A second signed message type with its own sequence
discipline interleaved with the payload's, so that a re-balance and a
payload can arrive out of order and a cell must decide which one bounds it
now — a second source of truth for the cell's share, which the boundaries
rule forbids. And an asymmetry: a widening for cell A is only safe if the
narrowing for cell B was applied *first*, which reintroduces (b)'s ordering
problem between two processes in miniature.

*Not taken as written.* Its useful content is absorbed into (a) as one
sentence: the centre re-partitions from the deltas it receives, on its own
cadence, and the cadence is the owner's to shorten.

## Recommendation — marked as a recommendation, not a decision

**Option (a).** Disjoint per-cell shares the centre computes from the
allocation plan it already produces, signed into the `capital_grants` slot
of the payload each cell already verifies, applied at the cell by re-basing
a `RegionTable` that becomes the cell's private view of its own share.

Why, in the order the reasons weigh:

- **It is the only option under which the bound holds when the network does
  not.** (b) holds when cells can hear each other; (c)'s re-balance holds
  when the narrowing lands before the widening. (a) holds because nothing
  that could exceed *G* was ever shipped. That is the ADR 0008 standard —
  the bound is what a cell may do before anyone can intervene — applied one
  level up, to a region.
- **It adds no mechanism the tree does not already have.** The payload is
  signed and per cell; the slot exists; the additive-field pattern is
  proven on `CycleWhitelist`; the per-cell number is `for_cell`; the
  sequence discipline that makes a share un-replayable is `apply_policy`'s.
  What is new is one field, one fold, two ledger methods and one branch.
- **Its cost is measurable from the log alone.** Idle share is
  `free` at each cell, which the delta can carry; starvation is refusals
  under `region_reservation`, already charted. The desk can see exactly
  what (a) leaves on the table, and shortening the payload cadence recovers
  it without a new decision.
- **It keeps the cell's hot path a comparison under a mutex.** (b) makes it
  a message; (c) makes it a race between two messages.

The honest cost of this recommendation: a region whose cells are unevenly
busy under-uses its grant by its idle shares between payloads, and a fresh
node sends nothing until it has been granted to once. The first is priced
above and is the desk's to tune; the second is the owner's second decision.

## What changes, by crate, if accepted

| Crate | Layer | Change |
|---|---|---|
| `qip-contracts` | lib | `RegionShare`; `GrantManifest::region_share`, additive and skipped when absent; the slot's doc comment says what it now carries and why |
| `qip-kernel` | runtime | `CentralConfig::regions` (cell to region, region to *G*); `CentralPlane::region_shares` — deterministic, refuses rather than scales |
| `qip-api` | app | `pending_policy` places each cell's share; a refusal ships the slot without one and says why |
| `qip-edge` | edge | `RegionTable::unfunded`, `RegionTable::rebase`; `apply_policy` re-bases on a share for its own region; two `Decision` variants (share applied, share deficit); refusal gate `region_share`; `free` and the effective bound in the delta |
| `qip-edge-node` | app | `assemble` opens `unfunded(ceiling)`; `QIP_REGION_ALLOCATION` documented as the ceiling |
| `qip-mesh` | service | nothing; `PolicyFrame` is transparent over the payload |

The traceability F6 row and `.claude/rules/domains/risk-and-execution.md`
are re-scored with the applying commit, not before.

## The tests that would prove it

Each new test asserts its premise first, is named as the property, and is
mutation-verified; the mutation is named beside it.

- `qip-contracts/tests`: `a_payload_signed_before_region_share_existed_still_verifies`
  — mutation: drop `skip_serializing_if`, watch the stored signature fail.
- `qip-kernel/tests/region_shares.rs`:
  `a_regions_shares_are_disjoint_and_sum_to_at_most_its_grant` over a
  fixture plan with three cells in two regions — mutation: assign each cell
  the whole *G*, watch the sum assertion fail;
  `a_plan_whose_cells_exceed_a_regions_grant_is_refused_not_scaled` —
  mutation: scale instead of refuse, watch the `Err` assertion fail;
  `a_cell_in_no_region_receives_no_share`.
- `qip-api/src/mesh.rs` tests:
  `pending_policy_ships_each_cell_its_own_share_and_no_two_shares_overlap`
  — mutation: ship `plan.budget` to every cell.
- `qip-edge/tests/region_table.rs`:
  `two_cells_in_two_processes_under_disjoint_shares_cannot_together_exceed_the_regions_grant`
  — two cells over two *separate* tables (the deployment's shape), each
  re-based from a payload carrying its share, total sent ≤ *G*, with the
  contrast that shares summing past *G* let both send (so it is the
  partition, not the amount) — mutation: make `rebase` ignore `amount`;
  `a_share_below_what_the_cell_already_committed_narrows_free_to_zero_and_journals_the_deficit`
  — mutation: let `free` go negative;
  `a_share_for_another_region_is_refused_and_the_table_narrows_to_nothing`
  — mutation: apply it anyway;
  `an_older_sequence_cannot_widen_a_share` — mutation: drop the sequence
  check in the re-base branch only;
  `a_partitioned_cell_keeps_spending_within_its_last_share_until_its_envelopes_expire`
  — the ADR 0008 conformance test: no payload after the first, the cell
  sends within its share, refuses past it, and stops when the envelope
  expires — mutation: zero the share on staleness, watch "keeps spending"
  fail.
- `qip-edge-node/tests/pass.rs`:
  `an_unfunded_node_sends_nothing_until_its_first_share_arrives_and_then_sends_within_it`
  — mutation: open at the ceiling.
- `qip-acceptance/tests/architecture.rs`: the existing
  `no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy` and
  `a_library_never_depends_on_a_service_or_an_application`, unchanged and
  still passing — the evidence that the change did not move the boundary.
- `qip-acceptance/tests/resilience.rs`: a cell partitioned mid-share, then
  reconnected to a payload with a smaller share, journals the deficit and
  never overspends.

## What it costs

Stated for the recommended option; each other option's cost is beside it
above.

- **Idle capital between payloads.** A region's usable grant is *G* minus
  the shares of its quiet cells, until the next payload. Visible in each
  cell's `free`, recoverable by cadence, not by code.
- **A node that has never been granted to does nothing.** The ADR 0035
  probe, once this is applied, refuses under `region_reservation` until the
  `dev` API ships it a payload with a share. That is a change to what the
  probe teaches and must be read as such, not as a fault.
- **Two more inputs the centre must be configured with** — cell membership
  and *G* per region — neither of which exists today anywhere at the centre.
  A misconfigured membership refuses shares (a cell sends nothing) rather
  than granting wrongly; that is the intended direction and it is still an
  outage an operator has to notice.
- **The `capital_grants` slot's documented meaning changes** from "manifest
  only" to "manifest plus the one bound with no other channel". The comment
  at `policy.rs:267-274` is rewritten, not softened.
- **A share coupled to the payload's sequence** arrives only with a whole
  payload. A share cannot be re-issued alone, which is deliberate (one
  source of truth) and means the payload cadence is the share cadence.
- **`QIP_REGION_ALLOCATION` changes meaning** from "the amount" to "the
  ceiling". Every place that documents it says so, or an operator sets it
  believing it funds the cell.

## What would make this wrong

- **The regions' cells are so unevenly and so unpredictably loaded that the
  idle share dominates.** Then the cadence is not enough, and the honest
  next record is (b) argued properly — with its consensus problem named and
  solved, not gossiped around.
- **The centre cannot be told which cells are in which region by
  configuration.** If membership must be discovered from what cells claim
  about themselves, a cell's share would rest on the cell's own word, and
  the share is no longer capital granted in advance. Stop there.
- **A second signed path to the cell's bound appears** — a re-balance
  message, a per-strategy envelope that also carries a region amount, an
  operator override over the mesh. Two claims about one bound will disagree
  and the wider one will win by accident. This record would then be
  superseded, not patched.
- **The conformance test cannot be made to fail by zeroing the share on
  staleness.** Then it is not exercising the partition, and the ADR 0008
  argument above is asserted rather than proven.
- **The blueprint's own reservation table (§33, "reservation table plus
  projected timeline") turns out to require a settlement timeline shared
  across cells.** Per-cell shares carry no timeline; if settlement-aware
  reservation must see the region's whole projected timeline at once, the
  per-cell view is insufficient and the centre must ship a projection with
  the share — a different slot and a different record.

## What this does not decide, and what the owner must decide

Nothing is applied. In order:

1. **Accept, amend or decline option (a).** Declining leaves F6's
   cross-process half as operator discipline, which is the status quo the
   matrix records honestly; it should then be recorded as a deliberate
   absence beside `execution_nodes = {}` rather than left as a gap.
2. **A fresh node opens unfunded (recommended) or at its ceiling.** Unfunded
   is "capital granted in advance" read strictly and changes what the ADR
   0035 probe does until the API ships it a share; at-ceiling keeps today's
   behaviour and keeps today's double-spend window open until the first
   payload lands. The record recommends unfunded and says what it costs.
3. **Where region membership and *G* come from.** Recommended:
   `CentralConfig`, operator-set and committed, as the arbitrage policy is.
   The alternative — deriving *G* from `qip-capital-fabric`'s location
   balances — ties a grant to treasury on hand, which is a different
   number with a different owner, and is not recommended without its own
   record.
4. **The payload cadence** the region's efficiency depends on. A number,
   from the owner, once the probe has shown what idle share looks like.

This record does not decide the settlement timeline of §33, does not touch
the family budget of §54.1, does not add a crate, and does not change the
paper-trading boundary: Terraform's plan-time refusal,
`AutonomyLevel::deployable` at the three roots, and `Cell::new`'s paper-only
constructor are unaffected by every option, and a share is checked after all
three.

## Dependency-direction argument

The graph today: `qip-contracts` (lib) ← `qip-mesh` (service) ←
`qip-kernel` (runtime) ← `qip-api` (app); `qip-contracts` ← `qip-edge`
(edge) ← `qip-edge-node` (app); `qip-capital` (service) ← `qip-kernel`, and
*not* ← `qip-edge`, which `architecture.rs:681` enforces.

Under option (a): `qip-contracts` gains a field in a type it owns and no
edge. `qip-kernel` gains a function over `AllocationPlan` (already from
`qip-capital`) and `RegionShare` (already from `qip-contracts`) and no edge.
`qip-edge` reads `RegionShare` through `qip-contracts`, which it already
depends on, and gains no edge — in particular not to `qip-capital` and not
to `qip-mesh`. `qip-mesh` is unchanged. `qip-api` and `qip-edge-node`
compose what they already import. No lib comes to depend on a service, no
service on the runtime, no crate on an app, and the edge crate's dependency
set is the same set it has today. The two ends of the wire still agree on
nothing but the vocabulary in `qip-contracts`, which `spine.rs:70-78` names
as the whole of the contract, deliberately.

## Applied

**Partially, on 2026-09-05, in `qip-edge` and `qip-kernel`'s central plane
only.** The status line above is the owner's to move; this section records
what is in the tree and what is not, so the two are not confused.

What is applied is the part of option (a) that needs none of the four
decisions above, with one deviation from the shape described, stated first
because it is the thing a reader would otherwise be misled by.

**The share is carried by reference, not as a number.** `qip-contracts` was
not edited, so `GrantManifest` gains no `region_share` field. Instead the
`capital_grants` slot carries what it already carried — the signatures of
the grants the centre believes live for the cell — and the cell's share is
the sum of `gross_limit` over the verified, deployed, still-live envelopes
the manifest names (`Cell::apply_region_share`, `cell.rs`). The centre
guarantees that sum never exceeds the share it computed: `partition`
(`qip-kernel/src/central/regions.rs`) withholds a manifest from any cell
whose live grants already sum past `plan.for_cell(cell)`, with the reason,
rather than naming them anyway. So the share and the envelopes are one fact
from one source (`CLAUDE.md` principle 6), and the reason the manifest
refused to be "a delivery path" — a second claim about a fact the envelope
channel carries — does not arise, because nothing is delivered twice. What
this loses against the explicit field: the cell's share can never be
*narrower* than its envelopes' sum by the centre's say-so alone; narrowing
below that needs the envelopes renewed smaller, which is the allocator's
existing path. Rule 2 of the option (a) cell design — a share naming another
region — has no counterpart, because nothing on the wire names a region;
the payload is already per cell and `apply_policy` already refuses a
payload for another cell. The owner may still add the explicit field under
its own commit; `RegionAllocation::rebase` takes an amount and would not
change.

Applied, by crate:

- `qip-kernel` (runtime): `central/regions.rs` — `RegionMembership` (cell to
  region, region to *G*; validated, `BTreeMap` throughout), `RegionShare`
  (region, amount, the grants named, their gross) with `manifest()`,
  `RegionShares` (shares and withheld cells, each with why), and
  `partition`, which refuses — never scales — a plan whose cells' shares
  would exceed a region's *G*, withholds a cell in no region, and gives a
  cell in a region but absent from the plan a stated share of zero.
  `CentralPlane::region_shares(&plan, &membership, now)` is the entry point.
  Membership is an argument, not `CentralConfig`: decision 3 is untouched.
- `qip-edge` (edge): `RegionAllocation` gains a ceiling (the operator's
  amount), an effective bound, and the applied share's owner and sequence;
  `RegionAllocation::unfunded(ceiling)` and `RegionTable::unfunded` open at
  nothing; `rebase(owner, share, sequence)` sets the bound to
  `min(share, ceiling)`, refuses a sequence at or below the last applied,
  refuses a second owner, and reports a deficit rather than letting `free`
  go negative. `Cell::apply_policy` re-bases after the swap when the slot is
  produced; an unproduced slot leaves the table as it was, which is why a
  node opened at its operator amount today (decision 2) behaves exactly as
  before. `Decision::RegionShareApplied` journals every re-base; refusals
  journal under the `region_share` gate. `VerifiedEnvelope::signature` and
  `::gross_limit` are exposed for the sum.
- Tests: `qip-kernel/tests/region_shares.rs` (the three named above, plus
  the membership validator), `central.rs::a_cells_manifest_names_only_grants_whose_gross_fits_its_share`,
  `qip-edge/tests/region_table.rs` (the disjoint-shares test, the replay
  test, the absent-cell test, the deficit test, the ADR 0008 conformance
  test) and three unit tests beside `RegionAllocation`. Each was
  mutation-verified; the report of the applying session names each mutation
  and the assertion it tripped.

**Second slice, 2026-09-05, in `qip-kernel`'s central plane, `qip-edge` and
`qip-edge-node`.** Three of the gaps the first slice left are closed on the
crates this slice may edit; the one that is not is named last.

- **The producer's call exists at the centre.** `CentralPlane::grant_manifests(cells, membership, drawdown, now)`
  (`central/plane.rs`) sizes the plan with the same `allocate` the
  envelopes were issued against, partitions it, and answers every
  configured cell with a `ManifestDecision` (`central/regions.rs`): `Ship`
  the share's manifest, or `Withhold` with the reason. A plan the
  partitioner refuses withholds every cell with the refusal; a cell absent
  from the membership is withheld as "in no region", never given one by
  default. `RegionMembership::parse` reads a membership from a committed
  declaration — `region=grant:cell,cell;…` — and `RegionMembership::covering`
  refuses, by name, a served cell the declaration does not file, for a
  composition root to call at start. Proven by
  `central.rs::the_centres_manifests_for_a_regions_cells_never_together_exceed_its_grant_and_each_payload_carries_its_own`
  (two cells, one region, grants issued through the plane's own door; the
  manifests' gross sums to at most the grant; a grant one unit short ships
  nothing to either cell) and two unit tests beside the types.
- **The node opens unfunded.** `qip_edge_node::assemble` calls the new
  `Cell::with_unfunded_region(ceiling)`; `QIP_REGION_ALLOCATION` is the
  ceiling and is documented as such in `allocation.rs`, the banner prints
  `region_ceiling` and `region_bound` as two facts, and the health body
  carries a `region_share` block (`qip_edge_node::share::RegionShareStatus`)
  with `funded`, `bound`, `free`, `ceiling`, `sequence` and `why` — the
  reason a node that places nothing places nothing, since from the order
  count alone it reads like a quiet market. Proven in
  `qip-edge-node/tests/pass.rs` by
  `an_unfunded_node_sends_nothing_until_its_first_share_arrives_and_then_sends_within_it`,
  `a_second_node_under_the_same_regions_grant_cannot_exceed_it_with_the_first`
  (two `assemble` calls, one grant of one pass's worth, the second node
  refused under `region_reservation`, the two together charged exactly the
  grant, with the contrast that a second node shipped its own grant sends)
  and `a_replayed_lower_sequence_payload_leaves_the_nodes_table_unchanged`.
- **A share is re-derived when the grant it names lands.** The node's
  exchange applies a payload before it deploys the plan that payload names
  (`qip-edge-node/src/mesh.rs`, the policy poll precedes `strategies.install`),
  so the first sum over the manifest found no grant and the table narrowed
  to nothing until the next payload. `RegionAllocation::rederive` re-sums
  under the sequence already applied — for the same owner, refusing any
  other sequence and a table no share was applied to — and `Cell` calls it
  after a deploy, a renewal and a withdrawal. It is not a second path to
  the bound: the inputs are the applied signed payload and the signed
  envelopes it names, and the centre ships a manifest only when their gross
  fits the share it computed. Proven by
  `reservation.rs::a_share_is_rederived_only_under_the_sequence_that_named_it`
  and by the strategies chain test in `qip-edge-node/tests/strategies.rs`,
  whose payload now carries the manifest and whose table reads zero until
  the installer deploys the grant it names.

**Third slice, the same day, in `qip-api`.** `pending_policy` now takes the
membership and calls `platform.central().grant_manifests(cells, membership,
platform.drawdown(), now)`: a shipped share produces the slot from
`decision.manifest()`, a withheld one ships the slot unproduced, and every
decision's `describe(cell)` line travels in the cycle response beside the
whitelist lines (`PolicySummary::shares`) and on stderr. The membership is
read at the API root from `QIP_MESH_REGIONS` (`region=grant:cell,cell;…`),
parsed with `RegionMembership::parse` and checked with `covering` against
`QIP_MESH_CELLS` before the backbone opens, so a served cell filed nowhere
stops the process naming the cell. Left unset, every live grant ships to
every cell as before — the one-cell-per-region shape — and each cycle says
so beside the payload, naming the variable, rather than defaulting a share
silently. Proven by `qip-api/tests/mesh.rs::with_a_membership_declared_the_cycle_ships_the_cell_its_share_of_the_regions_grant`
(the cell's verified payload names exactly the grant the ladder issued),
`without_a_membership_the_cycle_says_every_live_grant_shipped_and_names_the_variable`,
and the settings test that refuses a membership missing a served cell; three
mutations fired (the seam ignoring the membership, `covering` not consulted,
the undeclared line no longer naming the variable), each restored
byte-for-byte. `QIP_MESH_REGIONS` is argued unset for every Cloud Run
deployment in the manifest-wiring suite for the reason `QIP_MESH_CELLS` is:
the mesh has no port there.

Still not applied, and why:

- Which configuration carries the declaration is still the owner's third
  decision: `QIP_MESH_REGIONS` is the API's own environment, chosen because
  it sits beside `QIP_MESH_CELLS`, which it must cover; membership is not a
  `CentralConfig` field and is not derived from treasury, and moving it is
  a one-line change at the root once decided.
- `qip-contracts` (the explicit field) and `qip-mesh` (nothing to do) —
  see the deviation above. The delta carries neither `free` nor the bound.
- The payload cadence (decision 4), the traceability F6 row, and
  `.claude/rules/domains/risk-and-execution.md` are untouched.
- The cross-crate end-to-end test — a payload the kernel produced, applied
  by a `qip-edge` cell — belongs in `qip-acceptance`, which this slice may
  not edit; the two halves are each proven on their own side of the wire.

The paper-trading boundary is unchanged: `Cell::new` remains the only
constructor and takes no ceiling; `with_unfunded_region` is a builder over
the region table and names no ceiling of any other kind; a share is checked
after the envelope, after autonomy, and after every other gate in
`Cell::work`.
