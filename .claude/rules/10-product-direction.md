# Product direction

## What the platform is for

A research desk needs to know not just what a system decided, but why, on what
evidence, at what cost, and whether the reasoning held up. Everything here is
shaped by that: the hash-chained event log, exact attribution (ADR 0007),
confidence as arithmetic rather than vibes (ADR 0005), and a cost router that
records the rationale for the rung it chose.

## Standing product decisions

These are settled. Reopening one requires an ADR, not a code change.

- **Paper trading by default and in fact** (ADR 0003). Not a phase.
- **Rust everywhere** (ADR 0001, ADR 0011). Latency-sensitive and core
  services are Rust; the browser layer is not, and that is the only exception.
- **Two dependencies** (ADR 0002, ADR 0009). The cost of a dependency is not
  its download size; it is the supply chain, the audit surface, and the
  semantics somebody else may change.
- **Cells decide alone** (ADR 0008). Seven regional edge cells; a cell that
  cannot reach the centre keeps working within its envelope rather than
  stopping. Regional low-latency execution is separate from centralised
  training and portfolio intelligence, and the split is load-bearing.
- **A classical baseline always** (ADR 0006). Quantum methods run only where a
  measured benefit is demonstrable against that baseline. "We used a quantum
  computer" is not a result.
- **Bounded retention.** Streaming with explicit bounds, never uncontrolled
  duplication. Working sets are capped and the event log is the record.

## Mandatory architecture concerns

Not features to be prioritised — preconditions:

- **Capital protection.** Limits are checked before an order exists, and a
  limit that cannot fire is a defect, not a spare part.
- **Risk controls.** Pre-trade deterministic checks never route to a model;
  `qip-cost-router`'s `Determinism::Required` arm makes that structural.
- **Observability.** A process nothing can see is a process nobody can operate.
- **Auditability.** Every decision reproducible from the log alone.
- **Regulatory compliance.** Licensing posture is checked before a data source
  is used, not after.

## Judging a proposal

Ask, in order:

1. Does it keep the paper-trading boundary structurally intact?
2. Can its result be reproduced from the event log?
3. Does it add a dependency? Then it needs an ADR first.
4. Does it make a guarantee weaker in exchange for speed? Then say so out loud
   in the diff, or do not do it.
5. Is the evidence it produces something a person can check, or only something
   the system asserts about itself?
