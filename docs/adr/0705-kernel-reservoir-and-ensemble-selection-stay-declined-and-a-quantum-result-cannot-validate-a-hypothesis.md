# ADR 0705: Kernel, reservoir and ensemble-selection rows stay declined, and a quantum exploration result cannot validate a hypothesis by any path

- **Status**: Accepted 2026-11-16 under decision 40 of
  docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of
  2026-11-15). It amends no Accepted record and touches no risk limit, capital
  control, billed infrastructure or paper-trading layer. It applies ADR 0150
  and ADR 0190 to three rows and narrows one open gap on a fourth. It records
  decisions; it claims nothing built or tested.
- **Date**: 2026-11-16
- **Rows**: R12-QUANT-4036, R12-QUANT-4037, R12-QUANT-4041, R12-QUANT-4042

## Context

Four open rows sit in the Quantum Foundry scope.

- R12-QUANT-4036 asks that quantum kernels and variational circuits be
  evaluated for out-of-sample uplift over a strong classical model. The
  register marks it MISSING and flags NEW_DEPENDENCY: a variational or kernel
  model implies a dependency, which needs its own ADR under ADR 0109.
- R12-QUANT-4037 asks for a reservoir or extreme-learning experiment beside a
  classical reservoir on the same task. MISSING.
- R12-QUANT-4041 asks that model and scenario subsets be selected under a
  compute budget while keeping minority hypotheses, and that a selection that
  drops all of them is refused. MISSING.
- R12-QUANT-4042 asks that quantum-assisted exploration be research only and
  never validation. The register records PARTIAL work: `EvidenceKind::
  QuantumExploration` exists, is not primary and has a ceiling no higher than a
  rumour; `RedTeam::review` raises a Fatal `ResearchOnlySupport` challenge when
  every supporting item is one. The open items are named in the register: the
  guard sits on one path, because `Hypothesis.status` is a public field that any
  holder can set to Approved; a quantum result reaching an agent finding is
  typed by `engine.rs` as a computation unless its evidence id carries
  `QUANTUM_EVIDENCE_PREFIX`; and no production finding cites a quantum id yet.

ADR 0150 decision A2 allows a family beyond the portfolio QUBO only after a
committed note shows that the instance sizes the platform poses lie above the
exact-enumeration limit and at or below the provider's qubit ceiling. ADR 0190
kept the parked families parked under the 2026-10-24 delegation. A patch holding
kernel and reservoir experiments and a scenario-selection check is parked,
unlanded, at `docs/blueprint/lanes/L4-quant-research-families-adr0150.patch`.
No benefit note exists. ADR 0704 handled four sibling rows the same way.

## Decision

1. **R12-QUANT-4036 is declined for now.** The kernel and variational family
   stays "not built" in the Foundry registry. Its encoding, baseline and
   benchmark are written only after the benefit note ADR 0150 A2 requires. The
   NEW_DEPENDENCY flag stands: if a note ever justifies the family and the
   family needs a crate, that crate needs its own ADR under ADR 0109 first.
   Nothing here admits a dependency.
2. **R12-QUANT-4037 is declined for now**, on the same ground and with the same
   condition. The reservoir family needs no dependency flag, but it is held by
   A2 all the same. The parked patch is not applied.
3. **R12-QUANT-4041 is declined for now.** Scenario selection is a feature or
   scenario selection family and A2 holds it. The register also notes that the
   parked check covers scenarios only, not world-model members. If the benefit
   note is ever committed, the requirement to refuse a selection that drops
   every minority hypothesis applies to world-model member selection too, and
   the build must cover both before the row can close. Until then no selection
   exists to refuse, and none is added.
4. **R12-QUANT-4042 is built, on the open items only.** This is the one row
   here that does not wait on a benefit note, because it adds a guard and
   enables no quantum path. The decision has three parts.
   - A hypothesis reaches Approved only through one transition method that
     runs the research-only check. The status field stops being writable from
     outside the module. Making the guard structural follows principle 4 of the
     project rules: a check one path performs and another can skip does not
     hold.
   - A finding whose evidence id carries the quantum prefix is typed as
     exploration by every path that builds evidence from a finding, not only
     the one `engine.rs` path.
   - The check refuses where exploration is the only support, and a weak
     primary item beside exploration does not clear it. That behaviour exists
     in the register's evidence and must be kept, not loosened.
   The change touches `qip-reasoning-engine` only. Whether any in-tree code
   outside that crate sets the status field is not known to this record; the
   build lane must find every such site before closing the field, and must
   refuse to land if it cannot.
5. **No figure is chosen.** No instance-size threshold, qubit ceiling,
   uplift margin or compute budget is set here. The benefit note must name the
   instance sizes; this record names them as needed figures and chooses none.
6. **The paper-trading boundary is untouched.** Nothing here adds an order
   path or changes a limit. No quantum result becomes a decision input.

## What it costs

Three rows stay MISSING. The desk has no kernel, reservoir or selection
experiment, so no measured answer exists to "does a quantum model add value
here", and the Foundry registry keeps saying not built. That is the honest
state, and ADR 0150 accepted it. The parked patch carries a maintenance cost:
it applied cleanly to one commit and ages with every change to the crate it
touches. Row 4042 stays PARTIAL until the guard is on every path and a
production finding actually cites a quantum id, and closing the status field
may break callers that set it directly, which the build must fix rather than
re-open. The cost of that work is unmeasured.

## What would make this wrong

If a committed note shows that the platform poses instance sizes above the
exact-enumeration limit and within the simulator, A2 no longer holds the three
declined rows, and declining them here would withhold work the owner wants.
If a hypothesis can still reach Approved after the status transition is made
the only path, because a deserialiser or a test helper builds an Approved value
directly, then decision 4 has moved the hole rather than closed it. If
research-only support is later needed in a state other than Approved, such as
a proposal that sizes capital before approval, the check in decision 4 sits too
late. If the owner decides that world-model selection is a different family
from scenario selection under A2, decision 3 should be split.
