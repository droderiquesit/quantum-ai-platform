# 0077 — A counterfactual finding that asks a control to move is charged a trial, and shares the book but not the budget

**Status:** *accepted*, 2026-09-15.

**Relates to:** blueprint §12.4 ("Guardrails", third row), §12.3, §14.3 and
§20.1; ADR 0055 (the fixed threshold this puts a gate in front of), ADR 0061
(the approval path a recalibration proposal ends in), ADR 0063 (the
larger-size proposal), ADR 0023 (cumulative trial accounting).

**Does not touch:** `backend/crates/services/qip-risk-engine/**`,
`qip-execution-engine/**`, `qip-capital/**`, any risk limit, any capital
envelope, any autonomy ceiling, and the paper-trading boundary in all three of
its layers. Nothing here approves, places or routes an order. It can only
*stop* a record being written and a proposal being opened.

---

## Context

`docs/DELIVERY-STATUS.md` scored blueprint §12.4 `PARTIAL` on one row of four.
The blueprint's control is stated in a single line — "counterfactual findings
enter the same statistical gate as any other hypothesis, with the same trial
accounting" — and the triage's finding was exact: ADR 0055's discipline (a
minimum sample of ten, an unfavourable fraction of three quarters) "is a fixed
threshold rule stated and defended in the record, not the trial accounting
this row asks for".

That is not a complaint about the numbers. A fixed threshold is a test the
platform may run as often as it likes at no cost, and this platform runs it
constantly: `review_rules` reviews every rule in the boot limit set on every
LEARN stage, against a 256-entry window that turns over as scores land, and
`review_sizing` does the same per instrument. Ten scored refusals landing
eight-and-two happens under a coin about once in nineteen. A platform that
looks nineteen times sees it, and on the strength of it opens a proposal to
*loosen a risk rule* and puts two operators in front of a signature page. The
row's own risk is named "overfitting to counterfactual results", and the
number that would reveal it is the number of looks — which nothing counted.

## Decision

**Every counterfactual finding that asks a control to move is charged one
trial to the trial book before it is judged, and must clear a bar corrected
for how many trials the quarter has already been charged.**

Two seams, and they are exactly the findings with a consequence:

- `Platform::review_rules`' recalibration proposal (§12.3's first row) — the
  only counterfactual finding in this tree that can end in a loosened risk
  bound, via ADR 0061's two signatures and a mounted file.
- `Platform::review_sizing`' larger-size proposal (ADR 0063's loosening half).

`qip_kernel::counterfactual_trial` holds the arithmetic. A finding of
`supporting` out of `sample` is tested against the null that a scored path
favours it as often as not (`p₀ = 0.5`) — the weakest null available, chosen
because any stronger one would be a figure nobody here has measured. The tail
is the normal approximation to the binomial with the half-unit continuity
correction, through `qip_numerics::distributions::normal_cdf`, the same
function `deflated_sharpe` reads. The bar is Bonferroni: `0.05` divided by the
trials charged to the counterfactual family in the calendar quarter, this
finding's own included.

Refusal is fail-closed at both seams: no proposal is opened, nothing is
journaled, the bound stays where it is, and the refusal is reported as a
problem on the cycle carrying the evidence it weighed, so an operator sees the
review ran and found the evidence wanting rather than seeing silence.

### Shared book, separate budget

The residual doubt the triage raised was whether the promotion trial book can
be reused for counterfactuals or whether they need their own. **They share the
book and they do not share the budget**, and both halves are load-bearing.

**The book must be shared.** `TrialBook::open` scans one key prefix and
replays every family's hash chain; two books over one store would each replay
the other's records and then append at colliding sequences, which fails
`TrialBook::verify` — reported as tampering, which is what it would look like.
A second store would need a composition root to open it, and the three roots
already call `open_trial_book` exactly once. Sharing the book also means the
counterfactual journal inherits the properties ADR 0023 argued for and no
others: a count that only rises, hash-chained, refusing an altered, removed,
reordered or backdated record, and surviving the process once a root opens it
durably.

**The budget must not be shared, and it is not, structurally.** `TrialBook`
budgets per *family* per quarter, not per book. `COUNTERFACTUAL_FAMILY` is a
reserved name and `TrialBook::enrol` now refuses both crossings: a strategy
into the counterfactual family, and a counterfactual subject into any other.
So a quarter of rule reviews cannot leave a sweep with no budget to promote
on, and a sweep of five hundred configurations cannot leave a finding
untestable. Without the second refusal in particular the two would cross by
accident rather than by malice: an instrument and a strategy sharing a name is
ordinary, and the finding would have spent that strategy family's budget.

### Quarterly and not lifetime

The correction counts the quarter's trials, not the family's lifetime. The
deflated Sharpe corrects against lifetime and this deliberately does not,
because the two are answering different questions about different evidence.
A promotion's evidence is fixed and its trial count is the size of the search
that produced it; a counterfactual finding's evidence is a rolling window that
turns over, and the family-wise error rate that matters is over a review
period. Correcting against lifetime would drive the bar to zero — after a
hundred thousand looks no sample a 256-entry window can hold clears `5e-7` —
and the gate would stop being able to admit anything at all. That is the
`MaxExpectedShortfall` defect with the sign reversed: a control that reads as
discipline and is in fact a stop. The quarter is also the window the book
already budgets in (§20.1), so the correction and the budget are read off one
record rather than two clocks.

### What is deliberately not gated

`Platform::counterfactual_sizing_multiplier`, `sizing_review::cap_multiplier`
and the rule **defence** are not charged and not judged here.

The first two can only narrow: their finding halves what the platform will
size into. A trial gate in front of either would be a statistical test whose
*refusal makes the platform trade larger* — a gate that can only loosen, which
is precisely what §12.4's fourth guardrail forbids. This is the one place in
the design where "put every finding through the gate" and "fail closed" point
in opposite directions, and fail-closed wins.

The defence records that a rule earned its place, moves nothing, and is
re-derived whenever a new score lands, so charging it would spend on
restatement the budget the proposals need — the same budget-crossing argument
one level down.

## What was checked before committing to it

- **No new dependency.** `normal_cdf` is `qip-numerics`, already a
  `qip-kernel` dependency; `TrialBook` is `qip-lifecycle`, likewise. `serde`
  and `serde_json` remain the only third-party crates (ADR 0002, ADR 0009).
- **"Never loosened automatically" is intact and was re-checked, not
  assumed.** No branch added here returns more than `Decimal::ONE`; nothing
  added here returns a `Decimal` at all. The change can only subtract a
  proposal, never add one, and `security.rs::no_code_path_assigns_a_limit_set_after_boot_and_no_root_reads_one_from_anywhere_but_its_configuration`
  still passes.
- **The gate can refuse and can admit**, and both are proven at the production
  seam rather than against the arithmetic alone
  (`platform::counterfactual_trial_seam_tests`). A gate proven only to refuse
  is the defect this record's own "quarterly and not lifetime" section
  describes.
- **Cardinality.** One new series, `qip_counterfactual_trials_total{outcome}`,
  bounded by three source-file literals. The subject is deliberately not a
  label: a rule name or an instrument is the unbounded cardinality
  `.claude/rules/domains/observability.md` refuses.

## What it costs

**A finding now needs more evidence than ADR 0055 asked for, and how much
more depends on how often the platform has already looked.** At the first look
of a quarter, ten unanimously correct declines still clear; eight of ten no
longer does, though ADR 0055's fixed bar admitted it. At the hundredth look,
neither does, and a finding at the 0.75 fraction needs roughly sixty
observations. At the five-hundredth — the budget — nothing further is tested
at all until the quarter turns, and the refusal says so under its own
`uncharged` arm rather than reading as a quiet quarter with nothing found.

**Findings will be missed.** A rule that is genuinely too tight, discovered
late in a heavily-reviewed quarter, is not proposed; the evidence stays in the
window and is proposed next quarter if it survives. That is the price of the
correction and it is the intended direction: the thing being deferred is a
*loosening*.

**The default book is in-memory.** `StrategyFactory::new` builds
`TrialBook::in_memory()`, so until a composition root calls
`Platform::open_trial_book` the counterfactual quarter count is this
process's. A restart therefore resets the correction, exactly as it resets the
promotion count, and for the same reason and with the same remedy.

## What would make this wrong

- **A third seam is added that acts on a counterfactual finding and does not
  come through `Platform::counterfactual_trial`.** The invariant is not how
  many seams there are; it is that every finding which can move a control is
  charged. Check with
  `grep -n 'counterfactual_trial(' backend/crates/runtime/qip-kernel/src/platform.rs`
  and read each hit.
- **A counterfactual subject is enrolled in a strategy family, or a strategy
  in the counterfactual family.** Either makes the two budgets one, and the
  failure is silent: a promotion refused for a reason that has nothing to do
  with the strategy. `TrialBook::check_reservation` is what holds it, and
  `neither_kind_of_subject_can_be_enrolled_in_the_others_family` is what
  proves it holds.
- **`COUNTERFACTUAL_FAMILY_WISE_ALPHA` is moved after seeing which findings
  the gate refused.** That is the failure this record exists to stop, one
  level up, and ADR 0054 names the same one for a different threshold.
- **The correction is changed to lifetime** without re-reading the argument
  above. It will look more rigorous and it will turn the gate into a stop.
- **A gate is put in front of the narrowing multipliers** because "every
  finding should go through the gate" reads well. Its refusal widens.
