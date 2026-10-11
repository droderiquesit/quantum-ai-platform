# Development-agent controls

R12 GOV-9067, SEC-9068, SEC-9077, SEC-9078, SEC-9084, CICD-9076, CICD-9082,
CICD-9083 (W6.7). What CI enforces about the agents that write this
repository, where each rule lives, and what none of it can see. Labels as in
[the operator runbook](../RUNBOOK.md).

## The pieces

| Piece | What it refuses |
|---|---|
| `scripts/agent-policy.json` | The rules: agent markers, the role table (the agents' own "Paths you may change"), protected paths, the workflows no agent may start, and lane budget ceilings |
| `scripts/agent-guard.mjs change` | An agent-authored change outside the role its `Agent-Role:` trailer declares; an unknown role; an agent-authored change touching a protected path without a person's approval **of the head commit** |
| `scripts/agent-guard.mjs dispatch` | An agent actor starting `deploy.yml`, `infra.yml`, `fleet.yml`, `image.yml`, `vendor.yml` or `branch-cleanup.yml`. Each runs it as its first step, from the default branch, before it authenticates |
| `scripts/agent-guard.mjs sandbox` | A sandbox workflow that references a secret, can mint an OIDC token, names an environment, grants any write, runs on `pull_request_target`, or has a job with no time limit |
| `scripts/agent-lanes.mjs` | A lane plan with no acceptance command, no or an over-ceiling budget, a cycle, a path the lane's role may not edit, or two lanes owning one boundary with no order between them; a lane's diff outside what it owns; an acceptance command running past the lane's minutes (killed) |
| `.github/workflows/agent-boundary.yml` | Runs `change` on every pull request and every review of one |
| `deploy.yml`, gate job | Runs `change` again on the commit being deployed, against the pull request it was merged through — so the boundary holds whether or not branch protection requires `agent boundary` |
| `.github/workflows/agent-sandbox.yml` | Runs one lane's acceptance from a manifest on the default branch, within the lane's budget, with `contents: read` and nothing else; ci's `scripts` job holds it to the sandbox rule |

All of it is tested by `node --test scripts/agent-guard.test.mjs
scripts/agent-lanes.test.mjs` (CI's `scripts` job), including a test that the
policy's role table equals every `.claude/agents/*.md` "Paths you may change"
line, so neither can widen alone.

## Marking agent work

An agent's commit carries `Co-Authored-By: … <noreply@anthropic.com>` (the
marker this repository's agents already write) and, from now on,
`Agent-Role: <role>` naming the `.claude/agents/` file it runs as. A lane
manifest lists the lane's role, its owned paths, its dependencies, its
acceptance commands and its budget — see `scripts/agent-lanes.mjs`'s header
for the shape. Commit the manifest to the default branch before dispatching
`agent-sandbox.yml` against a lane branch.

## Approving a protected change

A person reviews and approves the pull request's **current head**. A push
after the approval needs a fresh approval; a later plain comment does not
withdraw one; a request for changes does. Approvals by a `[bot]` account or a
login matching the agent patterns do not count.

## What none of this can see — read before relying on it

- **An agent indistinguishable from its user.** Every check keys on the
  markers in `agent_markers`: author name or email, the co-author trailer, a
  `claude/` or `worktree-agent-` branch, and actor patterns (`*[bot]`,
  `claude*`). An agent that commits under its user's identity with no trailer
  on an ordinary branch, or dispatches a workflow with its user's token, is a
  person to every check here. The durable fix is a distinct GitHub identity
  for agent sessions (a GitHub App or machine user) whose token cannot
  dispatch deploying workflows at all; that is a repository-settings act, not
  a file in this tree.
- **Whether `agent boundary` is a required check.** Branch protection is a
  setting this repository cannot commit. `deploy.yml`'s re-check is the half
  that holds without it: an unapproved protected agent change can be merged,
  and then cannot be deployed.
- **Model access (SEC-9078).** `scripts/model-gateway.mjs` is the one
  gateway the fleet worker calls through, and it screens and records; nothing
  here makes a direct provider call from a developer's machine fail. **Not
  enforced.**
- **Egress and quota for the fleet's Cloud Run Job (SEC-9077).** The worker
  identity's grants are fixed and tested (`infrastructure/fleet/tests/`), it
  holds no Secret Manager, KMS, registry-write or data-store grant, and the
  fleet refuses prod; its egress is not policy-limited. **Not enforced.**
