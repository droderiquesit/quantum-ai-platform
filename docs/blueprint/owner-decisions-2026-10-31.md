# Owner decisions, 2026-10-31

The owner wrote, in the orchestrator session (session_01DCyuGC9sjZ7o9nZDdT166K),
on 2026-10-31 between 00:28 and 00:33 UTC, after being told what the open
register rows needed:

> Use this going forward. You can make all my decisions go. Do the 1 2 3 and 4
> all at once. Fan out use caveman and complete all sessions use many agents do
> this to finish what you can by morning.

> I want a 2 minute not hourly

"1 2 3 and 4" were the four items the orchestrator had just listed as the
owner's alone: approve the rolling pull request's head and apply the sast
barrier; apply the lane-written protected patches; bring dev up (plan, up,
one execution node, funded stores); answer the owner-bucket register
decisions. This file records what was decided under that delegation and what
it does not lift, in the register of `owner-decisions-2026-10-24.md` and
`owner-decisions-2026-10-25.md`, which it extends and does not replace.

## What the delegation does not lift

The same four exceptions the owner set on 2026-10-24 stand, because they are
the repository's own rules and not the owner's preference: no live-order path
and no real capital or money movement (ADR 0003, rule 01); no weakening of a
test, a guard, a paper-trading layer or an existing limit value; no new
dependency without an ADR (ADR 0109); and no billed infrastructure created
without its monthly cost named first. Two further limits are mechanical, not
chosen: billing activation needs the owner's console session, and the session
permission layer refuses an agent any edit under `.claude/**` and any change
that narrows a security scanner, whatever the owner has delegated.

## Decisions

1. **Orchestrator.** Session session_01DCyuGC9sjZ7o9nZDdT166K continues the
   wave580 programme. The first orchestrator (session_01RsTafvtWPKcWCWMz5tqDWH)
   stood down at 00:08 UTC after the two had merged the same lanes on two
   branches; its hourly check-in trig_01YC63HGiQdj7TehsQsraEvJ is disabled.
   The integration branch is `ccr-116bbb27-osu8n8`; `ccr-d1bc1e54-kf6mr0`
   (PR #243 to main) is fast-forwarded to it after every gated push, so one
   tree carries both names. The check-in runs every two minutes in the
   orchestrator session, with trig_014RbraY6zTjp2P8ShHwXgyv hourly as the
   backstop.
2. **Lanes.** The six primaries continue; L1b, L2b, L3b, L4b and L5b are
   unparked with disjoint domains (FABRIC/FINOPS, REASON/AMBIENT/AGENCY, TICK,
   EXPAND, MESH/REFLEX) and the primaries keep the rest. Every lane may now
   decide a row that waited only on a product decision, with a new ADR claimed
   in `docs/adr/README.md` in its own pushed commit, choosing the conservative,
   paper-only, fail-closed reading. Pacing stays at a push every two rows,
   because the account's weekly usage limit killed every lane on 2026-10-25
   and its five-hour limit did so again at 00:50 UTC today (reset 02:00 UTC).
3. **Protected patches.** Applied on the integration branch, each adding a
   refusal, a scheduled check or a null-default field and loosening nothing:
   `l9-CICD-9083.patch` (the `Agent-Lane` trailer gate, shipped off),
   `l9-CICD-8032.patch` (`drift.yml`, a daily dev plan that fails on drift),
   `l9-CICD-9095.patch` (the two main-branch rulesets as files plus a daily
   live-rule check, enforcing nothing until POSTed) and
   `l5b-peer-mesh-root.patch` (the `peer_mesh` object on `execution_nodes`,
   null in every environment, no node, no cost); `node --test
   scripts/agent-guard.test.mjs` prints pass 40, fail 0 with the first two
   in. Decided but **not applied, because the session permission layer refused
   the agent** and the refusal is not routed around: `l5-edge-GOV-9075.patch`
   (`.claude/agents/learning-agent.md`), `l9-P3.patch` and
   `l6-res-8026-observability-rule.patch` (`.claude/rules/**`) and
   `l8-sast-barrier.patch` (the one CodeQL barrier row of decision 1 on
   2026-10-25, the only patch that narrows a scanner). These four are the
   owner's to apply with `git apply docs/blueprint/lanes/<name>.patch` on
   this branch. Two patches are refused and stay as records:
   `l6-ncc-hubs-parked.patch` (decision 4 of 2026-10-25 declined the hubs and
   `gcp_placement` refuses them) and `L4-quant-research-families-adr0150.patch`
   (ADR 0150 forbids a new family before a committed sizing note; landing it
   would reopen an ADR by code).
4. **The rolling pull request's head** still needs a person's approving review
   for the `agent boundary` check: no agent can give one, so that click stays
   the owner's, as does the `POST` of the two rulesets once they are in main.
5. **Dev deployment.** The dev WIF bootstrap is already applied (infra.yml run
   118, 2026-10-13, "Apply complete"; run 121 "qip-dev is Synced and Healthy";
   deploy.yml run 284 succeeded), so the plan's W4.1 text was stale and is
   corrected. What stops a dev `up` today is, in order: main's ci is red on the
   zstd advisories until the fix on this branch reaches main; `infra.yml` and
   `deploy.yml` refuse an agent actor (`scripts/agent-policy.json`
   `dispatch_refused_for_agents`), so every dispatch is the owner's; a core
   change lands only inside the change window (Tuesday to Thursday, 14:00 to
   20:00 UTC); the image registry answered 403 on the last two pushes, which is
   billing or IAM and unverified; and an execution node additionally needs
   `artefact_trust_bundle_file`, `google_tls_roots_file` and a CA Service pool
   (`execution-nodes-enabled.tf`), none of which exists. The decision is: the
   owner dispatches `infra.yml` `{dev, urls}` first (free, read-only), then
   `{dev, plan}`, and the orchestrator prepares the trust-bundle and roots
   files and the CA-pool declaration as a reviewed change so the node switch
   has something to flip. The runbook's estimate stands at $780 to $960 a
   month for what dev's tfvars already enable; nothing new is funded by this
   decision.
6. **R12-RISK-4116.** The joint tail world's survival reserve may be applied
   on the default path: it raises a reserve, which is a tightening, and the
   lane's test holds the applied reserve where it was until the switch is on.
   Lane L1 decides the switch and records it in the row.
7. **The 208 owner-bucket rows** (OWNER_DECISION 92, OWNER_COST 35, EXTERNAL
   28, PROTECTED 28, DEPLOY 22, OWNER_RISKLIMIT 2, OWNER_LIVE 1) are decided by
   a read-only decider per batch of twelve with a skeptic per batch, under the
   rules above; the first run died on the five-hour limit and re-runs after
   02:00 UTC. Each ruling lands here as a numbered decision when it is made,
   and the OWNER_LIVE row is refused in advance.
8. **A failed or halted pass still reports what it sent** (R12-MESH-7143,
   03:53 UTC). Lane L5b found that `qip-edge-node`'s scheduler keeps no
   `WorkReport` for a pass that returned an error and resets to the default
   report on a halt, so an order sent to a venue before a cycle broke never
   rides the mesh delta and is never charged into the aggregate the Hedge
   Brain plans against; the centre would later meet its fill as a fill on an
   order it never saw sent. The decision, ADR 0209 claimed by lane L5b: the
   scheduler keeps the report of a failed or halted pass, marks it with the
   gate and the pass number, ships it on the next delta exactly once, and
   sends nothing new after the failure; the centre settles those orders as
   any other and records the halt beside them. A report the scheduler
   cannot mark is refused whole, never shipped unmarked. This is the
   "bill what ran, not what was planned" principle applied to a pass that
   did not finish.
9. **The counterfactual twin's direction was inverted** (not a product
   decision, recorded because it changes every regret figure the register
   cites). Until 2026-10-31 `qip-twin`'s `ActualTrade::direction` read a
   trade against the bid as the long while every producer records a buy
   against the ask, so every declined-path and fill score settled with the
   opposite sign, and four kernel fixtures, one deep-brain fixture and one
   acceptance label had been bent to agree with it. The fix flips the twin,
   turns the fixtures, and removes the cross lane L5 had added to match
   the old reading. Journaled `DeclinedScore` and `FillScore` records
   written before the fix carry an inverted `regret` and
   `would_have_earned`; a register row whose evidence rests on one of
   those figures is to be re-read before it is promoted.
