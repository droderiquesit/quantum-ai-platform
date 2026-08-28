# Non-functional requirements

Each is stated so it can be checked. Where the repository already enforces one,
the enforcement is named; where it does not yet, that is said plainly rather
than implied.

## Correctness and safety

| # | Requirement | Enforced by |
|---|---|---|
| S1 | No live order may be submitted in any configuration | Terraform validation, `AutonomyLevel::deployable`, `Cell` having no live constructor |
| S2 | A live-trading configuration value stops start-up rather than being lowered | `AutonomyLevel::deployable`; tested in `qip-acceptance` |
| S3 | Pre-trade risk checks are never answered by a model | `Determinism::Required` returns a type with no rung field |
| S4 | The UI states `PAPER TRADING` wherever posture is shown | `qip-web` posture banner |
| S5 | No `unsafe` code | `unsafe_code = "forbid"` at the workspace root |
| S6 | No `unwrap()`/`expect()` outside tests | clippy lints, CI `-D warnings` |

## Auditability

| # | Requirement | Status |
|---|---|---|
| A1 | Every cycle is sealed into a hash-chained log | Enforced |
| A2 | A truncated or edited history is detectable | Enforced |
| A3 | P&L decomposes exactly into causing decisions | Enforced (ADR 0007) |
| A4 | Routing decisions record their own rationale | Enforced (`ReasonRouting`) |
| A5 | A cycle is billed for what ran, not what was planned | Enforced |

## Supply chain

| # | Requirement | Enforced by |
|---|---|---|
| D1 | Exactly two third-party crates | `./scripts/check-dependencies.sh` |
| D2 | No secret in any committed file | `./scripts/check-secrets.sh` |
| D3 | No downloaded service-account keys | Workload Identity Federation; acceptance tests |
| D4 | Deployed images are signed and attested | Binary Authorization; `deploy.yml` |
| D5 | Upstream images pinned by digest, not tag | Reviewed per manifest |

## Availability and resilience

| # | Requirement | Status |
|---|---|---|
| R1 | A cell that loses the centre keeps deciding within its envelope | Enforced (ADR 0008) |
| R2 | A duplicated or out-of-order feed is absorbed without loss of legibility | Tested in `resilience.rs` |
| R3 | Storage is proven writable before a process reports healthy | Enforced in each composition root |
| R4 | A failed vendor degrades the answer rather than losing it | Enforced (local solver fallback) |

## Cost

| # | Requirement | Status |
|---|---|---|
| C1 | No rung costs more than the decision is worth | Enforced via `Router::assess` |
| C2 | Every cycle's compute spend is ledgered | Enforced |
| C3 | The stack can be torn down between test sessions | `infra.yml down` — built, **not yet exercised against a live cluster** |

## Observability — the honest gap

| # | Requirement | Status |
|---|---|---|
| O1 | Every process exposes metrics | **Not met.** Nothing writes to `Telemetry`; `/metrics` serves an empty surface |
| O2 | Alert policies exist for workload failure | **Blocked by O1.** The four policies are gated behind `workload_metrics_exist = false` because no descriptor has been ingested |
| O3 | Health endpoints report real readiness | Met |

O1 and O2 are the largest known non-functional gap. They are tracked, not
hidden, and no document in this repository may describe the platform as
observable until they close.

## Performance

Budgets live in `docs/performance/` and are asserted in
`qip-acceptance/tests/performance.rs`, which fails on regression rather than
reporting a number nobody reads.
