# Observability

## What is instrumented

`qip-observability` provides metrics, traces and structured logs, injected
through `Telemetry` so that a test and a deployment differ only in what they
pass.

Every event carries a correlation id shared by the whole cycle, so any decision
can be reconstructed from the event log by a single key:

```rust
event_log.by_correlation(&correlation_id)
```

The log is a hash chain. `EventLog::verify_chain` returns the first broken
sequence number, so tampering is detectable rather than merely discouraged.

## The seven alerts

Seven, because each one means somebody should look now. An alerting policy that
fires on something nobody acts on trains people to ignore the ones that matter.

| Alert | Threshold | Runbook |
|---|---|---|
| Kill switch tripped | any trip, no duration | [kill-switch](../../operations/kill-switch.md) |
| Live fill in a non-production environment | any fill | should be impossible; two controls have failed |
| Risk limit breached for 15 minutes | 900s | [limit-breach](../../operations/limit-breach.md) |
| Agent attempted an ungranted capability | any, over 5 minutes | [permission-violation](../../operations/permission-violation.md) |
| An execution node halted, by kill switch, by policy or by the polled flag | any, per cell and source | [kill-switch](../../operations/kill-switch.md) |
| A node's book and its venue's record disagree | any break, over 5 minutes | [reconciliation-break](../../operations/reconciliation-break.md) |
| The centre acted on a report whose exposure disagrees with its envelope | any break, over 5 minutes | [reconciliation-break](../../operations/reconciliation-break.md) |

The seventh watches `qip_central_reconciliation_breaks_total`, whose
`direction` label is one of `cell_over_venue`, `venue_over_cell`,
`detail_only` or `unsent_fill`. The first three are the cell's own finding,
carried up on its report; the fourth is the centre's, since `3c2b789`: a
cell reported a fill on an order the centre never saw sent, or beyond the
quantity it saw sent. Both kinds halt that cell through the same path
(`CentralPlane::record_halt`, `backend/crates/runtime/qip-kernel/src/central/plane.rs:1398`),
and the policy's query has no direction filter, so a fill the platform has
no order behind pages once the policy exists.

Two series exist so that the two claims about a cell's trading can be read
against each other rather than one standing in for both:
`qip_central_orders_sent_total` counts the orders a cell reported sent —
accepted by the venue, not filled — and `qip_central_fills_attributed_total`
counts the fill shares the centre booked. For one slice the centre billed
every sent order as a fill, and there was no series in which the two could
disagree.

The fifth row's `source` label has three values, not two: `kill_switch` is an
operator or the platform tripping the switch, `policy` is the cell refusing on
its own envelope, and `polled` is the halt flag the node reads on its own
filesystem — the second wire of §46.2, so a node cut off from the centre can
still be stopped by hand on the machine. This table named only the first two
until 2026-09-05, while the policy's own inline documentation
(`modules/observability/main.tf:196-202`) named all three; an operator paged
on `source=polled` would have been reading a table saying that source does not
exist.

Defined in `infrastructure/terraform/modules/observability/main.tf`, each with
its runbook text inline, so the alert that fires carries its own instructions.
Every descriptor named is one `qip-observability` registers, and
`every_metric_an_alert_policy_queries_is_one_the_platform_emits` refuses a
policy naming one it does not.

## Why none of the seven exists yet

All seven stay gated on `workload_metrics_exist`, which is `false` in every
environment — commented out at `environments/dev/terraform.tfvars:128` and
defaulting false in both `terraform/variables.tf` and the module — because
Cloud Monitoring refuses a policy naming a descriptor it has never ingested.
Both planes emit and both are scrapable. **What is missing is proof of
ingestion**, and on today's evidence nothing has been shown to scrape any
process. A reconciliation break on either plane is recorded, and pages
nobody. Do not read this platform as observable.

The two halves of that gap are in different states, and the difference
matters to whoever picks this up:

* **The execution node** — the Ops Agent Prometheus receiver is declared in
  `modules/execution-node/templates/startup.sh.tftpl:209-231`, on the health
  port, every 30s. It is blocked only on a node existing: `execution_nodes`
  is `{}` in all four environments. Nothing is refused here.
* **The Cloud Run services** — the managed-Prometheus sidecar is declared in
  `modules/cloudrun`, rendered only once `metrics_collector_image_digest`
  names a mirrored, attested image. No environment does, and this half is
  **refused rather than pending**: the only version Google publishes
  (`cloud-run-gmp-sidecar` 1.9.2) fails the platform's own Trivy gate on
  three unfixed advisories in `golang.org/x/crypto/ssh`, and the newest
  published tag is still 1.9.2 as of 2026-09-05. The blocker is upstream.

`infrastructure/terraform/modules/observability/NOT-SCRAPED.md` is the record,
with the commands that establish each of those claims, and it must be read
before `workload_metrics_exist` is flipped anywhere.

## What to watch that is not an alert

* **Budget utilisation per agent.** `AuditTrail::utilisation_by_agent`. An
  agent habitually running at 98% of its allowance is about to start failing.
* **Red team rejection rate.** `RedTeam::rejection_rate`. Near zero means the
  review is not doing anything; near one means the hypothesis generator is not.
* **Calibration.** `FeedbackEngine::calibrate`. A platform that is
  systematically overconfident will size positions on confidences it has not
  earned.
* **Suppressed opportunities.** The opportunity engine caps emissions per cycle
  and reports `suppressed_count` rather than silently truncating. A rising
  count means the queue is not being worked.

## Health

`GET /api/v1/health` answers the two questions a monitor needs:

```json
{"status":"ok","halted":false,"autonomy":"paper_trading","live_capable":false}
```

`live_capable` is read from the assembled platform, not from configuration, so
it answers "could this process reach a venue" rather than "what does the config
say".
