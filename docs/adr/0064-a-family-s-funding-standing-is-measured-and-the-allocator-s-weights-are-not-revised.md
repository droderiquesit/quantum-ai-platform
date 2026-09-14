# ADR 0064: A family's funding standing is measured, and the allocator's weights are not revised

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0055 (a counterfactual result narrows sizing and only narrows it), ADR 0061 (rule regret is a proposal, a defence or a dormancy finding), ADR 0062 (a venue is withdrawn on feasibility evidence), ADR 0063 (a sizing cap is the second consequence of counterfactual scores)

## Context

Blueprint §12.3's fifth row reads "a strategy family consistently
underperforms" → "allocator objective revised". It is the last row of that
table with nothing behind it. `docs/DELIVERY-STATUS.md` has scored it `absent`
since ADR 0055 with one sentence of argument: no declined path is attributed to
a strategy or a family anywhere in `DeclinedPath` or `DeclinedScore`, so the
evidence the other rows are keyed on cannot be grouped by family at all.

That argument is still correct and is not disturbed here. `DeclinedScore`
carries an `order_id`, an `object_id`, a gate, a venue and the rules charged —
`grep -n 'pub struct DeclinedScore' -A 25 backend/crates/runtime/qip-kernel/src/platform.rs`
— and inventing a declined-path-to-family link would be inventing the finding.

But the row has a second half that ADR 0055 did not reach, because it did not
need to: *underperforms* need not be measured from declined paths. Every
candidate the strategy foundry registers carries the family whose sweep
produced it (`StrategyCandidate::family`, required at construction), holdout
evidence, and — through the lifecycle ledger — a rung that says whether it
holds capital. That is enough to say which families the desk is paying for and
what the evidence says about each. It is not enough to revise an allocator
objective, and the second half of this record is about why.

## What the data actually supports

Three findings from inspection, each located by symbol because each is the kind
of fact a line number rots on.

**One. There is no reachable weight.** Every quantity a family finding could
narrow sits behind a writer that no production path calls.

- `CentralPlane::issue` is the only writer of a capital envelope. Its callers,
  workspace-wide: `grep -rn '\.issue(' backend/crates --include=*.rs` — one in
  `qip-kernel/tests/central.rs`, the rest in `qip-capital`'s own suite and its
  module doc example.
- `CentralPlane::set_proposal` is the only writer of an allocator proposal.
  `grep -rn 'set_proposal' backend/crates --include=*.rs` — three test call
  sites and `central/learning.rs::resize`, which mutates a proposal that must
  already exist and therefore cannot create the funding it would resize.
- `optimization_engine::family_horizons` computes a family budget and the crate
  says so itself: "`family_horizons` and `family_horizons_settled` still have
  no caller" (`qip-optimization-engine/src/lib.rs`).
- The correlation calendar `CentralPlane::family_structure` clusters over is
  fed from grants, so it depends on the first of these. It returns `None` every
  cycle in every deployment.

A `FamilyCap` built on top of any of them today would be a control that cannot
fire. This repository already records what that costs, under
`MaxExpectedShortfall`: a limit that shipped in every default set, could never
trigger, and read as protection for as long as nobody checked. Building the
same shape again — in the lane whose subject is what counterfactual evidence
may and may not change — would be worse than building nothing, because the
delivery register would then say the row was reached.

**Two. `approve_promotion` governs the rung, not the money.**
`GateStage::holds_capital` is `Pilot | Scaled`, and a promotion to either takes
two signatures; but the promotion does not issue an envelope. The envelope is
`CentralPlane::issue`, which the promotion path does not call and which has no
route. So "funded" in this record means *standing at a rung that may hold
capital*, and not *holding any*. Nothing in any deployment of this platform
holds any.

**Three — found while implementing, and the one that would have shipped a dead
control.** The obvious way to score a family is `HoldoutGate::deflated`, the
gate's own accessor. It resolves the lifetime trial count from
`StrategyEvidence::trial_account`, and **no candidate in this platform ever
carries one**. `qip_lifecycle::ledger::charge_holdout_trials` charges the
account into a `Cow` that `attempt_promotion` hands to the gate and then drops;
`StrategyFactory::submit_evidence` is the only writer that could put it back
and has no caller at all
(`grep -rn 'with_trial_account\|submit_evidence' backend/crates --include=*.rs`).
A review built that way would have refused every member of every family for
ever, while reading in the code like a working comparison. The count is
therefore resolved by the caller from `TrialBook::lifetime_trials` — a read,
never a charge — and the deflation is the same `deflated_sharpe` call the
gate's own last line makes.

## Decision

**The platform measures where every registered strategy family stands against
funding, and revises no weight.**

`qip-kernel/src/family_review.rs` groups the factory's registered population by
the family each candidate was enrolled under and produces, per family: the
member count, how many stand at a capital-holding rung, how many members'
evidence could be read, how many were refused, and a deflated figure.

The figure is the member mean of `observed - expected_maximum` — how far the
family's holdout Sharpe stands above what its own search alone would produce.
It is deliberately **not** `DeflatedSharpe::observed`, which is the
*undeflated* Sharpe: a review keyed on that would rank a family that tried ten
thousand configurations level with one that tried ten, which is the selection
bias the holdout gate exists to correct arriving through the back door.
`a_family_s_figure_is_the_gate_s_own_deflation_and_not_the_raw_sharpe` holds
the review's figure to the gate's own, as bits rather than to a tolerance.

Both sides of the comparison are the same measure. A realised return over a
grant and a backtested holdout series are different quantities; comparing them
would be a finding about regime wearing a finding about allocation's clothes.

The LEARN stage then does three things, in this order:

1. Writes `qip_family_standings{standing}` — `funded` and `unfunded`, two fixed
   values, never a family name — **every cycle, including when both arms are
   zero**, the discipline `qip_rule_dormant` follows.
2. Journals a `FamilyAllocationReview` under
   `learning.family_allocation_reviewed`, once per cycle, where there is at
   least one family.
3. Where an unfunded family with at least `FAMILY_REVIEW_MIN_MEMBERS`
   evaluated members stands at least `FAMILY_REVIEW_MARGIN` above the *best*
   funded family, journals a `MisallocationFinding` — journal first, adopt
   after — and counts `qip_family_misallocations_total`.

The bars are ADR 0055's minimum sample by reference, for the reason
`rule_review`, `sizing_review` and `venue_review` all give, and one stated
margin of half a unit of annualised Sharpe. The comparison is against the best
funded family rather than any funded family, which is the strict reading:
"ahead of everything we are paying for" is a finding, "ahead of the worst thing
we are paying for" is true of almost any population.

## Why no weight moves, and what would have to exist before one could

The guarantee is the **absence of a code path**, not a check.

- `family_review` exports no function returning a `Decimal` or any multiplier.
- `MisallocationFinding` carries two family names, two member counts, an
  outcome, a cycle and a timestamp. No margin, no excess, no ratio. The
  magnitude lives on the `FamilyAllocationReview` beside it, because a number
  on the *finding* is the one field the obvious next edit — "size the
  reallocation by how far ahead it is" — would reach for.
- `qip-acceptance/tests/security.rs` scans every shipped `impl` of `Platform`,
  `CentralPlane` and `StrategyFactory` for a `&mut self` method whose name or
  parameter list names a family and whose body assigns a proposal, an envelope
  or a factory field. The same shape ADR 0061's limit-set scan takes, and for
  the same reason: the guarantee is held on every shipped `impl` rather than by
  nobody having written the method yet.

Before a weight could honestly move, three things would have to exist that do
not: a production caller for `CentralPlane::issue` (so that "funded" means
holding capital rather than standing at a rung that may), a production caller
for `set_proposal` or `family_horizons` (so that there is a weight to revise),
and a decision — its own record — about what a family-level revision may do to
a strategy that has its own approvals. `qip_family_standings` is the series in
which the day the first of those arrives becomes visible: its `funded` arm
leaves zero.

## What it costs

**A measurement nobody can act on.** The finding is a record. A desk that reads
it and wants to act must re-propose the strategy through the promotion ladder,
which is the path it already had. The honest description of what this buys is
*visibility*, and visibility is worth less than the row asks for.

**A line on every LEARN stage.** The cycle detail names each family with its
member and funded counts. That is bounded by the number of sweeps the foundry
has run — small, and set by the desk — unlike a per-instrument line, which is
why the sizing review counts where this names. It is still a line every cycle.

**One journal record per cycle per platform with a registered family**,
permanently retained because it is a Learn finding. On a platform with no
families it is nothing, which is every deployment today.

**A `deflated_sharpe` call per member per cycle.** Bounded by the registered
population, which the foundry's own rounds bound. It is arithmetic over a
holdout series already in memory; no I/O, no allocation beyond the standings
map.

**The risk of being read as more than it is.** A row that moves from `absent`
to `measured` reads, to somebody skimming, as a row that moved to done. The
delivery register keeps §12.3 at `PARTIAL` and keeps ADR 0055's `absent`
clause for R5 verbatim, with the measured half stated beside it rather than
replacing it, precisely so that skim fails.

## What would make this wrong

- **A production caller appears for `CentralPlane::issue`.** Then "funded"
  starts meaning "holds capital", the correlation calendar starts returning
  something, and the two family notions in this platform — provenance and
  correlation — become confusable in a way they are not today. That needs its
  own record before either is wired to a weight.
- **Something attaches a trial account to a registered candidate.** Then
  `HoldoutGate::deflated` becomes callable and the count this review resolves
  from the book should be checked against it rather than assumed equal. The
  bit-equality test is written so that the day it stops agreeing is a test
  failure and not a silent divergence.
- **The margin turns out to be measuring estimation error.** Half a Sharpe over
  ten members each is a stated bar, not a fitted one. If real populations
  produce findings every cycle, the bar is too low and the finding is noise
  with a name; if they never produce one over a year in which a desk believed a
  family was misallocated, the bar is too high. Either is a reason to revisit
  the number, and neither is a reason to make the finding do more.
- **Somebody adds a numeric field to `MisallocationFinding`.**
  `a_family_whose_evidence_beats_every_funded_family_moves_no_weight_anywhere`
  asserts the record's field count and refuses a fractional number on it. That
  test failing is the intended alarm, not an inconvenience.

## Consequences

- §12.3's R5 keeps its `absent` clause verbatim — ADR 0055's argument about
  declined-path attribution stands — and gains a measured half with the three
  blockers named by command. §12.3's section verdict stays `PARTIAL`.
- §23.1's LEVEL 2 stays `ABSENT`. A provenance-family review now reaches LEARN
  and allocates nothing; the correlation family still has no consumer.
- No verdict in the shape table moves. 23/128/1/14/15 is unchanged.
- Two new metric names, neither in `SERIES_THAT_MUST_PAGE`: a finding nobody
  can act on should not wake anybody.
- One new topic, `learning.family_allocation_reviewed`, in `TopicGroup::Learn`
  and therefore permanently retained.
- No new dependency. No infrastructure change. The paper-trading boundary is
  untouched at all three layers, and `MisallocationFinding` names no money type
  at all.
