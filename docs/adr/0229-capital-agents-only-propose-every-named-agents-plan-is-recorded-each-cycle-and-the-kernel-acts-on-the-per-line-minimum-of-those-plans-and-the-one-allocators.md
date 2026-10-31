# ADR 0229: Capital agents only propose: every named agent's plan is recorded each cycle, and the kernel acts on the per-line minimum of those plans and the one allocator's

- **Status**: **Accepted 2026-10-31 under the owner's delegation** (decision
  10 of `docs/blueprint/owner-decisions-2026-10-31.md`, the ruling on
  R12-CAPITAL-4085). The limits of that delegation hold here: no risk
  number, no limit, threshold or limit set changes, nothing new is sized
  (decision 7 of the same file), no billed infrastructure, and no
  paper-trading layer is touched. Tighten-only by construction: no plan can
  widen an allocation line, so the plan acted on is never larger than the
  allocator's.
- **Date**: 2026-10-31
- **Rows**: R12-CAPITAL-4085.
- **Extends**: ADR 0127 (the capital-side brains are crate boundaries and
  loops the kernel composes), ADR 0153 (tax has no ledger input).

## Context

R12-CAPITAL-4085 asks for a "capital society": treasury, liquidity,
funding, leverage, collateral, strategy-allocation, region-allocation,
venue-allocation, tax-aware and survival agents submitting competing capital
plans, with the kernel recording all of them each cycle before it chooses.

The platform has one allocator, `qip_capital::CapitalAllocator`, reached
from `CentralPlane::allocate`. Its plan is the only one that exists. The
modules a society would draw on (treasury, funding, margin, collateral,
the survival waterfall) all exist in `qip-capital` and the kernel, but none
of them proposes an allocation line, and several have no production input
at the central plane today.

The ruling approved agents as proposers only, and nothing new sized. That
excludes the reading in which ten agents each size a portfolio and the
kernel picks a winner: a winning plan larger than the allocator's would
size capital no risk record argued for.

## Decision

1. **Ten named agents, one plan each, every cycle.** `qip_capital::society`
   names the ten as a closed enum, `CapitalAgent`, and each produces a
   `CapitalPlan` per allocation: either lines (strategy to notional) or an
   abstention that names the input it lacks.
2. **A plan only narrows.** A line an agent proposes above the allocator's
   own line for that strategy is clipped to it. A strategy an agent's plan
   omits is not dropped by that omission: an agent narrows the lines it has
   an opinion on and is silent on the rest. An abstention constrains
   nothing.
3. **The kernel records before it chooses.** `CentralPlane::allocate`
   computes the allocator's plan, collects all ten agents' plans, records
   them on the plane (`CentralPlane::society`), and only then returns the
   plan acted on: each allocation line at the minimum of the allocator's
   notional and every non-abstaining plan's line for that strategy. A line
   an agent narrowed names the agent among its binding constraints.
4. **What each agent reads today.** The strategy-allocation agent proposes
   the allocator's lines: the allocator is that agent. The survival agent
   and the region-allocation agent narrow by figures already on record, the
   survival budget and the region ceilings, when the central plane is given
   them, and abstain naming the missing figure when it is not. The other
   six (treasury, liquidity, funding, leverage, collateral, venue
   allocation) abstain naming the input the central plane does not yet hand
   them. The tax-aware agent abstains citing ADR 0153 until tax has an
   input. Wiring an input to an agent is a later change that can only
   narrow.

## Consequences

- With no figure handed to any agent, the plan acted on is exactly the
  allocator's, so this changes no allocation on any path today.
- Every cycle's allocation now carries ten recorded plans, so a reader can
  see which agents had an opinion and which abstained, and why.
- A future agent input is safe to wire without a new review of the
  arithmetic: the per-line minimum cannot exceed the allocator's line.

## What it costs

- Ten small proposers in `qip-capital` and one record on the central plane,
  bounded by the number of strategies holding capital.
- A reader must remember that an abstaining agent is not a zero-line agent:
  abstention constrains nothing, and that is deliberate (a missing input is
  not evidence that a line should be zero).

## What would make this wrong

- A requirement that agents *compete* to size, with the kernel adopting the
  best plan whole. That reading sizes capital outside the allocator and
  needs its own risk record with the numbers argued (decision 7).
- An agent whose honest answer is "this line should be zero" for want of
  input. That agent should propose zero on the lines it can see, not
  abstain; abstention is for an agent that has no opinion at all.
