# ADR 0170: A refutation vetoes an approved prediction, and a missing input is material only when an explaining agent reports it

- **Status**: **Accepted**, 2026-10-24, decided under the owner's 2026-10-24
  delegation ("Make all decisions for me, I want the end result to have a
  full system"), relayed to this lane by the parent session; its record is
  `docs/blueprint/owner-decisions-2026-10-24.md` on the base branch.
  It admits no dependency, no paid resource and no external action, and it
  changes no paper-trading layer and no financial risk limit.
- **Date**: 2026-10-24
- **Implements**: ADR 0125 decision 2 for the verifier society, which that
  ADR already lists among the advisory stages that may veto a hypothesis.
- **Rows**: R12-REASON-3108, R12-REASON-9032.
- **Implementation**: pending in lane L2-INTEL at the time of acceptance.

## Context

R12-REASON-3108 asks that every important forecast be attacked by eight named
critics, each able to refuse it. The society runs in REASON
(`TournamentStage::observe_reasoning`) and its refutation only held the
question out of the tournament: `Platform::observe_hypothesis` returned a
clause and nothing else, so a refuted leading explanation was still sized.
ADR 0125 decided that such a refutation may veto. The code was left.

Lane L2-INTEL built that veto on 2026-10-24 and reverted it unpushed, because
on the default path it stopped every proposal. The missing-variable critic
refutes any prediction whose panel reported a missing input, and every
default panel reports two that are not variables of the explanation at all:

- `quantum-optimization` (a Construction agent): "a reachable quantum backend;
  this deployment has no credential or endpoint configured, so the classical
  solver is used". It is a fact about the deployment, true of every
  hypothesis, and ADR 0006 already makes the classical solver the answer.
- `compliance-control` (a Control agent): "compliance register entry for
  <instrument>". It is a control's coverage gap, which that control already
  escalates on its own finding.

Every approved hypothesis was refuted, and 27 `qip-kernel` tests lost their
proposals.

## Decision

1. **A refutation of an important prediction vetoes it.** `observe_reasoning`
   returns the society's refutation with its clause (`ReasonVerdict`), and
   REASON honours it like the rule pack's veto: an approved hypothesis the
   society refuted forms no thesis and no proposal, recorded as "approved but
   refuted by the verifier society". It is a veto, never a size, and it can
   only remove a thesis.
2. **A missing input is material to an explanation only when an explaining
   agent reported it.** The agents that explain are those whose manifest role
   is `Research` or `Adversarial`. A missing input reported by a
   `Construction`, `Control`, `Execution`, `Learning` or `Coordination` agent
   is about how the platform acts, or about a control's coverage, and is not
   a variable the explanation left out. The prediction dossier the society
   attacks carries only the material missing inputs; the panel report keeps
   all of them, unchanged, for the record and for the controls that own them.
3. Nothing else about the critics changes. The other seven attack as before.

## Consequences

- A refuted leading explanation is no longer sized and traded.
- A deployment-wide absence no longer refutes every hypothesis, so the veto
  can be on without stopping the desk. A research analyst that reports a
  missing input about the subject still refutes.
- The compliance register's coverage gap stays the compliance control's
  finding, as it was; it does not become a reason to doubt the explanation.

## What it costs

- A prediction an explaining agent flagged as missing data is now refused
  rather than sized small, so the desk trades less often on thin evidence.
  That is the intended direction; the cost is fewer paper positions while
  research analysts report gaps.
- Missing inputs from non-explaining agents no longer count against an
  explanation. If one of them did matter, the society now misses it (see
  below).
- The veto adds one more reason a cycle can end with no proposal, and an
  operator reading the REASON detail has one more cause to tell apart.

## What would make this wrong

A non-explaining agent whose missing input does bear on the explanation. If
one appears, the remedy is to give that agent a research role for that input,
not to widen the materiality rule.
