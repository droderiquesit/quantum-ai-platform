# 0027 — Concentration limits are a share of what

**Status:** proposed — the owner decides. This record decides nothing; it
frames one decision a kernel change surfaced, the completion plan filed as
D13, and no record has yet named, and it carries a recommendation marked as
one.
**Would amend, if accepted in any direction:** nothing above the risk lib.
Every option below is a change to `LimitSet::conservative_default` and, for
two of them, to `LimitKind`. No option adds a crate, so ADR 0002 and ADR 0009
are not touched.
**Does not touch:** ADR 0003, ADR 0021, ADR 0023. A limit's denominator is
not the paper-trading boundary, and no option here creates, eases or implies
an order path. `.claude/rules/domains/risk-and-execution.md` is the owner's
and is quoted here rather than corrected.

## Context, as verified in the tree at `cfe11c1`

### The finding

Commit `588335a` — "Charge every desk fill to its sector, country, class and
venue buckets, so the two bucket limits can fire" (`.git/logs/HEAD:162`) —
closed a defect of the class the risk-and-execution rule names first: two
limits in every default set that could never fire. Before it, the kernel fed
the aggregate no exposure axis at all, so `MaxConcentration` and
`MaxBucketExposure` evaluated against empty buckets on every cycle
(`backend/crates/runtime/qip-kernel/src/platform.rs:1144-1148`). After it,
every instrument's `sector`, `country`, `asset_class` and `venue` are
projected from the universe at assembly (`platform.rs:483-496`, `:1149-1157`),
every desk fill is charged to them (`backend/crates/libs/qip-risk/src/aggregate.rs:189-195`),
and the pre-trade projection adds the proposed order's own notional to its
buckets before the limits read them
(`backend/crates/services/qip-risk-engine/src/pretrade.rs:244-251`, called
from `platform.rs:4486-4498`).

Feeding the buckets exposed what the limit says. `LimitKind::MaxConcentration`
is documented as "Maximum share of gross exposure in one bucket of a named
axis" (`backend/crates/libs/qip-risk/src/limits.rs:57-58`), and its
evaluation does exactly that: it sums every bucket on the axis
(`limits.rs:488`), returns early only when the axis is absent or the sum is
not positive (`:485-491`), and records each bucket's value divided by that
sum (`:494`). The conservative default carries two of them:
`sector-concentration` at 0.35 (`limits.rs:641-650`) and
`country-concentration` at 0.60 (`:651-660`).

So the first order into an empty book is, after projection, one bucket
holding the whole of the axis total. Its share of gross is 1.0. Both caps
refuse it, and they refuse it for every instrument, every size and every
book, because the arithmetic does not depend on any of those. This is not a
bug in the evaluation — a share-of-gross ratio on a one-position book *is*
100% — it is a statement about what the limit was measuring that was
invisible while the limit could not fire.

### Where the tree stands

- **Two kernel fixture helpers drop both entries and say why.**
  `backend/crates/runtime/qip-kernel/tests/capital.rs:53-70` and
  `tests/risk_aggregates.rs:132-146` each define a `limits()` that takes
  `LimitSet::conservative_default()` and `retain`s everything except
  `LimitKind::MaxConcentration`. Every test in `capital.rs` (five `#[test]`
  functions, `:100-204`) reaches the platform through that helper; the seam
  tests in `risk_aggregates.rs` (five, `:229-486`) reach it through
  `platform()` at `:159` or `limits_with_sector_bucket_cap()` at `:194`,
  which adds one named `MaxBucketExposure` to the same reduced set. The
  `capital.rs` comment closes with the sentence this record answers:
  "whether a share-of-gross cap belongs in a pre-trade set at all is the risk
  desk's question, not this fixture's."
- **Production is unaffected today, for one reason that is changing.** All
  three composition roots assemble `Universe::new()` beside
  `LimitSet::conservative_default()` — `qip-api/src/main.rs:75-76`,
  `qip-fastbrain/src/main.rs:133-134`, `qip-deepbrain/src/main.rs:147-148`.
  An empty universe projects no axes (`platform.rs:1149-1157`), the proposed
  order carries none (`pretrade.rs:244`), the bucket map stays empty, and
  `MaxConcentration` takes its early-return arm (`limits.rs:485-487`). The
  moment a root assembles a universe whose records carry a sector, the first
  desk order in that deployment is refused by `sector-concentration`. Another
  agent is changing the roots' universe now; this record is the decision that
  change will run into.
- **The traceability matrix and the plan already carry the gap.**
  `docs/architecture/algorik-blueprint-traceability.md:502-506` records both
  open facts under the §26/§33 re-score; `docs/plan/completion-plan.md:350`
  files them as D13 and says the semantics "belong in an ADR, not a fixture".
- **The default set has no `MaxBucketExposure`.** `conservative_default`
  (`limits.rs:618-709`) holds twelve limits; its only bucket-shaped entries
  are the two `MaxConcentration` caps. `MaxBucketExposure` — "Maximum gross
  exposure to one named bucket, as a fraction of equity" (`limits.rs:59-64`,
  evaluated at `:501-518` through `RiskState::ratio`, whose denominator is
  equity, `:252-257`) — exists in the type and in the lib's own fixtures
  (`qip-risk/tests/limit_fixtures.rs`), and in one kernel fixture
  (`risk_aggregates.rs:194-198`), and nowhere in a default. Option (d) below
  has to be read with that in mind.
- **The lib's own fixtures never exercised the empty book.**
  `limit_fixtures.rs:224-250` proves `MaxConcentration` passes a spread book
  and vetoes a 600/300 split; neither starts from nothing. The kernel is
  where the first-order case lives, and the kernel is where it was found.
- **The rest of the default set bounds the first position already.**
  `order-notional` caps one order at 250,000 (`limits.rs:624`) and
  `position-weight` caps one name at 10% of equity (`:632`); the default
  book is 10,000,000 (`qip-kernel/src/config.rs:278`, pinned at `:522`). A
  first position under the defaults is therefore at most 2.5% of equity, and
  the concentration caps refuse it anyway.

### What the architecture of record says

The blueprint uses the word "concentration" in three places that bind, and
in none of them does it say what the denominator is for a per-bucket cap:

- §25.3's exposure envelope (`blueprint.md:2121-2139`): per asset class,
  "aggregate signed and gross notional"; per venue, "exposure and
  concentration at one counterparty"; per region, "aggregate exposure and
  capital deployed"; global, "gross, net, leverage, drawdown breaker". Every
  non-global row is phrased as an *exposure*, which is an absolute or an
  equity-relative quantity, not a share of the book.
- §28.1's correlated-exposure controls (`blueprint.md:2373`): "Family
  concentration caps — No family may exceed a share of gross exposure
  regardless of how many strategies are individually within limits." This
  is the one place the blueprint names share-of-gross, and it is a family
  cap on a book of ten thousand strategies, whose first-order problem is
  identical to this one. Whoever builds §28.1 meets this record again.
- §33's gate table (`blueprint.md:2731`, `:2735`): "Instrument, class, venue,
  region, factor, family, causal-driver exposure — Incremental aggregates,
  O(1) — Veto" and "Gross, net, leverage, drawdown breaker — Aggregates —
  Veto and halt". The gate "returns veto or silence, never approval"
  (`:2709`); silence on the first order is the property this record is
  trying to restore without making the gate silent on anything else.

The blueprint names no minimum-invested threshold, no warm-up, and no
number X for option (b). That was checked by searching the blueprint for
"concentration", "of equity" and "single name" and reading every hit; the
absence is reported rather than filled.

### The rule this record sits between

`.claude/rules/domains/risk-and-execution.md` says: "Adding a limit that
cannot fire. `RiskState::expected_shortfall` was once always empty, so
`MaxExpectedShortfall` shipped in every default limit set and could never
trigger — the template for what not to add. A control that cannot fire
reads as protection and is not." `588335a` closed that defect for the bucket
limits, and in doing so produced its mirror image.

**The mirror: a limit that always fires is a kill switch wearing a limit's
name.** The kernel's own tail-limit tests already say it
(`platform.rs:5895-5899`): "A limit that fires on every book is not a
control either — it is an outage — and a test that only ever asserts a
breach cannot tell the two apart." A limit's job is to separate the books it
admits from the books it refuses; a limit whose refusal set is *every book
that has ever placed one order* has no admit set, so it carries no
information about the book. Worse, because it fires first, it masks every
other limit: nothing behind `sector-concentration` will ever be the reason a
first order was refused, so nothing behind it is exercised in production.
Both halves of the rule are the same requirement, that a control's verdict
be a function of the risk it names.

### Why the denominator is the whole question — ADR 0005

ADR 0005 made confidence arithmetic so that "the same evidence gives the same
confidence every time" and "a change in confidence can always be traced to a
change in evidence." Its correction section records what went wrong the
first time: the formula asserted a claim was *less* likely than its own base
rate purely because its explanation was long, and the fix was to make the
arithmetic answer the question actually being asked.

Share-of-gross has the same shape of defect. The ratio is honest arithmetic,
but on a thin book its denominator is the position under test, so the number
it produces is a statement about the book's *size*, not its *composition*.
It asserts a one-name book is maximally concentrated in the same way ADR
0005's first formula asserted a well-explained claim was improbable: a true
consequence of the formula and a false answer to the question. The choice
between the options below is the choice of denominator, and ADR 0005's
standard applies to it — the number must mean what its name says, on every
book, and be reproducible from the log alone.

One arithmetic fact makes the point checkable rather than rhetorical. Under
the defaults, a single-name sector can only breach `sector-concentration`
when gross is below 10% / 35% ≈ 28.6% of equity: above that, `position-weight`
already holds one name under 10% of equity, which is under 35% of any gross
at or above 28.6%. So share-of-gross, for a one-name sector, binds *only* on
a thin book — exactly the region where "concentration" is least meaningful
and the absolute exposure is smallest. The number was never measuring the
risk its rationale names ("a sector bet must be deliberate, not
accumulated", `limits.rs:649`) on the books where a sector bet is large.

## The decision to be taken

What is the denominator of a per-bucket pre-trade cap in the default set:
gross exposure, equity, or nothing — and if gross, under what condition does
the ratio apply?

Secondary, and dependent on the first: whether the answer is expressed by
redefining `MaxConcentration` or by adding a kind beside it. This record
treats that as part of the option, because `LimitKind` is serialised with a
`kind` tag (`limits.rs:44-46`) and a persisted limit set whose `kind` keeps
its name and changes its meaning is the second-source-of-truth failure the
boundaries rule forbids.

## Options

### Option (a) — share of equity: a bucket may not exceed X% of equity

Each bucket on the axis is measured through `RiskState::ratio`, the same
equity denominator `MaxPositionWeight`, `MaxLeverage`, `MaxBucketExposure`
and `MaxCounterpartyExposure` already use (`limits.rs:252-257`). A first
position of 1% of equity is 1% of equity, and passes. The caps become
absolute in the sense that matters: the number they refuse on is the money
at risk in that bucket, and it is the same money whether the book holds one
name or fifty.

*Shape.* A new kind — call it what it measures, a per-axis bucket weight —
with `axis` and `limit`, evaluated like `MaxBucketExposure` but over every
bucket the axis carries rather than one named in advance. `MaxConcentration`
keeps its name, its share-of-gross meaning, and its two fixtures in the lib;
it leaves `conservative_default`. The two default entries are replaced by
the new kind on the same axes.

*What it costs.* The cap stops enforcing diversification on a thin book: a
book at 20% gross with all of it in one sector is 20% of equity in that
sector and passes a 35% cap. That is a real property the share-of-gross
version had and this one gives up. It is also the property that was refusing
every first order. The blueprint's §28.1 family cap is share-of-gross, so
this option leaves the family cap, when it is built, to make its own case
for a different denominator and a minimum-invested condition — this record
does not decide that for it. The default numbers are the desk's to set; the
only arithmetic this record offers is that under a 10% position weight and a
1.5× leverage cap, a 35%-of-equity sector cap admits three-and-a-half
full-size names per sector.

*What evidence closes it.* A passes/vetoes pair for the new kind in
`qip-risk/tests/limit_fixtures.rs`, with the table at the head of that file
gaining its row; a kernel test that the **unmodified** `conservative_default`
admits the first order into an empty book with a fed universe, and refuses
the order that takes one sector past the cap — replacing both `retain`
helpers, which is the test the fixtures currently cannot write; and the
mutation report for each: put the denominator back to the axis total and
watch the first-order test fail for the right reason. Nothing outside the
backend names `sector-concentration` (grep at `cfe11c1`: only
`docs/plan/completion-plan.md:350` and the matrix), so no rendered surface
changes.

*Dependency direction.* `qip-risk` (libs) gains a variant. `qip-risk-engine`
(services) already projects axes generically (`pretrade.rs:244-251`) and
needs no change. `qip-kernel` (runtime) changes only its tests. No lib gains
an edge to a service, no service to the runtime, nothing to an app.

### Option (b) — share of gross, applied only once gross exceeds X% of equity

Keep `MaxConcentration` as it is and gate its evaluation: the ratio is
computed only when `gross_exposure / equity ≥ X`. Below X the limit is silent
by rule rather than by accident.

*What the blueprint implies X should be.* Nothing. The blueprint names no
such threshold anywhere, and inventing one and attributing it to the
architecture of record would be exactly the failure ADR 0022's "What it
costs" warns of. The only number the tree itself suggests is the derived one
above — X = 0.10 / 0.35 ≈ 0.286 makes the cap dormant precisely where
`position-weight` already bounds a one-name sector more tightly — and it is
derived from two other limits, so it moves when either of them does.

*What it costs.* A second parameter with no source, coupled to two others.
A discontinuity at X: the order that carries gross across the threshold can
be refused for a book state that was admitted one order earlier, which is
deterministic and replayable from the log but hard to explain to the desk
that receives the refusal. And the same hole as (a), expressed differently:
a book that sits just under X carrying 100% of its gross in one sector is
admitted by rule. The difference from (a) is that (a) bounds that book's
sector exposure in equity terms and (b) does not, so (b) is the weaker of
the two on the thin book it was designed to admit.

*What evidence closes it.* The same first-order kernel test as (a); a fixture
pair on either side of X on the same book; and a mutation that removes the
gate and watches the first-order test fail. Plus a documented X with its
derivation, and a test that recomputes X from the position-weight and
concentration limits so the coupling cannot drift silently — which is the
`with_tail_risk` key-format lesson (`limits.rs:270-274`) one level up.

*Dependency direction.* Unchanged from today; the change is inside
`limits.rs`.

### Option (c) — a warm-up count: the first N positions exempt

Rejected here, listed because it is the obvious patch and someone will
propose it.

A count is not a risk quantity. N positions at the 10% position weight are
N × 10% of equity in whatever buckets they land, exempted by construction;
with N = 3 that is a book 30% of equity in one sector that no bucket control
has looked at. The exemption is in different units from the limit — count
against exposure — so no number of positions makes it safe and no number
makes it useless; it is a hole whose size depends on what happens to fill
it. It is also the pattern the risk rule names: a control that reads as
protection ("there is a sector cap") and, for the positions that matter
most on a small book, is not. The domain rule's own phrasing applies — this
is a control with a hole, and the hole is where the first bets go.

*What evidence would close it.* None that this record can name honestly: a
test that the (N+1)th order is refused proves the rule fires, not that the
first N were safe.

### Option (d) — remove `MaxConcentration` from the default set; `MaxBucketExposure` is the only bucket control

Delete the two entries. Leave the kind in the type for a configured set.

*What it costs, and the fact the option's phrasing hides.* `conservative_default`
holds **no** `MaxBucketExposure` today (`limits.rs:618-709`). Removing the
two `MaxConcentration` entries leaves the default set with no bucket control
at all, so `588335a`'s feeding of the buckets would reach nothing in any
deployment on the defaults. Adding `MaxBucketExposure` entries instead means
naming buckets — it is "one named bucket" by definition (`limits.rs:59-64`)
— so a default would have to enumerate every sector and every country it
cares about, and is silent on any bucket it does not name. That is a
mandate-calibrated set, which `conservative_default` says it is not
(`limits.rs:616-617`: "not calibrated to any particular mandate"). The
option is coherent for a deployment with a written mandate and wrong for a
default.

*What evidence closes it.* A test that the default set contains no
`MaxConcentration`, which is trivial, and a documentation change stating
that the default has no bucket control, which is the honest cost and the
reason not to take it.

*Dependency direction.* Unchanged.

## Recommendation — marked as a recommendation, not a decision

**Option (a)**, shaped as a new per-axis, share-of-equity kind beside
`MaxConcentration` rather than a redefinition of it, with the two default
entries replaced on the same axes and the numbers left to the desk.

Why, in the order the reasons weigh:

- **It is the only option whose number means what its name says on every
  book.** Equity is the denominator every other exposure limit in the set
  already uses, so "35% in one sector" means the same money at risk whether
  the book is one name or fifty. That is ADR 0005's standard applied to a
  limit, and it is the standard the risk rule's two halves — cannot fire,
  always fires — both reduce to.
- **It keeps the blueprint's word for the blueprint's use.** §28.1's family
  cap is share-of-gross by the blueprint's own text; leaving
  `MaxConcentration` intact in the type, with its fixtures, means that cap
  can be built as specified and argue its own minimum-invested condition
  when it is. Redefining the denominator under the same `kind` tag would
  make a persisted limit set change meaning without changing text.
- **It costs the least in parameters.** (b) adds a threshold nobody
  specified and couples it to two other constants; (c) adds a count in the
  wrong units; (a) adds nothing the set does not already have.
- **The diversification it gives up on a thin book is bounded by the limits
  that remain.** A 20%-gross book entirely in one sector is 20% of equity in
  that sector, under the cap, and under `leverage`, `position-weight` and
  `order-notional` at every step. Whether that is acceptable is the desk's
  call; the record says what is given up rather than pretending nothing is.

The honest cost of this recommendation: it removes the one control that
enforced composition rather than size, and the desk may reasonably want
both. If so, (a) for the pre-trade default and (b) for a monitor-side
`MaxConcentration` with a stated X, in a set the desk calibrates, is a
coherent pair — but that pair is two decisions, and this record asks for
the first.

## What it costs

Stated for the recommended option; each other option's cost is beside it
above. A thin book may be entirely in one sector without a bucket control
objecting, up to the cap's share of equity; the first bets a fresh
deployment places are therefore bounded in size and not in composition.
`LimitKind` gains a variant, which every exhaustive `match` on it — the
label at `limits.rs:86-105`, the fixture table's kind list at
`limit_fixtures.rs:548` — has to name, and the lib's fixture table has to
grow a row or its own test fails. The two default entries change kind and
lose the name `concentration`, so `docs/plan/completion-plan.md:350` and the
matrix's §26/§33 row are re-scored, and the `capital.rs` and
`risk_aggregates.rs` helpers lose their `retain` and their doc comment —
which is the point, since a fixture that removes a default limit to pass is
a fixture testing a set nobody deploys. The family cap of §28.1, when built,
inherits an open question rather than an answer.

## What would make this wrong

- **The desk wants composition enforced on a thin book.** Then share-of-gross
  is the right numerator and denominator, and the question becomes (b)'s X.
  This record would then be superseded by one that states X and its
  derivation, and the pre-trade default would carry both kinds.
- **A persisted limit set exists somewhere in the tree that names
  `max_concentration` and expects it in a default.** No grep at `cfe11c1`
  found one outside the backend, but a stored configuration surviving in a
  deployment this tree cannot see would be admitted under the old set and
  refused under the new one on its first order — the failure this record is
  trying to prevent, reproduced by the fix.
- **`MaxPositionWeight` is removed or raised.** The arithmetic that says
  share-of-gross only binds a one-name sector on a thin book depends on the
  10% position weight; without it, the thin-book argument weakens and (b)'s
  derived X has no derivation.
- **The blueprint's §28.1 is built with a share-of-equity family cap.** Then
  the blueprint's one use of share-of-gross has been quietly reinterpreted,
  and this record's reason for keeping `MaxConcentration` in the type is
  gone; delete the kind rather than leave a limit nothing configures.
- **The first-order kernel test cannot be made to fail by restoring the
  gross denominator.** That would mean the test is not exercising the seam,
  and the record's closing evidence is not evidence.

## What this does not decide

- It does not set the numbers. 0.35 and 0.60 were share-of-gross figures;
  what the same axes should be capped at as a share of equity is the desk's,
  and this record only supplies the arithmetic that relates them to the
  position weight and the leverage cap.
- It does not decide §28.1's family cap or its minimum-invested condition.
- It does not decide whether `MaxConcentration` belongs in a monitor-side
  set, or in a mandate-calibrated set a deployment supplies instead of the
  default. It stays in the type either way.
- It does not change what the roots assemble. The universe change is
  another agent's, in flight, and is the reason this record exists rather
  than its subject.
- It does not touch the paper-trading boundary. Terraform's plan-time
  refusal, `AutonomyLevel::deployable` at the three roots, and `Cell::new`'s
  paper-only constructor are unaffected by any option; a limit's denominator
  is checked after all three and cannot reach past them.
- It adds no dependency. `check-dependencies.sh` and `architecture.rs`'s
  two-crate test are unchanged by every option.

## Dependency-direction argument

Every option changes `backend/crates/libs/qip-risk` only, and (a) adds a
variant to a type that crate already owns. `qip-risk-engine` (services)
projects axes generically and gains no edge; `qip-kernel` (runtime) changes
tests only and gains no edge; the three roots (apps) are untouched. No lib
comes to depend on a service, no service on the runtime, nothing on an app.
The graph's shape is unchanged; what changes is one variant at its base.
