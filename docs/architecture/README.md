# Architecture

## The shape of the thing

The platform is an organisation, not a program. 18 governed agents with
declared purposes, explicit capability grants, resource budgets, owners and
expiry dates; a reasoning stage that attacks their conclusions before anyone
acts on them; two control functions that can say no; and a learning stage that
scores what actually happened.

Everything below follows from that.

## Crate layers

Dependencies point one way, and the compiler enforces it: a library cannot
depend on a service, and a service cannot depend on the runtime.

```
apps        qip-api  qip-fastbrain  qip-deepbrain  qip-edge-node
            qip-cli (operator tool)   qip-web (library, linked by qip-api)
              │
runtime     qip-kernel  ─── the only place that knows how it fits together
              │
agents      qip-investment-agents
              │
services    ingestion → normalisation → entity-resolution → world-model
            → opportunity-engine → reasoning-engine → simulation-engine
            → optimization-engine → portfolio-engine → risk-engine
            → execution-engine → learning-engine
              │
quant       qip-quant
              │
libs        qip-core  qip-numerics  qip-events  qip-financial  qip-market
            qip-portfolio  qip-risk  qip-ai  qip-agents  qip-quantum
            qip-storage  qip-observability
```

`qip-core` is the substrate: exact decimals, deterministic time, seeded
randomness, typed identifiers, lineage. Nothing above it may read an ambient
clock or an unseeded random number, which is what makes a replay reproduce.

## The eight stages

| Stage | Crate | What it produces |
|---|---|---|
| SENSE | `qip-market-ingestion`, `qip-normalization` | observations with provenance |
| UNDERSTAND | `qip-entity-resolution`, `qip-world-model` | a bitemporal model of the world |
| DISCOVER | `qip-opportunity-engine` | ranked opportunities |
| REASON | `qip-investment-agents`, `qip-reasoning-engine` | reviewed hypotheses |
| SIMULATE | `qip-simulation-engine` | outcome distributions and stress results |
| DECIDE | `qip-optimization-engine`, `qip-portfolio-engine` | a sized proposal |
| ACT | `qip-risk-engine`, `qip-execution-engine` | orders and fills |
| LEARN | `qip-learning-engine` | attribution, calibration, lessons |

## Five structural guarantees

These are properties the type system or the arithmetic enforces, not
conventions anyone has to remember.

### Look-ahead is unrepresentable

A backtest strategy reads market data only through a `PointInTimeView`, which
borrows from the clock and filters every read against the current instant.
There is no method that returns a bar with a close time after the view's
instant. Bars are keyed on *close* time, so a daily bar stamped with today's
date does not exist until the session ends.

The same guarantee appears three more times: bitemporal facts in the world
model, point-in-time features in the feature store, and evidence filtered to a
hypothesis's as-of time before anything is computed.

### A language model cannot produce a number a decision depends on

`FieldKind` has no numeric variant, so a schema cannot ask a model for one.
`NumericGuard::enforce` rejects any numeric leaf in a structured completion.
And an agent's `NumericFact` carries a provenance that is either *observed* or
*computed* — there is no third variant.

### Confidence is arithmetic

`Hypothesis` has no constructor that accepts a confidence. It is computed from
the evidence, and `validate` rejects a hypothesis whose stated confidence has
drifted from what its evidence implies. The only way to a higher number is
better evidence.

### An agent cannot reach a facility it was not granted

Every facility on the shared desk is wrapped in a `Gated<T>` whose inner value
is private and whose accessor takes the run context. Reaching a facility
therefore passes the capability check, charges the budget and writes an audit
entry. An agent that forgets to check its own permissions is still contained.

### Attribution is exact

`Attribution::residual` must be zero and `validate` refuses an attribution
where it is not. There is no "other" bucket, because unexplained P&L is exactly
where whatever nobody understood is hiding.

## Determinism

Every derived random stream comes from one seed. There is no ambient clock and
no unseeded RNG anywhere above `qip-core`. A cycle run twice with the same
inputs produces the same report, which is what makes the audit trail worth
having.

## Quantum

`qip-quantum` has a statevector simulator, QAOA on top of it, and a provider
port. `qip-optimization-engine`'s compute router enforces the rule that
matters: **no quantum result is used without a classical baseline solved on the
same problem, and a tie goes to the classical solver.**

Both paths express the same problem, every candidate is scored against the real
constraints including the ones its own relaxation dropped, and a quantum answer
must beat the baseline by a stated margin. Running QAOA against the simulator
validates the formulation and proves nothing whatever about advantage —
simulating the circuit costs more than solving the problem it encodes.

## Reading order, and where the rest lives

This document is the shape of the system. Three companions carry the detail:

1. `docs/adr/` — twenty-one numbered decisions. The reasoning is the point, and
   several of them are settled: reopening one takes a new ADR, not an argument.
2. `docs/architecture/canonical-platform.md` — the target architecture as 104
   addressable component ids.
3. `docs/architecture/diagram-reconciliation.md` — each id scored against the
   tree, with implementation path, runtime evidence, named test and gap. A row
   counts as complete only where both an implementation and a *named passing
   test* exist, and any component no deployable binary composes is capped at
   "implemented but unverified" however good its own tests are.
4. `docs/architecture/algorik-blueprint-traceability.md` — the same tree scored
   against a *different* reference, the Algorik Master Blueprint v10.1-4. It is
   kept separate from 2 and 3 on purpose: the two references disagree, and a
   row that is aligned against one can be missing against the other. Merging
   them would lose exactly the findings worth having.

The rules an agent must follow when changing this architecture are in
`.claude/rules/architecture/`; the per-area constraints are in
`.claude/rules/domains/`. `docs/claude/SIX_LEVEL_SYSTEM.md` maps how those fit
together.
