# 0017 — Delivery becomes GitOps: a Helm chart, Argo CD, and Kargo

**Status:** accepted

## Decision

The cluster stops being pushed to and starts pulling. Three pieces, each
with one job:

- **A Helm chart** (`infrastructure/helm/qip`) replaces the sed-rendered
  manifests in `infrastructure/kubernetes/base/`. Every value the templates
  need is declared `required`, which keeps the property the placeholder
  guard enforced — nothing reaches the cluster with a hole in it — as a
  property of the templating engine rather than of a grep.
- **Argo CD** runs in the cluster and continuously reconciles it against
  the chart at a git revision. The desired state of every environment is a
  committed values file; the only writer to the cluster is the operator
  that reads git. This is also what the desk's v4 architecture already
  names for the warm tier ("Argo CD — GitOps, syncs desired state").
- **Kargo** turns promotion into declared pipeline stages. A Warehouse
  watches the registry for image digests that CI has built, scanned,
  signed and attested; Stages promote that freight dev → test → stage by
  writing the environment's values file back to git, where Argo picks it
  up. Production is a stage a human promotes; nothing promotes it
  automatically, which keeps the standing rule that prod needs a person.

What CI keeps is everything that makes an artefact trustworthy: build,
Trivy scan, push, Binary Authorization signing and attestation, all under
keyless Workload Identity Federation. What CI loses is `kubectl` — the
credential that could rewrite the cluster disappears from the pipeline,
and with it the class of half-applied deployments a dropped runner leaves
behind. Binary Authorization stays the admission gate either way: Argo
syncing an unattested image gets the same refusal a kubectl apply did.

## Why now

The push pipeline works, but every failure this week showed its shape: a
runner outside the VPC pushing state through a gate it can only see at
apply time. A reconciler inside the cluster inverts that — drift is
detected continuously rather than at the next deploy, the audit trail of
"what ran when" is the git history plus Argo's sync record, and a rollback
is a revert.

## What it costs

Three more controllers in the hardened cluster (Argo CD, Kargo, and
cert-manager, which Kargo requires), each a pinned version someone must
upgrade deliberately. The default-deny network posture must grow explicit
allowances: Argo CD needs egress to GitHub and the DNS control-plane
endpoint's front door; Kargo needs the registry. The acceptance suite's
workflow assertions — which today map each live gcloud command in
deploy.yml to the IAM grant permitting it — must be rewritten in the same
commit that removes the commands, or they become tests of a pipeline that
no longer exists. And the migration itself runs both paths briefly: the
chart is introduced beside the manifests, proven equivalent by rendered
diff, and only then does the kubectl path retire — carrying two copies of
the truth for that window is the price of never having zero.

## What would make this wrong

If the desk never operates more than one environment, Kargo is a
promotion engine with nothing to promote and should come back out. If
Argo CD's egress allowance ever widens beyond git and the control plane,
the reconciler has become the attack path the default-deny posture
exists to prevent. And if the chart's values ever stop being `required`,
the placeholder guard's property has been silently traded away.
