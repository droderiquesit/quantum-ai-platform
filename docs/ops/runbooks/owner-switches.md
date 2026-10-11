# The owner's switches

Four settings, each committed off, each turned on only by a person in a
reviewed diff followed by a plan that person reads. An agent may prepare
any of them and may not flip one: `scripts/agent-policy.json` makes every
environment's tfvars and the files declaring the switches protected paths —
an agent-authored change to them needs a person's approval of its head
commit, re-checked at deploy — and refuses an agent actor on `infra.yml`. Labels as in
[the operator runbook](../RUNBOOK.md).

## `execution_nodes_enabled` — dev's one execution node (W4.3)

**State: off.** `infrastructure/environments/dev/terraform.tfvars` declares
ADR 0035's node, `newyork-1` in `us-east4`, shadow mode, simulated venue
feed, with `boot_image = null` and `region_allocation = null`. With the flag
off the root hands the node module an empty map and the entry plans nothing
(`terraform/tests/execution-node-enable.tftest.hcl`, **ran**, 7 passed).

To turn it on, in this order:

1. Uncomment `image_bake_subnet_cidr` in dev's tfvars, run `infra.yml`
   `action=plan`, read it, `action=up`.
2. Dispatch `image.yml` for `c3-highcpu-8`. Its run prints `self_link`:
   `projects/algorik-platform-dev/global/images/qip-execution-node-c3-highcpu-8-<commit12>-r<run>`.
3. In one reviewed diff: write that self-link into `boot_image`, write the
   allocation a person chose into `region_allocation` (ADR 0045 proposes one;
   proposing is not choosing), set `execution_nodes_enabled = true`.
4. `infra.yml` `action=plan`; read that it creates one node's subnet,
   identity, grants, template and group and nothing else; `action=up`.

The plan refuses step 3 with either value null, with nothing declared, with a
zero allocation, or with an image baked for a machine shape other than the
entry's `machine_type`.

## `cloud_build_private_pool_enabled` — builds in a private pool (R2.10, CICD-9097)

**State: off.** Builds run on GitHub-hosted runners. On, `module.build_pool`
creates a Cloud Build private pool `qip-dev-build` peered into the
Engineering VPC, the build identity `qip-build-dev` (reads the staged
source, writes its saved image under `out/` in the staging bucket, nothing
else), the bucket `<project>-qip-dev-build-source`, and four grants for the
pipeline's identity, whose one write is the checkout under `source/` — it
cannot write under `out/`, so the image id the pool reports and the image
the runner loads both come from the build identity, not from the identity
comparing them. `deploy.yml` reads the same tfvars line and submits
`infrastructure/cloudbuild/build-image.yaml`, which builds twice from
nothing cached and hands back an image only if both image ids agree; the
runner loads it, refuses a different id, and pushes, scans, signs and
attests as before.

To turn it on: set it `true` in dev's tfvars (dev only — the root refuses it
elsewhere), plan, read, apply; the next `deploy.yml` run builds in the pool.
A run before the apply fails naming the pool and builds nothing on the
runner instead.

What is open, said rather than hidden: the workers have public addresses
(`no_external_ip = false`) because the build fetches its base image from
Docker Hub and its crates from crates.io, and Cloud NAT does not translate a
peered producer network; `true` needs both served from inside the project
first. **Not run**: no build has run in the pool.

## The SAST blocking threshold (R2.8, CICD-9105)

**State: on, at 7.0.** `scripts/sast-policy.json`'s `min_security_severity`
is the CodeQL security-severity at or above which a finding fails the `sast`
job, and so ci, and so `deploy.yml`'s gate. 7.0 is CodeQL's "high" band, the
band the Trivy gates stop on. There is no input or override; changing it is
an edit to that file, which is a protected path.

## Require `agent boundary` before merge — branch protection (GOV-9067, ADR 0117)

**State: not set by this repository, and it cannot be.** Branch protection
is a repository setting; no file in this tree creates it, and no agent may
change it. Until it is set, nothing stops an agent-authored pull request
that touches a protected path from being *merged* without a person's
approval — `deploy.yml`'s gate stops it from being *deployed*, judging every
commit since the last deployment, but a merged change already sits on `main`
for every later branch to build on.

What the check refuses, once required: a pull request whose commits carry an
agent marker and touch a protected path, unless a reviewer whose repository
permission is write, maintain or admin approved its **head commit**; a commit
declaring an `Agent-Role` outside that role's paths; and a protected change
with no agent marker unless every commit is signed by a person's registered
key or the head is approved the same way. It re-runs on every review, so an
approval turns it green without a push.

To turn it on — a person, in the GitHub settings, not an agent:

1. Settings → Rules → Rulesets → New branch ruleset (or Branches → branch
   protection rule) targeting `main`.
2. Require a pull request before merging, with at least one approval, and
   dismiss stale approvals when new commits are pushed.
3. Require status checks to pass, and add **`agent boundary`** (the job name
   in `.github/workflows/agent-boundary.yml`), alongside ci's checks.
4. Do not allow bypassing the above, including for administrators, and do not
   add any agent identity or GitHub App used by agent sessions to the bypass
   list.
5. Leave the merge queue off unless `agent-boundary.yml` gains a
   `merge_group` trigger: as written it judges pull requests, and a required
   check that never reports on a merge-group ref blocks every queued merge.

Signing your own commits (so your unmarked changes to protected paths need no
second approver): `git config commit.gpgsign true` with an SSH or GPG key
added to your GitHub account as a **signing** key. **Not verified here**:
whether the setting is on is visible only in the repository settings.
