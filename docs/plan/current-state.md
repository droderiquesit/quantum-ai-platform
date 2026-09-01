# Current state, as measured

Established by running the gates and reading the tree on the commit that closed
the trading spine. Every number here came from a command whose output was read.

## Shape

| Fact | Value |
|---|---|
| Rust crates | 59, in 8 groups (`libs`, `services`, `apps`, `edge`, `agents`, `quant`, `runtime`, `tests`) |
| Tests | 3,192 passing, 0 failing, 0 ignored, across 290 binaries (`cargo test --workspace --no-fail-fast`), measured on the commit that added the §6.2 degradation contract |
| Clippy | 0 warnings, `--all-targets` |
| Third-party crates | 11 packages, all permitted (`serde`, `serde_json` and their trees) |
| Frontend | Next.js + TypeScript, 47 tracked files |
| Cloud | GCP: GKE, Secret Manager CSI, KMS, Binary Authorization, WIF |
| Pipelines | `ci.yml`, `deploy.yml`, `infra.yml` — all deriving identity from committed tfvars |
| Decision records | 21 ADRs |

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
| SENSE | Absorbs 11 record kinds. Works. `feed.rs` can now open `Live` as well as `Synthetic` and `Replay`, but no deployment has been observed running on it |
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

Five gaps this document reported when it was written have since been closed,
and are recorded below as closed rather than deleted — a plan that quietly
drops what it once said was broken cannot be audited against.

**Closed:**

- ~~Multi-leg execution is unbuilt.~~ `services/qip-execution-engine/src/multileg.rs`
  carries leg risk, deadlines and unwind, on the invariant that a group which
  cannot complete is unwound rather than abandoned.
- ~~Champion/challenger and drift detection are unwired.~~ Both now have
  production callers: `apps/qip-deepbrain/src/evolution.rs:426` runs the
  contest against a policy constructed at `:228`, and `apps/qip-deepbrain/src/learning.rs:279` records a drift report
  built at `:425` — both above the `#[cfg(test)]` boundary at line 516, which
  is the check that distinguishes a wired control from a tested one.

- ~~Nothing writes to `Telemetry`.~~ The kernel now records at the seams where
  facts become known, and a collector scrapes the brains. The deeper defect
  this uncovered: the four Cloud Monitoring alert policies queried metric names
  that appeared in **zero** Rust files, so the alerting layer was unreachable
  by construction rather than merely gated off. Both halves now name the same
  series, and a test asserts they keep doing so.
- ~~`RiskState::expected_shortfall` is always empty.~~ Both it and
  `value_at_risk` are populated from the book's own realised equity path, keyed
  by each configured limit's own confidence. Two limits that
  `conservative_default` ships can now fire, and tests prove they fire on a
  book with a tail and stay quiet on one without.
- ~~The mesh is turned on in no manifest.~~ Wired, with a test asserting every
  variable a manifest sets is one its binary reads, and the converse.
- ~~No TLS egress proxy is deployed.~~ The manifest is committed, with twelve
  mutation-verified tests.
- ~~Every stage reported `Duration::ZERO`.~~ `StageOutcome::with_elapsed` had
  existed since the loop was written and was never called.

**Still open:**

- **No live data source has been proven end to end.** The wiring is no longer
  the gap: `feed.rs` now declares `Live(Box<RestMarketDataAdapter>)` at line 61
  and constructs it at line 108 behind the licensing gate, alongside
  `Synthetic` and `Replay`. What is still missing is evidence — no deployment
  has been observed absorbing a cycle of real data through it, so the honest
  statement is that the path exists and has not been exercised, which is a
  different gap from the one this document used to describe.
- **`workload_metrics_exist` is still `false`** in every environment, and
  correctly so: the endpoints exist and a collector is declared, but no pod has
  been observed to scrape. Flipping it requires that evidence.
- **The Secret Manager CSI credential chain has never been exercised live.**
- **`infra.yml down` has never been run against a live cluster.**
- **Capital reservation is unbuilt.** A proposal that passes a capital check
  does not hold the capital, so two concurrent proposals can each pass against
  the same free balance. This is what remains of canonical area 5; leg risk,
  deadlines and unwind landed with `qip-execution-engine/src/multileg.rs`.

## Latency

No end-to-end latency class has been measured. This document makes no latency
claim, and neither should any other until a reproducible benchmark exists that
records hardware, topology, dataset and percentiles. The canonical diagram's
"microseconds" is an aspiration for a colocated path, not a measured property
of anything in this repository.
