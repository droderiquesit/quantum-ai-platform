# ADR 0498: The NOW Brain's standard ensemble carries a neural state-space estimator and a symbolic estimator, each abstaining where it would only repeat the Kalman answer

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**
  (a session decides the in-repository design question it meets, by ADR),
  session t50-36. The delegation's limits hold: it sets no figure the owner
  has not stated, changes no requirement's wording, adds no dependency and
  touches no paper-trading layer. The constants it introduces — two lags, a
  learning rate of one half, an error decay of one fifth, a binding mass of
  one in a thousand — are tuning of an estimator, not limits; nothing sizes,
  vetoes or trades on them.
- **Date**: 2026-11-13
- **Rows**: R12-WORLD-3078.

## Context

R12-WORLD-3078: "Bayesian/state-space filters, neural state-space models,
graph models, causal models and symbolic constraints can contribute
competing state estimates." Before this record `NowBrain::standard`
registered the Kalman, particle and robust filters and the graph estimator.
`EstimatorKind::Neural` named a family with no implementation, and the
catalogue's `Constraint` could only veto a candidate, never propose one, so
two of the five named kinds contributed nothing. The causal estimator exists
but stays out of the standard set (see "What remains").

The hazard the requirement names — "a single estimator's failure mode
becomes the platform's" — has a mirror image the earlier graph and causal
work met: an estimator that only repeats another's answer is counted by the
ensemble as agreement that was never independent, and shrinks the disputed
spread on no new evidence.

## Decision

1. **A neural state-space estimator, `now::NeuralFilter`
   (`neural-adaline`).** One adaptive linear neuron per variable, recurring
   over the variable's last two increments and trained online by normalised
   least mean squares. Its transition is learned where the Kalman filter's is
   assumed to be a random walk, so a trending or mean-reverting variable is
   predicted differently by the two. The prediction, at the network's running
   innovation variance, is combined with the readings by precision. Built in
   tree with no randomness and no new crate, so a replay reproduces it. It
   abstains until it holds three reading means, because before then its
   answer is the readings themselves.
2. **A symbolic estimator, `now::SymbolicFilter` (`symbolic-constraint`,
   new `EstimatorKind::Symbolic`).** The declared constraint, now carried on
   `Evidence::constraint`, is read as knowledge: the estimator forms the
   local-level posterior and proposes its moments truncated to the
   admissible set. Where almost the whole posterior lies past a bound it
   proposes the bound. It abstains where the rule does not bind — an
   unbounded variable, or less than one thousandth of the posterior outside
   the set — because there its answer is the Kalman one. The veto is kept:
   the symbolic estimator adds a proposer; it removes no check.
3. **Both join `NowBrain::standard`.** The only promoted belief read
   downstream is the instrument `return` the lattice reads; it is unbounded,
   so the symbolic estimator never touches it, and the network moves it only
   once it has learned a transition its own record supports.

## Consequences

- A share read past its hard bound is estimated at the bound instead of
  leaving nothing admissible and the last state standing.
- Each disagreement the new estimators raise is journaled as a
  `ModelDisagreement` naming them; a full cycle shows the network's dispute
  on an accelerating return.
- The NOW review's `ensemble` lists six workers, not four.

## What it costs

- Two more workers run on every variable each step. The network keeps three
  reading means and two weights per variable, bounded by the store's own
  variable bound and dropped on expiry; the symbolic estimator keeps nothing.
- Every downstream reader of the instrument `return` now reads an ensemble
  that includes a learned transition, so a lattice forecast moves slightly
  when the network has learned a trend, and a replay of an older log under
  this build promotes different means from the one that wrote it.
- The NOW review's `ensemble` lists six workers, and any reader that
  counted four had to be updated (two kernel tests were).

## What remains

The causal estimator is still not standard. The causal graph's edge
strength is a Granger partial R² without its lag, and fused by default it
would bias every promoted belief toward `R²·parent`. It joins once edges
carry a fitted coefficient applied at its lag, restricted to return.

## What would make this wrong

- The network fits noise: on a variable whose increments are white, its
  weights wander and it disputes the Kalman filter on nothing. The running
  innovation variance then stays large and the prediction is weighted
  little, but a record of spurious disputes from it would argue for a
  longer warm-up or a shrinkage of the weights.
- A reader starts thresholding a bounded variable at its bound (`halted`
  read as `> 0`). The truncated mean of a posterior resting on the bound
  sits just inside it, and a reader that wanted the reading and not the
  belief must read the reading.
