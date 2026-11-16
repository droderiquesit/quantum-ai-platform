# ADR 0712: Assumption checks wait on a stored fit and desk bands, value of information is priced over the claim, and the Curiosity Engine is the acquisition rank

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-AMBIENT-3068, R12-AMBIENT-3104, R12-AMBIENT-3106

## Context

R12-AMBIENT-3068 asks for sentinels that test whether the premises behind
strategies, beliefs and risk models still hold. The Falsification Book checks
thesis falsifiers once per LEARN. Two premises are unchecked. The risk model
re-estimates betas and specific variance from the retained tape each cycle, so
nothing stores the fit a position was sized under and there is nothing to
compare against. No per-strategy premise is recorded either. The drift route
onward exists: a model-drift observation is filed as an assumption signal.

R12-AMBIENT-3104 asks that a major uncertain decision compute the expected
value of more information and launch the cheapest lawful observation that
discriminates. Today DECIDE records an uncertainty for each sized leg at
conviction one half or below, and the deep brain's sweep turns it into an
information request. The row's register entry lists five shortfalls: no
expected value over competing worlds; a fixed observation order, with the
launched re-sweep inert because the discovery desk is off by default; no
licence check; only sized legs covered; and a test that lowers the review floor.

R12-AMBIENT-3106 asks that the Curiosity Engine rank by significance x
uncertainty reduction x urgency x cost. The acquisition rank
(`AcquisitionCriteria::net_value_bp`) does this. The research queue the desk
starts from is ordered by the expansion curriculum's own score, which keeps a
zero urgency from zeroing a task (EXPAND-6063). The owner ruled on 2026-10-31
that the Curiosity Engine of section 9.10 is the acquisition rank. ADR 0628
fixed that an information request names its acquisition and is dispatched to
its own handler. No Accepted record is amended here.

## Decision

1. **R12-AMBIENT-3068, risk-model premise: need a figure.** The fix is a
   stored fit snapshot (betas, market volatility, stressed correlation) written
   when a position is sized, and a sentinel that compares it with the current
   estimate and raises an assumption signal when they diverge. The snapshot is
   a record read by the attention router and by nothing that sizes or vetoes:
   it adds no limit and relaxes none. The figures needed are the horizon over
   which a fit is trusted and the divergence band per quantity. They are a
   risk-desk decision. None is chosen here.
2. **R12-AMBIENT-3068, strategy premise: leave to the owner.** What counts as
   a premise of a strategy, and who states it, is a choice only the desk can
   make. Until a strategy carries a stated premise there is nothing to
   falsify, and inventing one would be the sentinel asserting about itself.
   The row stays PARTIAL.
3. **R12-AMBIENT-3104, expected value: build.** Value of information is
   computed over two worlds, the thesis claim resolving true or false. The
   probability of each comes from the thesis conviction, as arithmetic and
   with the source stated. An observation is scored by the expected change in
   the decision it would cause, net of its cost, and the cheapest observation
   that clears that bar is the one launched, replacing the fixed first-viable
   order. The payoff of the decision in each world is not fixed here: it is a
   figure the risk desk must state (see 8). Until it does, the score is
   recorded as unpriced, never as zero.
4. **R12-AMBIENT-3104, lawful: build.** `admit_decision_uncertainties` gets
   the same licensed-source check `admit_federation_requests` applies, and an
   observation whose source has no evaluated licensing posture is refused, not
   launched. Licensing is checked before use, as the data rules require.
5. **R12-AMBIENT-3104, coverage: build for unsized, capped, refused and
   vetoed decisions; decline hedges and capital moves.** A rejected or vetoed
   hypothesis is still an uncertain decision and is covered. A hedge or a
   capital move has no claim that resolves true or false, so no observation
   discriminates between worlds for it, and covering it would be a score with
   no premise.
6. **R12-AMBIENT-3104, observation that executes: leave to the owner.**
   Enabling the discovery desk (`every_cycles` above zero) changes what the
   deployment spends and fetches. Turning it on is a configuration and budget
   choice for the owner. Nothing here enables it or raises a budget.
7. **R12-AMBIENT-3104, default path: build.** The integration test runs
   under the default `ReviewPolicy`, not a lowered floor.
8. **R12-AMBIENT-3104, per-world payoff: need a figure.** The payoff of the
   decision in each world, and the conviction level counted as high
   uncertainty, are risk-desk figures. The existing floor of one half is kept
   as the restrictive bound until the desk states another. None is chosen.
9. **R12-AMBIENT-3106: decline widening, record the narrowing.** The
   Curiosity Engine of section 9.10 is the acquisition rank, as the owner ruled
   on 2026-10-31. The expansion curriculum keeps its own order and its zero-
   urgency rule, so EXPAND-6063 stands. `Criteria::score` is not changed.
   Cost enters the rank as the share of the decision's value it leaves,
   `(1 - cost)`, not as a bare multiplier, so a free observation scores at the
   top and an observation costing the whole value scores zero. The uncertainty
   factor is the gap's severity, equal across options, not a per-observation
   measured reduction. That is a stated limit of this rank, and a per-option
   reduction is the work decision 3 builds for decisions.
10. **R12-AMBIENT-3106, unproved derivation: build a test.** A test drives
    `ResearchLoop::admit` and fails when urgency is replaced by the floor. It
    is mutation-verified, with the mutation and the failing assertion reported.

## What it costs

Decision 1 adds a stored record per sized position and a comparison each
cycle. Its size and cost are unmeasured. Until the desk states bands, the
risk-model premise stays unchecked and 3068 stays PARTIAL. Decision 3 makes
the sweep do more work per uncertain decision, unmeasured, and the two-world
reading is coarser than a full posterior: a thesis with more than two
outcomes is scored on its stated claim only. Decision 5 widens the set of
decisions that raise requests, which grows the request queue; the existing
admission cap bounds it, and no cap is changed. Decision 9 leaves two
rankings in the deep brain, which a reader must keep apart. Nothing here
makes an observation run, so 3104 stays PARTIAL until the owner acts on 6.

## What would make this wrong

If conviction is poorly calibrated, the probabilities in decision 3 are
wrong and the expected value ranks the wrong observation first; the
calibration series from LEARN is the check, and a persistent miss means
conviction is the wrong prior. If the stored fit moves every cycle by more
than any band the desk can state, the premise check is noise and the snapshot
should be dropped. If the desk finds hedges and capital moves do have
discriminating observations, decision 5's decline is wrong. If the two rankings in
decision 9 produce research that the desk finds contradictory in practice,
the curriculum and the acquisition rank need one order, and that needs its own
ADR amending EXPAND-6063. If the licence check in decision 4 refuses every
cheap source, the posture evaluation, not the gate, is what is missing.
