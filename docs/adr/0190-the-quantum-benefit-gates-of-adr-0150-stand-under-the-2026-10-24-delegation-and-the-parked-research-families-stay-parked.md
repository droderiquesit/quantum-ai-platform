# ADR 0190: The quantum benefit gates of ADR 0150 stand under the 2026-10-24 delegation, and the parked research families stay parked

- **Status**: **Accepted**, 2026-10-24, decided under the owner's delegation
  of the same day (`docs/blueprint/owner-decisions-2026-10-24.md`: "Make all
  decisions for me"). The parent session that allocated this block said, in
  as many words, that ADR 0150's quantum benefit gates stay and that the
  classical-baseline principle is not relaxed by the delegation. This record
  is that decision taken explicitly rather than by omission, so that each row
  waiting on it can cite a decision instead of a silence.
- **Date**: 2026-10-24
- **Keeps**: ADR 0006 (a classical baseline for every quantum result), ADR
  0150 decisions A1 to A6 unchanged, ADR 0003 (paper only).
- **Extends**: ADR 0150 by recording what a delegated decision may and may not
  do to it.

## Context

Twenty-five register rows in lane L4-quant wait on ADR 0150. They ask for
graph search, feature and ensemble selection, quantum kernels, reservoir
learning, scenario and hypothesis selection, evidence-graph clustering, and
for each the classical check the blueprint attaches to it. They also ask for
QUBOs for capital placement, collateral, funding, hedging, multi-leg paths
and simulation sampling, and for a default-path problem that exercises the
quantum arm. ADR 0150 answered all of them:
- A1 keeps the portfolio QUBO as the only quantum family.
- A2 lets a new family's encoding, baseline and benchmark be written only
  after a committed note shows the platform's own problem sizes lie above
  the exact-enumeration limit and at or below the provider's qubit ceiling.
- A3 adds no default-path problem engineered to switch the arm on.
- A4 gives a continuous problem no QUBO.
- A6 makes no cycle problem QUBO-admissible.

The window A2 names is two variables wide on today's constants:
`DEFAULT_EXACT_ENUMERATION_LIMIT` is 22, `MAX_QUBITS` is 24, and nothing
the cycle poses lands in it.

On 2026-10-24 lane L4-quant wrote the five research families anyway:
encodings, greedy or in-sample-chosen classical baselines, a validated
experiment record carrying a problem digest, and registration in
`Foundry::platform`. The lane then read A2, reverted the code, and parked it,
unlanded, as `docs/blueprint/lanes/L4-quant-research-families-adr0150.patch`.
Its tests and mutations ran green before the revert.

The delegation lets this session supersede a decision. That leaves two
questions: whether it should supersede this one, and what the rows say if
it does not.

## Decision

1. **ADR 0150's gates stand.** A1, A2, A3, A4 and A6 are not superseded.
   The delegation is used to *confirm* them.
   - A2 is the platform's statement of ADR 0006's principle applied to
     research: a quantum method runs where a measured benefit against the
     classical baseline is demonstrable.
   - On today's sizes no family can show one. Every posed instance is
     exactly enumerable, so a quantum answer could at best tie, and ADR
     0006 never adopts a tie.
   - Landing encodings for problems nobody poses would produce benchmark
     records that read as evidence and are not. That is the outcome A3
     already refused for the portfolio arm.
2. **The parked patch stays parked.** It is not deleted: it is the work a
   benefit case would land, and its tests are the proof it would carry.
3. **The rows are graded honestly against this record.** A row whose
   requirement asks for a family or a check that A1 or A2 excludes stays
   PARTIAL, or MISSING where nothing in the tree serves it, and its `gap`
   cites this record and ADR 0150. Under the COMPLETE bar's part 4, a clause
   an ADR declined stays unmet unless the requirement's own text allows
   withdrawing it. This lane does not reword requirement text: rewording is
   the owner's act on `requirements.md`, not a lane's on the register.
4. **Clauses that hold for the one family that exists are proven where they
   hold.** The portfolio arm's checks are tests:
   - classical validation of the QAOA answer
     (`ClassicalValidator::validate_qaoa`, R12-QUANT-1012);
   - transaction cost and capital invariants
     (`a_quantum_allocation_that_wins_only_by_ignoring_transaction_cost_is_refused`);
   - pre-trade risk after a winning answer
     (`a_position_sized_from_a_winning_quantum_answer_is_still_refused_by_pre_trade_risk`).
   A row that names the portfolio family among others records the portfolio
   half as proven and the rest as declined.

## What would make this wrong, and what changes it

- **A benefit note.** If a committed measurement shows a posed problem in
  the window above enumeration and within the qubit ceiling, then A2's
  condition is met for that family and the matching part of the parked
  patch lands under its own ADR. A hosted provider with a larger ceiling
  is a billed decision the owner reserved, and the same holds for a raised
  `MAX_QUBITS` tested against it. Either one widens the window.
- **The owner rewording a requirement** to the scope ADR 0150 decided: for
  example, R12-QUANT-4057 to the portfolio family, or R12-QUANT-4040 to a
  capability test of the encoder. The rows would then be graded on the new
  text.
- **A family whose problem stops being enumerable**: a cycle that approves
  many theses at once, or a universe large enough that a discrete
  allocation exceeds the enumeration limit. That would change A3's premise
  on the default path, and it is a risk setting
  (`Mandate.maximum_holdings`) the owner reserved, not a solver setting.

## What it costs

- Twenty-five rows stay open, and the register reads as less complete than
  a lane could make it look by landing code nobody poses a problem for.
  That is the intended cost.
- The parked patch drifts as the tree moves. It applies cleanly to the
  commit named in its filename's lane notes, and whoever lands it re-runs
  its tests first.

## Rows

R12-QUANT-1077, -1078, -1079, -1080, -1081, -1145, -1158, -4032, -4033,
-4034, -4035, -4036, -4037, -4039, -4040, -4041, -4050, -4054, -4055, -4056,
-4057, -4058, -4060, -4063 and R12-EXPAND-6036. Each stays PARTIAL, or
MISSING where nothing in the tree serves it, with its gap citing this
record.
