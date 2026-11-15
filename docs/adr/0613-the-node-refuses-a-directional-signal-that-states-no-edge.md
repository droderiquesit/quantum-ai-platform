# ADR 0613: The node refuses a directional signal that states no edge

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

## Consequences

- Nothing was compiled or run (owner instruction of 2026-11-15).
- Until a strategy declares an edge (`Rule::with_expected_edge`, for example
  over a promoted model per ADR 0195), a node entering the market enters
  nothing. That is honest: the cell does not trade what it cannot judge.
- Existing node tests that drive entries through `cell_configuration` would
  now see refusals. The one known pass test builds its own configuration; the
  pipeline is the first to run the rest.
- Not addressed and still open on the row: no committed strategy declares a
  model edge; the forecast horizon is seconds-scale, not microsecond-to-minute;
  continuation past the capital envelope's expiry; ADR 0195 decision 5 still
  declines the warm-backup clause.

## What would make this wrong

If a deployment needs entries before any strategy declares an edge, reverse
this with an ADR naming the risk accepted, not by dropping the call.
