# 0006 — A classical baseline for every quantum result

**Status:** accepted

## Decision

`ComputeRouter::solve` always runs the classical solver. It runs the quantum
path only where the problem's structure could justify it, and returns the
quantum answer only when that answer is feasible *and* better than the baseline
by more than a stated margin.

A tie goes to the classical solver.

## Why

Quantum optimisation attracts claims that do not survive scrutiny. The three
ways a claim goes wrong are: comparing against a weak classical baseline,
comparing on a slightly different problem, and treating noise as an advantage.

The router closes all three. The baseline is computed on every call, so there
is always something to measure against. Both paths express the same
`PortfolioProblem`. And every candidate is scored against the *real* problem,
including the constraints its own relaxation dropped — so a QUBO answer that
ignored the cardinality limit comes back infeasible rather than winning on an
objective it was not entitled to.

The margin exists because a tie is not evidence. Preferring the quantum answer
at parity would let sampling noise decide which solver the platform reports as
better.

## What it costs

The classical solve happens even when the quantum path wins, which is wasted
work in exactly the case the quantum path was supposed to help with. That is
the price of being able to substantiate the claim.

## What would make this wrong

If quantum hardware reached a point where the classical baseline was genuinely
intractable for the problem sizes in question, computing it would stop being
possible. At that point the honest thing is a *bounded* classical baseline — a
best-effort heuristic with its own quality bound — not no baseline.

Running QAOA against the in-tree simulator proves nothing about advantage, and
`RoutingDecision::quantum_note` says so on every such result: simulating the
circuit costs more than solving the problem it encodes.
