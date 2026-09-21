# modules/iap-run

Identity-Aware Proxy on a Cloud Run service itself. No load balancer, no
reserved address, no managed certificate, no Cloud Armor policy, no DNS zone,
no registrar. ADR 0095.

The console is reached at the URL Google issued it. **Read that URL off the
service; do not paste the derived one.**

```
gh workflow run infra.yml -f environment=dev -f action=diagnose   # then read `url:`
```

`terraform output console_front_door` prints a *derivation* —
`https://qip-dev-portal-95200532413.us-east4.run.app`, the service name, the
project number and the region, which is the form Google documents for Cloud
Run services created since 2024. **In `algorik-dev` that form is not what
Cloud Run hands out, and this is measured rather than feared.** `infra.yml`
run 35636247990 (`diagnose`, dev, 2026-09-21) read `status.url` off
`qip-dev-openobserve`, the one service in the project with `RoutesReady=True`,
and got the **legacy** form: the service name, a project-and-region token no
configuration can compute, a two-letter region code, and `.a.run.app`. A
hostname of the wrong family does not resolve, and it fails looking exactly
like a DNS problem.

The derivation is kept rather than replaced by a `data` source read, because
a data source on a service that does not exist yet fails the plan in every
environment whose portal has not been deployed — which is all of them. What
makes that safe is that the reading is a minute away and free: `diagnose`
applies nothing, waits for nothing and prints `url:` and `iapEnabled:` for
every service. The `apps` stage prints the same `status.url` after it enables
the gate.

Nobody types either form into a registrar and nobody waits for it to
provision; that part is true of both families.

## The fact this rests on

Cloud Run enforces IAP **on the service**, across every ingress path,
including the default `run.app` URL. Google's page says it in one sentence:

> By enabling IAP on Cloud Run directly, you can secure traffic with a single
> click from all ingress paths, including default `run.app` URLs and load
> balancers.

— `cloud.google.com/run/docs/securing/identity-aware-proxy-cloud-run`, read
2026-09-21. The same page carries the limitation that decides the shape of
this tree: **"You cannot configure IAP on both the load balancer and the
Cloud Run service."** The two doors are alternatives, not layers, and the
root module picks between them on `gitops_portal_hostname` for exactly that
reason.

## The three pieces, and why only one of them is here

A working Cloud Run IAP door is three things. This module is the third.

| Piece | Where it lives |
|---|---|
| The service | `gitops/envs/<env>/portal.yaml`, a Config Connector `RunService` (ADR 0036). Ingress `INGRESS_TRAFFIC_ALL`, no `allUsers` invoker |
| `roles/run.invoker` for IAP's service agent | `gitops/envs/<env>/invokers.yaml`, the `qip-<env>-portal-invoker-iap` binding. Unchanged from ADR 0094 — IAP forwards as its own agent whichever side of the service it sits on |
| `iapEnabled` on the service | **`infra.yml`'s `apps` stage.** See below |
| The access list | Here |

### Why the enable bit is not here, said plainly

`iap_enabled` is a field on the Cloud Run service. This tree does not own that
resource — ADR 0036 moved it into a `RunService` — and Config Connector's
`RunService` CRD has no `iapEnabled` field. Checked rather than assumed, on
2026-09-21:

```
curl -s https://raw.githubusercontent.com/GoogleCloudPlatform/k8s-config-connector/\
v1.156.0/config/crds/resources/apiextensions.k8s.io_v1_customresourcedefinition_\
runservices.run.cnrm.cloud.google.com.yaml | grep -c -i iap
0
```

The same is true on that project's `master`. The cause is mechanical and it
says when the gap closes: Config Connector's `RunService` is generated from
the **GA** `google` provider, and `iap_enabled` exists only in `google-beta`.
At provider 6.50.0, `terraform providers schema -json` shows the field on
`google-beta`'s `google_cloud_run_v2_service` and not on `google`'s. When it
graduates, Config Connector gets it, the manifest carries `iapEnabled: true`,
and the workflow step below is deleted.

Until then the enable bit is set imperatively, once per apply, by `infra.yml`'s
`apps` stage — and that step **reads `iapEnabled` back and fails the job if it
is not true**, because a step that enables a gate and does not check is a step
that reports a protected console either way.

### What happens if that step never runs

The portal answers **403 to everybody**, and that is the designed failure.
Ingress is `INGRESS_TRAFFIC_ALL`, so the URL resolves and Google's front end
accepts the connection — but the service has no `allUsers` invoker and a
browser cannot mint a Google ID token, so every anonymous request is refused
by Cloud Run's own IAM check. An unenabled gate here is a console nobody can
reach, never a console anybody can reach. That is the direction a safety
default must fail in, and it is why this shape was preferred to one that
needed the enable bit to be true before the door was safe.

## Granting somebody access

Nothing in this repository names a person; an IAM member is an account
identifier and `.claude/rules/00-enterprise-governance.md` refuses one in a
committed file. `iap_members` is `[]` in every environment and the door comes
up admitting nobody.

```
gcloud iap web add-iam-policy-binding \
  --project=algorik-dev \
  --resource-type=cloud-run \
  --region=us-east4 \
  --service=qip-dev-portal \
  --role=roles/iap.httpsResourceAccessor \
  --member='user:YOU@example.com'
```

`terraform output console_front_door` prints this command filled in, because
`--resource-type=cloud-run` is the part that is easy to lose: the same
subcommand without it edits the **project's** IAP policy, which is the wide
grant ADR 0095 narrowed away from.

## The narrowing, which is the second reason this module exists

ADR 0094 decision 3 refused to hold an access list at all, and was right to.
A GKE Gateway's backend service is named by its controller at reconcile time
and has no Terraform address, so `roles/iap.httpsResourceAccessor` had to be
granted at the **project** level — and project-level IAM is inherited by every
IAP-protected resource in the project. One grant admitted a person to Argo CD,
to Kargo and to the console at once, and a per-resource list beside it could
only widen that, never narrow it. A list that cannot exclude anybody is not an
access list.

IAP on Cloud Run is addressable per service:
`google_iap_web_cloud_run_service_iam_member` takes a project, a location and
a service name, none of which this module has to own. So the console's list is
the people who may read the console. ADR 0094's own reversal condition —
"the backend service becoming addressable, then the grant narrows and decision
3 reverses on its own terms" — is met, by a route the ADR did not anticipate.

`_member` and not `_binding`, deliberately. A `_binding` is authoritative: it
revokes every principal not in its list, so an operator granted by hand during
an incident disappears at the next apply, silently, with nothing in the plan
naming them.

## What this module still refuses

The same refusal `modules/iap-edge` makes, copied rather than referenced
because both modules can publish a Cloud Run service and a rule true of only
one of them is not a rule: a door in front of any trust zone but the two
§46.1 marks client-reachable is refused at plan time. An identity check does
not change which room a door opens onto (§40.14).

## Evidence

`tests/iap-run.tftest.hcl`, mocked provider, `command = plan` throughout — no
credential, no project, nothing created. Eight runs: the empty list plans and
grants nobody; a named member produces exactly one binding, in the right role,
naming one service and one region; `allUsers` and `allAuthenticatedUsers` are
refused alone and refused when appended to a list that already works; a
trading zone is refused and both client-reachable zones are admitted; a
project id in the numeric slot is refused.

Every refusal is paired with an admission of the same shape. A gate proven
only to refuse is a gate that may refuse everything, and
`qip-acceptance`'s `terraform_plan` suite fails a harness that proves only one
of the two halves.
