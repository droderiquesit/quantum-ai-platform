# ADR 0662: A directional band is reserved against the cell's share, never added to it, and is usable only while the peer mesh is degraded

- **Status**: Accepted 2026-11-15, lane 31 of wave b52. It builds ADR 0154
  decision 8 as a type and adds no dependency, no figure and no order path.
- **Date**: 2026-11-15
- **Rows**: R12-RES-7095 (stays PARTIAL; see "What this leaves open").

## Context

R12-RES-7095 asks that when the peer reflex mesh is degraded, local trading
continues and distributed cycles are disabled or reduced to pre-positioned
directional bands. The disabled half is built: a multi-cell cycle needs the
counterparty's reservation, so with no reachable peer none fires. The reduced
half had only a name in ADR 0154 decision 8 and no type.

## Decision

1. `qip_edge::band::DirectionalBand` is `(cell, direction, amount, epoch)`,
   refused for an empty cell or an amount not above zero.
2. `BandBook` is opened over the cell's applied region share and reserves
   bands against it. A band that would take the reserved total above the share
   is refused, not trimmed, so `reserved() + free() == share()` holds and a
   band never widens what the centre granted.
3. A band at or below the epoch already held for the same side is refused; a
   newer one replaces that side.
4. `BandBook::usable(mesh_degraded)` returns no band while the mesh is healthy
   and every live band while it is degraded, so a band cannot stand in for a
   coordinated cycle.
5. The type is arithmetic. A band authorises no order; the pass still sizes,
   gates and routes anything it sends.

## What this leaves open

- No code sends a band to a cell: the centre does not issue them and no node
  holds a `BandBook`, so nothing on a deployed path reaches the type.
- No pass sizes against `usable()`; wiring it touches order sizing and needs
  its own review.
- Whether a band is exposure-bounded the way a grant is, and who signs it, is
  not decided here; a band that arrives over the wire must be signed under
  ADR 0110 before it is reserved.

## What it costs

One more type beside the region share and a second ledger to keep equal to it.
The cost of not building it is that "reduced to bands" stays a phrase.

## What would make this wrong

- If a caller adds a band's amount to the share instead of reserving it
  against it, the centre's grant is silently widened. The sum test is the
  guard.
- If `usable` is read while the mesh is healthy, a band shadows a coordinated
  cycle's own reservation.
- If bands are accepted unsigned over the wire, a peer could position
  authority the centre never granted.
