# The IAP edge

One Identity-Aware Proxy front door, in front of one Cloud Run service.
ADR 0094.

## Why this is not one of the other two modules

`modules/gitops-gateway` reserves the address and orders the certificate for a
**GKE Gateway**, and a Gateway routes to in-cluster Services: an `HTTPRoute`'s
`backendRef` names a `Service` or a `ServiceImport`, and a `GCPBackendPolicy`'s
`targetRef` is `kind: Service`. There is no backend kind in the Gateway API,
nor in GKE's `networking.gke.io` extensions, that names a serverless network
endpoint group — and the URL map behind that Gateway belongs to its
controller, so a serverless backend attached from outside is drift the
controller reverts. The GitOps front door therefore cannot front Cloud Run at
all. Nor can the address be shared: one global address carries one forwarding
rule on 443.

`modules/public-edge` owns the only serverless NEG in the tree and is still
the wrong home. It is the *anonymous* customer edge — a Cloud CDN bucket as
its default backend, a static shell, `hostnames = []` in all four
environments on purpose — and standing it up to carry an operator console
creates a security policy, a shell bucket, a backend bucket, a CDN policy and
a public address so that one of its resources can be used.

## What it creates

A reserved global IPv4 address; a Google-managed certificate for one
hostname; a Cloud Armor policy with one per-address rate limit and a
`rate_based_ban`; a serverless NEG naming one Cloud Run service; a backend
service with `iap { enabled = true }` and no OAuth client; a `RESTRICTED`
TLS policy at 1.2; a URL map with one default service and no path matcher;
one global forwarding rule, on 443.

Nothing on port 80. A redirect from an unencrypted listener is still an
unencrypted listener, and the first request to it has already travelled in
the clear — carrying, here, whatever cookie the browser held for the name.

## What it refuses, and where

| Refusal | Where |
|---|---|
| A hostname with a scheme, port, path, upper case, wildcard or no dot | `var.hostname` |
| A service name that is not a Cloud Run name | `var.service_name` |
| A service name too long to derive `<name>-iap-https` from within 63 characters | `var.service_name` |
| A rate limit of zero, or one too large to bind | `var.rate_limit_requests_per_minute` |
| A backend in any zone but `public-edge` or `application-identity` | `lifecycle.precondition` on `google_compute_backend_service.service` |

Each has a paired admission in `tests/iap-edge.tftest.hcl`, which is a real
plan against a mocked provider: no credential, no project, nothing created.
`terraform init -backend=false && terraform test` from this directory runs it.
A gate proven only to refuse is a gate that may refuse everything, and the
pair is what tells the two apart.

The zone refusal is load-bearing and is copied from `modules/public-edge`
rather than referenced. Both modules can put a backend in front of a Cloud Run
service, so each has to hold §40.5's rule — customer traffic and trading
traffic share no load balancer — or the rule is only true of whichever one a
reader happened to open. An identity check in front of a trading zone is still
a route into it: IAP decides *who* may pass and says nothing about what is on
the other side.

## What it does not hold

- **The access list.** There is no `iap_members` input, deliberately.
  `modules/gitops-gateway` grants `roles/iap.httpsResourceAccessor` at the
  project level, because the backend service a GKE Gateway creates has no
  Terraform address; project-level IAM is inherited by every IAP-protected
  resource in the project, so that grant already admits to this door too. A
  per-resource list here could only widen it, never narrow it, while reading
  in the console as this door's own list — a control that cannot fire. One
  list, in the tfvars, for both doors. ADR 0094 decision 3.
- **The invoker grant.** IAP forwards as
  `service-<project-number>@gcp-sa-iap.iam.gserviceaccount.com`, which needs
  `roles/run.invoker` on the fronted service. Since ADR 0036 that service is
  Config Connector's, so the grant is an `IAMPolicyMember` beside the
  manifest, not a resource here.
- **The DNS record.** `algorik.ai` answers from nameservers outside this
  project. The address is an output and the record is an operator's step.
- **A certificate that has issued.** Google provisions a managed certificate
  only after the name resolves to the address this module allocates. No plan
  can tell you whether the record was made.
- **Anything an apply would find.** Nothing here has ever been applied.
