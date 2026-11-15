# ADR 0608: A synthetic world declares typed drivers, and only feed-backed kinds reach a market-facing reader

- **Status**: **Proposed**, 2026-11-14, by lane 04 of wave b52. The owner ruled
  on 2026-10-31 (ADR_THEN_IMPLEMENT) that policy, weather, default and supply
  drivers get a synthetic-only producer; this record is the ADR that ruling
  asked for. Not accepted until a person accepts it.
- **Date**: 2026-11-14
- **Amends**: [ADR 0151](0151-what-enters-understand-each-tick-plane-and-federation-input-has-a-named-producer-and-consumer-or-is-not-built-and-an-outside-prior-may-only-narrow-reason.md)
  decision 5, which said only `OwnAction` and `Price` have a producer and the
  rest "need data this repository does not hold".
- **Keeps**: ADR 0003 (paper trading), ADR 0006, ADR 0109 (no dependency).

## Context

`FutureTree::grow` varies shock size, propagation and sign per untyped
`WorldDriver`. A world that varies policy, weather, supply or default
assumptions could not say so, and nothing separated a branch a feed could ever
have produced from a pure hypothesis. R12-TICK-3097 asks for generators that
vary those dimensions.

## Decision

1. `qip_twin::worlds::DriverKind` is `Policy`, `Weather`, `Default`, `Supply`,
   `OwnAction`, `Price`. `has_feed_producer` is true for `OwnAction` and
   `Price` only.
2. A driver of any kind may be declared to `SyntheticWorlds::grow`. Every
   branch it grows is `Provenance::Synthetic`, as every branch already is.
3. `SyntheticWorlds::feed_backed` is the only accessor meant for a reader that
   reasons about the real market. It never returns a branch of a kind with no
   feed producer. No policy, weather, default or supply driver reaches
   UNDERSTAND or REASON through it.
4. A kinded driver that reaches no instrument the twin was fitted on is
   refused, because `FutureTree::grow` skips it silently and a declared kind
   that grew nothing reads as explored.

## What it costs

A caller that declares drivers now owes each one a kind and a unique id without
`:`, and a driver naming no instrument the twin was fitted on is refused rather
than grown empty, so a set of drivers that `FutureTree::grow` accepts with
some silently inert is refused whole by `SyntheticWorlds::grow`. Policy,
weather, default and supply branches are hypotheses that decision 3 keeps out
of `SyntheticWorlds::feed_backed`, so those four kinds do not inform a
market-facing reader through that accessor for as long as `has_feed_producer`
is false for them. `WorldDriver` and its
construction sites are unchanged; the kind lives in a wrapper. Nothing yet
calls `SyntheticWorlds` from the tick desk or the CLI, so no deployed process
grows a typed tree and the decision is unexercised outside tests. The kind is
held beside the tree, not in the `Branch` record, so a serialised branch does
not carry it. The map from driver id to kind and the per-branch lookups add
compute and memory that this record does not measure.

## What would make this wrong

A consumer that reads `FutureTree::branches` directly bypasses decision 3;
`SyntheticWorlds::tree` hands out the tree. That is a review rule today, not a
type.
It would also be wrong if a policy, weather, default or supply branch is
found in an input to UNDERSTAND or REASON, or if one of those kinds gains a
real feed producer while `has_feed_producer` still returns false for it, which
would hide a measured signal. Revisit it if the wrapper is bypassed in
practice; holding the kind in the `Branch` record is the alternative this
record did not take.
