# 0045 — The first execution node's capital ceiling is one tenth of the desk's book

**Status:** **Proposed.** Writing a number down is not choosing it, and this
record is not self-adopting: `execution_nodes = {}` stays `{}` until somebody
edits `environments/dev/terraform.tfvars` in a reviewed diff. Nothing in this
record changes code, tfvars, or a default. It has been *marked* proposed
deliberately, for the reason ADR 0039's own correction gives — a record whose
status says one thing while the tree says another is the drift this repository
exists to prevent — and the tree here says nothing at all yet.

**Number chosen: `region_allocation = "1000000"`**, one million units of the
mandate currency, for the single `newyork-1` node in `us-east4` that ADR 0035
authorises in `dev`.

**Date:** 2026-09-06
**Decides:** the value of `region_allocation` for the first execution node,
and the shape that value takes — one scalar per node entry rather than a
per-region table.
**Relates to:** ADR 0035 (one node, shadow mode, `dev` — this is the second of
the two values it names as missing), ADR 0039 (the centre partitions a
region's grant into disjoint per-cell shares; this number is the ceiling those
shares are narrowed by), ADR 0008 (cells decide alone on capital granted in
advance — the reason a cell may not choose this number for itself), ADR 0003
(paper trading; the capital is simulated and the number is still a limit),
ADR 0027 (a cap is a share of *equity*, which is where the denominator here
comes from), ADR 0002 and 0009 (no crate is added by any option below).
**Does not touch:** the paper-trading boundary's three layers. Terraform's
plan-time refusal of the three live ceilings in
`infrastructure/terraform/variables.tf`, `AutonomyLevel::deployable` at the
three central roots, and `Cell::new` as the only cell constructor — which
takes no ceiling of any kind — are all unaffected. A region allocation is
consulted after every one of them, at
`Cell::hold_region_capital`, under the `region_reservation` gate. Nothing here
creates, eases, or implies an order path.

## What was verified, and how

Everything below was read in the working tree on 2026-09-06 rather than taken
from a commit message or an earlier record. Line numbers move when the file
above them grows; re-read before quoting one again.

**One honest limitation of this session, stated first because the rest of the
record is evidence and this is the absence of some.** The lane that produced
this record had no shell: the searches below were run with ripgrep through the
agent's search tool and the files were read directly, so every claim about
*what the tree says* is first-hand, and no claim is made about what the tree
*does*. Nothing here was compiled, no test was executed, and
`./scripts/check-secrets.sh` was not run. That is the right shape for a record
that changes no code — but it means the arithmetic below is arithmetic over
constants that were read, not over values a process printed.

### The denominator: the desk's book is ten million, and it is configuration

`PlatformConfig::initial_equity` is a `Decimal`, in
`backend/crates/runtime/qip-kernel/src/config.rs:257-258`:

```rust
    #[serde(default = "default_initial_equity")]
    pub initial_equity: Decimal,
```

and the default it names, at `config.rs:370-372`:

```rust
fn default_initial_equity() -> Decimal {
    Decimal::from_int(10_000_000)
}
```

Its doc comment (`config.rs:245-256`) says what it is for: "The equity the
platform's book starts with, in the mandate currency … it is configuration
because it is a statement about the deployment's book, not about the code".
No environment overrides it — the three central roots take
`PlatformConfig::default()`'s value — so **ten million is the book in fact and
not only in the type's default**. The report accompanying this record names
that as the one input a reader should check before adopting the number, because
the whole construction below is a fraction of it.

### What the centre already does with that book

`Platform::assemble` derives the allocator's ceilings from it directly, at
`backend/crates/runtime/qip-kernel/src/platform.rs:2232-2240`:

```rust
        let quarter_book = initial_equity
            .checked_div(Decimal::from_int(4))
            .unwrap_or(initial_equity);
        let half_book = initial_equity
            .checked_div(Decimal::from_int(2))
            .unwrap_or(initial_equity);
        let pre_positioner = PrePositioningPlanner::new(
            CapitalAllocator::new(
                AllocationLimits::new(initial_equity, quarter_book, half_book, half_book)?,
```

and `AllocationLimits::new`'s parameter order
(`backend/crates/services/qip-capital/src/allocation.rs:96-102`) is
`(total_budget, per_strategy, default_per_cell, default_per_venue)`. So, on
today's book, the numbers the centre is already willing to issue against are:

| Limit | Field | Today's value |
|---|---|---|
| The whole risk budget | `total_budget` | 10,000,000 |
| The most any one strategy may hold | `per_strategy` | 2,500,000 |
| The most any one **cell** may hold across all its strategies | `default_per_cell` | 5,000,000 |
| The most that may sit at any one venue | `default_per_venue` | 5,000,000 |

Those are real bounds and not advisory: `AllocationLimits::new` refuses a
non-positive budget (`allocation.rs:103-105`) and a negative limit
(`:111-115`), and `AllocationPlan::is_within_budget` (`:301-303`) is an exact
fixed-point comparison, documented as "a real invariant rather than a
tolerance". The quantity is `Decimal` at every step — `total_budget`,
`per_strategy`, `default_per_cell`, `Allocation::notional`, and
`AllocationPlan::for_cell` (`:310-316`), which is the number ADR 0039's
partitioner uses as a cell's share. **Money is `Decimal` all the way from the
config field to the cell's ledger**, so nothing in this decision has to reason
about a float.

The consequence that matters for this record: **with the defaults as they
stand, the centre can lawfully fund a single cell to half the desk's entire
book**, and one strategy inside it to a quarter, while six other regions hold
nothing. That is not a bug in `qip-capital` — the limits were written for a
platform whose book is one book — but it is the concentration ADR 0008 says
the central plane exists to prevent, and it is the thing the operator's
ceiling is the last defence against.

### Where the number lives: both places, and the Terraform side is the one a person edits

It is a Terraform variable **and** a Rust configuration field, connected by one
environment variable, and each end refuses rather than defaults.

- **Root variable.** `region_allocation = string` inside the
  `execution_nodes` object type, `infrastructure/terraform/variables.tf:290`,
  with the comment above it (`:285-289`) saying it is "Required per entry and
  never defaulted".
- **Root wiring.** `infrastructure/terraform/main.tf:591` —
  `region_allocation = each.value.region_allocation`, under the comment "Per
  node and never defaulted".
- **Module variable.**
  `infrastructure/terraform/modules/execution-node/variables.tf:574-605`,
  `type = string`, no default, with a plan-time validation:

  ```hcl
    condition = can(regex("^[0-9]+(\\.[0-9]+)?$", var.region_allocation)) && var.region_allocation != "0" && !can(regex("^0+(\\.0+)?$", var.region_allocation))
  ```

  Digits and an optional fraction, nothing else, and zero refused in both
  spellings. The stated reason is not only arithmetic: the value is
  interpolated unquoted into `node.env`, so any other character is an
  injection rather than a number.
- **The wire between them.**
  `modules/execution-node/templates/startup.sh.tftpl:180` —
  `QIP_REGION_ALLOCATION=${region_allocation}`.
- **The Rust end.** `backend/crates/apps/qip-edge-node/src/allocation.rs:54`
  names the variable, and `RegionCapital::read` (`:73-97`) refuses absent,
  blank, unparseable and non-positive values, every message prefixed
  `configuration:` so `main` exits `EX_CONFIG`. Its module doc (`:27-38`)
  argues the absence of a default in the same register the tfvars comment
  does: "A default would be a number nobody chose — a large one is the
  double-spend with a different face, and a zero is a node that decides and
  sends nothing while looking healthy."
- **The ledger it opens.** `backend/crates/edge/qip-edge/src/reservation.rs`
  holds `RegionAllocation` with `free`, `committed`, `ceiling` and `bound`,
  all `Decimal` (`:122-130`). `unfunded(ceiling)` (`:185`) is what
  `qip-edge-node` calls under ADR 0039; `rebase` (`:243`) and `rederive`
  (`:291`) both land on `apply_bound` (`:324-326`), whose first line is the
  whole of this decision's mechanism:

  ```rust
        let bound = share.min(self.ceiling);
  ```

**So the ceiling can only narrow.** It never funds the cell, it cannot widen a
share, and a share below it passes through untouched. That single line is why
the number below is a backstop and not an allocation.

### What the wire actually carries, from the acceptance suite

`backend/crates/tests/qip-acceptance/tests/region_share.rs` is the end-to-end
proof of ADR 0039, and it carries five tests — a ripgrep for `^#\[test\]` over
that file returns matches at lines 611, 756, 872, 933 and 1022, named
`a_payload_the_centre_built_funds_each_cell_to_exactly_its_share_and_the_two_never_exceed_the_grant`,
`each_cell_places_within_its_share_and_a_cell_driven_past_its_share_is_refused_before_the_grant_is`,
`a_centre_that_no_longer_names_a_cells_grant_narrows_it_to_nothing_and_the_region_gate_refuses`,
`a_replayed_lower_sequence_payload_from_the_centre_changes_neither_cells_table`
and `a_cell_funded_by_the_centres_share_is_still_assembled_paper_only`, which
is exactly the count and the names ADR 0039 records. (Read, not run: see the
limitation above.)

What the wire carries is **not an amount**. The cell reads
`payload.capital_grants.value()` (`region_share.rs:656`, `:749`) — the
`GrantManifest` slot of the signed policy payload — and derives its bound by
summing `gross_limit` over the verified envelopes the manifest names. The
centre guarantees that sum never exceeds `plan.for_cell(cell)` by withholding
the manifest from any cell whose live grants already sum past its share. That
is ADR 0039's stated deviation, "the share travels by reference", and it means
**the number in this record is the only capital figure on the node that a
human types**. Everything else is derived, signed, and checked.

The suite's own choice of ceiling is a useful piece of evidence about how a
ceiling is meant to behave. `region_share.rs:86-91`:

```rust
/// The operator's ceiling on each cell's table: far above any share the
/// allocator sizes, so the bound the tests read is the centre's share and
/// never the operator's backstop.
fn ceiling() -> Decimal {
    Decimal::from_int(1_000_000_000)
}
```

A billion, chosen precisely so it cannot bind — because those tests are
measuring the centre. That is a test fixture's requirement and it is the
opposite of a deployment's: a ceiling that cannot bind in production is a
control that cannot fire, which
`.claude/rules/domains/risk-and-execution.md` names by its precedent
(`MaxExpectedShortfall`, which "shipped in every default limit set and could
never trigger"). **A deployment must not inherit the fixture's number**, and
the module README's illustrative `region_allocation = "250000"`
(`modules/execution-node/README.md:178`) is likewise an example in prose and
not a decision anyone took.

### What the node will actually do on the day it boots

Two facts that must be stated together with the number, because a reader who
has only the number will misread the first week.

1. **The node is opened unfunded and stays unfunded until the centre ships it
   a payload.** `qip-edge-node`'s `assemble` calls
   `Cell::with_unfunded_region(ceiling)` (ADR 0039, applied), so the table
   opens at `free = 0`.
2. **No deployment gives the centre a mesh to ship it over.**
   `infrastructure/terraform/catalogue.tf:32-38` records that `QIP_MESH_CELLS`
   is deliberately not set on the API: "The in-tree mesh binds one listener per
   cell on its own port, and a Cloud Run service exposes exactly one port.
   Unset, `qip-api` builds no mesh and `/api/v1/mesh` answers
   `available: false`". The ADR 0035 entry also carries
   `strategy_plan_path = ""`, which is the node's own "deploy nothing".

So on the first boot the node will run passes (its `QIP_VENUE_FEED=simulated`
line is written by the template at `startup.sh.tftpl:174`), hold no strategy,
receive no share, and place nothing — and `RegionShareStatus`
(`qip-edge-node/src/share.rs:46-61`) will say which of those it is, in words,
in the health body: *"no region share has been applied: this node opened
unfunded and places nothing until the centre's policy payload names grants
this cell holds"*, beside `funded`, `bound`, `free`, `ceiling` and `sequence`.

**This means every positive value of `region_allocation` produces identical
observed behaviour in the ADR 0035 deployment as configured today.** The
number is therefore not chosen for the probe. It is chosen for the first pass
after the mesh exists and a plan is deployed, because that is the first moment
it can bind — and choosing it now is what lets the probe happen at all, since
Terraform refuses the entry without it.

## The failure this number is the last defence against

Three failures are in scope, and only one of them is caught by nothing else.

**Caught elsewhere already: two strategies in one cell.** The reservation was
written to stop each of a cell's deployed strategies spending its own envelope
with nothing bounding their sum
(`qip-edge-node/src/allocation.rs:10-14`). Any positive ceiling closes that,
because the table bounds the sum by construction.

**Caught elsewhere already: two cells in one region.** ADR 0039's partition
closes it — disjoint shares the centre computed and signed, proven by
`a_payload_the_centre_built_funds_each_cell_to_exactly_its_share_and_the_two_never_exceed_the_grant`.
The ceiling contributes nothing here.

**Caught by nothing else: a centre that ships every live grant to every
cell.** `qip-api/src/mesh.rs:698-705` takes the membership as an `Option`, and
at `:743` the cycle says, when it is absent: *"region share for {cell}: no
QIP_MESH_REGIONS declared, every live grant ships"*. `QIP_MESH_REGIONS`
(`mesh.rs:101`) is set by no deployment. That is a deliberate and honest
default for the one-cell-per-region shape — and it is also the state in which a
cell's derived share is the gross of the *entire live grant book*, bounded
above only by whatever `AllocationPlan` the centre last produced, which is
bounded only by `total_budget` — ten million, the whole desk. A membership
typo, a cell filed in the wrong region, or the variable simply never being set
on the day the mesh is finally wired, all land in the same place: one node,
funded to the desk's whole book, in a region that was meant to hold a
fraction of it.

That is the failure. It is a misconfiguration at the centre, it is silent by
design (the line above is informational), and the only thing between it and a
cell holding ten million is `share.min(self.ceiling)` on the node. The number
must therefore be sized against *what one region should ever hold*, not
against what the node is expected to use.

## Options

### (a) 10,000,000 — the whole book

*Rejected.* The ceiling would equal `total_budget`, so `share.min(ceiling)`
could never narrow anything the centre is capable of issuing. It is a control
that cannot fire, which this repository has already shipped once and named as
the template for what not to add. It also fails the specific failure above
completely: the undeclared-membership case funds the cell to the whole book,
and this ceiling admits exactly that.

### (b) 5,000,000 — the centre's `default_per_cell`

*Rejected*, and this is the option worth rejecting carefully, because it is
the one that looks most principled. It fails on two counts.

First, **it is a second claim about a fact the centre already holds.** The
per-cell limit is enforced at the centre, in the allocator, over the whole
plan; copying it into a tfvars entry creates two independently editable
statements of one bound, and the day they disagree the platform behaves
according to whichever is smaller while a reviewer reads whichever they opened
first. `CLAUDE.md` principle 6 — "two independent claims about the same fact
will disagree, and the louder one will be wrong" — and the boundaries rule's
"no second source of truth for a fact the event log already holds" are both
against it.

Second, **it can only fire after the centre's own limit has.** A ceiling equal
to the limit upstream of it never binds first, so it adds no protection
against the failure that motivated it and only reproduces protection that
already exists. A backstop that duplicates the thing it is backing is not a
backstop.

### (c) 1,428,571.43 — the book divided by seven, exactly

ADR 0008's steady state is seven regional cells (`0008:70` — "Seven cells is
seven deployments"), so an equal share of the book is 10,000,000 ÷ 7.

*Rejected as written*, though its reasoning is the reasoning this record
takes. Three problems. The fraction does not terminate, so the written value
is a rounding somebody performed and nobody can check — and the two decimal
places are exactly the kind of digit a reviewer's eye slides over in a string
literal. Seven equal ceilings sum to 9,999,999.99+, which is the whole book:
under the undeclared-membership failure the fleet's ceilings together would
admit essentially the entire desk, leaving no margin at the level the failure
actually operates. And it encodes a seven-way split as though it had been
decided, when what exists is one node and a target.

### (d) 250,000 — the module README's example

*Rejected.* It is 2.5% of the book, and it is below `per_strategy`
(2,500,000) by a factor of ten. A single strategy funded at the centre's own
per-strategy limit would be narrowed by the operator's backstop on its first
pass, every pass, and the refusals under `region_reservation` would be the
ceiling's rather than the centre's. That is not merely conservative — it makes
the node stop teaching the thing ADR 0035 deployed it to teach, because the
envelope discipline ADR 0008 rests on would never be the binding constraint
and never be observed under load.

It is also, on inspection, not a chosen number at all: it appears in a README
as an illustration and in a validation error message as a formatting example
(`"250000.50"`). Adopting it would be adopting a sample.

### (e) 1,000,000 — one tenth of the book — **taken**

One million, written `"1000000"`.

## The decision

**`region_allocation = "1000000"` for `newyork-1`.**

The construction, in the order the reasons weigh:

**It is derived from the one number that describes the desk's book, not
invented.** The denominator is `PlatformConfig::initial_equity`, which
ADR 0027 already established as the right denominator for a share-of-what
question: a cap is a share of equity. The numerator is one tenth. The value is
a fixed fraction of a figure that exists in configuration, so when the book
changes the right revision of this record is arithmetic rather than a fresh
argument.

**One tenth, rather than one seventh, buys the margin the failure needs.**
Seven regions at one tenth each sum to 7,000,000 — 70% of the book. So even in
the total failure of the centre's partitioner, with membership undeclared and
every live grant shipped to every cell, the seven ceilings together cannot
admit the desk's whole equity. Equal sevenths sum to the book exactly and
leave nothing; a round tenth leaves 30%. That margin is the property being
bought, and it is the reason the number is not simply the equal share rounded.

**It sits strictly between the two limits that make a backstop meaningful.**
It is below `default_per_cell` (5,000,000), so it *can* fire — a cell the
centre is willing to fund to half the book is narrowed to a tenth, and the
narrowing is journaled as `Decision::RegionShareApplied` with the deficit
stated rather than clamped away. And it is 40% of `per_strategy`
(2,500,000), which is a deliberate choice discussed below rather than an
oversight.

**It is a number a reviewer can read.** Seven characters, no fraction, passes
the module's regex on inspection rather than on trust, and it is
unambiguously not a copy of the fixture's billion or the README's example.

**It costs nothing to be wrong in the safe direction.** The capital is
simulated (ADR 0003); the platform never submits a live order; the layers that
guarantee that are untouched. What a wrong ceiling costs is *evidence* — a
node that refuses more than it should teaches less — and evidence is
recoverable by editing one string in one tfvars entry and re-planning. A
ceiling that was too high costs a cell holding a concentration nobody
authorised, in a book whose whole purpose is to produce trustworthy numbers
about what the platform would have done. The asymmetry says: narrow.

### On the objection that it will pre-empt a strategy's envelope

It will, and that is intended, and it is worth saying plainly rather than
burying: a single strategy granted the centre's full `per_strategy`
(2,500,000) will find its cell bounded at 1,000,000 and will be refused past
it under `region_reservation`.

The reason that is right rather than wrong: `per_strategy` is a quarter of the
*desk's* book, and it was derived on the assumption that the desk is one book
in one place. It is not a statement about how much of the desk one region may
hold. A single cell in `us-east4` holding a quarter of the platform's entire
equity while six regions hold nothing is the aggregate-concentration failure
ADR 0008's "Why" section describes in as many words: "Three cells can each stay
inside their own limits and between them accumulate a concentrated position no
one authorised."

And crucially, the ambiguity this normally introduces — *was that refusal the
centre's sizing or the operator's backstop?* — does not exist here, because
both numbers are published. `RegionShareStatus` carries `bound` **and**
`ceiling` as two separate fields (`share.rs:27-31`), and `Rebase`
(`reservation.rs:151-159`) carries `share`, `bound`, `free` and `deficit`
separately for the journal. An operator can read which one bound the cell.
That is what makes a firing backstop diagnosable rather than confusing, and it
is why a ceiling that can fire is admissible here when it would not be in a
system that only reported the effective bound.

If the desk concludes it wants the first funded cell to exercise a full
quarter-book envelope, **the honest change is to lower `per_strategy` at the
centre, not to raise the ceiling at the node.** Raising the node's ceiling to
accommodate the centre's sizing is the operator's last defence being adjusted
to stop it defending.

### On scalar versus per-region table

**A scalar per node entry. The table already exists, twice, and neither copy
belongs in this variable.**

`var.execution_nodes` is already a map keyed by node id
(`variables.tf:281-296`), and `region_allocation` is a required field of each
entry, so the per-region table at the Terraform layer *is* the map: seven
regions eventually means seven entries, each carrying its own ceiling, each
reviewed in the same diff. Nothing is gained by adding a second, region-keyed
structure beside it, and something is lost — an entry and a table row could
disagree about one node.

At the Rust layer, `RegionCapital` holds one `Decimal`
(`allocation.rs:62-65`) and `RegionAllocation` holds one `ceiling`
(`reservation.rs:127`), because one process is one cell in one region (ADR
0024). A per-region table inside the node would be a table with one row it
could ever consult and six rows describing regions it is not in — and a second
source of truth for a bound the boundaries rule says must have one.

At the region level, the partition is already the centre's job and is already
done: ADR 0039's `RegionMembership` maps each cell to its region and each
region to its grant *G*, and `partition` refuses rather than scales a plan
whose cells exceed it. A per-region table in tfvars would be that mapping
declared a second time, in a place with no access to the allocation plan it
has to be consistent with. The fleet-level property this record cares about —
that all seven ceilings sum below the book — is a property of the map that a
reader can check by adding up seven strings, and it does not need a mechanism.

## What it costs

- **A cell narrowed below what the centre would fund it to, whenever a plan
  sizes it above a tenth of the book.** Priced above; visible as
  `bound` < `share` in the health body and as `RegionShareApplied` with a
  deficit in the journal. It is not free: capital the desk allocated sits
  unused at that cell.
- **A number that must be revisited when `initial_equity` changes**, and
  nothing enforces the link. `initial_equity` lives in Rust configuration and
  this ceiling lives in tfvars; there is no check that the second is still a
  tenth of the first, and this record deliberately does not propose one — a
  Terraform validation that reads a Rust default would be a coupling in the
  wrong direction. What it proposes instead is that the reversal condition
  below is checked when either number moves.
- **A number that must be revisited seven times**, once per region, if the
  regions are ever unequal. This record's fleet property (seven at a tenth sum
  to 70% of the book) holds only if the seven are equal. The moment one region
  is deliberately larger, the sum has to be recomputed by hand in the tfvars
  diff.
- **It reads as precision it does not have.** One tenth is a round fraction
  chosen for margin and legibility, not a number derived from a measured
  utilisation, because no utilisation has been measured — no execution node
  has ever run outside `cargo test`. The record says so here rather than
  letting the tidiness of "10%" imply otherwise.
- **It unblocks one of ADR 0035's two blockers and not the other.** The boot
  image is unchanged by this record: `.github/workflows/image.yml` exists and
  has never been dispatched, `image_bake_subnet_cidr` is commented out in
  `environments/dev/terraform.tfvars`, and `boot_image` has no value in any
  environment. **A node cannot be deployed on this record alone**, and anybody
  reading it as "the node is now unblocked" has read half of it.

## What would make this wrong

- **`initial_equity` is not ten million in the deployment that runs the
  node.** The whole construction is a fraction of that figure. If a
  deployment's stored `PlatformConfig` carries a different equity — and the
  field exists precisely so one can — then one tenth of ten million is one
  tenth of somebody else's book, and this number is arbitrary. The check is a
  single read of the running config; the revision is arithmetic.
- **The centre's default limits change.** The argument that this ceiling sits
  usefully between `per_strategy` and `default_per_cell` depends on those
  being a quarter and a half of the book, which is a derivation in
  `platform.rs`, not a contract. If `AllocationLimits` is ever constructed
  differently, the interval this number was placed in has moved and the number
  must move with it.
- **A measured utilisation contradicts it.** The first funded weeks produce
  the evidence this record does not have: `free` at the cell over time, and
  the count of refusals under `region_reservation` attributable to `bound`
  rather than to `share`. If the ceiling is what refuses in the ordinary case,
  it is too low and the node is teaching about the backstop instead of about
  the centre. If it is never within an order of magnitude of binding across a
  sustained period, it is decorative and should be narrowed until it is a real
  bound. Either reading revises the number; neither reading revises the
  method.
- **The membership failure is closed structurally.** If `QIP_MESH_REGIONS`
  becomes required at the API root — refusing to serve rather than shipping
  every live grant to every cell — then the specific failure this ceiling is
  the last defence against no longer exists, and the ceiling's remaining job
  is much smaller. It would then be honest to argue the number down toward the
  region's actual grant rather than up.
- **Regions stop being equal.** The seven-at-a-tenth property is the load-
  bearing half of the choice between a tenth and a seventh. A deliberate
  decision that `us-east4` should hold three times what `asia-northeast1` does
  supersedes this record rather than amending it, because the argument for the
  fraction would no longer be the argument for the value.
- **The number is read as a funding.** If an operator, a runbook, or a later
  agent treats `region_allocation` as what the cell has rather than as the
  most a share may fund it to, then the node will be expected to place orders
  it cannot place and the health body's `why` line will be read as a fault.
  ADR 0039 already renamed the variable's meaning; this record's value must
  never be quoted without the word *ceiling* beside it.

## What this does not decide

1. **The boot image.** ADR 0035's first blocker, untouched here. No image has
   been baked, the bake has never been dispatched, and `boot_image` has no
   value anywhere.
2. **Whether to apply.** This record supplies a number; editing
   `environments/dev/terraform.tfvars` and dispatching a plan are separate
   acts, and `execution_nodes = {}` is correct until they happen.
3. **The payload cadence** — ADR 0039's decision 4, still the owner's, still
   waiting on a deployed node to show what idle share looks like.
4. **Where the region membership is declared** — ADR 0039's decision 3.
   `QIP_MESH_REGIONS` at the API root is where it landed in code; this record
   argues that its *absence* is a failure mode worth a ceiling, not that its
   location is settled.
5. **The other six regions' ceilings.** The fleet property assumes they would
   be equal; nothing here sets them, and `execution_nodes` stays `{}` in
   `test`, `stage` and `prod`, which
   `adr_0035_authorises_one_shadow_node_in_dev_and_none_anywhere_else` in the
   `infrastructure` acceptance suite enforces.
6. **Any change to `qip-capital`'s defaults.** The observation that
   `default_per_cell` permits one cell half the desk's book is recorded here
   as context for the ceiling, not as a proposal to change it. If the desk
   wants that narrowed, it is its own record.

## Dependency-direction argument

The acceptance evidence this record owes: **nothing here moves an edge in the
crate graph, because nothing here is code.**

The value travels one way and only one way, from the outside in:
`environments/dev/terraform.tfvars` → `terraform/main.tf:591` →
`modules/execution-node` → `templates/startup.sh.tftpl:180` → `node.env` →
`std::env` read in `qip-edge-node/src/main.rs:183` — an **app**, which is the
only layer permitted to read the environment — → `RegionCapital::read`
(`qip-edge-node/src/allocation.rs`, the same app) → `assemble` hands the
proven value to `Cell::with_unfunded_region` in `qip-edge` (**edge**) →
`RegionAllocation`'s `ceiling` field (`qip-edge/src/reservation.rs`).

Every hop points inward. The edge crate is *given* the number by a composition
root and never reaches for it — which is the point of
`allocation.rs:6-9`: "a cell that chose how much it may risk would be deciding
the one thing ADR 0008 says it never does". No lib gains a dependency on a
service: `qip-capital` (service) still supplies `AllocationPlan` to
`qip-kernel` (runtime) and is still not reachable from `qip-edge`, which
`architecture.rs::no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy`
enforces and which this record does not touch. No service gains a dependency
on the runtime: the number never travels through `qip-mesh`, because the
ceiling is local to the node by construction and only the *share* crosses the
wire. Nothing depends on an app: the value's only consumer inside the
workspace is a field on a struct the edge crate owns.

The one direction that would break this is a ceiling shipped from the centre —
a `region_allocation` field on the policy payload. That would make the
operator's backstop and the centre's share two claims on one wire from one
sender, which is exactly the second-source-of-truth failure ADR 0039's option
(c) was rejected for. It is named here so that a later record proposing it has
to argue against this paragraph rather than around it.
