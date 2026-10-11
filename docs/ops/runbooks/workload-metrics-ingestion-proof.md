# Proving ingestion before flipping `workload_metrics_exist`

W4.7, written ahead of the flip and **not** the flip. `workload_metrics_exist`
stays `false` in every environment; this file says what evidence a person
must hold, and in what form, before changing that. Labels as in
[the operator runbook](../RUNBOOK.md): **ran**, **repo**, **not run**.

## Why a declaration is not enough

Every alert policy in `infrastructure/terraform/modules/observability/main.tf`
is created only when `workload_metrics_exist = true`, and Cloud Monitoring
refuses a policy whose PromQL names a descriptor the project has never
ingested. A flip on the strength of a scrape being *configured* fails the
apply partway, after other resources changed; a flip on a descriptor ingested
once, months ago, by a process that no longer runs, creates a policy that is
evaluated against nothing and reads in the console as a watched project. The
second is worse than the gap it replaces (`.claude/rules/domains/observability.md`,
"Prohibited").

## The gate is one switch over two halves, and only one half can ingest today

Recount before quoting, in the tree you are about to change (**repo**,
2026-10-11, this worktree):

```
grep -c '^resource "google_monitoring_alert_policy"' infrastructure/terraform/modules/observability/main.tf   # 9
grep -c 'count *=.*workload_metrics_exist'           infrastructure/terraform/modules/observability/main.tf   # 9
grep -n 'query *= *"' infrastructure/terraform/modules/observability/main.tf
```

The third lists the descriptor each policy queries. Two are edge series
(`qip_edge_halted`, `qip_edge_reconciliation_breaks_total`), reached through
the execution node's Ops Agent receiver. Seven are central series
(`qip_kill_switch_tripped`, `qip_live_fills_total`, `qip_limit_breaches`,
`qip_permission_denials_total`, `qip_central_reconciliation_breaks_total`,
`qip_risk_figures_unevaluated`, `qip_proposals_unsigned_total`), reached only
through the Cloud Run managed-Prometheus sidecar — which is **refused, not
pending**: `metrics_collector_image_digest` is null everywhere because the only
published sidecar fails the platform's own Trivy gate
(`modules/observability/NOT-SCRAPED.md`).

So, plainly: **the flag cannot be flipped as the module stands**, even after
a node is deployed, because one switch creates all nine and seven name
descriptors nothing can ingest. Flipping it requires one of:

1. The central half ingesting too (a sidecar digest that passes the scan, or
   another collector reviewed in its own change), and then the evidence below
   for all nine descriptors; or
2. A reviewed change to `modules/observability` that splits the gate into an
   edge switch and a central switch, each counted and asserted the way the
   single gate is today — and then the evidence below for the two edge
   descriptors only, flipping only the edge switch.

Neither is in this change.

## The evidence, per descriptor

For **every** descriptor a policy about to be created will query, collect all
four of the following, run by a person against the environment's project,
and paste the output (not a summary of it) into the pull request that flips
the switch. Replace `<project>` with the environment's `project_id` from its
committed tfvars, and `<series>` with the descriptor name.

1. **The process that emits it exists and is the one the plan describes.**
   For an edge series:
   `gcloud compute instances list --project <project> --filter='name~^qip-<env>-exec-' --format='table(name,zone.basename(),status)'`
   shows a `RUNNING` instance. For a central series: the Cloud Run service
   serving the image digest `gitops/envs/<env>/kustomization.yaml` names, and
   a revision that carries the collector sidecar.
2. **The descriptor exists in the project.**
   `gcloud monitoring metrics-descriptors list --project <project> --filter='metric.type = starts_with("prometheus.googleapis.com/<series>/")' --format='value(type)'`
   prints at least one type. An empty answer is a refusal to flip, however
   healthy the process looks.
3. **Points were ingested recently, not once.** Read the time series for the
   last hour through the Monitoring API:
   ```
   curl -sS -H "Authorization: Bearer $(gcloud auth print-access-token)" \
     "https://monitoring.googleapis.com/v3/projects/<project>/timeSeries?filter=metric.type%3Dstarts_with(%22prometheus.googleapis.com/<series>/%22)&interval.startTime=<now-1h RFC3339>&interval.endTime=<now RFC3339>&view=HEADERS"
   ```
   The response must hold at least one `timeSeries` entry. Quote the
   `metric.labels` of one entry; for an edge series the `cell` label must be
   the node id the tfvars declare.
4. **The policy's own query evaluates.** In Metrics Explorer's PromQL tab, or
   `curl` against `https://monitoring.googleapis.com/v1/projects/<project>/location/global/prometheus/api/v1/query?query=<url-encoded query from main.tf>`,
   the exact query string from `main.tf` returns `"status":"success"`. An
   empty result vector is acceptable — the condition is a fault that has not
   happened — but an error naming an unknown metric is not.

Then, in the same pull request:

- Recount the three commands at the top of this file and quote them.
- Set the switch in the environment's tfvars and nowhere else.
- Run `infra.yml` with `action=plan` for that environment and quote the plan's
  policy creations: exactly the policies whose descriptors you evidenced, and
  no other `google_monitoring_alert_policy`. A plan that creates a policy whose
  descriptor has no evidence above is a refusal to apply.
- A person applies. An agent does not (`scripts/agent-policy.json` refuses an
  agent actor on `infra.yml`).

## What would make this file wrong

- The module's gate changing shape (split, renamed, or a policy escaping it):
  the second count above stops equalling the first.
- `NOT-SCRAPED.md` recording a collector digest that passes the scan: the
  central half stops being refused and option 1 opens.
- An execution node being applied (`execution_nodes_enabled = true` in a
  reviewed diff, W4.3): the edge half stops waiting and option 2's evidence
  becomes collectable.

Each of those is a change to this file in the same diff.
