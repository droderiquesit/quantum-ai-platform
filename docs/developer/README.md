# Development

## Conventions

**Comments explain why, not what.** `// increment the counter` above `count +=
1` is noise. A comment earns its place by explaining a decision, a trade-off, or
a subtlety that would otherwise be a bug next time somebody edits it.

**Tests assert properties, not outputs.** `assert_eq!(sharpe, 1.234)` breaks
when anything changes and tells you nothing when it passes. The tests that have
caught real bugs in this codebase assert things like "realised plus unrealised
equals the change in equity", "the escaped output contains no bare delimiter",
and "a stronger mechanism never lowers confidence".

**Numbers that matter have a stated reason.** A threshold with no comment is a
threshold nobody can argue with. Every constant that encodes a judgement carries
one — see `ReviewPolicy`, `MonitorPolicy`, `PromotionBar`.

**Nothing reads an ambient clock or an unseeded RNG.** Time comes from an
injected `Clock`, randomness from a stream derived from the platform seed. That
is what makes a replay reproduce.

## Adding a stage to the loop

1. Add a variant to `qip_kernel::cycle::Stage` and to `Stage::all`.
2. Write `stage_<name>` on `Platform`. It must return a `StageOutcome` with a
   non-empty `detail` even when nothing happened: "nothing happened" and
   "nothing was attempted" are different, and a report that could not
   distinguish them would be no use at three in the morning.
3. Add it to the `vec![]` in `run_cycle`, in order.
4. The existing test `one_observation_traverses_all_eight_stages` will fail
   until the stage runs, which is the intended feedback.

## Adding an agent

1. Write its manifest in `qip_investment_agents::manifests`. Start from
   `AgentManifest::research` and add capabilities deliberately.
2. Implement `Agent`. The `analyse` method gets an `AgentContext` and a brief;
   reach facilities through the desk's `Gated` wrappers.
3. Add it to `manifests::roster` and to `Organisation::standard`.
4. `Roster::validate` runs in the tests. If the new agent breaks a separation
   of duties, it will say which rule and why.

Every finding an agent returns must state a falsifier if it takes a direction,
and every number it reports must carry a provenance. Both are enforced by
`AgentFinding::validate`.

## Running the checks

```sh
cargo fmt --all
cargo clippy --workspace --all-targets    # warnings are denied in CI
cargo test --workspace
./scripts/check-dependencies.sh
./scripts/check-secrets.sh
```

## Adding a dependency

Don't, unless the in-tree alternative is genuinely dangerous to write — TLS is
the clear example. If you must, add it to `PERMITTED` in
`scripts/check-dependencies.sh` with a comment saying why. The point is that
the decision appears in a diff where a reviewer will see it. See
[ADR 0002](../adr/0002-two-dependencies.md).

## What the tests are for

| Suite | Defends |
|---|---|
| `qip-core`, `qip-numerics` | the arithmetic everything else rests on |
| `qip-agents` | that a prohibition actually prohibits |
| `qip-reasoning-engine` | that confidence follows evidence |
| `qip-simulation-engine` | that look-ahead is unreachable |
| `qip-risk-engine`, `qip-execution-engine` | that live trading needs what it claims |
| `qip-acceptance` | that the infrastructure and the docs match the code |
