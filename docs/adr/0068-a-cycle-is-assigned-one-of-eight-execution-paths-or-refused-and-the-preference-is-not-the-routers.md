# ADR 0068: A cycle is assigned one of eight execution paths or refused, and the preference is not the router's

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0008 (cells decide alone), ADR 0002 and ADR 0009 (two dependencies), ADR 0003 (paper trading by default and in fact)

## Context

Blueprint §30.2 is a nine-row table: eight edge compositions, eight assigned
execution paths, and one sentence saying the assignment is "a policy decision
made globally with full cost and risk information, not a local heuristic".
§31 gives each path a mechanism, a coordination model, a latency budget and a
primary risk. §33.1 adds a per-path check to the unified risk gate, and §31.1
specifies path 3 in full.

`docs/DELIVERY-STATUS.md` scored §30.2 `ABSENT`, and re-running its commands
on this checkout confirms it:
`grep -rni 'MirroredInventory\|HedgedBridging\|PassiveAnchoring\|FirmQuote\|RepresentationBasis\|PayoffEquivalence\|ExecutionPath' --include=*.rs backend/crates/`
returned nothing. What existed was `qip_arbitrage::graph::PathKind` — four
*shapes* (`CrossVenue`, `Triangular`, `CrossInstrument`, `Mixed`) derived from
a cycle so a rejection message can describe it — over `EdgeKind`'s three
classes (`Transfer`, `Trade`, `Synthetic`) against §30's six.

`PathKind` reads like the missing answer and is not. It labels what was found;
§30.2 assigns how it will be executed. The gap that matters is that the
arbitrage graph cannot express the difference between a venue hop inside one
region and the same hop across an ocean, because `EdgeKind::Transfer` holds no
region — and that single fact is what separates §30.2's row 2, whose mechanism
is "latency-equalised parallel dispatch from pinned I/O slots" inside one
process on a 5-to-15 millisecond budget, from rows 3 to 6, whose budgets run
from milliseconds to minutes. A cycle routed as row 2 when it is row 4 is a
cycle sized for one process and executed over seconds.

## Decision

**One.** The path router lives in `backend/crates/edge/qip-routing/src/path.rs`
and holds §30's six edge classes, §30.2's eight paths, a validated
`Composition`, and `assign`. It reads a description of a cycle and returns an
assignment or a refusal. It is inert: no gateway, no venue class, no size, no
order, and no `Decimal`.

**Two. It is at the edge, not in the kernel, and that is forced.**
`.claude/rules/architecture/00-boundaries.md` runs `libs ← services ← runtime ←
apps` with edge branching off it; `qip-kernel` depends on no crate under
`backend/crates/edge/`, and the edge is where both follow-on sections land —
§33.1's unified risk gate is `qip-edge`'s `Cell` (per DELIVERY-STATUS's §33.1
row), and §31.1's cross-region solve is regional by definition. Putting the
vocabulary in the runtime would have required a `runtime → edge` edge that does
not exist today, or a second copy of the enum. `qip-contracts` would have been
the third option and is another lane's territory this week; if the centre ever
needs to name a path, moving the vocabulary there is the follow-on, and the
enum is deliberately small enough to move.

**Three. The preference is the caller's.** `PathPolicy` is a total ranking of
all eight paths, supplied by whoever holds the cost and risk information.
`PathPolicy::least_exposed_first` is offered as a documented default and named
as a default rather than as a finding. A ranking that omits a path is
**refused**: an unranked path is one the router can find eligible and can never
assign, which is the `MaxExpectedShortfall` shape named in
`.claude/rules/domains/risk-and-execution.md` — a row of the blueprint the
platform would claim to support and would not.

**Four. Two refusals rather than two guesses.**

- A composition containing a **settlement** edge is refused. Settlement is one
  of §30's six classes and §30.2 gives it no row. Treating it as a conversion
  would price a lagged leg as an instant one. The acceptance suite asserts both
  halves of that asymmetry against the blueprint text, so if §30.2 ever gains a
  settlement row the refusal is revisited rather than quietly outliving its
  reason.
- A **mirror edge with no facts** is refused, rather than read as a mirror with
  none of the four facts true. Those are different statements and only one of
  them is about the market. An empty eligible set *is* returned as `Ok` when the
  facts are supplied and none of them holds, because that is a real cycle the
  platform cannot execute; `assign` then refuses, naming what evidence would
  have admitted it.

**Five. Paths 3 to 6 are decided per mirror edge, and a path is eligible only
if every mirror edge admits it.** A closed cross-region cycle carries at least
two mirror edges — the asset and the cash are each held in both regions — and
taking the better of the two would size the cycle against its easier half.

**Six. `ExecutionPath` is not `#[non_exhaustive]`.** §33.1 adds a per-path
check; a ninth path must be a compile error in every `match` that dispatches on
one. A path extension that silently defaults to "nothing extra" for a path
nobody considered is a control that reads as protection and cannot fire.

**Seven. `pathcycle::CycleRouter` is the one seam to `qip-arbitrage`,** and it
supplies the two facts the graph cannot hold — a venue's region, and whether a
synthetic is a basis or an equivalence. Both are refused when unrecorded. An
unmapped venue assumed local is the London leg dispatched as though it were
next door; a synthetic defaulted to basis is an options structure routed under
a carry check instead of a Greeks gate.

## The paper-trading boundary

Intact at all three layers. The record originally said the change "touches
none of them", which stopped being true when the router gained its caller: one
of the three layers is a file this change edits, and the correction is in
point 3 rather than left as a softened adjective.

1. **Terraform.** `infrastructure/terraform/variables.tf` is untouched; no file
   under `infrastructure/` is in this change.
2. **The composition roots.** `AutonomyLevel::deployable` is untouched; no file
   under `backend/crates/apps/` is in this change.
3. **The type system.** `qip-edge`'s `Cell` **is** touched now — this
   paragraph said it was untouched, and that was true only while the router had
   no caller. What holds the layer is unchanged and is worth restating
   precisely. `Cell` still has no constructor taking a ceiling other than paper
   trading; `Cell::send` is still the one place a `Placer` is called and still
   refuses a live-class gateway unconditionally, before the order sequence
   advances; and the router cannot reach either, because `PathAssignment`
   carries an enum, a set and a string, and nothing in `path.rs` or
   `pathcycle.rs` can construct an order, name a venue class, or hold a
   `Decimal`. An assignment can stop an order existing and can do nothing else
   to one. `qip-cost-router` is not involved.

   The new risk the caller creates is not the router but the **dependency
   edge**: `qip-edge → qip-routing` makes `qip_routing::gateway`,
   `qip_routing::router` and `qip_routing::children` importable from `cell.rs`
   for the first time, which would be a second order path beside the `Placer`
   seam. `path_router.rs::the_edge_cell_reaches_the_path_vocabulary_and_no_other_part_of_the_routing_crate`
   refuses all six of the routing crate's other modules in `qip-edge`'s shipped
   source, with a vacuity guard requiring each to be a module that exists and a
   positive half requiring `path` and `pathcycle` to be reached — so the test
   cannot be satisfied by a cell that reaches nothing.

Additionally, the router names no venue class at all. A live class cannot enter
through a type the router does not mention, and
`path_router.rs::the_path_router_names_nothing_that_could_place_an_order_or_name_a_venue_class`
holds that: it refuses `Gateway`, `ChildOrder`, `is_simulated` and `VenueClass`
in the router's source, strips doc comments first so the scan does not read its
own prose, and carries a vacuity guard requiring every forbidden token to be
findable by the same scan elsewhere in the tree.

## Dependencies

No third-party crate, and none is added by the caller either. Three
workspace-member edges now: `qip-routing → qip-arbitrage`, `qip-edge →
qip-routing`, and, as a dev-dependency, `qip-acceptance → qip-routing`. The
second was added when the router gained its caller and this paragraph named
only two until then.

`qip-edge → qip-routing` is acyclic and points the permitted way.
`qip-routing`'s whole dependency closure is `serde`, `qip-core`, `qip-market`,
`qip-contracts` and `qip-arbitrage`; none of the five names `qip-edge`, four are
under `libs/` and the fifth is the edge crate `qip-edge` already depended on.
`architecture.rs::the_dependency_graph_is_acyclic` is the check, read from what
cargo resolved rather than from this sentence. `./scripts/check-dependencies.sh`
still counts eleven permitted third-party packages, because a workspace member
is not a dependency in the sense ADR 0002 and ADR 0009 bound. Both are
untouched.

## Consequences

**What is now possible.** §31.1 can refine `MirrorFacts::both_sides_at_target`
into the four-row direction-gating table without breaking a caller — the struct
has private fields and one fallible constructor for that reason. §33.1 can
dispatch a per-path check on `PathAssignment::assigned` and will get a compile
error for any path it forgets.

**The caller, and what the caller can actually reach.** This paragraph read
"as of this record the router has **no production caller**" until 2026-09-14.
It now has one: `qip_edge::Cell` builds a `CycleRouter` in `install_arbitrage`,
beside the arbitrage desk and from nothing but the cell's own region and its
own venue list, and `Cell::scan_cycles` routes every cycle the scan finds. The
assignment goes onto the `WorkReport` as a `RoutedCycle` and onto the cell's
hash-chained journal as `Decision::CyclePathAssigned`; a cycle §30.2 assigns no
path is refused whole under the `path_router` gate, counted on
`qip_edge_refusals_total{gate="path_router"}` like every other pass-time gate,
and none of its legs is sent.

**Two of the eight rows are reachable in a deployment, and the other six are
not.** Saying so is the point of this paragraph, because a table with eight
rows and two implementations reads as eight delivered rows, and every row of
§30.2 carries a coordination model and a latency budget somebody will
eventually plan against.

- **Rows 1 and 2 are reachable and proven.** `path_assignment.rs` drives a
  one-venue triangle to row 1 and a two-venue cycle inside one region to row 2,
  through real `Cell::work` passes over real books.
- **Rows 3 to 6 are unreachable.** `VenueRegions::all_in` places every venue the
  cell may trade in the cell's own region, so a transfer edge is always a
  transport edge and a composition never holds a mirror edge. Reaching them
  needs four things §31.1 owns: a whitelist naming venues outside the cell's
  region, a region map built from more than the cell's own region, real
  inventory, hedge, resting-support and firm-quote facts per mirror edge, and
  the direction-gating table. Until all four exist a cross-region cycle would be
  **refused**, not mis-assigned, which is the fail-closed answer and not a
  delivered row.
- **Rows 7 and 8 are unreachable one layer earlier than the router.**
  `ArbitrageDesk::new` already refuses a graph holding any synthetic edge,
  because the cell has no book to re-quote a synthetic from. The cell therefore
  supplies `RepresentationClasses::new()` — empty — and the router's refusal of
  an unclassified synthetic is the second of two refusals rather than the first.

**What the gate can refuse today.** Exactly one class of input a cell's own
configuration can present: a cycle longer than `MAX_COMPOSITION_EDGES`, reached
by a desk whose `SearchSettings::max_cycle_edges` is above eight. That is not a
hypothetical arm — `path_assignment.rs` builds a nine-hop ring, watches the
cycle be refused and no leg reach the gateway, and then builds an eight-hop ring
and watches it be assigned, because a bound that refused everything would prove
nothing.

## What it costs

**A dependency edge and a second vocabulary.** `qip-routing → qip-arbitrage`
means the venue-routing crate now links the arbitrage graph, and a reader of
`qip-routing` must know that `path` and `pathcycle` are the only two modules
that use it. The alternative — making every caller restate a cycle it already
holds — was worse, but the cost is real.

`PathKind` and `ExecutionPath` now both exist and both describe a cycle. They
answer different questions, and the module doc says so at length, but two
enums about one object is a thing a future reader will try to merge. The
argument against merging is in `path.rs`'s first section, deliberately placed
where that reader will be standing.

**Two facts the caller must now supply and could get wrong.** A venue's region
and a synthetic's class are configuration, and configuration is a second place
for the truth to live. Both are refused when absent rather than defaulted,
which converts a silent wrong answer into a loud refusal — but it also means a
caller with an incomplete region map routes nothing at all until it is
completed. That is the fail-closed trade, taken deliberately.

**A bound that will be argued with.** `MAX_COMPOSITION_EDGES` is eight where
§30.1's candidate index runs to four. Somebody will eventually find a
profitable nine-edge cycle from the background sweep and be refused. Raising it
is a one-line deliberate act, which is the point of it being a named constant
and a refusal rather than a silent truncation.

**What is deliberately not built, and why.**

- *Cost.* There is no `Decimal` and no `f64` in the router. Expected profit and
  worst-case unwind cost are the optimiser's, and restating either here would be
  a second claim about the same fact. `PathPolicy` is the one hook by which cost
  information reaches the assignment.
- *The path extensions themselves.* §33.1 is a separate lane. Building a partial
  version here would put half a gate in the router and half in the cell.
- *Direction gating.* §31.1's inventory-band table is that lane's, and
  `both_sides_at_target` is deliberately the coarse form it refines.
- *A settlement row.* Inventing coordination semantics the blueprint has not
  specified is the guess this record refuses.

## What would make this wrong

**A settlement row in §30.2.** The refusal of a settlement edge is the single
most opinionated thing here, and it rests on the blueprint having no row for
it. `path_router.rs::the_routers_six_edge_classes_are_exactly_the_six_the_blueprint_names`
fails the moment §30.2 names a settlement path, which is the signal to replace
the refusal with a row rather than to relax the test.

**A caller that needs the centre to name a path.** §30.2 says assignment is a
global decision, and this record puts the vocabulary at the edge because both
follow-on sections are edge-side. If the centre ever has to *ship* an
assignment — tag the candidate index at policy load, as §30.1's table
describes — the enum belongs in `qip-contracts` and this decision is superseded
rather than amended. It was kept small on purpose so that move is cheap.

**Evidence that the default ranking is wrong.** `least_exposed_first` orders by
how long the platform is exposed to something it does not control. That is an
argument, not a measurement. If realised outcomes say a resting remote leg beats
a hedged bridge on cycles where both are eligible, the fix is a policy supplied
by the optimiser, not an edit to the default — and the default should then be
documented as superseded in practice rather than silently retuned.

**A path becoming unreachable.** `every_path_is_reachable_from_some_composition_and_facts`
asserts all eight are produced by some input. If a future refinement makes one
of them unreachable, that test fails, and the right response is to find the
input that reaches it or to record that the platform does not implement that
row — never to delete the assertion.
