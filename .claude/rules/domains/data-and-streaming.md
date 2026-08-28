# Domain: data and event streaming

**Scope** — `crates/services/qip-market-ingestion/**`, `qip-normalization/**`,
`qip-data-finder/**`, `qip-streaming/**`, `crates/libs/qip-events/**`,
`crates/edge/qip-sequencing/**`

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
- Using a source whose licensing posture has not been evaluated.

## Required evidence

The absorption test for the arm you touched, plus `resilience.rs` where
ordering or duplication is involved.
