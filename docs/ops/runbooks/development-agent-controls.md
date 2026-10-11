# Development-agent controls

R12 GOV-9067, SEC-9068, SEC-9077, SEC-9078, SEC-9084, CICD-9076, CICD-9082,
CICD-9083 (W6.7); decision recorded in
[ADR 0117](../../adr/0117-the-development-agent-guard-and-the-sast-gate-are-node-scripts-on-workflow-paths-judged-by-a-committed-policy.md). What CI enforces about the agents that write this
repository, where each rule lives, and what none of it can see. Labels as in
[the operator runbook](../RUNBOOK.md).

## The pieces

| Piece | What it refuses |
|---|---|
| `scripts/agent-policy.json` | The rules: agent markers, the role table (the agents' own "Paths you may change"), protected paths, the unmarked protected-change rule, the workflows no agent may start, lane budget ceilings (minutes, USD, tokens), and the sandbox's egress allow-list and quotas |
| `scripts/agent-guard.mjs change` | An agent-authored change outside the role its `Agent-Role:` trailer declares; an unknown role; an agent-authored change touching a protected path without a person's approval **of the head commit**; a change with **no** agent marker touching a protected path unless every non-merge commit is signed by a person's registered key (not GitHub's web-flow key) or a person approved the head commit. The committer is read as well as the author |
| `scripts/agent-guard.mjs dispatch` | An agent actor starting `deploy.yml`, `infra.yml`, `fleet.yml`, `image.yml`, `vendor.yml` or `branch-cleanup.yml`. Each runs it as its first step, from the default branch, before it authenticates |
| `scripts/agent-guard.mjs sandbox` | A sandbox workflow that references a secret, can mint an OIDC token, names an environment, grants any write, runs on `pull_request_target`, has a job with no time limit, or leaves the lane's network open (no group firewall, no `no_new_privs`, lane not under the firewalled group, no egress proxy, no egress verdict) |
| `scripts/agent-lanes.mjs` | A lane plan with no acceptance command, no or an over-ceiling budget (minutes, USD, tokens), a cycle, a path the lane's role may not edit, or two lanes owning one boundary with no order between them; a lane's diff outside what it owns; an acceptance command running past the lane's minutes (killed); `spend`: a lane whose model-gateway ledger shows more tokens or USD than its budget, or a billed line whose spend cannot be read |
| `scripts/agent-egress.mjs` | The sandbox's one way out: anything but HTTPS `CONNECT` on 443 to a host on `sandbox.egress_allow` (no model provider is listed), past 4,000 tunnels or 4 GiB; `verdict` fails the run on any refusal or a missing log |
| `.github/workflows/agent-boundary.yml` | Runs `change` on every pull request and every review of one |
| `deploy.yml`, gate job | Runs `change` again on the commit being deployed, against the pull request it was merged through — so the boundary holds whether or not branch protection requires `agent boundary` |
| `.github/workflows/agent-sandbox.yml` | Runs one lane's acceptance from a manifest on the default branch, within the lane's budget, with `contents: read` and nothing else, under the `lane-egress` group the runner's firewall confines to loopback (DNS included) with `no_new_privs`, so the root-run egress proxy is its only route out; the verdict is taken in the same step after the group's processes are killed. ci's `scripts` job holds it to the sandbox rule |
| `qip-acceptance/tests/model_gateway_boundary.rs` | Platform code naming a model provider outside the declared adapter, a second `LanguageModel` implementation, a model invoked anywhere but `AgentRuntime::complete` (grant and budget first), the hosted adapter built outside the composition root, or the organisation convened before `qip-cost-router` places the decision (SEC-9078) |

All of it is tested by `node --test scripts/agent-guard.test.mjs
scripts/agent-lanes.test.mjs scripts/agent-egress.test.mjs` (CI's `scripts`
job) and `cargo test -p qip-acceptance --test model_gateway_boundary`, including a test that the
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

## Signing your own protected changes

A change you push yourself, with no agent marker, that touches a protected
path needs either a person's approval of its head or every commit signed by a
key registered on your GitHub account as a **signing** key. GitHub's own
web-flow signature — on every commit made through the API or the web editor —
does not count, because an agent working through your token gets it too. A
one-time setup: `git config commit.gpgsign true` with an SSH or GPG key you
have added to GitHub as a signing key. The merge button's own commit is
exempt.

## Approving a protected change

A person reviews and approves the pull request's **current head**. A push
after the approval needs a fresh approval; a later plain comment does not
withdraw one; a request for changes does. Approvals by a `[bot]` account or a
login matching the agent patterns do not count.

## What none of this can see — read before relying on it

- **An agent indistinguishable from its user, off the protected paths.**
  Every check keys on the markers in `agent_markers`: author or committer
  name or email, the co-author trailer, a `claude/` or `worktree-agent-`
  branch, and actor patterns (`*[bot]`, `claude*`). On a protected path, no
  marker is no longer enough — a person's signature or approval is required.
  Elsewhere, an agent that commits under its user's identity with no trailer
  is a person, and one dispatching with its user's token is a person to the
  dispatch check. An agent running on its user's machine with its user's
  signing key signs as that person. The durable fix is a distinct GitHub
  identity for agent sessions (a GitHub App or machine user) whose token
  cannot dispatch deploying workflows; that is a repository-settings act,
  not a file in this tree.
- **Whether `agent boundary` is a required check.** Branch protection is a
  setting this repository cannot commit. `deploy.yml`'s re-check is the half
  that holds without it: an unapproved protected change can be merged, and
  then cannot be deployed.
- **Merge commits** are exempt from the signature rule; one crafted through
  the git data API with content of its own passes as a merge.
- **Model access (SEC-9078).** Platform code: one route, pinned by
  `model_gateway_boundary.rs`; its scans read text, so a URL assembled at run
  time names no provider and `egress.rs` bounds where a configured address
  leads. Development agents: a lane in the sandbox cannot reach any model
  host. Outside the sandbox, `scripts/model-gateway.mjs` is the gateway the
  fleet worker calls through and nothing makes a direct provider call from a
  developer's machine fail. **Not enforced there.**
- **Egress and quota (SEC-9077).** Enforced in `agent-sandbox.yml` as above.
  An admitted tunnel is opaque and every allowed registry accepts uploads
  with a credential, which the sandbox holds none of but a lane's diff could
  carry; loopback stays open because tests bind ephemeral ports. The fleet's
  Cloud Run Job keeps its fixed, tested grants and its token/USD ceilings
  (ADR 0102), but its egress is not policy-limited. **Not enforced there.**
- **Session spend.** `agent-lanes.mjs spend` reads the gateway ledger; an
  agent session's own model use outside that gateway is in no ledger here.
