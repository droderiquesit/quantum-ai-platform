# ADR 0391: A routing choice is scored by a judgement the router mints for the rung that ran, carried from the decision and resolved in LEARN against the thesis it bought; a declined or deterministic routing mints none, and hedges stay unscored

- **Status**: **Accepted 2026-11-11 under the owner's 2026-11-11 delegation
  to the orchestrator**, batch l2-d1-01. The delegation's limits hold:
  - it sets no limit, threshold, rate or cost;
  - it touches no paper-trading layer, and `Determinism::Required` still
    returns a type that cannot name a model rung and now also cannot mint a
    judgement;
  - it adds no crate edge: `qip-kernel` already depends on `qip-cost-router`;
  - it puts no behaviour in `qip-contracts`.
- **Date**: 2026-11-11
- **Amends**: the `Judgement` contract in
  `qip-cost-router/src/reputation.rs`, one clause: "`model` must be a
  `qip_ai::ModelCard::reference`". It may now also be a rung reference,
  `<rung>@<build>`, minted only by `JudgedRouting::judgement`. No accepted
  ADR is amended.
- **Rows**: R12-MODEL-10123 (the compute-routing class only).

## Context

The requirement asks that resolved outcomes score compute-routing choices.
`qip-cost-router` has a `ReputationBook` that keeps a record per model per
decision-time conditions, and it is written only through a `Judgement`
minted at the decision. Nothing outside the crate's own tests called it. The
register said "cost-router keeps reputation"; that described a type.

The kernel's one routed decision that can be wrong is REASON's: the router
places the question of whether to act on an opportunity on a rung, and the
platform convenes the panel or declines. The answer that decision buys is a
thesis, and LEARN grades theses. The grade never reached the routing.

`Judgement` named a model card. The kernel's routing chooses a rung, not a
card, so no judgement could be minted for it, and `Judgement::minted` was
private to the reputation module.

## Decision

1. **The router mints the judgement, for the rung that ran.**
   `JudgedRouting::judgement(ran, build)` returns a `Judgement` naming
   `<rung>@<build>` under the routing's own decision-time conditions. `ran`
   is a `ModelTier`, so deterministic code cannot be named, and the method
   is on `JudgedRouting`, so a determinism-required routing has no value to
   call it on. The rung is the one the platform ran on, not the one the
   router placed the decision on: bill what ran, score what ran.
2. **The judgement is minted at the decision and carried.**
   `ReasonRouting` carries it (`judgement`, absent on a record written
   before the field existed and on a routing that convened nothing). When
   REASON forms a hypothesis from the convened panel, the kernel holds the
   judgement against that hypothesis id.
3. **LEARN resolves it.** When `calibrate_resolved` grades the thesis on
   that hypothesis, an informative verdict is recorded in the platform's
   `ReputationBook` through `ReputationBook::observe`, correct where
   `Verdict::counts_as_correct`. An inconclusive verdict records nothing
   and releases the judgement: nothing happened either way.
4. **Each resolution is journaled** as `RoutingScored`, with the judgement,
   the hypothesis, the verdict and the instant, so the book can be rebuilt
   from the log and an auditor reads the score rather than a claim of one.
5. **The score decides nothing.** No path reads the book back into a
   routing, a budget or a size. Letting it narrow a routing is a later ADR,
   once there is a record to argue from.
6. **Held judgements are bounded.** One per open prediction: a judgement
   whose hypothesis has no open prediction at LEARN is dropped.
7. **Hedges stay unscored** (owner decision 7): no composition root declares
   a hedge policy, so there is no hedge to resolve. The capital allocator's
   and the capital society's per-cycle plans are not addressed here.

## Consequences

- The routing class of the requirement has a default-path score: every
  convened REASON decision whose thesis is graded moves one cell of the
  book, keyed on the conditions the routing was made under.
- The book has one populated rung today, `multi_agent_reasoning`, because
  the panel is the only reasoner the platform implements. The record of
  where the router placed the decision stays on `ReasonRouting`.

## What it costs

- One map entry per open prediction and one journal record per graded
  thesis.
- A judgement held in memory is lost on a restart: the routing in flight is
  not scored. The record on the log names the judgement, so a rebuild is
  possible; it is not built here.

## What would make this wrong

- Reading the book into `Router::select` without an ADR. A rung with one
  populated cell would be ranked against rungs with none.
- Minting the judgement at resolution from the market then in front of the
  kernel. The type prevents it: only a `JudgedRouting` mints, and it carries
  the conditions it was made under.
- Keying the rung without the build. A new build's panel would inherit the
  record of the one it replaced.
