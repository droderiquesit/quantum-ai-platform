# 0009 — A tiered dependency policy, so the core stays at two

**Status:** accepted

## Decision

ADR 0002 permits exactly two third-party crates. That policy now applies to a
named **decision core** rather than to the whole workspace, and an **I/O edge**
is permitted a vetted allowlist.

The decision core is every crate that decides, prices, sizes, risks or places a
trade:

```
qip-core          qip-contracts     qip-numerics      qip-financial
qip-market        qip-portfolio     qip-risk          qip-strategy
qip-arbitrage     qip-capital       qip-risk-engine   qip-execution-engine
qip-orderbook     qip-feature-dag   qip-compliance
```

These keep `serde` and `serde_json` and nothing else. The boundary is enforced
by `the_decision_core_named_by_adr_0009_is_the_set_actually_held_to_two` in
`crates/tests/qip-acceptance/tests/architecture.rs`, which reads the list
**out of the fenced block above** rather than keeping a copy. A crate added to
the core in this document is a crate that test starts checking, and a crate
quietly dropped from it to let a dependency through is a diff a reviewer sees.

Today a second test, `no_crate_declares_a_third_party_dependency_beyond_the_two_permitted`,
holds *every* crate to the same two, because no edge crate has yet taken
anything. That is stricter than this ADR and it should stay until the first
client lands. The tier check exists now rather than then for a specific reason:
whoever adds Pub/Sub will relax the strict check, and on the day they do,
something has to still be holding the core.

Everything else — transport, cloud clients, serialisation formats, model
serving — may take dependencies from an allowlist, in crates that hold no
decision logic.

## Why

The target architecture mandates Google Pub/Sub, Vertex AI, BigQuery, Spanner
and IBM Quantum. A single-tier two-dependency policy cannot accommodate any of
them, and the ways of appearing to are all worse than the problem:

* **Hand-rolling the clients.** A gRPC implementation, a TLS stack and a Google
  auth flow written in-tree, guarding real money, reviewed by nobody who writes
  TLS for a living. This is the worst option and it looks like the most
  principled one.
* **Abandoning the policy.** Every crate free to take anything, and the
  property that made the platform auditable — that the code moving money has a
  supply chain small enough to read — gone in exchange for a Pub/Sub client.
* **Refusing the managed services.** Coherent, but it declines the architecture
  rather than building it.

Tiering keeps the property where it earns its cost. The argument for two
dependencies was never aesthetic: it was that a transitive dependency can
change what the platform computes, and nobody would notice. That risk is
concentrated almost entirely in the code that computes prices, sizes and
risks — which is precisely the set kept at two. A Pub/Sub client cannot change
what `NetEdge` deducts.

It also matches how the boundary is already tested. `architecture.rs` asserts
that no edge crate can reach a language model and that only the cell holds an
order manager. Adding "no core crate reaches a non-permitted dependency" is the
same shape of test over the same parsed manifests.

## What it costs

**The single sentence is gone.** "Two dependencies" was true, memorable and
checkable by anyone in five seconds. "Two in the core, an allowlist at the
edge" needs a list, and a list needs maintaining. The test is what stops the
list drifting from the code, but somebody still has to review additions.

**A new argument at every boundary.** Whether a crate belongs in the core is
now a decision with consequences, and the incentive runs one way: a crate that
wants a dependency has an interest in not being core. The list is therefore
explicit and in this document rather than inferred from a directory name.

**Offline builds become partial.** `cargo build` for the whole workspace will
need a registry. The core still builds offline, which is the half that matters
for reproducing a decision, but the claim has to be stated precisely from now
on rather than made flatly.

**Supply-chain surface grows.** A Google client pulls a large transitive tree.
`scripts/check-dependencies.sh` must move from an exact permitted list to a
per-tier check, and the edge tier's tree will be too large to read line by line.
That is a real reduction in auditability and it is the price of the managed
services, not an accident of this decision.

## What would make this wrong

If the managed services turn out not to be needed — if Pub/Sub is replaced by
the existing durable log, if training moves in-tree, if the quantum work stays
on the simulator — then the edge tier holds nothing, the tiering buys nothing,
and the honest response is to delete this ADR and return to ADR 0002 unmodified.

The second reversal condition is a breach. If a dependency in the edge tier is
ever found to have influenced a number the core computed — through a shared
type, a serialisation quirk, a float that crossed the boundary — then the
separation is not real, and the answer is to widen the core rather than to
patch the instance.
