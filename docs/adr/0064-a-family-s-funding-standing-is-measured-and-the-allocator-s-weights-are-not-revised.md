# ADR 0064: A family's funding standing is measured, and the allocator's weights are not revised

- **Status**: Proposed
- **Date**: 2026-09-14
- **Supersedes**: nothing
- **Related**: ADR 0055 (a counterfactual result narrows sizing and only narrows it), ADR 0057 (round five: a predicate deleted rather than narrowed a sixth time), ADR 0061 (rule regret is a proposal, a defence or a dormancy finding), ADR 0062 (a venue is withdrawn on feasibility evidence), ADR 0063 (a sizing cap is the second consequence of counterfactual scores)

**Amended in place, 2026-09-14, after a fourth round on the family-weight
scan.** The amendment changes what this record *claims*, not what the code
does: the sentence "the guarantee is the absence of a code path" appeared four
times here and in `family_review.rs`, and it was an overclaim. Three
successive versions of the acceptance scan were each defeated by ordinary code
an independent security review compiled into the tree and ran. What holds the
absence is **review**, against an enumerated list, with a scan that forces the
review to happen. Downgrading the claim is the substance of the amendment; the
scan was also rebuilt, and the rebuild is what makes the smaller claim
checkable. Every paragraph below that said otherwise is corrected in place
rather than argued with.

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

**Four — found by review after this record was first written, and it makes
the paragraph above true only in part.** `TrialBook::lifetime_trials` is *not*
the number `HoldoutGate::charged_trials` resolves to. `charged_trials` returns
`TrialAccount::lifetime()`, the family's total as it stood when that member
was charged: a snapshot. `lifetime_trials` returns the last journal record's
`lifetime_after`, the family's total now, which every later sibling's charge
has raised. They coincide for the member charged last, and therefore for every
member of a one-member family — which is the arity the bit-equality test was
written at, so the claim held exactly where it could not be tested. Moving
that fixture to two members prints `left: 12`, `right: 24`. This record said
the two agreed, and the code never made them agree.

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
the review's figure to the gate's own, as bits rather than to a tolerance, at
one member — the arity where the two trial counts coincide, and the only one
where "the gate's own deflation" is a claim about arithmetic rather than about
arity.

**The count that deflation is against is the family's whole search as of the
review, and that is deliberate.** A deflated Sharpe exists to correct a result
for the multiple testing that produced it. A comparison drawn *now* between
two families must therefore correct each for the search each has actually
done; grading family A on the twelve configurations it had tried when its
first member ran, while family B is graded on ten thousand, re-introduces on
the review's own axis exactly the bias the deflation removes. The gate's
per-member snapshot is right for the gate, which decides one admission at one
instant, and wrong for a cross-family comparison drawn afterwards.
`a_family_s_count_is_the_whole_family_s_search_and_not_one_member_s_snapshot`
holds the general relation at two members: one count per family, at least as
large as every member's snapshot, equal to the snapshot of the member charged
last.

**It follows that a member's contribution is not stationary, and that is a
property of the record rather than a defect.** Evaluating any sibling raises
the family's lifetime count and lowers every other member's excess, so two
`FamilyAllocationReview` records built from byte-identical evidence at
different cycles disagree. That does not break "every decision reproducible
from the log alone": each record carries its `cycle` and its `at`, and what it
claims is the family's standing *against the search as of that cycle* — a
statement about a moment, which replays to the same value because the trial
book is itself a hash-chained journal whose totals are reconstructed rather
than remembered. What is not reproducible is one member's figure compared
across cycles, and nothing here invites that comparison: the finding names
families, never members.

**The arithmetic is biased toward producing findings, and the bias is bounded
only by the fact that a finding moves nothing.** Deflating every family
against its own *current* total penalises the family that has searched more.
The review's direction is "an unfunded family beats the best funded one", and
unfunded families are systematically the younger ones with the smaller
searches — so the correction is systematically lighter on exactly the side of
the comparison a finding is raised for. This is not a rounding detail: it is a
structural tilt in the test statistic, in the direction of the alarm. It is
accepted here because the preceding paragraph's argument is the stronger one —
a cross-family comparison drawn now must correct each family for the search it
has actually done, and the alternative tilt (grading a young family on twelve
configurations while an old one is graded on ten thousand) is the bias the
deflation exists to remove, arriving on the review's own axis. Two things
follow and both belong on the record. First, the margin bar is doing more work
than a symmetric statistic's bar would, which is another reason the "too many
findings" half of the bar's revisit test below is the likely one. Second, and
this is why the tilt is tolerable at all, **the finding is a record and moves
no weight** — so a biased statistic produces reading matter rather than a
biased allocation. That makes this bullet an argument *for* this record's
central decision rather than an exception to it: the day a weight moves on
this figure, the tilt stops being bounded, and the decision that wires it must
either correct the statistic or state why it does not.

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

**Evaluated, and the first implementation of it counted registrations.**
`FamilyStanding::clears_the_member_bar` tested `members` while the figure
being compared is a mean over `admitted` alone, so a family of ten
registrations with one readable holdout series cleared a ten-*observation*
bar on one observation, and the finding reported ten as its sample size. The
constant is `COUNTERFACTUAL_SIZING_MIN_SAMPLE` — a sample bar applied to a
population count. The bar is now `admitted >= FAMILY_REVIEW_MIN_MEMBERS`, the
finding's two counts are named `unfunded_evaluated` and `funded_evaluated` and
carry the admitted counts, and
`a_family_registered_ten_times_and_read_once_has_one_observation_and_not_ten`
holds both halves. The `>=` on the margin — argued in the code and untested
until the same review — is held by
`a_gap_of_exactly_the_margin_is_a_finding_and_one_ulp_below_it_is_not`.

The bars are ADR 0055's minimum sample by reference, for the reason
`rule_review`, `sizing_review` and `venue_review` all give, and one stated
margin of half a unit of annualised Sharpe. The comparison is against the best
funded family rather than any funded family, which is the strict reading:
"ahead of everything we are paying for" is a finding, "ahead of the worst thing
we are paying for" is true of almost any population.

## Why no weight moves, and what would have to exist before one could

**No weight moves, and what holds that is review against an enumerated list.**
This heading's first line read "the guarantee is the absence of a code path,
not a check" until 2026-09-14. That sentence was doing real damage: it told
three successive implementations that a scan over source text could establish
the absence, and each of the three was defeated by ordinary code.

Two of the four supports below *are* structural and survive the correction.
The third and fourth are not, and are now described as what they are.

- `family_review` exports no function returning a `Decimal` or any multiplier.
  Enforced by an allow-list of the return types the module may name, not by a
  search for the word: `pub fn family_cap(…) -> FamilyWeight`, a newtype over
  `Decimal`, passed the search and is refused by the list.
- `MisallocationFinding` carries two family names, two evaluated-member
  counts, an outcome, a cycle and a timestamp. No margin, no excess, no ratio.
  The magnitude lives on the `FamilyAllocationReview` beside it, because a
  number on the *finding* is the one field the obvious next edit — "size the
  reallocation by how far ahead it is" — would reach for.
- **State the scope of that honestly, because it is narrower than it reads.**
  `standings()` returns `FamilyStanding { deflated_excess: f64, … }` and any
  caller can multiply by that number. The guarantee is not "no number leaves
  the module"; it is that the *finding* carries none and no exported
  *function* hands one out. The magnitude must be readable somewhere, and it
  is on the measurement — beside the counts that qualify it — rather than on
  the record that names two families. A Sharpe excess is also not a multiplier
  in any case: unbounded, routinely negative, and a caller that scaled a
  notional by it would be inventing a unit conversion nobody recorded.
- **The return-type allow-list does not bound transitively, and the next
  person adding a type to it needs to know that before they do.**
  `FAMILY_REVIEW_RETURN_TYPES` permits `FamilyStanding`, which carries the
  public `f64` above, so `pub fn family_cap(&self) -> FamilyStanding` passes
  the list and a caller reads the field and sizes with it. That is accepted
  rather than closed. Closing it would mean forbidding the module's own
  `standings()`, which returns `BTreeMap<String, FamilyStanding>` and is the
  point of the module; a magnitude nobody can read is not a measurement. So
  the list answers "does this module hand out something *shaped* like a
  multiplier — a `Decimal`, a `Money`, a newtype over either, a bare number" —
  and it does not and cannot answer "can a caller obtain a number here". A
  caller can, deliberately. The statement is written where the array is
  defined as well as here, because the next person to add a type will be
  reading the array and not this record. Adding one that carries a `Decimal`
  field is a change of posture and belongs in an amendment to this record, not
  in the commit that was making a red test green.
- **`qip-acceptance/tests/security.rs` enumerates every shipped function in
  the workspace that moves a weight, and refuses one that is not on a reviewed
  list.** That is a tripwire over an enumerable set, not a proof. It is the
  fourth version of this check and the first that does not claim to be the
  guarantee.

### The three rounds, and why the method changed rather than the predicate

Each of the first three rounds decided what to look at with a **precondition
on names**, and each was defeated by code an independent review compiled into
the tree and ran, leaving the test printing `ok. 1 passed`.

- **Round 1** matched a deny-list of `Decimal` and `Money` in a return
  position. Any newtype walks past it.
- **Round 2** matched method *names* containing `famil`, and detected only
  whole-field reassignment (`self.proposals = …`). So
  `Platform::discount_family` calling `self.central.set_proposal(…)` — the
  call this record's own argument names as the allocator's reachable writer —
  passed, as did `self.proposals.get_mut(…).weight = …` and
  `self.envelopes.insert(…)`. This repository shipped that class once before
  and fixed it in `28857ed`, where an acceptance scan matched method names and
  a working `adopt(&mut self, LimitSet)` passed.
- **Round 3** added call-shaped detection, parameter-type matching and
  comment/literal blanking, and each detector gained a positive and a negative
  control asserted inside the test. It was real progress: it is the first
  version that caught a field mutation, reporting
  `CentralPlane::apply_misallocation_probe(&mut self, &FamilyStanding) {
  self.proposals.clear(); }` as *names a family and calls
  `self.proposals.clear(…)`*. It was then defeated twice over, by two shapes
  with no family name on the writing method:
  - **one-hop delegation** — a family-named entry point that writes nothing,
    `pub fn apply_family_finding(&mut self, finding: &MisallocationFinding) {
    self.rebalance_book(&finding.unfunded) }`, calling a neutrally named
    private helper that does the write;
  - **neutral naming** — `pub fn defund_group(&mut self, group: &str)` calling
    `set_proposal`, which trips neither the name half nor the parameter-type
    half of the precondition, so its body is never read.

  Both were compiled into `impl Platform` and both left the scan green. Round
  3's own documentation named the intraprocedural limit of round 2 as the
  defect and then reproduced it one level out.

**The discriminator is not the detector, it is the precondition.** Round 3's
detector was better than round 2's and would have caught both probes had it
been allowed to look at them. What could not work is the step before it: a
guess, from a method's name and its parameter types, about whether a write
came from a family finding. Nothing in the text of a Rust file records where a
value came from, so `self.central.set_proposal(p)` inside
`discount_family(&str)` and inside `resize(&StrategyId, …)` are the same
bytes. Narrowing the guess a fourth time would be the fourth round of the
shape this repository already resolved once: ADR 0057's redaction predicate
went five rounds of narrowing a host-shape test and round five (`ca3d581`)
**deleted the predicate** rather than narrowing it, on the finding that no
predicate over a string's own shape can distinguish two cases when nothing in
the string says which it is. The same conclusion applies here and the same
resolution is taken.

### What the fourth round does instead

The precondition is deleted. The scan walks every `fn` with a body in every
shipped file under `backend/crates` — no `impl` marker, no holder list, no
family test — and reports every one whose body reassigns a weight field, calls
a mutating method on one, or calls `set_proposal`, `issue`, `family_horizons`
or `family_horizons_settled`. What it finds is compared, in both directions,
against `REVIEWED_WEIGHT_MOVERS`: eleven functions on 2026-09-14, each with a
written reason it is not a family finding reaching a weight. A twelfth fails
the test until somebody writes the reason down, and a reviewed row with no
site behind it fails it too, because that is either a scan that broke or a
list that rotted and a person has to say which.

Three things change as a result. It **fails closed**: the way to make it pass
is to write an argument, not to rename a method. It closes both probes —
verified by re-compiling them into `impl Platform` and re-running, which
reported `probe_defund_group(…) calls set_proposal(…)` and
`probe_rebalance_book(…) calls set_proposal(…)` — and the two further holes
the reviewer named, a weight-mover on a type outside the holder list and an
`impl Platform` written with generics or a `where` clause, since neither the
type name nor the `impl` form is read any more. And it **refuses more than its
predecessor**, deliberately: four of the eleven reviewed rows have nothing to
do with families, including a builder that assigns a `CentralConfig` and the
edge node's held-grant book. Those are kept as rows with reasons rather than
excluded in code, because an exclusion is invisible at review time and an
invisible exclusion is how the previous three rounds came to guard nothing.

**Four limits remain, and none of them is closed.** A call reached through a
trait object, a function pointer or an alias. A weight held in a field the
scan does not name. A call generated by a macro. And the one that matters
most: **the scan cannot tell whether a reviewed site is fed by a family
finding**, because that is a dataflow question and this is a text scan. That
step is held by a person reading the eleven rows. Saying so is the point of
this amendment — a reader who takes the check for a mechanical guarantee stops
looking, and three rounds of this scan are the evidence that they do.

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
  `HoldoutGate::deflated` becomes callable — and it must not simply be
  substituted, because it deflates against the member's snapshot rather than
  the family's search. This bullet claimed "the bit-equality test is written
  so that the day it stops agreeing is a test failure and not a silent
  divergence", and that was false when written: the two counts already
  disagreed at every arity above one, and the test was pinned at one. The
  relation is now asserted rather than the equality
  (`a_family_s_count_is_the_whole_family_s_search_and_not_one_member_s_snapshot`),
  so a change to either quantity fails a test at the arity the platform
  actually runs at.
- **A member's figure is read as stationary.** It is not, and nothing should
  chart one member's excess over time or compare two `FamilyAllocationReview`
  records field by field across cycles expecting agreement. A later sibling's
  evaluation legitimately moves it. If a consumer ever needs a stationary
  per-member figure, that is a different statistic and needs its own record:
  the honest one would fix the trial count at the member's own charge and stop
  being comparable across families, which is the trade this record chose the
  other side of.
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
- **`REVIEWED_WEIGHT_MOVERS` grows without anyone arguing about it.** The
  whole of what this record now claims rests on eleven rows being read by a
  person. A row added with a reason that restates the code — "writes the
  proposal book" — is the same failure as the three bypassed scans wearing
  different clothes: something that reads as review and is not. The reason on
  a row must say why the write is *not* a family finding reaching a weight.
- **The scan is cited as the guarantee again.** The sentence "the guarantee is
  the absence of a code path" is the one this amendment removed, and it will
  be tempting to restore it the next time somebody reads the test passing and
  wants a stronger claim in a status document. It is false and was false when
  first written. What is true is the smaller sentence: no shipped function
  moves a weight except eleven that have been read.
- **A test that proves a property of a copy of the production path.** The
  fixture `Population::members` in `family_review.rs` resolves a family's
  lifetime trial count the same way `Platform::family_standings` does, by
  duplication and not by calling it, so every test here that turns on the
  count would keep passing if the production resolver were changed to a
  per-member snapshot. The resolver is correct today — `family_standings`
  reads `TrialBook::lifetime_trials` once per family and never charges it — so
  this is a limit, not a defect, and it is the same class of thing as the
  bypassed scans: a check that reads as protection over a path it does not
  touch. **Closing it is a change in `platform.rs`**: both the production
  resolver and the fixture should call one function, which
  `family_review` should export and `family_standings` should use in place of
  its inline `trial_counts` map. That was not done in this amendment because
  `platform.rs` belonged to another lane at the time, and it is recorded here
  rather than left in a comment so that the next person to touch either side
  finds it.

## Consequences

- §12.3's R5 keeps its `absent` clause verbatim — ADR 0055's argument about
  declined-path attribution stands — and gains a measured half with the three
  blockers named by command. §12.3's section verdict stays `PARTIAL`.
- §23.1's LEVEL 2 stays `ABSENT`. A provenance-family review now reaches LEARN
  and allocates nothing; the correlation family still has no consumer.
- No verdict in the shape table moves. 23/128/1/14/15 is unchanged.
- `docs/DELIVERY-STATUS.md`'s §12.3 R5 wording must say **evaluated** members
  and mean it: the bar is `admitted`, and the finding reports the admitted
  count as its sample size. It already said "evaluated"; the code did not.
- Two new metric names, neither in `SERIES_THAT_MUST_PAGE`: a finding nobody
  can act on should not wake anybody.
- One new topic, `learning.family_allocation_reviewed`, in `TopicGroup::Learn`
  and therefore permanently retained.
- No new dependency. No infrastructure change. The paper-trading boundary is
  untouched at all three layers, and `MisallocationFinding` names no money type
  at all.
