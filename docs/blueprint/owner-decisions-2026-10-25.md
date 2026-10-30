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

11. **R12-GOV-6127, an arbitrage leg's grounds: the requirement text
    stands, and the row stays PARTIAL.** Lane L5 resolved every order's
    evidence at the send seam (ADR 0200) and asked whether a leg whose
    only ground is a standing arbitrage mandate is outside what GOV-6127
    forbids. It is not. The bar's fourth part makes the requirement text
    authoritative, and the text forbids an action resting on profit alone;
    a mandate is the same authorisation for every cycle and names no
    per-action ground. Reading the mandate as a ground would redefine the
    requirement to pass the row, which is the thing the 2026-10-24 record
    rules out. The row records the two skeptics' refutation and waits on a
    per-action, non-price ground, should one exist; none is invented here.

12. **R12-CICD-9095, the admin bypass in the review ruleset (2026-10-25,
    under the standing delegation).** Lane L9's `main-review.json` lets the
    admin role bypass required review, and only through a pull request.
    Decision: the patch stays as written. Without any bypass every merge is
    blocked, because the one code owner also authors the pull requests and
    GitHub refuses self-approval; a bypass that also covered a direct push
    would let a commit skip CI. Posting the rulesets changes a production
    security boundary, so only the owner does it, after reading both files.
    R12-CICD-9095, 8032, 9083 and R12-GCP-9086 stay PARTIAL until the patches
    are applied and, for 9095, the rulesets are live.

13. **R12-CICD-10117, the four files two builds of one tree still differ in
    (2026-10-30, under the standing delegation).** Lane L9 made the portal's
    `generateBuildId` read `SOURCE_DATE_EPOCH` and measured two builds of the
    same tree: 4 of 2,921 files differ, and all four hold per-build secrets
    (the preview-mode keys and the Server Actions key). Decision: exclude,
    not inject. Sharing a per-build secret across builds to make them
    byte-equal would trade a security property for a reproducibility
    figure. The deploy gate compares the two builds with exactly those four
    files excluded, named one by one in a committed list, and refuses when
    any file outside the list differs or a listed file stops existing. That
    gate is a `deploy.yml` change, so it arrives as a patch under
    `docs/blueprint/lanes/` for the owner, and the row stays PARTIAL until
    it is applied.

14. **R12-CICD-9085, a committed baseline plus a delta test (2026-10-30).**
    `RECORDED_DIGEST` in `tick_replay.rs` is a baseline for replay
    fidelity, not performance, and `performance.rs`'s ratio tolerances are
    bounds, not a baseline. What closes the row is a committed baseline of
    relative figures recorded on a named runner class with a delta test
    against it; the runner class is not known before a deployment, so the
    row stays PARTIAL and waits on deploy. A file of absolute timings is
    refused: it would fail on any slower runner and pass on any faster one,
    which measures the runner, not the change.

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
- `git apply` lane L9's three gate patches, each adding a refusal and
  weakening none (decision 3; written 2026-10-25 at L9 head `a0e727bf`):
  - `docs/blueprint/lanes/l9-CICD-8032.patch` (R12-CICD-8032, R12-GCP-9086):
    `.github/workflows/drift.yml`, a daily `terraform plan` of dev with
    `-detailed-exitcode` that fails the run on exit 2 or a plan error;
    agents are refused its dispatch.
  - `docs/blueprint/lanes/l9-CICD-9095.patch` (R12-CICD-9095):
    `.github/rulesets/main-{integrity,review}.json`,
    `.github/workflows/rules.yml` (a daily read of the live rules) and the
    `branch_rules` acceptance suite. The files enforce nothing by
    themselves: the owner, as repository admin, must also POST both
    rulesets. The review ruleset lets the admin role bypass, but only
    through a pull request; decision 12 says why.
  - `docs/blueprint/lanes/l9-CICD-9083.patch` (R12-CICD-9083): agent-guard
    step 1c, requiring an `Agent-Lane` trailer after
    `lane_manifest.trailer_required_after`. The switch ships null because
    `round-3.json` names no lane of the running round; it turns on with
    that round's manifest.
- Approve the rolling pull request's head, so that `agent boundary` passes
  (decision 3).
- When ready to deploy, follow `docs/ops/runbooks/dev-redeploy-readiness.md`
  §4: enable billing, raise the budget alert (decision 10), then dispatch
  `infra.yml` and `deploy.yml` in the order given.
