# Operations, security and policy

* [Operator runbook](RUNBOOK.md) — what runs where (and what does not), a local start, the halt procedures, CI/CD, Terraform, secrets, cost, backup, known gaps; with [per-binary pages](runbooks/binaries.md) and [incident playbooks](runbooks/incidents.md)
* [Security](security/README.md) — threat model, controls, what is not covered
* [Policies](policies/README.md) — model, agent and data governance; change management
* [Observability](observability/README.md) — what is instrumented and what to watch

## Index of runbooks and records

Every page under `docs/ops/runbooks/` and every page directly under `docs/ops/`
is listed here, once. `architecture_memory_index.rs` fails when a page is added
and not listed, because a runbook nobody can find from the index does not
exist at the moment it is needed.

* [Binaries](runbooks/binaries.md) — the config keys each binary reads
* [Environment variables](runbooks/environment-variables.md) — every `QIP_*` variable Terraform sets on a workload
* [Incident playbooks](runbooks/incidents.md)
* [Break-glass](runbooks/break-glass.md) — changing a running environment when GitOps cannot
* [Development-agent controls](runbooks/development-agent-controls.md)
* [A new project, from scratch](runbooks/new-project-from-scratch.md)
* [The owner's switches](runbooks/owner-switches.md) — each setting committed off, and who turns it on
* [Dev redeploy readiness](runbooks/dev-redeploy-readiness.md) — the free agent work, the protected patches and the owner's ordered steps to bring dev back to serving, with the image and CVE status and a cost estimate (2026-10-24 draft)
* [Proving ingestion before flipping `workload_metrics_exist`](runbooks/workload-metrics-ingestion-proof.md)
* [Operator runbook](RUNBOOK.md)
* [M5 deployment guide](M5_DEPLOYMENT.md) and [M5 operational runbook](M5_RUNBOOK.md) — the first vertical slice
* [Execution region selection](execution-region-selection.md) — measured findings
* [Execution measurements](execution-measurements.md)
* [The ECB reference rates through the loop, live (2026-09-06)](live-source-frankfurter-2026-09-06.md)
* [Hermes baseline (2026-10-04)](hermes-baseline-2026-10-04.md)

Runbooks are in [docs/operations](../operations/README.md), and the one
deployment path is [docs/operations/deployment-path.md](../operations/deployment-path.md).

Dated records of what was run against something real, and what the run does
not prove:

* [The ECB reference rates through the loop, live (2026-09-06)](live-source-frankfurter-2026-09-06.md)
* [Execution measurements](execution-measurements.md)

Both registers of what is switched off or missing were consolidated into
`docs/DELIVERY-STATUS.md` on 2026-09-07 and deleted; that file was itself removed on 2026-10-11 and is in history at `f65c2c48`.
They were "kept as a pair so that they cannot disagree about one switch",
which is the argument for one document rather than two: what the architecture
of record requires and the tree does not provide is now the §41–§46 rows of
the delivery status, and the switches held closed are a section of it.
