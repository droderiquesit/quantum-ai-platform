# ADR 0405: The risk limit in force is the operator's ceiling narrowed by the desk mandate's risk tolerance

- **Status**: **Accepted 2026-11-11 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR). The design is owner decision 30, fixed by the
  orchestrator from the owner's instruction of 2026-11-11: "Make the risk
  limitimit dynamic based on tolerance." Decisions 28 and 30 reach the
  integration branch with round 27; until then the orchestrator's spawn
  message is their text, and this record restates decision 30 in full below.
  The delegation's limits hold:
  - it sets no risk number: the tolerance is the one the ledger already
    holds, and its default is the value the desk has always had;
  - it relaxes no limit, guard or refusal, and it only narrows;
  - it touches no paper-trading layer and adds no path to a venue.
- **Date**: 2026-11-11
- **Extends**: ADR 0061 (a bound moves only through the file the composition
  root reads at boot). That file remains the only way a *ceiling* moves.
  This record adds a second, narrowing-only factor and changes no clause of
  ADR 0061.

## Context

The limit set a platform runs under is fixed at boot. The capital ledger
holds a risk tolerance on every mandate (`Mandate::risk_tolerance`, a
`Decimal` fraction in [0, 1], refused outside that range), and the registry
never lets a user hold one above the desk's. Until this record the tolerance
did one thing: `UserLedger::assess` refused a funding request that would put
more than `capital × tolerance` at one strategy
(`RefusedLimit::RiskTolerance`). Nothing connected it to the limit set. A
desk could state a tolerance of 0.5 and be checked against the same bounds
as a desk that stated 1.

## Decision

1. **One tolerance.** The tolerance is `Mandate::risk_tolerance`. No second
   figure is introduced and no value is invented for it.
2. **The operator's limit set is the ceiling.** The limit in force is the
   ceiling narrowed by the tolerance. A maximum kind is multiplied by the
   fraction. A minimum kind (`LimitKind::is_minimum`: liquidity, cash
   buffer, funding buffer) is left exactly where the operator put it.
   Tolerance 1 gives the ceiling exactly. Tolerance 0 gives a maximum of
   zero. Nothing can raise a limit above its ceiling.
3. **Deterministic and early.** The narrowing is plain arithmetic, done when
   the pre-trade checker is built and therefore before any order object
   exists. It is never routed to a model. No model output, agent finding or
   side-channel setting changes the tolerance: the kernel reads it from the
   mandate registry and from nowhere else.
4. **Refuse, do not clamp.** A tolerance outside [0, 1], or one that is not
   a finite number in [0, 1] once it has crossed to `f64`, is an error whose
   message says what to do instead. So is a maximum whose ceiling is
   negative or not finite, because multiplying a negative bound by a
   fraction would loosen it.
5. **A refusal explains itself.** A breach under a narrowed limit names the
   ceiling, the tolerance and the limit in force, in its text and as fields.
6. **It must be able to fire.** A test shows a narrowed limit refusing an
   order that the ceiling alone admits.

### Decisions this session made, each with its reason

Decision 28's six preferences are not in this branch. Each choice below was
made by the six principles in the root `CLAUDE.md` and the five questions of
"Judging a proposal" in `.claude/rules/10-product-direction.md`, and the
principle used is named.

**D1. The desk's tolerance governs the desk's limits.** Several mandates
share one book, and the limit set is checked against that one book. The
registry already makes the desk's tolerance the ceiling every user mandate
is admitted under, so it is the one figure that is a statement about the
whole book. The least tolerant user's figure was considered and refused: it
would let the smallest mandate narrow capital that is not its own, and a
user's tolerance already bounds that user's own funding through
`RefusedLimit::RiskTolerance`, which stays. A capital-weighted blend was
refused because it is a new figure nobody stated (decision 1).

**D2. The desk's tolerance needed a producer, and has one through the
mandate path.** `Mandate::desk_exploring` pinned the desk's tolerance at 1,
so a narrowing that read it could never fire in any process: the
`MaxExpectedShortfall` shape, a control that reads as protection and is not.
The desk mandate is assembled from `PlatformConfig` (its capital and its
exploration share already are, and every user mandate arrives in
`PlatformConfig::user_mandates`). This record adds
`PlatformConfig::desk_risk_tolerance`, default 1, which becomes a term of
the desk mandate through `Mandate::desk_tolerating`. `Mandate::new`
validates it and the registry holds it. The kernel's narrowing reads the
registry, not the configuration field. Clause 3's "no configuration value
may change the tolerance" is read as: no setting may override or bypass the
mandate's own term. A stricter reading, under which the desk's mandate may
not be stated at assembly at all, would leave the narrowing unable to fire;
if the owner holds that reading, D2 is the clause to amend and the rest
stands. The three roots that assemble a platform (`qip-api`,
`qip-fastbrain`, `qip-deepbrain`) read the term from
`QIP_DESK_RISK_TOLERANCE`. Unset is 1, the operator's limits exactly as
written, and each root says so in a start-up line. A value that is present
and is not a fraction in [0, 1] stops the process with the variable named;
it is never clamped and never read as unset. dev states `1` (owner
decision 33, 2026-11-13); test, stage and prod state nothing. Stated or
unset, every binary still runs at tolerance 1 until a person gives a lower
figure. See "What remains".

**D3. Tolerance 1 is the ceiling, byte for byte.** At tolerance 1 the
narrowed set equals the ceiling under `PartialEq`, carries no narrowing
mark, and the kernel journals no record. Nothing any existing limit set
allows, reports or serialises changes at tolerance 1. A breach with no
narrowing mark is a breach of the operator's own bound.

**D4. The severity ladder follows the limit in force.** The warning
threshold and the critical multiple are a fraction and a multiple of the
bound a limit is checked at, so they scale with the narrowed bound and are
not themselves changed. A warning is raised at `warning_threshold ×
in force`, a breach above `in force`, a critical breach at
`critical_multiple × in force`. At tolerance 0 the bound is zero: any
positive observation is a critical breach and there is no warning band,
which is what `Limit::assess` already does at a zero bound. Keeping the
ladder on the ceiling was refused: an order could then breach the limit in
force and be reported as a warning.

**D5. Where `Decimal` crosses to `f64`.** The two money-bounded kinds
(`MaxOrderNotional`, `MaxPositionNotional`) are narrowed in `Decimal` and
never cross. Every other maximum is an `f64` statistic, so the tolerance
crosses once, in `LimitKind::narrowed_by`, through `Decimal::to_f64`, and is
checked again after the crossing. The comment is at the crossing.

**D6. Tolerance 0 refuses every order that carries risk.** Every maximum is
zero, so an order is refused if the book it would produce shows any positive
figure under any maximum. Every shipped limit set carries
`max_order_notional`, so every order of non-zero notional is refused. No
separate "halt" arm was added: the arithmetic already refuses, and a second
mechanism for the same fact would be a second claim about it.

**D7. The narrowing reaches the pre-trade checker and nothing else in this
change.** Three readers of the limit set stay on the ceiling:
- *The risk monitor.* Its breaches can force a liquidation, which is a
  larger act than refusing an order, and decision 30 speaks of the pre-trade
  check.
- *Rule review and recalibration (ADR 0061).* A proposal is a new *ceiling*
  for the operator's file. Derived from a narrowed bound it would be
  narrowed again at the next boot.
- *The policy payload's envelope, and so the edge cell.* A cell decides
  alone inside the envelope it was sent (ADR 0008). Sending it the narrowed
  set needs a decision on what a cell does when the tolerance it holds is
  stale, and a payload version. That is a separate decision and is not made
  here. Until it is, a cell's own orders are bounded by the ceiling and by
  its region share (ADR 0039), not by the tolerance.

**D8. The limit in force is reproducible from the event log.** When the
tolerance is below 1 the kernel journals one `LimitsNarrowed` record at
assembly, on `Topic::RiskEvaluated` under the origin
`kernel/limit-narrowing`: the tolerance, the ceiling set's name, and for
every limit its ceiling and its bound in force. The desk's tolerance cannot
change inside a process (the registry has no path that replaces the desk's
mandate), so one record per boot states the limit in force at every
decision of that boot. Each refusal also carries the three figures itself.

## What changes and what does not

- `qip-risk` gains `LimitKind::narrowed_by` and `LimitSet::narrowed_by`, and
  an optional `narrowing` mark on `Limit` and `LimitBreach`.
- `qip-risk-engine` gains `PreTradeChecker::under_tolerance`.
- `qip-capital` gains `Mandate::desk_tolerating`.
- `qip-kernel` gains `PlatformConfig::desk_risk_tolerance`, builds the desk
  mandate with it, builds the order manager's checker from the registry's
  figure, and journals `LimitsNarrowed`. `Platform::limits_in_force` and
  `OrderManager::limits_in_force` read the narrowed set.
- `qip-api`, `qip-fastbrain` and `qip-deepbrain` read
  `QIP_DESK_RISK_TOLERANCE` through `with_desk_risk_tolerance_from`; the
  parser and the variable's name are in `qip_kernel::limit_narrowing`.
- No limit set, bound, threshold or default changes. `Platform::limits` and
  `Platform::risk_limits` still return the ceiling.
  `RefusedLimit::RiskTolerance` stays as it is.
- No dependency is added. No protected path is touched.

## What it costs

A desk that states a tolerance below 1 has orders refused that the
operator's file alone admits. That is the purpose. Two bounds now exist for
one rule, the ceiling and the limit in force, and a reader must know which
one a surface shows: the monitor and the console show the ceiling, a
pre-trade refusal shows both.

## What would make this wrong

- A limit kind whose ceiling is legitimately negative. None exists;
  `narrowed_by` refuses one rather than loosening it.
- A desk mandate that can be replaced while the process runs. Then one
  record per boot would no longer state the limit in force, and the checker
  would have to be rebuilt and the record journalled at each change.
- The monitor forcing a reduction at the ceiling while pre-trade refuses at
  the narrowed bound turns out to confuse the desk. Then D7's first bullet
  is the clause to amend.

## What remains

- A narrowing figure. The Terraform variable `desk_risk_tolerance` carries
  the term to the three roots and refuses a value outside 0 to 1 at plan
  time (`infrastructure/terraform/tests/desk-risk-tolerance.tftest.hcl`).
  dev states `1`, the operator's limits exactly as written, under owner
  decision 33; no environment states a lower one. A figure below one is the
  desk's risk appetite and a person's to give, per environment.
- The edge cell's envelope (D7).

## Evidence

The tests and their mutations are quoted in the commits that build this
record.
