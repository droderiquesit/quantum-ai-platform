# ADR 0086: A correlation family is measured on a granted corpus, and allocated across only as a cap, after a first real grant

- **Status**: Accepted, under the authority the owner delegated to the policy
  lane on 2026-09-19 over legal, business and policy decisions. Nothing is
  built by this record; what it decides is what a later lane may and may not
  build, and what has to be true first.
- **Date**: 2026-09-20
- **Related**: ADR 0064 (a family's funding standing is measured and the
  allocator's weights are not revised — this record does not reopen it),
  ADR 0075 (the capital route is authorised in shape and refused in fact),
  ADR 0065 and ADR 0076 (why it is refused in fact), ADR 0039 (the region
  share, which is the platform's one existing example of an allocation that
  only ever subtracts), ADR 0079 (a derived quantity that could only loosen
  is refused), ADR 0007 (attribution)

## Context

Blueprint §23.1 names three levels:

```
LEVEL 1  cluster ~10,000 strategies into 128 families by return correlation
LEVEL 2  allocate across 128 families, cardinality-constrained, breadth objective
LEVEL 3  distribute each family's budget across its strategies by capacity
```

LEVEL 1 has been scored `REACHED` in the register and was, in code, measuring
nothing: `CentralPlane::family_structure` runs on every cycle over the
realised calendar, the calendar is filled by `retain_grants`, and
`retain_grants` iterates `self.envelopes`, whose only writer is
`CentralPlane::issue`. That function had no production caller, and its own
doc said so and forbade a cycle stage becoming one, because a stage would
have to manufacture the approval and the credentials.

**That is no longer true, and this record exists because of what changed.**
`Platform::issue_capital` (2026-09-20) raises the operator intent and
`POST /strategies/:strategy/capital-grants` is its route, so
`grep -rn '\.issue(' backend/crates --include=*.rs | grep -v /tests/` now
prints a `src/` caller in `platform.rs` beside the issuer's and the recalls'.
`the_learn_stage_measures_family_structure_on_a_corpus_granted_through_the_operator_intent`
in `qip-kernel/tests/central.rs` grants 120 sessions through that intent and
clusters three strategies into two families, so LEVEL 1 is now demonstrably
fed by the path a deployment has rather than only by a test calling the
plane. What still stops it in a *deployment* is the presence gate: every
credential this API accepts is a standing secret, so the route refuses in
fact (ADR 0065, ADR 0075, ADR 0076).

One correction of fact this record must carry, because the register's own
row misled two readings. Row 23.1 says `retain_grants` additionally filters
on `self.factory.baseline(strategy).is_some()` and that
`grep -rn 'set_baseline'` "prints the definition and no caller at all". The
grep is still true and the implication is false: the baseline is written by
`StrategyFactory::promote` directly — `grep -rn 'baselines.insert' backend/crates
--include=*.rs` prints two lines, one inside `promote` and one inside
`set_baseline` — so a promotion to a capital-holding rung fills the gate and
`set_baseline` is a second door nobody uses. The test above proves it
empirically: it never calls `set_baseline`, and the calendar retains every
one of its 120 sessions.

So the question this record was opened on is live for the first time: **may
`optimization_engine::family_horizons` allocate across correlation families,
with the grant as the only funding act?**

### What exists, and what each thing would have to become

- `FamilyClustering` / `FamilyAssignment` (`qip-optimization-engine/src/families.rs`)
  — the LEVEL 1 clustering, consumed by `qip-kernel/src/central/structure.rs`
  and by nothing that decides.
- `family_horizons` and `family_horizons_settled` (`horizons.rs`) — map a
  family to the capital pool it may be allocated against, refusing a family
  that straddles two horizons. `grep -rn 'family_horizons' backend/crates
  --include=*.rs | grep -v /tests/` prints their definitions, the crate's own
  module doc saying they have no caller, and one kernel comment. They do not
  allocate; they answer "against which pool".
- `FamilyBudget::from_weight` and `reconcile` — the arithmetic that would.
- `CapitalAllocator` inside `CentralPlane::allocate` — the thing that
  actually sizes an envelope today, per strategy, and the thing ADR 0064
  refuses to revise the weights of.

There are therefore two different acts hiding behind the phrase "allocate
across families", and the whole decision turns on telling them apart:

1. **A weight.** Family A is worth 1.4× family B, so the allocator sizes A's
   members up. This *creates* exposure that would not otherwise exist.
2. **A cap.** No family may hold more than its share of the book, so a
   family already at its cap has its members sized down. This only ever
   *removes* exposure.

## Decision

### 1. No family weight. ADR 0064 stands and is not reopened

`family_horizons` may **not** be given a production caller that allocates a
budget across correlation families by weight, and no `FamilyBudget` built
`from_weight` may reach `CentralPlane::allocate`. ADR 0064 decided that a
family's standing is measured and the allocator's weights are not revised,
and nothing has changed that bears on its argument. What has changed is the
*supply of evidence*, not the case for acting on it.

The reason is the shape of the failure, not a preference for caution. A
weight derived from a correlation clustering is a number that increases the
size of a position, computed from a matrix estimated on the platform's own
realised returns, which are themselves the product of the last sizing. A
weight that is wrong sizes up exactly the family whose correlation estimate
is most contaminated, and there is no separate observation against which the
platform could notice. Every other number that increases exposure here —
belief confidence, the allocator's edge — is bounded by a control that could
refuse it; a family weight would be bounded by nothing but its own estimate.

### 2. A correlation family may become a **cap**, and only a cap

A later lane may build LEVEL 2 as a **subtractive constraint**: a bound on
the gross a single correlation family may hold, applied inside
`CentralPlane::allocate` the way the per-cell and per-venue limits already
bind jointly, refusing or reducing and never raising. Three properties are
required of it and each is testable:

- **It can only lower.** For any book and any clustering, the envelope a
  strategy is issued under the cap is less than or equal to the envelope it
  would be issued without it. A test that holds this for the empty
  clustering, the singleton clustering and a real one is the acceptance bar,
  and it is the same property ADR 0079 demanded of the dark-region recompute
  and did not get.
- **A family that cannot be measured does not bind.** `family_structure`
  answers `Ok(None)` for most corpora — fewer than `CLUSTERING_WINDOW`
  closed sessions, fewer than two strategies granted on all of them — and
  `Ok(None)` must mean "no cap", never "cap of zero". A control that fires
  hardest when it knows least is worse than no control.
- **The cardinality constraint and the breadth objective of §23.1 are part
  of LEVEL 2 or none of it is.** A cap alone is not the blueprint's LEVEL 2,
  and a row that claims LEVEL 2 on a cap would be the optimistic scoring
  this register has been wrong with before. A cap is scored as a cap.

### 3. Nothing is built until a first real grant exists in a deployment

Even the cap waits, and the condition is precise: **a clustering measured on
a corpus some deployed process actually granted.** Today no deployed process
can complete the capital route, because the presence gate refuses every
standing credential (ADR 0065). Until that is closed, every clustering the
platform can produce comes from a test fixture, and a cap calibrated on a
fixture is a bound whose only evidence is a number somebody chose to make a
test interesting.

This is the `MaxExpectedShortfall` rule applied one step earlier. That limit
shipped in every default set and could never fire because nothing filled the
state it read. A family cap shipped now would be its mirror: a control that
*can* fire, on a clustering that is always `None`, which reads in a review
as a diversification constraint the platform enforces and is in fact a
branch never taken.

### 4. LEVEL 3 is untouched and stays absent

Distributing a family's budget across its members by capacity needs a family
budget, which §1 refuses and §2 defers. `CapacityModel` exists and is read
per strategy by the allocator already; nothing here changes that, and LEVEL 3
is recorded as absent rather than blocked.

## Consequences

- §23.1's row stays `PARTIAL`. LEVEL 1 is reached *and now reachable*: the
  measurement has a production funding path for the first time, and the row
  says so with the command. LEVELs 2 and 3 stay `ABSENT`, with LEVEL 2's
  blocker changed from "no grant is possible" to "no deployed grant has
  happened", which is a different and smaller thing.
- `family_horizons` and `family_horizons_settled` keep no caller, and their
  module doc's statement that they have none stays true and stays honest.
- The register may not score LEVEL 2 as reached on a cap, per §2's third
  bullet.
- Nothing about the paper-trading boundary moves: a cap subtracts from what
  a simulated desk may hold, and a family weight — the thing refused — would
  have been the only half that could add.

## What it costs

**The platform diversifies by cell, venue and instrument, and not by
correlation family.** Two strategies that are near-substitutes in fact can
both be funded to their individual caps, and §23.1's own argument — that
"allocating across families is where diversification is won or lost" — is
accepted and not acted on. That is the real cost and it is stated rather
than softened: the blueprint's central allocation claim is measured and
unenforced.

**A first grant in a deployment is now on LEVEL 2's critical path**, and it
is gated on a credential this platform cannot issue (ADR 0076). So this
record's own condition depends on somebody else's. That is said plainly so
that a later lane does not read "after a first real grant" as a scheduling
note.

## What would make this wrong

- **A measured clustering from a deployment, plus a concentration that the
  per-cell and per-venue limits demonstrably did not catch.** That is the
  evidence for §2's cap, and it is also the only evidence that would reopen
  §1: if a cap is shown insufficient *and* a weight is shown to be the only
  remedy, the weight is argued then, from the measurement, with ADR 0064
  amended by name rather than quietly outgrown.
- **The clustering proving unstable across windows.** If the same population
  clusters differently from one `CLUSTERING_WINDOW` to the next, neither a
  cap nor a weight should be built on it, and the finding belongs in the
  register as a fact about the estimator.
- **`family_horizons` acquiring a caller for some other reason.** Its
  refusal — a family straddling two horizons cannot be reconciled against
  either — is a useful diagnostic, and a lane may legitimately want it as a
  *report*. That is permitted and is not LEVEL 2; a report that names no
  budget allocates nothing.

## Alternatives considered

**Build LEVEL 2 now as a weight, on the clustering the test proves is
produced.** Rejected on §1's argument: the first number in this platform
that could increase a position with nothing able to refuse it.

**Build the cap now, calibrated on the test fixture.** Rejected on §3: a
bound whose evidence is a fixture, in a branch nothing takes, reading as
protection.

**Record LEVEL 2 as blocked and stop.** Rejected as the weaker half of the
truth. It is not blocked by policy or by a missing type; it is waiting on a
deployed grant and on a decision about caps-versus-weights, and this record
takes the second half so the next lane inherits one open condition instead
of two.

**Score LEVEL 1 as no longer "measures nothing".** Accepted, with care: the
row now says the measurement has a production path and is empty in every
deployment because no deployed process can complete the route. Both halves
or neither — the previous row said the first without the second for a while
and it read as delivery.

## Dependency-direction argument

No new edges, because nothing is built. Were §2's cap built, it would live in
`qip-kernel`'s `CentralPlane::allocate`, which already depends on
`qip-capital` and `qip-optimization-engine`; no lib would gain a service
edge, and `qip-optimization-engine` would gain no caller of `family_horizons`
under §1.
