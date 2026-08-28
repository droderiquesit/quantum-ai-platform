# Current state, as measured

Established by running the gates and reading the tree on the commit that closed
the trading spine. Every number here came from a command whose output was read.

## Shape

| Fact | Value |
|---|---|
| Rust crates | 59, in 8 groups (`libs`, `services`, `apps`, `edge`, `agents`, `quant`, `runtime`, `tests`) |
| Tests | 3,078 passing, 0 failing (`cargo test --workspace --no-fail-fast`) |
| Clippy | 0 warnings, `--all-targets` |
| Third-party crates | 11 packages, all permitted (`serde`, `serde_json` and their trees) |
| Frontend | Next.js + TypeScript, 47 tracked files |
| Cloud | GCP: GKE, Secret Manager CSI, KMS, Binary Authorization, WIF |
| Pipelines | `ci.yml`, `deploy.yml`, `infra.yml` — all deriving identity from committed tfvars |
| Decision records | 12 ADRs |

## Against the canonical architecture

104 component ids scored in `docs/architecture/diagram-reconciliation.md`:

| Status | Count |
|---|---|
| Partially implemented | 66 |
| Complete and verified | 19 |
| Implemented but unverified | 13 |
| Missing | 6 |
| Implemented differently | 4 |
| Obsolete or duplicated | 2 |
| Stub or mock only | 1 |

"Complete and verified" requires both an implementation path and a **named
passing test**. Any component no deployable binary composes is capped at
"implemented but unverified" however good its own tests are — a crate nothing
composes is a crate the platform does not run.

## What the eight cycle stages actually do

| Stage | State |
|---|---|
| SENSE | Absorbs 11 record kinds. Works. Every deployment's data is synthetic or replayed — the live REST adapter is reached only from tests |
| UNDERSTAND | Entity resolution and the bitemporal world model. Works |
| DISCOVER | Detectors run; opportunities queue newest-highest-value first. Works |
| REASON | **Now routed.** Prices the decision, asks the cost router where it belongs, convenes the panel when it is affordable, records the rationale either way |
| SIMULATE | Resampling and counterfactuals. Works |
| DECIDE | Sizes approved theses into proposals with legs; records a `nothing_to_do` proposal on quiet cycles |
| ACT | **Now closed.** Two controls sign, legs become orders, orders go through the deterministic pre-trade path, released proposals are retired |
| LEARN | Attributes fills exactly. Now has fills of its own to attribute |

## The three breaks found and closed in the spine

Each was invisible until the one before it was fixed:

1. `stage_act` counted approved proposals and returned. No release loop existed.
2. `Proposal::approve` had **no production caller**, so nothing was ever
   releasable and the release loop was unreachable even once written.
3. Approved proposals were never retired, so one sizing decision would have
   been re-offered every cycle and pyramided into a position nobody chose.

## Known gaps, stated plainly

- **Nothing writes to `Telemetry`.** `/metrics` serves an empty surface, and
  the four Cloud Monitoring alert policies are gated off *because* no metric
  descriptor has ever been ingested. The platform is not observable.
- **`RiskState::expected_shortfall` is always empty**, so the
  `MaxExpectedShortfall` limit is structurally incapable of firing. A control
  that cannot fire reads as protection and is not.
- **The mesh is turned on in no manifest.** A complete, tested backbone serves
  nothing in any deployment.
- **No TLS egress proxy is deployed**, so every live vendor adapter — IBM
  Quantum, Vertex AI, market data, brokers, chain RPC — is inert in the
  cluster. The HTTP client speaks plaintext HTTP/1.1 by design.
- **Live data sources are unwired.** `feed.rs` can open Synthetic or Replay
  only.
- **The Secret Manager CSI credential chain has never been exercised live.**
- **`infra.yml down` has never been run against a live cluster.**

## Latency

No end-to-end latency class has been measured. This document makes no latency
claim, and neither should any other until a reproducible benchmark exists that
records hardware, topology, dataset and percentiles. The canonical diagram's
"microseconds" is an aspiration for a colocated path, not a measured property
of anything in this repository.
