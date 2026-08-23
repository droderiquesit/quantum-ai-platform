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
