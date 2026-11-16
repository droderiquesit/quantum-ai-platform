# ADR 0704: Four quantum family rows stay declined until a benefit note names the instance sizes the platform poses

- **Status**: Accepted 2026-11-16 under decision 40 of
  docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of
  2026-11-15). It amends no Accepted record and touches no risk limit, capital
  control, billed infrastructure or paper-trading layer. It applies ADR 0150
  and ADR 0190 to four rows and builds nothing.
- **Date**: 2026-11-16
- **Rows**: R12-QUANT-1145, R12-QUANT-1158, R12-QUANT-4033, R12-QUANT-4035

## Context

Four open rows ask for quantum families beyond the portfolio QUBO:

- R12-QUANT-1145 asks that SymbolicCombinatorialSearch and
  ScenarioHypothesisSelection be built with baselines.
- R12-QUANT-1158 asks that the Foundry carry its whole scope: QAOA/QUBO,
  graph/search, feature/ensemble selection, kernels/variational,
  reservoir/ELM and a benchmark harness, each reached from a composition root.
- R12-QUANT-4033 asks that multi-leg path sets be selected by QUBO under
  inventory, settlement, capital and concurrency constraints, against a
  classical solver.
- R12-QUANT-4035 asks that feature sets for specialist or reflex models be
  chosen by quantum-assisted binary selection, against a classical selector.

The register records the same state for all four. `Foundry::platform` marks
these families as not built. A patch holding graph-search, feature-selection
and scenario-selection QUBO families and two QML families is parked, unlanded,
in `docs/blueprint/lanes/L4-quant-research-families-adr0150.patch`. ADR 0150
decision A2 permits a new family's encoding, baseline and benchmark only after
a committed note shows the sizes the platform poses lie above the
exact-enumeration limit and at or below the provider's qubit ceiling. ADR 0150
A6 makes no cycle problem QUBO-admissible. ADR 0190 confirmed both under the
2026-10-24 delegation. No such note exists. The register's own `gap` text says
the rows close on a committed benefit note, a wider qubit ceiling, or an owner
decision superseding A2.

This record does not reopen those gates. It decides, row by row, what each
gap needs, so that a later reader finds a stated next step instead of a block.

## Decision

1. **R12-QUANT-1145: decline.** ScenarioHypothesisSelection stays not built
   under A2 and ADR 0190 decision 2. SymbolicCombinatorialSearch stays not
   built under A6 and A2 together: no symbolic search problem is posed to the
   platform, so there is no instance whose size could be measured. The row is
   graded against the declined clause, as ADR 0190 decision 3 directs. The
   parked patch is neither landed nor deleted.
2. **R12-QUANT-1158: decline the five families beyond the portfolio QUBO, and
   record the portfolio half as the part that holds.** The scope item "Hybrid
   QAOA/QUBO optimization" and the benchmark harness exist for portfolio
   allocation. Graph/search, feature/ensemble selection, kernels/variational
   and reservoir/ELM stay not built, each under A1 and A2. The row stays
   PARTIAL and is not closed by an inspection that counts registered but unbuilt
   families. No work item to "integrate" an unbuilt family is opened.
3. **R12-QUANT-4033: need a figure; choose none.** The figure is the largest
   number of simultaneous candidate legs or cycles the platform's path router
   actually poses in one selection, taken from a committed measurement over the
   event log or a replay. Until it exists the row is declined under A6. If the
   figure falls in the A2 window, the owner or a later ADR must also state how
   A6 is amended, because A6 currently rules cycle problems out regardless of
   size. The benchmark must be against `qip-arbitrage`'s own path router, not
   only greedy-by-weight, since the parked patch has not been run against it.
4. **R12-QUANT-4035: need a figure; choose none.** The figure is the number of
   candidate features or learners a specialist or reflex model selection
   actually chooses among, from a committed measurement. A2 keeps
   FeatureEnsembleSelection not built on purpose, and forward selection over
   fewer candidates than the enumeration limit is exactly solvable, so a
   quantum selector could at best tie. The held-out-loss check named in the
   requirement is not built until the figure shows otherwise.
5. **One benefit note serves all four rows.** The note is a committed document
   giving, per family, the instance size the platform poses, the exact
   enumeration limit and the provider ceiling in force when it was written,
   and the classical baseline it is measured against. It is a measurement, not
   a projection. Its authorship and the choice to commit it are the owner's;
   this record sets no deadline and no threshold.
6. **A wider qubit ceiling is left to the owner.** The register names it as a
   way to close these rows. It needs a vendor, a credential and billed hardware
   time, none of which this session can supply. No constant in the Foundry or
   the router is changed here.
7. **Nothing here lands or runs a quantum path in a binary.** Even after a
   benefit note, the families run in no binary until a deep-brain research
   sweep is built. That sweep is a separate decision, and every quantum run
   keeps the classical baseline of ADR 0006 beside it.

## What it costs

Four rows stay open, and three of them (1145, 1158, 4035) stay MISSING or
PARTIAL for an indefinite time. The parked patch, written and mutation-checked
on 2026-10-24, sits idle and drifts from the tree it applied to, so it will
need re-applying and re-running when a case appears. The research desk has no
quantum family beyond portfolio allocation to experiment with, so a
quantum-assisted idea for paths or feature sets cannot be tried even as a
tie. Decisions 3 and 4 ask the owner for measurements that nobody has taken,
and until they are taken the rows cannot move in either direction. No figure
is chosen, so no threshold can be wrong, but no threshold exists either.

## What would make this wrong

If a committed note shows the path router or a feature selection posing
instances above the exact-enumeration limit and within the simulator's qubit
ceiling, then declining here only delayed a justified family and the parked
patch should be re-applied and re-verified. If the owner decides that
research families should exist ahead of a benefit case, ADR 0150 A2 must be
superseded by a record that says so, and this one falls with it. If the
benefit note comes back showing every posed instance is exactly enumerable,
the right outcome is to withdraw these requirements by the owner's act on
`requirements.md`, not to keep them open indefinitely. If a reader treats the
Foundry's registered-but-unbuilt entries as evidence of capability, this
record has failed to make the gap visible.
