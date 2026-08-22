# 0002 — Two third-party dependencies

**Status:** accepted

## Decision

The platform depends on `serde` and `serde_json`. Everything else is written
in-tree: the linear algebra, the statistics, the distributions, the optimisers,
the HTTP server, SHA-256 and HMAC, the random number generator, the statevector
simulator, and the hidden Markov model.

`scripts/check-dependencies.sh` enforces this against `Cargo.lock` in CI.

## Why

Three reasons, in order of how much they mattered.

**A supply chain small enough to audit.** A typical Rust service of this size
pulls in several hundred transitive crates. Nobody reads them. For a system
that moves money, "nobody has read the code that computes our risk numbers" is
not a defensible position, and the 11 packages in this lockfile can
actually be read.

**A build that works offline and reproducibly.** The platform builds with no
network access beyond the two dependencies. A numeric result computed today
will be computed identically in five years, because nothing underneath it can
change.

**No dependency can change what the platform computes.** A minor version bump
in a statistics crate that changes a quantile interpolation would change every
risk number in the system, silently. Writing the quantile means the change is a
diff.

## What it costs

A great deal of code. The in-tree implementations are perhaps fifteen thousand
lines that a dependency would have provided, and each of them is a place a bug
can live. The mitigation is that each is tested against known values — the
normal CDF against libm to one ULP, the QP solver against analytically known
optima, the hash functions against published vectors.

It also costs performance in places. The ADMM solver is not OSQP and the
statevector simulator is not a vectorised one.

## What would make this wrong

A requirement the in-tree code genuinely cannot meet: real TLS, for instance.
Writing a TLS implementation would be far more dangerous than depending on
`rustls`. If the platform needs to reach a real venue or a real quantum
backend, that is the point at which this decision gets revisited — and the
adapter interfaces are already written so the change is contained.
