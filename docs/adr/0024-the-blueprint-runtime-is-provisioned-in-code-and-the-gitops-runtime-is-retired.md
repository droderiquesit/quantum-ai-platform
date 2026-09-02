# 0024 — The blueprint runtime is provisioned in code and the GitOps runtime is retired

**Status:** accepted
**Supersedes:** ADR 0011 (the "on Kubernetes" half) and ADR 0017 entirely.
Amends ADR 0020: its sequence was executed in code rather than step by step,
and the correction appended to that record says how.

## The authorisation

ADR 0022 made the Algorik blueprint the architecture of record and said, in
terms, that the decision authorised no execution: every step of ADR 0020's
migration sequence still needed recorded human approval naming the step.

The owner then gave that instruction. Verbatim:

> create the new infrastructure while devouring the old

That sentence is the authorisation for the direction of this record and for
every code change it describes. It is quoted rather than paraphrased so that a
reader can judge for themselves what it covers, and what it covers is the
code: the Terraform that provisions the blueprint's runtime, the workflow
that deploys to it, the removal of the chart and the controllers, and the
acceptance suites and documents that describe the result.

It does not cover an apply, and nobody has read it as one. See "Nothing was
applied" below.

## Decision

1. **The blueprint's runtime is provisioned by this repository's Terraform.**
   Every warm binary is a Cloud Run service from
   `infrastructure/terraform/catalogue.tf` through `modules/cloudrun` —
   internal ingress, secrets mounted as volumes and never as environment
   values, an image pinned by the digest `deploy.yml` attested. The execution
   node is one Compute Engine C3 per region from `modules/execution-node`,
   running `qip-edge-node` bare under systemd with no container runtime and
   no external address, in shadow mode by a literal in `main.tf` that no
   tfvars value can turn off. The thirteen trust zones of blueprint §46.1 are
   `modules/trust-zones`, default deny in both directions. The egress path is
   `modules/egress-proxy`.

2. **The GitOps runtime is retired.** The Helm chart, the rendered manifests,
   Argo CD, Kargo, KEDA and cert-manager are gone from the tree, and the
   acceptance suite refuses their return:
   `no_kubernetes_manifest_helm_chart_or_gitops_controller_remains` fails on a
   manifest directory, a chart, a controller, a workflow running their tooling
   or a GKE resource in the Terraform.

3. **Delivery is the pipeline moving the service and proving it.**
   `deploy.yml` builds, scans, signs and attests each image, then moves each
   Cloud Run service to the attested digest with `gcloud run services
   update`, which blocks until the revision is Ready, reads the serving
   revision back, and fails unless it carries the digest that was signed. The
   digest is recorded in `infrastructure/environments/<env>/images.tfvars`
   only after that proof. This closes the gap the GitOps cut-over left and
   `docs/operations/gitops-exceptions.md` recorded: a build that crash-looped
   on boot used to produce a green pipeline.

4. **The egress proxy on Cloud Run is the first proxy this platform has ever
   had.** ADR 0020's corrections record that the chart's Envoy `Deployment`
   was committed commented out, so no proxy pod ever existed and every
   outbound adapter refused at construction in every deployment. The proxy is
   now a loopback sidecar beside the API and the deep brain, a systemd unit
   beside the execution node, and deliberately absent from the fast brain,
   all rendered from the one committed bootstrap at
   `infrastructure/egress/envoy.yaml` and gated at plan time against the
   allowlist the root declares. It will exist when the first plan is applied;
   it did not exist before this record on any runtime.

5. **The paper-trading boundary is untouched.** The three layers ADR 0021
   names are exactly as they were: the plan-time refusal of the three live
   rungs in `infrastructure/terraform/variables.tf`, `AutonomyLevel::deployable`
   in the three composition roots, and `Cell::new` taking no ceiling but paper
   trading. Every catalogue workload takes its ceiling from the one root
   variable and never from a literal; the node sets none, because a cell's
   ceiling is structural. The venue credential's IAM grant still exists only
   at a ceiling no plan can carry, and the node's venue egress rule does not
   exist while the node is in shadow mode. A migration was never a reason to
   reopen any of this, and none of it was reopened.

## What each committed change is, against ADR 0020's sequence

ADR 0020 fixed a five-step order and required a named approval per step.
The owner's instruction executed the sequence in code, so the steps map onto
commits rather than onto approvals. What each did, and which step it is:

| Commit | What it did | ADR 0020 step |
|---|---|---|
| `8c73610`, `b6cca79`, `6cde5d7` | Wrote `modules/cloudrun`, `modules/trust-zones` and `modules/execution-node`, each unwired, before the instruction | Preparation for 2, 3 and 4; ADR 0020's own correction records them as unwired |
| `c924191` | `modules/egress-proxy`: the egress path off GKE that ADR 0020's correction named as an unstated precondition of steps 2 and 5 | The precondition |
| `808ca32` | Wired the catalogue, the execution node and the trust zones into the root module; removed the cluster's Terraform, its workload identities, its node pools and its backup plan | 2 and 4 for all three warm services at once; 3 in code with `execution_nodes` empty in every environment; 5's Terraform half |
| `b85684f` | Retargeted `deploy.yml` at Cloud Run with the readiness proof; targeted `infra.yml`'s `down` at the execution nodes alone | 5's delivery half: the Kargo promotion and the Argo CD sync replaced by the pipeline |
| `67b3e92`, `7d79161` | Deleted `infrastructure/gitops/**`, `infrastructure/helm/**` and `infrastructure/kubernetes/base/**` | 5's artefact removal — see the history note |
| `81dd1cd` | Retargeted the infrastructure, manifest-wiring and egress acceptance suites at the new artefacts, keeping every property and adding the four this record relies on | The evidence that each step kept what it moved |
| `ecfb0a6` | Corrected the rules, runbooks, threat model and `infrastructure/CLAUDE.md` that still described the retired runtime | Neither topology may become undocumented |

Step 1 — establish which GKE workloads ever actually ran — was not done, and
its evidence was never gathered. Under the owner's instruction it did not
need to be for the code: the cluster's Terraform was removed whether or not
anything ran on it. It still matters for the apply, because a project that
has a live cluster the state file knows about will plan its destruction, and
that plan is a human's to read.

**ADR 0020's per-step approval rule is superseded for the code by the
owner's instruction, and stands unchanged for the apply.** That is the whole
of the amendment. "Evidence earns a conversation, not a machine" was written
about provisioning compute, and no compute has been provisioned.

## A history note, so `git log` makes sense

The deletion of the chart, the manifests and the GitOps controllers landed
in `67b3e92` and `7d79161`, two commits whose subjects are about the kernel's
bridge transfers and cell reports. That was a mistake in staging by the
agent doing the infrastructure work: the deletions were in the working tree
when a kernel agent committed, and were swept in. History is not rewritten
in this repository — a merge commit keeps other checkouts valid and a
rewrite does not — so the record stays as it is and this paragraph explains
it. A reader who finds a Helm chart removed by a commit about bridge
reorganisations has found the right commit and should read this record
rather than the subject line.

## Nothing was applied

No plan has been produced and nothing has been applied, by this record's
changes or by any earlier one on this branch. On the machine that made every
commit above:

```
$ which terraform helm gcloud kubectl
terraform: not found
helm: not found
gcloud: not found
kubectl: not found
```

There is no Terraform binary, so **`terraform fmt -check` and `terraform
validate` were NOT RUN** for any of these commits. What ran in their place is
a structural pass in Python over every `.tf` file: balanced braces, every
`module` source directory present, every `var.` reference declared in its
directory, every module argument declared by the module and every required
one passed, every tfvars key a root variable. It proves less than `validate`
would — no provider schema, no type checking — and the first `terraform
init` may find something it did not.

There is no `gcloud` and no `kubectl`, so nothing could have reached a
project, a cluster or a Cloud Run service. The cluster's removal from the
Terraform is a removal from the configuration, not from any project.

**A real apply still needs a plan shown to a human.** `infra.yml`'s `up`
is a manual dispatch that prints the plan before applying; it refuses
`prod`; its `down` is targeted at `module.execution_node` alone and refuses
an untargeted destroy. The first plan against a project that ran the GKE
runtime will propose destroying that runtime, and the person who reads it
decides. That is where ADR 0020's approval rule still applies, unweakened.

## What it costs

The runbooks. Five of them — deploying a cell, disaster recovery,
multi-region, scaling and availability, enabling live trading — were written
for StatefulSets, node pools, KEDA and the ConfigMap. Each now carries a
banner saying so and what replaced the thing it describes; none has been
rewritten. The acceptance suite keeps their names on a list that expires
entry by entry, so the debt is visible, but an operator opening one at three
in the morning will find a banner and then a procedure that no longer
applies.

Four scripts — `scripts/bootstrap-gitops.sh`, `scripts/verify-argocd.sh`,
`scripts/bootstrap-kargo-admin.sh`, `scripts/open-consoles.sh` — still
install, verify and open things that no longer exist. They are on the same
list. `.claude/agents/sre-release-engineer.md` still names
`infrastructure/kubernetes/**` as a path it may change.

The centre-to-node path is unwired. The in-tree mesh binds one listener per
cell on its own port, and a Cloud Run service publishes exactly one, so
`QIP_MESH_CELLS` is deliberately unset on the API and `QIP_MESH_PEER` on the
node; the API answers `available: false` and a node would start detached.
The blueprint's control fabric is Pub/Sub (§46.1), and building it is work
this record names and does not do.

Nothing scrapes a Cloud Run service. The node's Ops Agent receiver is
declared; the services have no collector, and
`modules/observability/NOT-SCRAPED.md` says what one would be. All seven
alert policies stay gated on `workload_metrics_exist = false` in every
environment until a scrape is a fact.

`execution_nodes` is empty in every environment, because a node needs a
venue and no venue decision exists. Step 3 is therefore done in code and not
in the world, and the blueprint's central topological claim is still unproven
by observation.

And the largest cost is the one every ADR since 0020 has paid: this
repository now has one runtime in its Terraform and none observed. Until a
plan is read and applied by a person, "the platform runs on Cloud Run" is a
statement about a configuration, and the honest sentence is that the
platform is not running anywhere.

## What would make this wrong

- **A plan showing the configuration does not do what this record says.**
  The Python pass is not `validate`, and `validate` is not `plan`. If the
  first real plan refuses on a provider-schema error, proposes a resource
  this record does not describe, or fails the egress module's preconditions
  against the committed bootstrap, this record overstated what was proven
  and needs a correction naming the line.
- **The GKE project turning out to carry traffic.** Step 1 was skipped. If
  the project's cluster is serving something the platform depends on, the
  Terraform removal in `808ca32` was premature, and the plan that would
  destroy it must not be applied until that something is moved.
- **Anyone citing this record as approval to apply.** It is approval for
  the code and says so. An agent that applies on its strength has misread
  the one paragraph this record spends most of its length on.
- **Any live-order or live-transfer path appearing** on the reasoning that
  the runtime is now the blueprint's. ADR 0021 and ADR 0023 govern that, and
  a runtime change is not an occasion to revisit them.
