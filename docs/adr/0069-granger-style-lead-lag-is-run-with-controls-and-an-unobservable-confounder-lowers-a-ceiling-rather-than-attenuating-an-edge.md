# 0069 — Granger-style lead-lag is run with controls, and an unobservable confounder lowers a ceiling rather than attenuating an edge

**Status:** *accepted*, 2026-09-14.

**Relates to:** blueprint §8.2, §9.1, §9.2, §9.3 and §9.4
(`docs/architecture/algorik-blueprint-v10.1-source.md`); ADR 0054, whose
"confounder adjustment for this method too" open item this closes; ADR 0002
and ADR 0009 (the two-dependency policy, untouched — nothing here adds a
crate).

**Does not touch:** `backend/crates/services/qip-risk-engine/**`,
`qip-execution-engine/**`, `qip-portfolio-engine/**`, `qip-capital/**`,
`qip-brokers/**`, `qip-edge/**`, or any order-placement, venue or autonomy
path. Everything recorded here is inference over the UNDERSTAND stage's world
model. Nothing in it sizes a position, places an order, names a venue, or
changes which autonomy ceiling is deployable. The three paper-trading layers
are untouched: Terraform's refusal of the three live ceilings,
`AutonomyLevel::deployable` in the three composition roots, and the type-level
guarantees in `qip-edge`'s `Cell` and `qip-cost-router`'s `Determinism`.

---

## Context

ADR 0054 gave the causal graph its first real production writer.
`Platform::discover_temporal_precedence` runs in `stage_understand` every
cycle, scans ordered pairs from `price_history`, and writes a `CausalEdge` for
each pair clearing a strict F-test — `p < 0.01` and partial R² ≥ 0.02, with
confidence capped at 0.5. That writer exists, runs, and writes edges built
from ingested bars. `docs/DELIVERY-STATUS.md`'s §9.2 row records it, and the
claim is true.

It also runs the test **uncontrolled**, and §9.2 does not name an uncontrolled
test. The blueprint's method is "Granger-style lead-lag **with controls** —
temporal precedence with confounders explicitly adjusted". ADR 0054 listed
confounder adjustment among its open items, alongside four establishment
methods it did not build, which put a missing qualifier and a missing method
in the same list. They are not the same kind of gap.

A missing method leaves the graph thinner than it should be. A missing
qualifier fills the graph with edges that are *wrong in a correlated way*.
Every instrument in one book moves partly with the book. Regress any
instrument's future on another's past and the shared component shows through,
because the effect's own lag — the only control an uncontrolled test has —
absorbs the driver imperfectly while the candidate cause's lag is a second,
independent proxy for the same thing. Adding it genuinely improves the fit.
The F-test is not malfunctioning; it is answering the question it was asked,
and the question was the wrong one.

The consequence is exactly what the whole of blueprint §9 was written to
answer, quoted from its own opening: "when a regime breaks, models that
learned the same spurious structure break together, and nothing in the system
can say which relationships should have survived." A graph fed by an
uncontrolled pairwise scan is not a sparse graph awaiting more evidence. Its
density *is* the artefact.

This matters beyond §9, because §8.2's traversal queries and §9.3's uses read
that graph. Building a hidden-concentration surface over manufactured edges
would produce a risk report that fires confidently and means nothing — the
`MaxExpectedShortfall` shape inverted. That control could not fire at all;
this one fires on noise, and both read as protection.

`grep -rni 'confounder' --include=*.rs backend/crates` returned three hits
before this change, all of them comments saying the layer was absent.

## Decision

**1. The controlled test is the method; the uncontrolled one is a special
case of it.**

`qip_numerics::stats::granger_causality_controlling_for` takes a set of
control series and puts each one's lags into *both* the restricted and the
unrestricted regression, so the F-test reports the cause's lagged information
about the effect's future beyond what the effect's own past and the named
controls' pasts already carry. `granger_causality` is now literally a call to
it with an empty control set — not a parallel implementation — so the two
cannot drift apart and an edge's p-value cannot depend on which function a
caller reached for. A test asserts they are equal bit for bit.

The unrestricted design is ordered `[effect lags, cause lags, control lags]`
and this ordering is a contract, not an implementation detail. The sign of the
cause's coefficient is what the domain layer turns into `TemporalPrecedence`
or `InverseTemporalPrecedence`. A layout placing the cause block last would
make the read index land on a control's coefficient on every call supplying
one, and the graph would fill with edges pointed exactly backwards while every
p-value stayed entirely plausible.

**2. Confounders are a typed layer with two structurally different kinds.**

`qip_world_model::confounder` holds §9.1's fifth layer.
`Confounder::observed` carries a series and is genuinely adjusted for.
`Confounder::unobserved` carries none, adjusts for nothing, and exists only so
that §9.4's handling can be applied. `ConfounderSet::observed_series` cannot
return an unobserved confounder, because it has no series to return —
recording an admission is structurally incapable of reading as an adjustment,
rather than depending on a caller filtering correctly. Both constructors
refuse a blank id and a blank rationale: an unexplained control is
indistinguishable a quarter later from a series somebody added because it was
to hand, and §9.1's word for this layer is *explicit*.

The set is a `BTreeMap` and the cap is eight. The ordering is load-bearing
rather than habitual: control columns enter a least-squares solve in set
order, floating-point addition is not associative, and a factor summed in hash
order would differ in the last bits and could, on a bar's worth of bad luck,
land on the other side of a significance bar. A replay that reorders is not a
replay. The cap is two constraints agreeing: each control spends `lag`
regressors in both fits, and an unbounded set is an unbounded working set.

**3. An unobservable confounder lowers a ceiling at creation. It never
attenuates an existing edge.**

`CausalEdge` gains `adjusted_for` and `suspected_confounders`, and
`CausalEdge::standing()` returns `EdgeStanding::Suggestive` exactly when the
second is non-empty — §9.4's "recorded as such, and the edge is treated as
suggestive rather than established", in the place a reader of the edge sees
it.

`standing` is a **mark**, for precisely the reason `decayed_at` already is
one: silently shrinking an edge's transmission because a confounder was
admitted would move every propagation result in the platform with nothing in
the record naming the number that changed. What changes instead is the
confidence *ceiling the establishment method applies at the moment it creates
the edge*, where the choice is visible in the edge it wrote.

**4. The two ceilings encode an ordering, and the ordering is the only claim.**

`TEMPORAL_PRECEDENCE_SUGGESTIVE_CEILING` is half
`TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING`. **Neither number was estimated from
anything, and this record says so rather than implying otherwise.** Nothing in
this platform measures how much an unnamed common cause should cost a claim,
and a figure presented as though it had been inferred would be a fabricated
measurement wearing a constant's clothes.

What *is* asserted needs no measurement: an edge carrying a confounder nobody
could adjust for must never rank above an edge produced by the same statistics
with that confounder removed. Half is one point satisfying that ordering. The
ordering is held by a `const` assertion that fails the build, not by a test —
a guarantee the compiler holds beating one a test run holds — and no test
asserts either value, so that moving the point cannot quietly become changing
the claim.

**5. The control audit reports and never retracts.**

`qip_kernel::causal_review::audit_controls` re-runs each temporal-precedence
edge's own test with a measured common driver held constant and reports the
edges that do not survive. It writes nothing to the graph. Three reasons, the
first being the one that matters: a control that both finds a problem and
silently fixes it leaves nothing in the record naming what changed. Second,
the audit's power depends on universe size, so an edge unsupported in a thin
universe may be supported in a fuller one later, and a retraction would be
irreversible on evidence that is not. Third, the graph has exactly two writers
today and a third firing from a read path is a seam nobody finds again.

The control is the equal-weighted cross-sectional mean return of the tracked
universe — a real series the platform holds, not a coefficient anybody chose —
and **the pair under test is excluded from its own factor**. A factor
containing the cause and the effect regresses each series partly on itself and
would refuse every edge for a reason that has nothing to do with confounding.
There is no honest fallback, so a universe too thin to build such a factor
yields no judgement and the edge is counted `unauditable`.

**6. An answer nobody could compute is never reported as a clean answer.**

`ControlAudit::was_answerable` and `ConcentrationReport::was_answerable` exist
because an empty finding list means "nothing wrong" only when something was
examined. With nothing examined it means "nothing was asked", and a caller
reporting the first while holding the second has built a control that cannot
fire and reads as protection. An unanswerable audit returns `None` from
`summary()` rather than a reassuring line.

## Consequences

Good. §9.2's method is now the method the blueprint names. §9.1's confounders
layer exists and is typed. §9.4's unobserved-confounder handling is applied
where an edge is created, and its "constrains sizing and explanation rather
than generating trades" limit is structural — none of the new surfaces can
name a venue, a side or an order. §8.2's hidden-concentration and two-hop
exposure queries exist and are point-in-time correct in both dimensions.

Bad, and stated plainly. **The uncontrolled writer in
`Platform::discover_temporal_precedence` is unchanged by this record**, because
`platform.rs` was outside the lane that wrote it. The controlled establishment
function and the audit both exist and are tested end to end, and neither is
called from a stage yet. Until a `stage_understand` call site is added, the
audit is a capability rather than a running control, and the edges the
platform writes each cycle are still uncontrolled. This ADR is therefore
*accepted as a decision and not yet fully realised as behaviour*, and no row
in `docs/DELIVERY-STATUS.md` should be read as moved on the strength of it
alone.

Also bad: the cross-sectional factor is one common driver, not all of them.
Controlling for it removes the market component and nothing else. Sector,
funding and liquidity drivers remain unadjusted and unnamed, which is exactly
the case §9.4 anticipates — and the honest response is to record them as
unobserved confounders where a caller can name them, not to claim the graph is
now clean.

## What it costs

**Fewer edges, and that is the point being paid for.** A controlled test is
strictly harder to clear than an uncontrolled one. A book driven by a common
factor will write far fewer edges once the factor is held constant, and a desk
reading edge counts as a health metric will see them fall. That is the graph
becoming smaller and truer at once, but it costs the appearance of progress.

**Compute, per pair and per cycle.** Each observed control adds `lag` columns
to two regressions, so a set of `k` controls makes each solve wider by `2k`
columns. The audit is a second pass over the graph on top of the writer's own
pair scan, capped at 200 edges per pass for that reason.

**Degrees of freedom.** Controls are not free statistically either: eight
controls at `lag = 1` cost ten parameters before the pair is even considered,
and a short history that comfortably fit an uncontrolled test will now be
refused. The refusal is deliberate — a regression with almost no residual
degrees of freedom reports confident-looking numbers computed from nothing —
but it means some pairs that previously produced an edge now produce none,
and "none" is the correct answer arriving as an apparent regression.

**A second concept a reader must hold.** `standing` sits beside `confidence`
and `decayed_at`, and three orthogonal qualifiers on one edge is more than any
of them alone. The alternative was folding it into `confidence`, which is
precisely the silent attenuation this record refuses.

**An unfinished seam, carried openly.** The decision is accepted while the
production writer it corrects is still uncontrolled, because the writer lives
in a file this lane could not edit. Until the call site lands, the repository
contains a better method that nothing calls — which is a cost, and is why the
Consequences section refuses to let any status row move on this record alone.

## What would make this wrong

**If the cross-sectional factor turns out to be the thing being traded.** The
audit assumes the mean return of the book is a nuisance driver. For a strategy
whose edge *is* the market component — an index-timing or beta-rotation
family — controlling for it would remove the signal and report the real
relationship as unsupported. If such a family is ever run here, the factor
must become a parameter of the audit rather than a fixed choice, or the audit
must be scoped out for it by name.

**If a caller starts recording unobserved confounders freely.** The
suggestive/established distinction is worth something only while an unobserved
confounder is a considered claim. A caller that attaches a boilerplate
"unmodelled macro" to every edge would make every edge suggestive, the
distinction would carry no information, and the ceiling would become a
uniform discount — at which point the two constants should be collapsed into
one and this record revisited rather than the field quietly ignored.

**If the graph acquires a consumer that sizes directly from an edge.** The
mark-not-attenuate decision rests on edges constraining sizing and explanation
rather than generating positions (§9.4). If anything ever multiplies a
position by `transmission()` without reading `standing()`, then a suggestive
edge would move capital at its full strength and the mark would be decoration.
The remedy is to make `standing` unavoidable at that seam, not to start
attenuating here.

**If somebody estimates what an unobserved confounder actually costs.** The
ordering is asserted because the magnitude cannot be measured. If a procedure
ever measures it — a held-out comparison of suggestive against established
edges' realised transmission, say — then the chosen ceiling should be replaced
by that measurement and this record's fourth decision superseded. A measured
number beats a chosen one; a chosen one presented as measured is the thing
being guarded against.

**If a second writer of the graph appears on a read path.** The audit reports
and does not retract partly because the graph has few writers and they are
findable. Should that stop being true, the argument for keeping the audit
read-only weakens, and the whole shape should be reconsidered together rather
than the audit being given a write on its own.

## Alternatives considered

**Leave the writer uncontrolled and filter downstream.** Rejected: a
downstream filter cannot recover the information the regression already threw
away, and the graph would go on recording spurious edges as facts.

**Attenuate a suggestive edge's strength instead of capping confidence at
creation.** Rejected for the reason `decayed_at` is a mark: it would move every
propagation result with nothing naming the change.

**Have the audit retract unsupported edges.** Rejected. The audit's power
varies with universe size; a retraction is irreversible on evidence that is
not, and it would add a third writer to the graph from a read path.

**Estimate the suggestive ceiling from data.** There is no data to estimate it
from — the quantity is "how wrong is an edge with an unnamed common cause",
and the confounder is unnamed precisely because nothing measures it. Asserting
an ordering is what the platform is entitled to; asserting a magnitude is not.
