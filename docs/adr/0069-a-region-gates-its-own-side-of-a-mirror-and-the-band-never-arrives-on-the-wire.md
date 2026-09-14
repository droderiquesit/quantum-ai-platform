# ADR 0069: A region gates its own side of a mirror, and the band never arrives on the wire

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0068 (a cycle is assigned one of eight execution paths or refused), ADR 0008 (cells decide alone), ADR 0039 (the region share), ADR 0062 (a venue is withdrawn on feasibility evidence), ADR 0003 (paper trading by default and in fact)

## Context

ADR 0068 gave blueprint §30.2's path router a caller in `Cell::scan_cycles`.
Two of its eight rows were reachable from a cell and six were not, and the
reason was one line: `Cell::install_arbitrage` built the router's region map
with `VenueRegions::all_in`, putting **every** venue the cell may trade in the
cell's own region. A transfer edge was therefore always a transport edge, no
composition a cell could build held a mirror edge, and a cross-region cycle
was not mis-assigned — it could not be composed at all.

Blueprint §31.1 is what the missing rows need. It has three parts:

- **SETUP** — hold asset X in both regions; distribute a reference price, a
  threshold and per-instrument targets.
- **EXECUTE** — each region compares its own local price against the
  reference and trades its own side: below by more than the threshold it
  buys, above by more than the threshold it sells.
- **The direction-gating table** — four rows mapping where a region's
  inventory sits relative to its target onto the direction it may take.

And one sentence that carries the design: *"Direction gating makes same-side
trading across regions impossible by construction rather than by
coordination."* §33.1 then names path 3's extension as *"Direction permitted
by inventory band. Reference inside TTL. Both, every time."*

Three facts about this platform shaped what could honestly be built.

1. **A cell cannot measure the remote region's inventory**, and nothing ships
   it one. `qip_routing::path::MirrorFacts::both_sides_at_target` — the fact
   §30.2's row 3 turned on — is a claim about two regions.
2. **The policy payload's tenth slot already carries two of §31.1's three
   distributed facts.** `qip_contracts::policy::InventoryTargets` holds
   `targets` and `reference_prices`, keyed by instrument, on a sixty-second
   time to live. It carries **no band field at all**, which
   `qip-kernel`'s whitelist module notes as a reason the slot has no producer:
   "a producer would be filling two thirds of a type whose third is
   undefined".
3. **Nothing produces slot 10**, and `qip-api`'s own acceptance test asserts
   so. That is unchanged by this ADR.

## Decision

**One. The band is local configuration and may never arrive on the wire, and
slot 10's missing band field is the design rather than a gap.**

The policy payload travels a wire that authenticates nobody beyond its
signature. `FeasibilityConstraints::withdrawn_venues` already states the rule
for that wire in its own doc: it may **subtract**, never add, and "the worst a
forged or replayed payload can do with this set is stop a cell trading
somewhere it was configured to trade". A band field on slot 10 would be the
opposite direction — a payload that could widen the deviation a region is
permitted to run before its direction is forced.

So the split is: the centre says **where** a region should be (the target) and
**what the world price is** (the reference); the operator's own configuration
says **how far off it may drift** (the two band half-widths) and **how small a
dislocation is worth trading** (the threshold). `MirroredInstrument` holds the
second pair and `Cell::inventory_targets` reads the first. A cell with a band
and no target refuses; a cell with a target and no band refuses.

This closes the objection in `qip-kernel`'s whitelist module without touching
slot 10: the undefined third of the type is not meant to be filled by a
producer, and a future producer of slot 10 would be filling the two thirds it
is entitled to fill.

**Two. Eligibility asks whether the mirror exists; the direction gate asks
whether this side may trade, and they are two different questions asked at two
different moments.**

`MirrorFacts` gains `established_mirror` beside `both_sides_at_target`, and
§30.2's row 3 is eligible on either. The coarse fact is kept, unchanged, for a
caller that can see both regions — the centre could. The new one is a **setup**
fact rather than a measurement: the centre named an inventory target for the
instrument and this cell's operator configured a band for it, which together
are §31.1's SETUP. It is what an edge cell can honestly assert.

Refining `both_sides_at_target` in place — which ADR 0068's own module doc
predicted — was rejected, and the prediction is left corrected in that doc
rather than deleted. Refining it would have made every cell assert a fact
about a region it cannot see. §31.1's construction is that it does not need
to, so the table went beside the router rather than inside it.

Where the local side sits inside its band is asked **after** assignment, in
§33.1's extension, which is where the blueprint puts it.

**Three. The four rows are evaluated outermost-first, and the hard band is
checked before the above/below rows.**

§31.1 prints the table as: below target, above target, at target inside band,
outside hard band. Read in that order a holding far past the hard band matches
"above target" first and the fourth row never fires — a hard band that cannot
fire, which is the `MaxExpectedShortfall` shape this platform has shipped
once already. `InventoryBand::posture` therefore tests the hard band first,
then the soft band, then the sign of the deviation. A breach permits the
reducing direction at full size; the fastest way back inside a bound the
operator set is not a smaller trade.

**Four. "Either direction, reduced size" is reported and then refused, because
nothing in the gate can make a scanned cycle smaller.**

This is the one place the implementation is deliberately narrower than the
blueprint, and it is stated rather than quietly dropped. §31.1's third row
permits either direction at *reduced* size. This gate never sees a quantity
and has no sizing authority; the cycle was priced by the scanner at the
policy's size. `RegionPosture::size` therefore reports
`SizeDiscipline::Reduced` and `extension::check` refuses on it, with the same
argument `Cell::scan_cycles` already uses verbatim for a narrowed degradation
multiplier: "a cycle re-priced at a narrower size is a different cycle, and
the scanner priced this one at the policy's size".

The consequence is worth stating plainly, because it looks like a defect and
is not: **a region at target refuses**, and a region holding nothing is at or
below target on everything. Every closed mirror cycle has exactly one object
this region acquires and one it disposes of, so one of its two legs is always
a local sell; a region holding nothing is never above its target and its band
never permits one. That is §31.1's SETUP line — *"hold asset X in BOTH
regions"* — enforced rather than assumed.

**Five. §33.1's extension is a check and can only subtract.**

`extension::check` returns a verdict or a refusal, and there is no return
value by which it can widen anything: it cannot name a venue, cannot choose a
path, cannot raise a size and cannot make a cycle eligible that `assign` did
not already assign. Its `match` names all eight paths, which is why
`ExecutionPath` is not `#[non_exhaustive]`; a ninth path is a compile error
rather than an arm that silently defaults to "nothing extra".

A missing fact is a **refusal**, never a pass. Paths 1 and 2 have no row in
§33.1's table, and their arms return a verdict that says so —
`ExtensionVerdict::has_row` is `false` — which is a different fact from "the
check passed" and reads identically in any log that stores only success.

**Six. A region annotation says where a venue is and never that a cell may
reach it.**

`CellConfig::venue_regions` annotates venues the cell is already configured
for. `CellConfig::with_venue_in_region` pushes the venue onto `venues` in the
same call, so the builder cannot produce an annotation for a venue the cell
may not trade; and because the field is `pub` and the builder is skippable,
`Cell::install_arbitrage` refuses such an entry at runtime as well. The three
existing guards are untouched: `graph_from_whitelist` against `QIP_VENUES`,
`Cell::install_arbitrage` against `self.config.venues` — whose per-edge check
still fires first for a graph edge at an unconfigured venue — and
`whitelist_for` under `envelope.permits_venue`.

**Seven. The two gates refuse under different names.**

`path_router` means the platform cannot say **how** it would execute the
cycle: a configuration or whitelist problem that will not change between
passes. `path_extension` means it can and the conditions that path needs are
not met **right now**: a market or inventory state that may be true next
pass. An operator reading one series for both cannot tell a mis-configured
cell from a cell correctly waiting for its band. A cycle refused at the
extension therefore leaves **both** marks — the assignment on the chain and in
`WorkReport::paths`, the refusal in `WorkReport::refusals` — and
`WorkReport::paths`' own doc was corrected from "exactly one of the two marks"
to "at least one".

## Consequences

**What is now reachable from a cell.** §30.2's row 3. A cell told one of its
venues is abroad composes mirror edges, is assigned `MirroredInventory`, and
has that assignment hash-chained and reported.
`qip-edge/tests/cross_region.rs::a_cell_told_one_of_its_venues_is_abroad_composes_mirror_edges_and_is_assigned_path_three`
is the proof.

**What is still not reachable from a cell, and why.** Rows 4, 5 and 6. A cell
measures no hedge book beside the cycle's own instruments, holds no
connectivity fact about whether a remote venue accepts a resting order, and
receives no firm quote from one, so `Cell::mirror_facts_for` supplies `false`,
`false` and `None` for the three and the router finds none of the rows
eligible. Each is a statement rather than a placeholder and each is
documented at the line. Rows 7 and 8 remain unreachable one layer earlier:
`ArbitrageDesk::new` refuses a graph holding a synthetic edge.

**What is not reachable in a deployment at all.** Two inputs have no producer.
`qip-edge-node` builds its `CellConfig` from `QIP_VENUES` with `with_venue`
and never annotates a region, so no deployed cell has a foreign venue; and
nothing produces policy slot 10, so no deployed cell has a target or a
reference. Both are outside this lane's territory and both are named in its
handoff. Until either changes, a deployed cell behaves exactly as ADR 0068
left it: every venue local, every transfer a transport edge, every cycle row 1
or row 2, and §33.1's extension returning the no-row verdict.

**A deviation from the blueprint, named.** §31.1's "reduced size" is not
implemented as a size; it is implemented as a refusal. Implementing it would
require the scanner to re-price a cycle at a narrower size, which is a change
to `qip-arbitrage` and a separate decision.

**Not built, and why.** §31.1's EXECUTE rule is implemented as a *gate* —
`DistributedReference::indication` says which direction the reference permits
and the cycle's own direction must agree — and never as a search. This
platform already has an opportunity scanner; deriving a second buy/sell
decision from a price dislocation inside the routing gate would be a second
source of truth for a fact the scan already holds.

**Cardinality.** One new value on `qip_edge_refusals_total{gate}`:
`path_extension`, the `GATE_PATH_EXTENSION` constant, refused through
`Cell::refuse` like every other gate. `grep -n 'metrics\.refusal(' backend/crates/edge/qip-edge/src/cell.rs`
still prints exactly two lines — one inside `Cell::refuse`, one naming
`GATE_LIVE_VENUE` inside `Cell::send` — so the bound observability's domain
rule states is unchanged and no third recording site was added.

**Paper trading.** Nothing here weakens any of the three layers. `Cell` gains
no constructor taking a ceiling; `Cell::send` remains the one place a `Placer`
is called and keeps refusing a live-class gateway unconditionally; every new
type in `qip-routing::mirror` and `qip-routing::extension` is inert and can
produce no order. The only effect any of it can have on an order is to stop
one existing.

## What it costs

**A third thing an operator has to configure correctly, and two of them are
safety parameters.** A band that is too wide never forces a direction; a
threshold that is too small makes every tick a dislocation. Neither arrives
from the centre, which is the point of decision one, and neither has a
default, which is decision one's price: `MirroredInstrument::new` refuses a
zero threshold and a hard band no wider than its soft band, but it cannot tell
a wrong band from a right one. A cell configured with a plausible-looking
wrong band is a cell gating on a number nobody checked.

**A composition built twice per cycle.** `Cell::route_one` builds one to key
the mirror facts against and `CycleRouter::route` builds the router's own.
`CycleRouter::route`'s signature is pinned by an acceptance test that proves
the router has a production caller, and reaching past it to
`qip_routing::path::assign` to avoid the second walk would give `cell.rs` a
second way to produce an assignment. The walk is at most
`MAX_COMPOSITION_EDGES` edges of clones; the duplication is the cheaper of the
two costs and is commented where it happens.

**A cycle can now leave two marks instead of one.** `WorkReport::paths` and
`WorkReport::refusals` can both name the same cycle, and a reader who counts
one against the other will double-count. The field's own doc says so. The
alternative — dropping the assignment when the extension refuses — would make
"the platform could not route this" and "the platform routed this and its band
said no" indistinguishable, and the second is the normal state of a working
mirror.

**Eight `match` arms that a cell reaches three of.** `extension::check` names
all eight paths and an edge cell can reach arms 1, 2 and 3. The other five are
reachable only from a direct call, which this crate's tests make. The
exhaustive `match` is what stops a ninth path defaulting to no check, and the
cost is five arms whose production reach is a future lane's.

## What would make this wrong

**If the centre ever ships a band.** Decision one rests on the payload being a
wire that may subtract and never add. If a future ADR gives the centre an
authenticated channel where that argument does not hold — an operator-signed
band, countersigned like an autonomy change — the split between local widths
and distributed targets should be revisited, because the reason for it would
have gone.

**If the scanner learns to re-price a cycle at a narrower size.** Decision
four refuses §31.1's at-target row because nothing here can make a scanned
cycle smaller. The moment `qip-arbitrage` can re-price one, the refusal
becomes a control firing on a case the blueprint says should trade, and the
`SizeDiscipline::Reduced` arm should become a resize rather than a refusal.
This is the condition most likely to be met, and the one whose staleness would
be least visible: the refusal would keep reading as correct.

**If a cell is ever given the remote region's inventory.** Decision two exists
because it cannot be. A centre that shipped both sides' holdings would make
`both_sides_at_target` the honest fact again and `established_mirror`
redundant, and two facts where one would do is exactly what this ADR argues
against elsewhere.

**If `qip_edge_refusals_total{gate}` gains a third recording site.** The
cardinality bound in the observability domain rule holds because
`Cell::refuse` and `Cell::send` are the only two places `metrics.refusal` is
called. `path_extension` is a constant passed to `Cell::refuse`, so the bound
is unchanged — but a later lane that records a path-extension refusal at a
seam with no `WorkReport` to push onto would break it silently, exactly as
`GATE_LIVE_VENUE` once did.

## Alternatives considered

**Put the band on policy slot 10.** Rejected under decision one. It is the
one field that would let a wire widen a capital discipline.

**Set `both_sides_at_target` from the local book alone.** Rejected: it is a
claim about two regions and a cell can see one. It would have been the
cheapest change and the one that made every cell assert a fact it does not
hold.

**Let the at-target row trade at full size.** Rejected. It would make the
row that the blueprint narrows the most permissive of the four.

**Default an unmeasured round trip.** Rejected. `MirrorFacts::new` refuses a
round trip of zero because it makes every firm quote look like it outlasts the
wire, and a default would be a number nobody measured sitting where §30.2's
row 6 is decided. `MirrorArrangement::round_trip` refuses instead, naming the
regions that *were* measured.
