# Vision

## The problem

Investment research systems are usually opaque about their own reasoning. They
produce a number and, at best, a feature-importance chart. When the number is
wrong nobody can say which step failed: the data, the model, the sizing, or the
execution. Post-hoc explanation is not the same as a decision that was
recorded as it was made.

## The intent

A platform where every decision carries its own evidence — the observation
that triggered it, the mechanism proposed, the panel that argued it, the rung
of intelligence it was worth spending on, the limits it was checked against,
and the fill it produced — and where all of it replays exactly from a
hash-chained log.

Paper trading is not a limitation of this vision; it is the setting in which
the claim can be tested honestly. A system that cannot explain a simulated loss
has not earned real capital.

## Users

The research and risk desk operating it. Not external customers, not retail.

## Outcomes

1. **Attributable.** Any P&L decomposes exactly into the decisions that caused
   it (ADR 0007).
2. **Reproducible.** The same log replays to the same decisions, including the
   reasoning that priced them.
3. **Affordable.** The platform refuses to spend more reaching an answer than
   the answer is worth, and records the refusal.
4. **Survivable.** A region that loses the centre keeps deciding inside its
   envelope rather than stopping (ADR 0008).
5. **Honest about quantum.** Quantum optimisation is used where it measurably
   beats a classical baseline computed in the same run, and nowhere else.

## Explicit non-goals

- Live order submission.
- A trading venue or brokerage.
- Retail distribution or a public API.
- A general-purpose ML platform.
- Benchmark outperformance as a software goal.

## How progress is measured

Not by feature count. By how much of `docs/architecture/canonical-platform.md`
is *Complete and verified* in `docs/architecture/diagram-reconciliation.md` —
which requires both an implementation path and a named passing test, and which
caps any component no deployable binary composes at *Implemented but
unverified*.
