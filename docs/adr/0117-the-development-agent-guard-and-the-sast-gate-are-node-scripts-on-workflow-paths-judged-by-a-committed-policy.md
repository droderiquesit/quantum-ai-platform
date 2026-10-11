# ADR 0117: The development-agent guard and the SAST gate are Node scripts on workflow paths, judged by a committed policy

- **Status**: **Accepted**, 2026-10-11, by the owner in the session
  conversation, under the instruction to adopt the recommended answer for
  every open decision: "Update as needed remove my decisions make the
  recommended".
- **Date**: 2026-10-11
- **Requirements**: R12 GOV-9067 (a planner agent cannot merge or deploy;
  agents recognised), SEC-9068 (a coder agent holds no production secret),
  SEC-9077 (agent sandboxes are egress-limited and under quota), SEC-9078
  (one policy-enforcing gateway to models; a direct call fails), SEC-9084
  (agent sandboxes hold no production credential), CICD-9076/9082/9083 (lane
  plans, acceptance, budgets, ownership), CICD-9105 (a CodeQL finding blocks
  a deploy).
- **Owes this ADR**: `blueprint_rules.rs`'s accepted non-Rust tooling list,
  which said of `scripts/agent-guard.mjs` and `scripts/sast-gate.mjs` that a
  file on a workflow path owes an ADR and none was written. This is it.
- **Implemented by**: `scripts/agent-policy.json`, `scripts/agent-guard.mjs`,
  `scripts/agent-lanes.mjs`, `scripts/agent-egress.mjs`,
  `scripts/sast-policy.json`, `scripts/sast-gate.mjs`, their `*.test.mjs`
  (CI's `scripts` job), `.github/workflows/agent-boundary.yml`,
  `.github/workflows/agent-sandbox.yml`, `deploy.yml`'s gate step, and
  `qip-acceptance/tests/model_gateway_boundary.rs`.

## Context

Blueprint §2.1 rule 1 says every application is Rust. Two programs this
repository now depends on to decide what may merge and deploy are not:

- **The agent guard** (`agent-guard.mjs`, with `agent-lanes.mjs` and
  `agent-egress.mjs` beside it) runs as the first step of every workflow that
  deploys, applies or reaches a credential, sparse-checked from the default
  branch, before anything authenticates. A guard that first had to compile
  the workspace would run after the steps it exists to precede, and a
  toolchain install is itself a step that runs code nobody judged yet. Node
  is on every hosted runner; these scripts use its built-ins only, so nothing
  is installed.
- **The SAST gate** (`sast-gate.mjs`) reads the SARIF CodeQL wrote and fails
  the `sast` job on a high finding. That job installs no Rust toolchain, and
  the gate must judge the analysis in the job that ran it, or "the analysis
  succeeded" goes on meaning "it ran" rather than "it found nothing".

Both are on workflow paths, so — as for `terraform-undeletable.py` — the
deviation is load-bearing and needs a recorded decision, not a comment.

Three of the requirements they serve were also weaker than their rows
claimed, and this ADR closes what it can of each:

- **SEC-9077.** The sandbox held no secret and could write nothing back, but
  a lane's acceptance command could still open a connection to anywhere: post
  the checkout, pull an unreviewed tool, or call a model with a key carried in
  its own diff.
- **SEC-9078.** The platform reaches a model through one port behind the cost
  router, but nothing failed if a crate grew a second route.
- **GOV-9067.** Agents were recognised only by markers. An agent pushing as
  its user with none was a person to every check, on every path.

## Decision

### 1. One committed policy, read from the default branch

`scripts/agent-policy.json` is the single rule file: agent markers, the role
table (equal to every `.claude/agents/*.md` "Paths you may change", tested),
protected paths, workflows no agent may start, lane ceilings
(`max_minutes` 360, `max_usd` 50, `max_tokens` 2,000,000), the sandbox's
egress allow-list and quotas (4,000 tunnels, 4 GiB), and the unmarked
protected-change rule. `scripts/sast-policy.json` holds the SAST threshold:
`min_security_severity` 7.0, CodeQL's high band, the band the Trivy gates
stop on. Every workflow reads both from the default branch, never from the
change being judged. There is no input, flag or repository variable that
overrides either; editing them is a protected change.

### 2. What the guard refuses

- An agent-authored change outside the role its `Agent-Role:` trailer
  declares, or naming a role the policy does not define.
- An agent-authored change touching a protected path without a person's
  approval **of the head commit**. A signature does not stand in for it.
- **New:** a change with **no** agent marker touching a protected path,
  unless every non-merge commit carries a signature GitHub verified against a
  key a person registered — not GitHub's web-flow key, which signs every API
  and web-editor commit, an agent's included — or a person approved the head
  commit. No marker is no longer proof of a person where it matters most.
- **New:** an agent committer under a person's author name (the committer
  is now read as well as the author).
- An agent actor dispatching `deploy.yml`, `infra.yml`, `fleet.yml`,
  `image.yml`, `vendor.yml` or `branch-cleanup.yml`.
- A sandbox workflow that references a secret, can mint an OIDC token, names
  an environment, grants a write, runs on `pull_request_target`, has an
  untimed job, or — **new** — leaves the lane's network open in any one of
  five ways (no group firewall, no `no_new_privs`, the lane not run under
  the firewalled group, no egress proxy, no egress verdict).
- A lane plan with no acceptance command, a budget missing or over a ceiling
  (minutes, USD and — **new** — tokens), a cycle, a path its role may not
  edit, or two lanes owning one boundary unordered; a lane's diff outside
  what it owns; a command past the lane's minutes; and — **new** — a lane
  whose model-gateway ledger shows more tokens or USD than its budget, or a
  billed line whose spend cannot be read, which is refused rather than
  counted as zero.

### 3. The sandbox's one way out (SEC-9077, SEC-9078's development half)

`agent-sandbox.yml` runs the lane's commands under the group `lane-egress`
with `no_new_privs` and an empty supplementary group list. The runner's
firewall lets that group reach loopback and nothing else, DNS included.
`agent-egress.mjs`, run as root from a root-owned copy, is the one route
off the machine: HTTPS `CONNECT` on 443 to a host on `sandbox.egress_allow`
(five package-registry hosts), within the request and byte quotas, no plain
HTTP, no IP literal, and no model provider — a test fails if one is listed.
A refused name is never resolved. The verdict is taken in the same step,
after every process in the group is killed, by root-owned node and scripts
reading a root-owned log; any refusal fails the run. Model spend inside the
sandbox is therefore structurally zero, and the lane's authoring spend is
checked against its budget by `agent-lanes.mjs spend` on the gateway ledger.

### 4. One route to a model from platform code (SEC-9078)

`model_gateway_boundary.rs` holds the path the tree already takes: the one
hosted adapter (`qip-reasoning-engine`'s `HuggingFaceModel`, ADR 0037) is
the only production code naming a model provider's host or chat path, beside
the Vertex training control plane (ADR 0083) and the composition root's
refusal text; only it and `qip-ai`'s own module implement `LanguageModel`;
only `qip-ai` and `AgentRuntime::complete` — which checks the
`CallLanguageModel` grant and charges the token budget first — invoke a
model; only `qip-deepbrain`'s root constructs the adapter; and the kernel
convenes the organisation exactly once, in the REASON stage, after
`qip-cost-router` has placed the decision. No violating path existed; the
test is what keeps it so.

## Known blind spots

- **An agent on its user's machine with its user's signing key signs as that
  person.** Signing proves the key, not the hand. The durable fix is a
  distinct GitHub identity for agent sessions whose token cannot dispatch a
  deploying workflow — a repository-settings act, not a file here.
- **Outside the protected paths, an unmarked agent is still a person.** The
  role table binds only commits that declare a role.
- **A merge commit is exempt from the signature rule**, because the merge
  button writes one with the web-flow key. A merge commit crafted through the
  git data API with content of its own passes as a merge.
- **Whether `agent boundary` is a required check** is branch protection,
  which this tree cannot set; `deploy.yml`'s re-check holds without it.
- **An admitted tunnel is opaque.** TLS is end to end, and every allowed
  registry accepts uploads with a credential; the sandbox holds none, but a
  lane can carry one in its own diff. The list is kept to registries for that
  reason.
- **Loopback is open to the lane**, because its tests bind ephemeral ports.
  A local service on the runner is reachable.
- **Only what the sandbox can see is metered.** The agent session's own
  model use outside the gateway is not in any ledger here.
- **The model-route scans read text.** A URL assembled at run time, or the
  generic HTTP client pointed at a configured address, names no provider;
  `egress.rs`'s check on the proxy's routes is what bounds where a configured
  address leads.
- **The SAST gate trusts CodeQL's severities** and blocks nothing a rule
  leaves unscored.

## What would make this wrong

- A Rust toolchain on every runner before the first step, cheap enough that
  the guard could be a compiled binary checked out from the default branch:
  then §2.1 should win and these scripts be rewritten.
- GitHub offering per-identity agent tokens or required commit attestation
  that names the agent: then the marker and signature rules are a weaker
  copy of a platform feature and should yield to it.
- A lane that legitimately needs a host off the list. Add the host to the
  policy, in a protected change a person approves — never widen the firewall.
- A measured false-positive rate at 7.0 that makes people suppress findings
  rather than fix them: then the threshold is wrong, and the fix is a
  reviewed edit to `sast-policy.json`, not an override.

## What it costs

The non-Rust tooling set grows by `agent-egress.mjs` and its test, recorded
in `blueprint_rules.rs` against this ADR. A person's unsigned commit to a
protected path now needs another person's approval or a registered signing
key; this costs a solo maintainer a one-time `git config commit.gpgsign` with
a key added to GitHub as a signing key.
