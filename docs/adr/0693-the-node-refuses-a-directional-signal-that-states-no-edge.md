# ADR 0693: The node refuses a directional signal that states no edge

- **Status**: Accepted, 2026-11-15, by lane 46 of wave b52 under the wave's
  delegation. Restrictive only.
- **Date**: 2026-11-15
- **Keeps**:
  - ADR 0155 section 9: an edge nobody computed gates nothing.
  - ADR 0195: the reflex forecast is a calibrated distillate a rule names as
    its expected edge.
  - ADR 0003: paper trading. The Terraform refusal, `AutonomyLevel::deployable`
    and the `Cell` constructor are untouched.

## Context

R12-WORLD-9023's skeptic found the net-edge gate does nothing in a deployed
cell: no committed strategy states an expected edge, and an edgeless signal
passes the gate and is only counted (`edgeless_signals`). The margin has since
been armed at the node (`NET_EDGE_MARGIN_BP`, 25 bp, decision 38). The second
switch, `CellConfig::with_edgeless_signals_refused`, was left off by default
because turning it on stops every strategy whose rules declare no edge.

## Decision

`qip-edge-node`'s `cell_configuration` calls `.with_edgeless_signals_refused()`.
A directional signal that increases risk and states no edge is refused under
`GATE_NET_EDGE` with a message naming the strategy and telling the author to
declare an edge. Exits and hedges are never judged by this gate.

This is the restrictive choice: it can only refuse. No figure is invented; the
25 bp margin stays the one person-set value.

## What it costs

- Until a strategy declares an edge (`Rule::with_expected_edge`, for example
  over a promoted model per ADR 0195), a node entering the market enters
  nothing. No committed strategy states an edge today, so every
  risk-increasing directional signal from a node is refused under
  `GATE_NET_EDGE`. The cell does not trade what it cannot judge, and an
  operator or strategy author must declare an edge on a rule before that
  rule's entries pass this gate. Exits and hedges are not judged by it.
- Existing node tests that drive entries through `cell_configuration` would
  now see refusals. The one known pass test builds its own configuration; the
  rest are unchecked, because this change was written without a build. How
  many break is unmeasured.
- Not addressed and still open on the row: no committed strategy declares a
  model edge; the forecast horizon is seconds-scale, not microsecond-to-minute;
  continuation past the capital envelope's expiry; ADR 0195 decision 5 still
  declines the warm-backup clause.

## What would make this wrong

If a deployment needs entries before any strategy declares an edge, the
refusal is blocking work it should allow. Reverse it then with an ADR naming
the risk accepted, not by dropping the call. It is also wrong if an entry
signal whose rule declares an edge is refused under `GATE_NET_EDGE` as stating
none, or if an exit or a hedge is ever refused under `GATE_NET_EDGE`. Either
observation means the gate refuses what this record says it does not.
