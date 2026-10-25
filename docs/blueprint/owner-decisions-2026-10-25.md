# Owner decisions, 2026-10-25: the open items the lanes handed up

The owner wrote, on 2026-10-25: "Make my decision on all lets move". That
extends the 2026-10-24 delegation (`owner-decisions-2026-10-24.md`) to the
items the lanes had passed up for an owner's call. Every decision below was
taken under it by the parent session, and each is the recommended call for
a paper-trading platform whose deploy is deliberately last because it costs
money.

The limits in the 2026-10-24 record still hold. No live order and no real
capital. No billing activation. No loosening of a test, guard or risk
control. A decision here can approve something an agent then cannot carry
out. Those are listed under "Owner action", because the session's
permission layer refuses them to an agent whatever the delegation says.

## Decisions

1. **CodeQL barrier for `MeshLink::connect_trusted_with`: approved.** The
   patch is `docs/blueprint/lanes/l8-sast-barrier.patch`, with lane L8's
   evidence in commit 9a1f6a2c. All fourteen findings on main's
   `sast / Analyze (rust)` have this one source, and the link holds no
   secret bytes. With the row, the gate reads "no finding at or above
   security-severity 7". Without it, main's ci stays red and `deploy.yml`
   cannot deploy. The permission layer refused the agent twice, so this is
   an owner action.
2. **R12-RES-7104 thresholds: approved as written.** These are 100 bps of
   live fill divergence, a 10-fill minimum, and a 0.5 multiplier on the
   capital grant until the strategy is re-promoted. The control can only
   narrow a grant, never widen one, so approving it tightens capital
   protection. No existing risk limit, threshold or limit set changes.
3. **Protected patches that add a gate or correct a comment: approved.**
   They are applied on the integration branch and ship in the rolling pull
   request. `agent boundary` still requires the owner's approving review of
   that head, which is the sign-off.
   - Lane L6: `l6-arch-9071`, `l6-cicd-9066`, `l6-cicd-7116-7119` (after
     9066), `l6-gcp-9120` and `l6-sec-10157`.
   - Lane L9: `P1`-`P7` once written. Each patch must add a gate or correct
     a comment and weaken nothing, or it is refused.
   - `l6-res-8026-observability-rule` edits `.claude/rules/**`, which the
     permission layer refuses to agents, so it is an owner action.
4. **NCC hubs (R12-GCP-8057, 8058): declined for now.** ADR 0035 runs one
   execution region and the hub is billed. The module stays parked as
   `l6-ncc-hubs-parked.patch` until a second region is decided.
5. **New billed infrastructure: declined for now.** This covers every row
   the 2026-10-24 triage marked OWNER_COST: the Spanner managed log,
   Bigtable, Dataplex, BigQuery reservations, Vertex GPU and TPU, and new
   replay and solver pools. Each stays PARTIAL with its cost named in the
   gap. Each is reopened by an ADR that states a measured need and a
   monthly ceiling. Under the owner's instruction the deploy is the cost to
   take first, and these come after it.
6. **Hosted language model: no vendor yet.** The deep brain keeps its
   deterministic default. A vendor needs a contract, a credential and an
   attestation, all paid and the owner's. The adapter and the attestation
   gate stay as they are.
7. **New capital or risk budgets (R12-CAPITAL-4085, R12-ASSET-4070):
   declined for now.** One allocator keeps deciding, and nothing new is
   sized. A per-strategy, per-class or per-region budget is a new risk
   limit, and it needs its own record with the numbers argued.
8. **Rows that need live capital stay BLOCKED.** This covers the 31 BLOCKED
   rows and R12-EXPAND-6065. ADR 0003 stands.
9. **Cheaper dev control plane: approved.** Lane L9 sets explicit, modest
   CPU and memory requests on the Argo CD, Kargo and cert-manager pods,
   which the runbook estimates at $250-$300 a month less. It also drops
   the unused Argo CD notifications and applicationset controllers. These
   are manifest changes with an acceptance test each, and they create
   nothing.
10. **The dev budget alert: owner action, recommended.** The runbook
    estimates dev at $780-$960 a month against a $100 alert. Raise the alert
    to about $1,000 when billing is re-enabled. The value lives in
    `infrastructure/foundation/terraform.tfvars`, applied only by the owner
    through `scripts/bootstrap-foundation.sh`.

## Owner action

The agents cannot carry these out, whatever the delegation says.

- `git apply docs/blueprint/lanes/l8-sast-barrier.patch`, then commit and
  push it, or merge it in the rolling pull request (decision 1).
- `git apply docs/blueprint/lanes/l6-res-8026-observability-rule.patch`
  (decision 3).
- `git apply docs/blueprint/lanes/l9-P3.patch` (decision 3): lane L9's
  correction of `.claude/rules/domains/infrastructure.md`'s "Known gap",
  which `e2948eab` closed. Its six siblings P1, P2, P4, P5, P6 and P7 were
  applied on the integration branch on 2026-10-25; each corrects a comment
  or a printed line and changes no value.
- Approve the rolling pull request's head, so that `agent boundary` passes
  (decision 3).
- When ready to deploy, follow `docs/ops/runbooks/dev-redeploy-readiness.md`
  §4: enable billing, raise the budget alert (decision 10), then dispatch
  `infra.yml` and `deploy.yml` in the order given.
