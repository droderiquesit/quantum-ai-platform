# ADR 0718: Agent sessions get no cloud principal and no per-run credential; write-capable federation refuses agent actors and is bound to named workflows, and an unmarked agent push is a stated residual

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-SEC-10201, R12-SEC-9068

## Context

R12-SEC-10201 requires that an AI agent never change production by kubectl,
SSH or console, only through GitOps or an audited runbook. R12-SEC-9068
requires that the coder agent work with sandboxed credentials and never hold a
production secret. Both rows are PARTIAL and both are blocked on owner
decision O18 (docs/blueprint/lanes/design-decisions-2026-10-18.json), which
proposed an agent Workload Identity pool with scoped per-run credentials.

The owner's second-pass ruling of 2026-10-31 (decision 14) declined O18 as
written. This record carries that ruling out; it adds no new ruling.

What exists. Shells are reachable only through IAP. The one declared agent
identity, `google_service_account.agent_identity`, holds no grants, pinned by
`the_agent_identity_holds_no_run_deploy_or_actas_role` and
`the_autonomous_agent_service_account_has_no_roles_on_capital_or_custody_resources`.
`agent-sandbox.yml` runs an agent lane with `contents: read`, no id-token, no
environment and no secret, and `scripts/agent-guard.mjs` refuses a sandbox
workflow that gains any of them. Deploying and infrastructure workflows run
`agent-guard dispatch`, which refuses an agent actor.

What is missing. In `infrastructure/terraform/modules/cicd/main.tf` the
provider's `attribute_condition` admits every `refs/heads/` ref of the
repository and maps no actor. `github_impersonation` and
`infra_impersonation` bind `principalSet` on `attribute.repository`, so any
workflow in the repository may impersonate the ci or infra account. The
model-pack publish provider in `artefact-signing.tf` has the same shape. An
agent is recognised only through `agent_markers` in `scripts/agent-policy.json`,
and that file's own comment says an unmarked agent push is invisible. The
agent branches of this programme (`ccr-*`) match no `branch_prefixes` entry.

## Decision

1. **R12-SEC-10201: build, tightening only.** No agent session receives a
   Workload Identity pool, provider or per-run cloud credential. O18 is
   declined as written, because a credential issued to an agent widens what an
   agent can reach, and the requirement is met by granting less.
2. **R12-SEC-10201: build.** The provider maps `attribute.actor`, and its
   `attribute_condition` refuses every entry of `agent_markers.actor_patterns`.
   The `refs/heads/` bound stands unchanged, consistent with R12-CICD-9103.
   The model-pack publish provider takes the same actor term.
3. **R12-SEC-10201: build.** `github_impersonation` and `infra_impersonation`
   narrow from `attribute.repository` to the named workflows that
   `dispatch_refused_for_agents` lists, so a write-capable identity is reachable
   only from a workflow that runs the dispatch judge.
4. **R12-SEC-10201: build, as a test.** An acceptance test in
   `workload_identity.rs`, named
   `every_federation_a_write_capable_account_trusts_refuses_an_agent_actor_and_no_binding_admits_the_whole_repository`,
   holds decisions 2 and 3. It is mutation-verified in two ways when built:
   remove the actor term and watch it fail, then widen one binding back to
   `attribute.repository` and watch it fail.
5. **R12-SEC-10201: stated residual, not a decision to close.** An agent that
   pushes as its person with no marker cannot be told apart from that person by
   any control in this repository. This record names that limit and does not
   claim to remove it. Closing it needs agents to push under a GitHub identity
   of their own, which only the owner can create (**owner**). Until then the row
   stays PARTIAL, and it also waits on a hosted run showing an agent-actor token
   refused.
6. **R12-SEC-9068: decline the credential, build the refusal.** The coder agent
   is given no sandboxed cloud credential at all; "sandboxed" is read as
   "grantless", which is the existing state of `agent_identity` and of
   `agent-sandbox.yml`. The requirement's forbidden action, holding a production
   secret, is enforced by decisions 1 to 4, so a run that presents an agent
   actor to a write-capable federation is refused. O18's pool is not built for
   this row either.
7. **R12-SEC-9068: tests are not yet evidence.** The three sandbox acceptance
   tests and the `agent-guard` tests are recorded in the register as not
   mutation-verified and not run. This record does not change that. The row
   stays PARTIAL until they are mutation-verified and a hosted run shows the
   refusal.

## What it costs

* Any legitimate agent task that would have used a per-run cloud credential
  must go through a pull request and the normal workflows instead. That is the
  intent, and it is slower.
* Narrowing the two impersonation bindings to named workflows means a new
  workflow that needs the ci or infra account must be added to the list in a
  reviewed change. A forgotten entry fails closed.
* The actor refusal depends on `agent_markers.actor_patterns` being complete.
  A pattern that is missing is a hole, and the list needs an owner who keeps it.
* The change alters a federation contract in Terraform. It is a tightening, but
  it is built only after a plan is shown that refuses an agent actor and admits
  a named workflow; a gate that only refuses is not shown to work.

## What would make this wrong

* If a real need for an agent to act in the cloud appears, a grantless identity
  will block it. The answer then is a new record that names the specific
  action and bounds it, not a quiet widening of this one.
* If agents come to push under their own GitHub identity, decision 5's residual
  closes and the actor term becomes the primary control rather than a partial
  one. The record should then be revisited.
* If a workflow outside the named list legitimately needs the ci or infra
  account, decision 3 is too narrow and the list, not the binding shape, should
  change.
* If the actor-pattern match proves easy to evade by renaming, the actor term
  provides less than it appears to and the residual in decision 5 is larger
  than stated.
