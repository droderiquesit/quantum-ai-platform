# 0012 — Where a library earns its place, and where it does not

**Status:** accepted
**Amends:** ADR 0002 (two dependencies) and ADR 0011, which together produced a
platform that hand-writes everything.

## The situation this responds to

The two-dependency policy has produced, in-tree: an HTTP/1.1 server and client,
an embedded storage engine with a write-ahead log, SHA-256, a seeded PRNG, a
hidden Markov model, Cholesky and symmetric eigendecomposition, a quadratic
programme solver, the special functions behind three distributions, a statevector
quantum simulator, and a venue matching engine. Eleven third-party packages, all
`serde`.

That is a real asset. It is also a lot of specialist code to own, and the
platform is now asked to be enterprise grade. The question is no longer
"can we write it" — demonstrably yes — but **where writing it is the better
engineering decision.**

## Decision

A dependency is admitted only where **all three** hold:

1. **Getting it wrong is silent.** The failure does not announce itself with a
   crash or a failing test; it shows up as a wrong number, a leaked secret, or
   a corruption discovered months later.
2. **The problem is adversarial or specialist.** Correctness depends on
   knowledge that is not general engineering — cryptanalysis, floating-point
   edge cases, protocol ambiguity that attackers probe deliberately.
3. **A mature, widely-audited implementation exists**, and its maintenance is
   somebody's actual job rather than a side effect.

Where all three hold, hand-writing is not principled; it is the riskier
option wearing the costume of the safer one.

### Admitted now

**TLS. `rustls`.** All three conditions, maximally. A hand-rolled TLS stack
guarding real money is indefensible, and this platform has said so twice
already — in ADR 0009 and in `qip-transport`'s own security doc, which states
plainly that the mesh is plaintext and that production must supply mTLS. That
is a gap the platform has been carrying with its eyes open, and it is the one
dependency that closes it.

Scope: the I/O edge only. `qip-transport` and any future venue or IBM Quantum
client. **Not** the decision core, which ADR 0009 names explicitly and which
keeps its two dependencies.

### Refused, and why

**An async runtime (`tokio`).** Not because async is wrong — because the
premise is wrong here. Mesh traffic is a few small documents per second per
cell; the crate's own doc already makes this argument. Converting 2,198
synchronous tests to async would be the largest single change in the
repository's history, in exchange for concurrency the measured workload does
not need. Revisit when a *measurement*, not an intuition, shows threads are
the constraint.

**A serialisation framework beyond serde, an ORM, a logging framework, a web
framework.** None satisfies condition 1: they fail loudly. The in-tree
versions are smaller than the configuration surface of the alternatives.

**A numerical library (`nalgebra`, `ndarray`).** Condition 1 is arguable —
numerics do fail silently — but condition 3 is not met the way it looks. What
this platform needs is a Cholesky, an eigendecomposition and a QP solver, all
of which exist here, are tested against known-good values, and are read by
people who understand the finance they serve. Importing a general tensor
library to get three routines trades a small audited surface for a large
unaudited one.

**A database.** Refused *for operational state*, per ADR 0011, and the
embedded engine now exists. Still recommended *for analytics*, and ADR 0011
already names that as the condition most likely to reopen it. Nothing here
changes that.

## What it costs

**The single sentence is gone for good.** "Two dependencies" was checkable in
five seconds. "Two in the core, plus TLS at the edge, under a three-part test"
requires reading this document. The dependency-policy script and
`the_decision_core_named_by_adr_0009_is_the_set_actually_held_to_two` are what
keep the list from drifting, but a human still has to apply the three
conditions to the next request, and reasonable people will disagree.

**The audit story weakens at the edge.** `rustls` pulls a transitive tree
larger than everything currently in the lockfile combined. The claim "every
number this platform computes traces to code in this repository" survives only
because the decision core is unchanged — and stating it now requires that
qualifier every time.

**A precedent that will be argued from.** The next proposal will cite this ADR.
The three conditions are deliberately hard to satisfy for that reason, and
"we already added one" is not an argument.

## What would make this wrong

* **If TLS is terminated elsewhere** — a service mesh sidecar doing mTLS, which
  is the standard Kubernetes answer — then the platform never speaks TLS itself,
  condition 3 is satisfied by the sidecar rather than by a crate, and this ADR
  should be reduced to nothing.
* **If the three conditions stop being applied honestly** and the allowlist
  grows past a handful, the policy has failed and the answer is to return to
  ADR 0002 unmodified rather than to keep amending.
