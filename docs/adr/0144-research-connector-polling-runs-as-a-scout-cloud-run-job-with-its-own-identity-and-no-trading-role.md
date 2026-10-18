# ADR 0144: Research connector polling runs as a scout Cloud Run job with its own identity and no trading role

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It moves work the deep brain already
  does into a separately identified job in the environment's existing project
  and admits no crate, no source and no licence. Nothing is applied by this
  record; the job and its bucket reach a project only through the owner's
  reviewed plan and apply, like every infrastructure change.
- **Date**: 2026-10-18
- **Extends**: ADR 0024 (the runtime is provisioned in code, one catalogue of
  Cloud Run workloads, each with its own identity from `modules/cloudrun`),
  ADR 0034 (licensing is re-asked before every poll).
- **Keeps**: ADR 0113 §4 and ADR 0118 (one project per environment until the
  second region; a separate scout *project* is the owner's question,
  R12-SEC-9064), the egress proxy as the only outbound path
  (`infrastructure/terraform/modules/egress-proxy`), the fast brain's market
  feed where it is.

## Context

R12-DATA-8112 asks that connectors "run in isolated pools or Cloud Run jobs
with egress, quotas and security posture separate from trading". Today the
deep brain polls catalogued research connectors inside its own process,
beside its own stream, under a licensing gate re-asked before every poll
(`qip-deepbrain/src/connectors.rs`). It does so with the deep brain's
service account, which also holds what the deep brain needs for research,
training and the reflex-package path. The fast brain's connector arm
(`qip_fastbrain::feed::Feed::Connector`) is different in kind: it replaces
the synthetic exchange and is the fast brain's market input.

Every Cloud Run workload is declared in `infrastructure/terraform/catalogue.tf`
and gets its own service account and grants from `modules/cloudrun`.

Locate them with `sed -n '1,25p' backend/crates/apps/qip-deepbrain/src/connectors.rs`
and `grep -n 'google_service_account" "workload"' infrastructure/terraform/modules/cloudrun/main.tf`.

## Decision

1. **Research polling becomes a scout job.** A Cloud Run job, declared in the
   catalogue, runs the deep brain's image with a `scout` entry point that
   polls the catalogued research connectors once per execution, under the
   same per-poll licensing gate, and exits. A Cloud Scheduler trigger, or the
   job's own schedule, runs it on the cadence the catalogue states.
2. **The scout has its own service account and no trading role.** It may
   write to one scout bucket and log; it holds no grant on any trading
   service, the fabric, the ledger, KMS signing keys, the deep brain's
   secrets, or any secret but its connectors' own credentials, which are
   mounted to it alone. An acceptance test asserts that the scout identity's
   grants are exactly that set.
3. **Egress goes through the proxy**, with a route set naming only the
   catalogued research vendors' hosts, so the scout cannot reach anything the
   trading workloads can and vice versa. Its quotas (CPU, memory, task
   timeout, retries) are the job's own.
4. **Results cross as immutable records.** The scout writes each fetch as an
   object named by source and digest with `ifGenerationMatch=0`, beside a
   `SourceManifest` (ADR 0132). The deep brain's service account may only
   read that bucket; it re-validates every record on read and treats a
   record it cannot validate as absent. The deep brain stops polling
   research connectors itself once the job is deployed in an environment.
5. **The fast brain's market feed stays in the fast brain.** It is a trading
   input on the fast lane; moving it behind a job and a bucket would put a
   hop and a schedule on the decision path.

## Consequences

- A compromised or misbehaving research connector runs under an identity that
  can touch nothing but its bucket, and its egress is separately bounded.
- Code slices left: the `scout` entry point in `qip-deepbrain`, the bucket
  reader on the deep brain's side, the job, schedule, bucket, identity and
  proxy route set in Terraform, and a `qip-acceptance` test that the scout
  identity holds no trading role and only its listed grants (DATA-8112).

## What it costs

- A job execution per cadence tick (billed per vCPU-second while it runs) and
  a bucket, in the existing project, once applied.
- Research data reaches the deep brain one job cadence later than an
  in-process poll, and through storage rather than memory.
- One more workload, identity and route set to keep correct; an operator
  debugging a missing vendor record now looks at a job's executions, not the
  deep brain's log.

## What would make this wrong

- If research data needed to reach the deep brain within seconds of
  publication, a scheduled job is too slow and the poll would need a
  long-running service with the same identity separation.
- If the owner splits scouts into their own project (R12-SEC-9064), the
  identity and bucket move with them and this record's "existing project"
  is superseded by that decision.

## Rows

R12-DATA-8112. It stays PARTIAL until its slices land with a mutated test and
the job is applied in an environment.
