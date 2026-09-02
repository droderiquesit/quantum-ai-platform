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
| An execution node halted, by kill switch or by policy | any, per cell and source | [kill-switch](../../operations/kill-switch.md) |
| A node's book and its venue's record disagree | any break, over 5 minutes | [reconciliation-break](../../operations/reconciliation-break.md) |
| The centre acted on a report whose exposure disagrees with its envelope | any break, over 5 minutes | [reconciliation-break](../../operations/reconciliation-break.md) |

Defined in `infrastructure/terraform/modules/observability/main.tf`, each with
its runbook text inline, so the alert that fires carries its own instructions.
Every descriptor named is one `qip-observability` registers, and
`every_metric_an_alert_policy_queries_is_one_the_platform_emits` refuses a
policy naming one it does not. All seven stay gated on
`workload_metrics_exist = false` until something is observed scraping a
process: the execution node's Ops Agent receiver is declared, and the Cloud
Run services have no collector yet
(`infrastructure/terraform/modules/observability/NOT-SCRAPED.md`).

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
