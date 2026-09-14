# ADR 0074: A streaming estimator declares a bound that can be exceeded, and is implemented in-tree

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0002 (two dependencies; the in-tree hashing it authorises by name), ADR 0009 (no in-tree clients), ADR 0043 (the three gaps no in-tree code may close), ADR 0057 (the count-min sketch and its error bound), ADR 0064 (a finding that is measured and moves nothing)

## Context

Blueprint §22.2's "Sufficient Statistics" table asks for seven streaming
methods. Before this record the workspace had two and a half of them: Welford's
accumulator and an exponentially weighted update in
`qip-numerics/src/stats.rs`, neither with a caller; a batch covariance with
one; and, since ADR 0057, a count-min sketch in `qip-numerics/src/sketch.rs`
with a real consumer in the deep brain's research campaign. Quantiles,
cardinality and representative samples were absent.

§21.1 says what they are all for — "Training Without an Archive", sufficient
statistics kept in the node instead of terabytes a year of capture — and
states the condition the whole arrangement rests on:

> every estimator declares an error bound, and one that drifts past it marks
> every model depending on it as degraded.

Two decisions follow from trying to satisfy that sentence, and both are worth
recording because both are easy to get wrong in ways that read as correct.

## Decision 1: the estimators are written in-tree

A t-digest, a HyperLogLog and a weighted reservoir are exactly the algorithms
somebody reaches for a crate to provide. They are not taken.

ADR 0002's Decision section authorises in-tree "SHA-256 and HMAC, the random
number generator" by name, and
`.claude/rules/architecture/00-boundaries.md` records the argument for why that
narrow case is defensible: both are **proven against published vectors**, which
is what a TLS or JWT implementation could never claim, because those fail
silently against a live adversary and never against a fixture.

The three estimators here are the same class of thing, and the same test
applies. Each has a stated mathematical guarantee and each is proven against a
distribution whose answer is known independently of the implementation:

- The t-digest returns a quantile at a rank within `2π√(q(1−q))/δ` of the one
  asked for, tested against a hundred thousand points laid exactly on the unit
  interval and, separately, exactly on the unit exponential — where the true
  rank of every value is its own `1 − e^{−x}` with no sampling noise between
  the assertion and the estimator.
- The HyperLogLog estimates distinct keys within `1.96 × 1.04/√m`, tested on
  streams of a hundred and of a hundred thousand distinct keys, and on ten keys
  repeated a hundred thousand times.
- The weighted reservoir retains an item with probability rising in its weight,
  tested against a population whose weight share is known by construction.

None of the three fails silently against an adversary, because none of them
faces one: they summarise the platform's own numbers. That is the property that
made hashing defensible and asymmetric signing not, and it is why this record
does **not** read as permission to widen ADR 0043's three gaps. Asymmetric
signing, a CSPRNG and anchoring the event-log chain remain closed to in-tree
code.

The alternative was three crates and their transitive trees, reviewed by nobody
here, guarding a figure the desk fits models on. The cost of a dependency is
not its download size (ADR 0002); it is the supply chain, the audit surface,
and semantics somebody else may change. A quantile sketch whose scale function
changes in a point release changes every backtest that read it.

## Decision 2: a declared bound is worth nothing unless it can be exceeded

This is the half that nearly went wrong, and the record exists mostly for it.

Every estimator here declares an error. A consumer that reads the estimate and
ignores the error has a number; a consumer that compares the estimate against a
threshold chosen without knowing the error has a number plus a superstition.
The rule this ADR fixes is that **a consumer must refuse where the estimators'
own error could have produced the finding**, and the refusal must be computed
from the declared bounds rather than chosen.

`qip_training::estimators::StreamingDrift` is the worked example. It computes
the population stability index the platform already uses
(`qip_ai::evaluation::DriftReport`), but from two t-digests rather than from
two retained samples — so a node measuring drift against a training window no
longer has to keep that window. Beside the index it carries a `floor`: the
index that the two digests' declared rank errors alone could manufacture
between two draws of *one* distribution. `is_material` is the index exceeding
the floor and nothing else.

Two failure modes sit either side of that, and both read in the code like a
working comparison:

- **A floor of zero.** Two estimates of one distribution never agree exactly,
  so the index is never zero and *every* feature reads as drifting *every*
  cycle. A control that fires always says nothing, and it is indistinguishable
  from a working one in every code review.
- **A floor above every achievable index.** No comparison is ever a finding.
  This is the `MaxExpectedShortfall` shape — a control that reads as protection
  and cannot fire — and **this lane shipped it in a first draft**. Sizing the
  estimators to fit two hundred features inside "~320 KB" produced compression
  20, a floor of 4.5, and indices around 3: nothing could ever be reported.
  The mistake was reading §21.1's 320 KB as the budget for all five estimators
  when §22.2 assigns it to the covariance row alone (`200² × 8` bytes to the
  byte) and budgets quantiles separately at "few KB per distribution".

The correction is recorded because the shape of the mistake matters more than
the number: **the bound chose the configuration, not the other way round.** The
compression was raised until the floor sat between an independent redraw of one
distribution and a genuine one-standard-deviation shift, and
`qip-acceptance/tests/streaming_estimators.rs` asserts it still does, with
margins on both sides rather than a single threshold.

## Decision 3: every estimator refuses past a named memory ceiling

A sketch exists to bound memory. One that allocates in proportion to its stream
defeats its own purpose, and one whose configuration is unbounded aborts the
process inside `vec!` rather than returning an error. So each of the four has a
ceiling, named as a constant, refused at construction with a message naming the
number, and admitting a configuration below it:

| Estimator | Ceiling | Refused by |
|---|---|---|
| Count-min | `MAX_COUNTERS` (65,536 counters) | `ErrorBound::new` (ADR 0057) |
| t-digest | `TDIGEST_MAX_COMPRESSION` (1,000 → 64 KB) | `Compression::new` |
| HyperLogLog | `HLL_MAX_PRECISION` (16 → 64 KB) | `Precision::new` |
| Reservoir | `RESERVOIR_MAX_ROWS` (65,536 rows) | `Reservoir::new` |

`every_streaming_estimator_refuses_a_configuration_past_its_named_memory_ceiling`
checks all four in one place, both halves — the refusal and the admission —
because a gate that refuses everything is not a working gate. Checking them one
file at a time is how three of them stay correct while the fourth quietly does
not.

## Decision 4: the capacity estimate invents no edge

§20.1's capacity control asks for "the capital level at which the edge decays,
from depth and impact modelling". A capacity figure normally needs the depth of
the book, an impact law, and the edge the strategy expects. This platform holds
no per-strategy expected return in basis points of traded notional, and a
desk-wide constant standing in for it would produce a capacity estimate whose
answer is mostly that constant.

So the edge is not invented. `ProposalLeg::estimated_cost_bps` is the cost the
platform's own sizing said the leg would pay, and the question
`qip_kernel::capacity_review` answers is the self-referential one: at what size
does the impact the observed depth implies exceed the cost this leg's own
sizing assumed? Both sides are the platform's own figures — the cost assumption
from the portfolio engine, the depth from what the SENSE stage absorbed into
`LiquidityTopology`.

The impact law is a uniform ladder, `cost_bps(Q) = (s/2)(1 + Q/D)`, giving
`Q*/D = 2e/s − 1`. One line, so a person can check it; conservative, because a
real book's deeper rungs are thicker than the touch so linear walking
overstates the cost; and it reaches zero on its own when a leg's assumed cost
does not cover half the quoted spread, which is a finding rather than an error.
The square-root law is deliberately not used: it needs a volatility and a
participation horizon the platform does not state per leg, which would
reintroduce exactly the invented constants this avoids.

## What it costs

**Four algorithms this repository now owns.** A t-digest's scale function, a
HyperLogLog's bias corrections and A-Res' key are somebody else's research, and
having them in-tree means having to be right about them. The first draft of the
t-digest was wrong in a way that passed its own accuracy test: the scale
function `4q(1−q)/δ` bounds each centroid's *weight* and not their *number*, so
the sweep exceeded its ceiling on every merge and the 10th percentile of the
unit interval came back as 0.17. That is the price, and it was paid once
already.

**A test that looked right on the wrong distribution.** The accuracy test
passed under a deliberately wrong scale function, because on a uniform
population the interpolation between two centroid means is exact wherever the
centroids fall. A second test on the unit exponential was needed to make the
scale function's claim testable at all. Any later estimator added here carries
the same trap and the same obligation.

**Roughly 7 KB per feature.** Two hundred features is 1.45 MB, against a
sixteen-megabyte raw window at a hundred observations each — an eleven-fold
saving that does not grow, rather than the hundred-fold a reader might expect
from "sufficient statistics". The digest dominates it, and the digest is that
size because the floor demanded the compression.

**A drift index that refuses more than the batch one.** `StreamingDrift` will
call a shift a finding only past its floor, so it reports *fewer* findings than
`DriftReport::compare` on the same data. Some of what it declines are real
shifts the estimators are too coarse to distinguish from their own error. That
is the intended direction and it is still a cost.

**A capacity number that invites over-reading.** `Q*/D = 2e/s − 1` is a uniform
ladder over visible touch depth. A real book's deeper rungs are thicker, so the
figure is conservative — but it is a ratio with two decimal places and it will
be quoted as though it were measured. Nothing sizes from it, which is the only
reason that is tolerable.

## What would make this wrong

**A calibrated impact model.** The moment the platform states a per-strategy
expected return in basis points of traded notional, or fits participation and
volatility from realised fills, the uniform ladder should be replaced by the
square-root law and this record's Decision 4 revisited. The condition is
concrete: a second arm on whatever type carries the cost assumption, holding a
fitted impact coefficient rather than an assumption.

**A floor that stops discriminating.** `the_drift_floor_sits_between_an_identical_redraw_and_a_genuine_shift`
asserts margins of five and three either side. If a change to the compression,
the bucket count or the scale function narrows those, the configuration is
wrong and not the test. Lowering either margin to obtain a pass is the move
this repository forbids.

**A fifth estimator without a ceiling.**
`every_streaming_estimator_refuses_a_configuration_past_its_named_memory_ceiling`
enumerates four by hand and cannot notice a fifth. An estimator added here
without a case in that test is unbounded memory nobody will see.

**A second implementation of any of them.**
`each_sketch_is_declared_exactly_once_in_the_workspace` is the guard, and it
excludes its own source file because the first run found itself — a
measurement instrument that reads itself, the failure mode
`.claude/rules/domains/observability.md` records elsewhere. If that exclusion
is ever widened to a directory, the guard stops covering the directory it was
widened past.

**A production caller for `StrategyFactory::set_baseline`.** That is the single
condition under which §20.1's live canary becomes buildable rather than
dormant, and it is the thing to grep for before concluding the row is still
blocked.

## Consequences

- Three §22.2 rows — quantiles, cardinality, representative samples — have
  implementations with declared, exceedable bounds, and `RunningStats` has its
  first caller.
- A drift index can be computed without retaining either sample. The
  `BTreeMap<String, Vec<f64>>` of feature columns `qip-deepbrain` keeps per
  registered model is a candidate for replacement; this record does not make
  that change and does not claim it.
- Nothing here moves a weight, a size or a limit. `capacity_review` returns two
  observed `Decimal`s and ratios, `feature_statistics` returns a record, and
  `StreamingDrift` returns a verdict — the ADR 0064 discipline, for the same
  reason: a finding that can only be read is one nobody can mistake for a
  decision.
- §20.1's **live canary is deliberately not built**, and the reason is a
  finding rather than a deferral: its only possible subject is a strategy at
  `GateStage::Pilot` with a baseline established, and
  `StrategyFactory::set_baseline` has no production caller at all
  (`grep -rn 'set_baseline(' backend/crates --include=*.rs`), while
  `qip-deepbrain`'s evolution turn promotes one rung only and says in its own
  comment that Pilot is "not reachable from here". A canary built on
  `CentralPlane::live_outcomes` today would be a control whose input is
  structurally empty — the `MaxExpectedShortfall` shape a second time, inside
  the very record that names it. The two writers it waits on are named so the
  work is a grep away rather than a memory.
