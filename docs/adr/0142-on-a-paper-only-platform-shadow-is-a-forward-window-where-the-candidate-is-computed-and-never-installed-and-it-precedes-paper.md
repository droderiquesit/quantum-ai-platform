# ADR 0142: On a paper-only platform, shadow is a forward window where the candidate is computed and never installed, and it precedes paper

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It defines a term and reorders one
  enum; it touches no live path, since there is none.
- **Date**: 2026-10-18
- **Extends**: the expansion campaign's four-stage record
  (`qip-expansion/src/plan.rs`, `promotion.rs`).
- **Keeps**: ADR 0003 (paper is the only acting environment), ADR 0006 (a
  baseline), ADR 0125 (a candidate that has not acted cannot enlarge
  anything).

## Context

A research campaign records four environments in a fixed order,
`qip_expansion::plan::Environment::ORDER = [Historical, Simulated, Paper,
Shadow]`, and `promotion.rs` refuses a stage out of that order. In
`run_fill_bias_campaign` (`qip-expansion/src/executor.rs`) the stages are:
Historical is the fit on fills before the campaign started; Simulated is a
seeded bootstrap of that sample; Paper and Shadow are the first and second
halves of the fills that arrived after the campaign started. The correction
is not installed in either half — it is produced only at the end — so the
"paper" stage measured a candidate that never acted, exactly as the
"shadow" stage did. R12-EXPAND-6050 is PARTIAL because what "shadow" means
on a paper-only platform was undecided.

In a platform with live trading, shadow means running a candidate beside the
live system without letting it act, before it is allowed to act on paper or
in production. Here paper *is* the acting environment, so the same idea puts
shadow before paper, not after.

Locate them with
`grep -n 'pub enum Environment\|pub const ORDER' backend/crates/services/qip-expansion/src/plan.rs` and
`grep -n 'pub fn run_fill_bias_campaign\|Environment::Paper\|Environment::Shadow' backend/crates/services/qip-expansion/src/executor.rs`.

## Decision

1. **Shadow is a forward window in which the candidate is computed beside the
   paper book and never installed.** Its fills arrive after the campaign
   opened; the candidate's effect is scored against them as if it had been
   applied; the log proves it was not (no installation record for the
   candidate falls inside the window).
2. **Paper is a forward window in which the candidate is installed in the
   paper book and acts.** Its fills arrive after the candidate's installation
   record, are produced under it, and are scored on what actually happened.
   A candidate that was not installed during the window has no paper stage.
3. **The order is Historical, Simulated, Shadow, Paper.** `Environment::ORDER`
   is changed to that, and `promotion.rs` keeps refusing a stage out of
   order. A candidate reaches the acting environment only after it has been
   scored forward without acting.
4. **The two windows are disjoint and ordered in time**, shadow wholly before
   paper, each with a minimum sample the policy states; a campaign whose
   windows overlap is refused.
5. **The fill-bias campaign becomes two-phase.** It records Historical,
   Simulated and Shadow, installs the correction in the paper fill model
   when shadow clears, and records Paper once enough fills have arrived under
   it; if paper does not clear, the correction is withdrawn and the
   withdrawal is on the log.

## Consequences

- "Shadow" and "paper" name different facts again: one says the candidate
  would have helped, the other that it did help when it acted.
- Campaign records written under the old order stay readable and are not
  re-scored; their paper stage is understood as a second shadow window.
- Code slices left: the reordered `ORDER`, the windows' disjointness check,
  the two-phase fill-bias campaign with installation and withdrawal records,
  and a `qip-expansion/tests/loop.rs` test that a candidate never installed
  produces no paper stage and that overlapping windows are refused
  (EXPAND-6050).

## What it costs

- A campaign takes longer: it must wait for a second window of fills after
  installation before it can promote.
- A correction acts in the paper book before it is promoted, so paper results
  during the window reflect a candidate; the installation record makes that
  attributable, but it is a change in what the paper book measures.

## What would make this wrong

- If a live-capital record were ever accepted, there would be an environment
  beyond paper, and shadow would mean running beside it; this record's order
  would then need a fifth stage, not a reinterpretation.
- If installing a candidate in the paper book could affect other strategies'
  results (a shared fill model), decision 2's paper window would contaminate
  their evidence, and installation would have to be scoped to the candidate's
  venue and strategy.

## Rows

R12-EXPAND-6050. It stays PARTIAL until its slices land with a mutated test.
