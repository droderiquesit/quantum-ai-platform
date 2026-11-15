# ADR 0633: The edge node hosts the microsecond issuer for the horizon the operator declares, and feeds it only the fill outcomes the pass already reports

- **Status**: **Accepted 2026-11-15 under the owner's 2026-10-31 delegation**,
  wave b52 lane 16. It builds the slice ADR 0506 left open ("the issuer is
  not yet fed from `Cell::work` or hosted by `qip-edge-node`"). It adds no
  dependency, introduces no figure and touches no paper-trading layer.
- **Date**: 2026-11-15
- **Rows**: R12-WORLD-3084, R12-WORLD-7197 (both stay PARTIAL).
- **Keeps**: ADR 0506 decision 5, ADR 0195 decision 1.

## Context

`ReflexIssuer` stated and scored the microsecond row and had no caller
outside its tests. The lattice's fifth row was therefore never issued on any
path a binary runs, and "promoted only to the local reflex" described
forecasts that did not exist.

## Decision

1. `qip-edge-node::reflex_host::ReflexHost` owns the issuer and is fed by
   `DecisionLoop::turn` from the `PassOutcome` the pass returns. It reads
   the orders the venue accepted and the fills the cell confirmed, and
   nothing else. It is read-only with respect to the pass: nothing the cell
   decides reads what it issues (ADR 0506 decision 5).
2. **Only `fill_probability` is issued.** A forecast is issued when the
   order is placed, before its outcome exists, and scored when the horizon
   has elapsed against whether a fill stamped inside the horizon was
   reported. Queue movement, adverse selection and executable edge need
   observations the pass does not carry and are not issued.
3. **The horizon is the operator's.** `QIP_REFLEX_HORIZON_US` declares it;
   unset, the node hosts no issuer. A value outside 1..=999,999 µs, or set
   on a node with no feed to run passes, stops the process at start. No
   default is chosen here, because a figure nobody gave is not chosen by a
   lane.
4. Bounds: 4,096 orders await their horizon, 256 issued forecasts are kept;
   an order past the first bound is counted and not tracked rather than
   evicting a pending one whose score would never arrive.
5. A refusal from the issuer is reported and does not stop the loop that the
   wires and the journal flush depend on.

## Not decided

Deploying the variable: the execution node's startup template and the
per-environment tfvars do not set it, so no deployed node issues until an
operator declares a horizon. The centre's re-scoring of reflex forecasts
(ADR 0133) and the `qip_lattice_brier{horizon="microseconds_ms"}` series.
A series for the issuer on the node's scrape.

## Evidence

`qip-edge-node/tests/reflex_host.rs`, each test mutation-verified (see the
commit).
