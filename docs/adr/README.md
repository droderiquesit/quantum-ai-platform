# Architecture decisions

Each record states what was decided, what it costs, and what would make it
wrong. A decision with no stated cost has not been thought about, and one with
no stated reversal condition cannot be revisited honestly.

| # | Decision |
|---|---|
| [0001](0001-rust-everywhere.md) | Rust for everything, including the web interface |
| [0002](0002-two-dependencies.md) | Two third-party dependencies |
| [0003](0003-paper-trading-by-default.md) | Paper trading by default, with a deployment ceiling |
| [0004](0004-capability-gated-agents.md) | Agents reach facilities only through capability gates |
| [0005](0005-confidence-is-arithmetic.md) | Confidence is computed, never assigned |
| [0006](0006-classical-baseline-always.md) | A classical baseline for every quantum result |
| [0007](0007-exact-attribution.md) | Attribution must reconcile exactly |
| [0008](0008-edge-cells-decide-alone.md) | Edge cells decide alone, on capital granted in advance |
| [0009](0009-tiered-dependency-policy.md) | A tiered dependency policy, so the core stays at two |
| [0010](0010-what-gets-deployed.md) | Four of the six application crates are deployed, and the other two are not |
| [0011](0011-everything-in-rust-on-kubernetes.md) | Everything is Rust on Kubernetes; IBM Quantum is the only integration |
| [0012](0012-where-a-library-earns-its-place.md) | A dependency is admitted only where getting it wrong is silent, the problem is specialist, and a maintained implementation exists |
| [0013](0013-identity-verification-earns-a-dependency.md) | Verifying an identity token earns a dependency; issuing sessions does not |
| [0014](0014-one-design-system-four-surfaces.md) | One design system, four surfaces, no second router |
| [0015](0015-licensed-templates-are-the-visual-source-of-truth.md) | The licensed templates are the visual source of truth |
| [0016](0016-repository-layout.md) | One layout: backend, frontend, data, infrastructure — each top-level directory answers one question |
