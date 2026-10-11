# The owner's switches

The settings below, each committed off, each turned on only by a person in a
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
pipeline's identity. `deploy.yml` reads the same tfvars line and submits
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

## Branch protection on `main` (R12 CICD-9066, CICD-9095)

**State: applied by hand, or not at all — this repository cannot see it.**
`recover/l016-cicd-037` declared it as a `github_branch_protection` resource
with the `integrations/github` provider. That was not ported: the provider
needs a GitHub token in whichever job plans it, and this repository's
workflows hold no such credential and may not take one from a repository
variable. So the settings are recorded here for the owner to apply in
*Settings → Branches* (or *Rules → Rulesets*) on `main`:

- **Require a pull request before merging**, with one approving review,
  **dismiss stale approvals on new commits**, and **require review from code
  owners** (`.github/CODEOWNERS` scales the reviewers by path risk).
- **Require status checks to pass**, branches up to date, naming ci's jobs
  as GitHub reports them: `format`, `clippy`, `test`, `attestation check`,
  `release build`, `dependency policy`, `security audit`, `cargo-deny`,
  `sbom`, `portal`, `landing`, `vulnerability scan`, `trunk meta-linter`,
  `infrastructure`, `secret scan`, `scripts`, `a changed rule is cited`,
  `change management hooks`, and every `sast / …` job. The recovered branch
  named `build / lint`, `build / clippy` and others that no workflow
  reports; a required check nobody reports blocks every merge for ever, so
  copy names from a recent run's checks list rather than from this page if
  the two disagree.
- **Do not allow bypassing** (include administrators), **no force pushes**,
  **no deletions**.

What holds without it, said so a reader does not overcount it:
`deploy.yml` refuses to sign a commit whose ci run was not a push to this
repository, or — dispatched — one with no successful push-started ci run
(`qip-acceptance`'s `deploy_provenance` suite runs that step), and the
workload identity provider admits only `refs/heads/` tokens from this
repository. Those keep an unreviewed pull request off a deployed
environment; only branch protection keeps one off `main`.
