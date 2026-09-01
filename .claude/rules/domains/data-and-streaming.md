# Domain: data and event streaming

**Scope** — `backend/crates/services/qip-market-ingestion/**`, `qip-normalization/**`,
`qip-data-finder/**`, `qip-streaming/**`, `backend/crates/libs/qip-events/**`,
`backend/crates/edge/qip-sequencing/**`

## Approved

- Bitemporal records throughout: the instant a thing was true, and the instant
  it became knowable. A feature store that cannot answer "what did we know
  then" produces backtests nobody should trust.
- Idempotent envelopes; dedup keyed on a stable fingerprint.
- Bounded retention and bounded working sets, always.
- Licensing posture evaluated **before** a source is used, in
  `qip-data-finder`. A research-only licence never reaches the catalogue.

## Prohibited

- Point-in-time leakage. A feature readable before its knowable instant is a
  defect however good the backtest looks.
- Unbounded buffers or unbounded history.
- A vendor call without a timeout, or one that assumes TLS — the HTTP client
  speaks plaintext HTTP/1.1 by design and needs the egress proxy in front.
  That proxy exists only as a Kubernetes manifest whose `Deployment` is
  committed commented out (`infrastructure/helm/qip/templates/egress.yaml`
  and its identical copy under `kubernetes/base/`), so today on GKE, and on
  any other runtime, a deployed process has no outbound HTTPS path; a
  connector proven in a session behind a local bridge is not proven in a
  deployment.
- Using a source whose licensing posture has not been evaluated.

## Required evidence

The absorption test for the arm you touched, plus `resilience.rs` where
ordering or duplication is involved.
