# 0008 — Edge cells decide alone, on capital granted in advance

**Status:** accepted

## Decision

The platform runs as **source-adjacent edge cells** sitting next to the venues
they trade, plus one **central plane** that researches, approves and allocates.

The hot execution path lives entirely in a cell: feed handlers, sequencing,
order books, the incremental feature DAG, compiled strategies, the arbitrage
graph, local pre-trade risk and the venue gateway. **No order waits on the
central plane.** A cell that has lost contact with the centre keeps trading.

What makes that safe is that a cell never decides how much it may risk. It
receives a `CapitalEnvelope` — signed, bounded, venue-scoped and expiring —
and the worst it can do while disconnected is spend an amount somebody already
approved, for as long as the envelope has left to run.

Three consequences follow, and each is enforced rather than encouraged:

1. **A cell's authority is a data structure, not a policy.** Every bound in
   `CapitalEnvelope` is private with no setter. Widening means asking the
   central plane for a new grant.
2. **Every envelope expires.** An unreachable cell is bounded by time as well
   as by size, so the failure mode of a network partition is a cell that stops,
   not a cell that runs on forever.
3. **Nothing on the hot path can consult a language model.** The compiled
   strategy IR has no node that calls out, and `qip-strategy` does not depend on
   `qip-ai`. A model contributes by being distilled into fixed coefficients
   ahead of time, never by being asked at decision time.

The central plane keeps everything that benefits from breadth and can afford to
be slow: point-in-time truth, research, training, backtesting, approval gates,
aggregate exposure across cells, and capital allocation.

## Why

Two forces pull in opposite directions and neither yields.

The hot path has a latency budget measured against other participants, and
every network hop toward a central service is a hop somebody else does not
take. Consolidating the decision loop centrally would be simpler in every
respect except the one that decides whether the strategy makes money.

Meanwhile, risk is not local. Three cells can each stay inside their own limits
and between them accumulate a concentrated position no one authorised. That is
a failure a single-process platform cannot have and a distributed one has by
default, so the central plane must see aggregate exposure even though it is not
in the decision path.

Granting capital in advance is what lets both be true. The centre reasons about
the whole book on its own schedule; the cell acts within a bound it was handed
earlier. Neither waits for the other.

## What it costs

**Consistency.** Aggregate exposure at the centre is stale by the round trip
from the cells. A cell can breach a global concentration limit for as long as it
takes the centre to notice and recall the envelope. The mitigation is that
envelopes are sized with that window in mind — the bound is what a cell may do
*before* anyone can intervene, not what the platform wants it to do.

**Duplication.** Feature computation, risk checks and book state exist in the
cells and again at the centre for research. Keeping the two implementations in
agreement is real work, and the shadow gate exists specifically to catch them
diverging: a strategy whose live decisions disagree with its backtest has
invalidated everything upstream of it.

**Operational surface.** Seven cells is seven deployments, seven sets of
credentials, seven places a clock can drift. Considerably harder to run than one
process.

**Testing.** The interesting failures are partitions, stale envelopes and
partial connectivity — states that only exist because of this decision, and that
have to be tested deliberately rather than encountered.

## What would make this wrong

If the strategies that survive the approval gates turn out not to be
latency-sensitive — if the edge lives over hours rather than microseconds —
then the entire cell architecture is cost without benefit, and the honest
response is to collapse it into the central plane rather than keep it for
elegance. The approval gates produce exactly the evidence needed to tell:
holding-period distribution and sensitivity of realised edge to execution delay.

The second reversal condition is a governance one. If aggregate exposure across
cells cannot be kept accurate enough to be worth having, then granting capital
in advance is granting it blind, and capital should return to being allocated
per order by a central risk service — accepting the latency, because an
unmeasurable risk is worse than a slow one.
