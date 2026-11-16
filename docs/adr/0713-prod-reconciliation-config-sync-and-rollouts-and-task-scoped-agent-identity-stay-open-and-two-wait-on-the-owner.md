# ADR 0713: Prod reconciliation from Git and a task-scoped agent identity wait on the owner, and Config Sync and Argo Rollouts stay declined

- **Status**: Proposed 2026-11-16, for the owner. Two decisions touch billed
  infrastructure (a production control plane, a funded fleet) and one concerns
  the deploy gate that holds the production boundary, so the delegation of
  2026-11-15 does not cover them.
- **Date**: 2026-11-16
- **Rows**: R12-CICD-10182, R12-CICD-8117, R12-CICD-9076

## Context

R12-CICD-10182 requires production workloads to be reconciled from Git and
admitted only under supply-chain policy. Today every Cloud Run service is a
GitOps manifest pinned by digest and subject to Binary Authorization, but prod
has no control plane and nobody promotes to it. The register holds the row on
owner decision O20: reopen prod reconciliation from Git (ADR 0036, ADR 0093).
The deploy gate and `gitops.rs` refuse prod promotion, and that refusal is a
guard this record does not touch.

R12-CICD-8117 names four controllers: Config Sync, Argo CD, Kargo and Argo
Rollouts. ADR 0114 D4 admits Argo CD, Kargo and Config Connector on the
control plane and refuses Config Sync and Argo Rollouts. The owner ruling of
2026-10-31 left D4 standing, did not narrow the requirement, and said the row
closes only on an ADR that admits both mechanisms.

R12-CICD-9076 requires ephemeral workers under task-scoped identities. The
fleet in `infrastructure/fleet` runs ephemeral Cloud Run Job tasks, but under
one shared keyless account, `fleet-worker`. ADR 0102 sets the funding ceiling
and is not amended here. Development-agent tasks run in the owner's sessions
(ADR 0098); only their acceptance commands run in `agent-sandbox.yml`.

## Decision

1. **R12-CICD-10182: owner.** Reopening prod reconciliation from Git is the
   owner's choice under O20, because it turns on a billed production control
   plane and on the prod refusal in the deploy gate and `gitops.rs`. This
   record does not write that choice, does not reopen prod promotion, and
   changes no guard. Until the owner answers, the row stays PARTIAL and the
   refusal stays in force. A later record that reopens it must say which
   decision of ADR 0036 or ADR 0093 it amends, and must keep a human dispatch
   as the only way to promote.
2. **R12-CICD-8117: decline.** No record is written that admits Config Sync or
   Argo Rollouts. ADR 0114 D4 stands. The baseline layer is reconciled by
   Config Connector, and progressive delivery is Cloud Run's per-service
   instance and concurrency bounds. No fifth controller joins the control
   plane, and the guard test in `gitops.rs` is unchanged. The requirement is
   not restated so that the equivalents satisfy it; the row stays PARTIAL and
   would reopen only if the owner asks for an admitting ADR.
3. **R12-CICD-9076: owner.** A task-scoped identity needs deployed
   infrastructure (a funded fleet, a recorded plan and apply) and an IAM
   design in which each task receives its own credential. Both are the
   owner's to fund and approve. If the owner asks for it, the design starts
   from two constraints: the grant is narrowed to the one task's packet and
   output path and never widened to make an error go away, and no
   development-agent task moves into a fleet worker unless ADR 0102 is
   amended in its own record. Until then the shared `fleet-worker` account is
   recorded as unmet against "task-scoped", and the row stays PARTIAL.

## What it costs

Nothing is built. Three rows stay PARTIAL, and two of them wait on an owner
answer that has not been given. Production keeps no reconciling control plane,
so a change to prod is still promoted by a person. The Config Sync and Argo
Rollouts clauses of R12-CICD-8117 stay unmet in the letter of the
requirement. Agent workers share one identity, so one compromised task
carries the whole account's read and write scope over packets, output and
ledger. Declining decision 2 costs nothing further, because it records what
ADR 0114 already holds.

## What would make this wrong

- The owner answers O20 yes and funds a prod control plane. Decision 1 then
  only deferred, and the amending record is the next step.
- A prod incident shows that human promotion is the weaker control, which would
  argue for reconciliation from Git over the present gate.
- A measured need appears that Cloud Run's instance and concurrency bounds
  cannot meet and that a rollout controller would, which would be the case for
  an ADR admitting Argo Rollouts.
- Google publishes a Config Sync release that passes the scan gate and the
  owner wants the layer split Config Sync's way, which would reopen decision 2.
- Fleet tasks begin to hold credentials or take actions beyond one model call,
  which would make the shared account a live exposure rather than a recorded
  gap and move decision 3 ahead of the owner's schedule.
