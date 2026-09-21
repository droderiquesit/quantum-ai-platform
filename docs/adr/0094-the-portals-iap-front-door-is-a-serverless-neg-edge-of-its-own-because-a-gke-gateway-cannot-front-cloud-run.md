# 0094. The portal's IAP front door is a serverless NEG edge of its own, because a GKE Gateway cannot front Cloud Run

Status: **Accepted on delegated authority, 2026-09-21.** Nothing is applied by
this record; it decides where a thing goes and the code follows it.

Supersedes nothing. Amends `modules/gitops-gateway`'s stated caveat about a
second IAP backend, and names a third Cloud Run ingress posture that
`console_route.rs` previously admitted for exactly one service.

## The ask

The owner wants a working URL for `frontend/portal`, in dev only, reachable
only behind Identity-Aware Proxy, and chose "behind the same IAP front door"
over a public surface. The name is `portal.algorik.ai`, beside
`argocd.algorik.ai` and `kargo.algorik.ai`.

## The finding that decides it: a GKE Gateway cannot front a Cloud Run service

The front door that exists is a GKE Gateway. `infrastructure/gitops/bootstrap/gateway/base/gateway.yaml`
declares `gatewayClassName: gke-l7-global-external-managed`, and
`overlays/dev/routes.yaml` sends each hostname to an in-cluster Service:

    backendRefs:
      - name: argocd-server
        port: 80

and enables IAP with a `GCPBackendPolicy` whose `targetRef` is
`kind: Service`, `group: ""` — the core Kubernetes Service, in the same
namespace.

Both halves are the obstacle. An `HTTPRoute`'s `backendRef` names a
`Service` or a `ServiceImport`; there is no backend kind in the Gateway API,
and none in GKE's `networking.gke.io` extensions, that names a serverless
network endpoint group. A Cloud Run service is not a Kubernetes object in
this cluster at all — Config Connector's `RunService` is a *representation*
of one, not something with endpoints a Gateway can program. And the URL map,
the target proxy and the backend services behind that Gateway are owned by
the GKE Gateway controller: a serverless-NEG backend added to them by hand or
by Terraform is drift the controller reconciles away.

So "the same front door" in the sense of *the same load balancer* is not
available. It is not a matter of a missing field.

The address cannot be shared either. `google_compute_global_address.gitops`
carries one forwarding rule on 443; a second 443 forwarding rule on the same
address is refused. A second door needs a second address and therefore a
second A record.

## The second candidate, and why it is not the answer either

`modules/public-edge` holds the only serverless NEG in this tree —
`google_compute_region_network_endpoint_group` with
`network_endpoint_type = "SERVERLESS"` — so it is the module that knows how
to put a global external Application Load Balancer in front of Cloud Run.

It is still the wrong home, for three reasons that are its own:

* It is the **anonymous customer edge**. Its default backend is a Cloud CDN
  bucket holding a static shell; its `README.md` opens with "Nothing here
  exists anywhere" and argues that `hostnames = []` in all four environments
  is the honest state, because no customer surface is deployed. An
  IAP-gated operator console is not that surface.
* Turning it on in dev to carry the portal creates a Cloud Armor policy, a
  shell bucket, a backend bucket, a CDN policy and a public address — five
  objects nobody asked for — so that one of its resources can be used.
* Its `url_map` serves the static shell by default and the application
  backend only under `/api/*`. The portal is a standalone Next.js server that
  serves its own routes, its own static assets and its own `/api` — there is
  no shell to put in front of it, and a path matcher that splits it in two
  would break it.

## Decision

**1. A new module, `infrastructure/terraform/modules/iap-edge`: one
IAP-protected global HTTPS front door for one Cloud Run service.** It is the
serverless-NEG twin of `modules/gitops-gateway` — a reserved global address,
a Google-managed certificate, a `RESTRICTED` TLS policy, a forwarding rule on
443 and nothing on 80, a Cloud Armor rate limit, and a backend service whose
`iap { enabled = true }` is the gate. `modules/public-edge` keeps its charter
and keeps creating nothing anywhere.

**2. IAP with Google-managed OAuth, so there is no client secret.** The
provider's own schema says it: "If OAuth client is not set, the Google-managed
OAuth client is used." That is the same choice `modules/gitops-gateway`
argues for, reached from the same starting point — a first draft with a
`google_iap_brand` and a `google_iap_client` — and it means this module mints,
stores and rotates nothing.

**3. One access list, and this module holds none.** `modules/gitops-gateway`
grants `roles/iap.httpsResourceAccessor` at the **project** level, because the
backend service a GKE Gateway creates is named by the controller at reconcile
time and has no Terraform address to bind to. Its own comment predicted this
day: "A second IAP-protected service would need this narrowed to per-resource
bindings first."

Narrowing is not available. Project-level IAM is inherited by every
IAP-protected resource in the project, so a per-resource binding on the
portal's backend service could only ever *widen* the set — never shrink it.
A second members list would therefore be a control that reads as protection
and cannot fire, which is the `MaxExpectedShortfall` failure in a new place.
So this module takes no members input at all, and the one project-level grant
is the access list for both doors. That is what "behind the same IAP front
door" means once the Gateway's backend service has no address to bind to,
and it is said here rather than discovered by someone who grants a person
Argo CD and finds they also have the console.

`gitops_iap_members` stays `[]`, granted out of band by name, as it already
is. Nothing in this change commits an account identifier.

**4. The portal runs as the console identity `qip-<env>-console`, not a new
one.** ADR 0018 created that account for exactly this workload — its display
name is "The portal, reading the platform as viewer" — and it already holds
the three grants the portal needs: accessor on `qip-token-viewer`, the
`console_profile_claims` custom role, and `roles/run.invoker` on `qip-<env>-api`
(`gitops/envs/dev/invokers.yaml`). A `modules/cloudrun` instantiation would
create `qip-portal-<env>` beside it and then need every one of those grants
made a second time — including a **second invoker on the API**, which is a
widening of who may call the platform to make a module fit. One workload, one
identity, and the identity that exists.

What Terraform adds for the portal is therefore grants and not an account:
`roles/iam.serviceAccountUser` for Config Connector on that one account (so
the reconciler may create revisions as it), `roles/logging.logWriter` and
`roles/monitoring.metricWriter` on it, the session-secret container, and an
accessor grant on that container. Those live in a `--- The portal ---`
section of `catalogue.tf`, beside the OpenObserve instantiation, which is the
existing precedent for a workload that is not one of ADR 0010's three
binaries.

**5. A third Cloud Run ingress posture, granted to exactly one service:
`INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER`.** A serverless NEG behind a global
external ALB requires it; `INTERNAL_ONLY` refuses the load balancer and the
door answers 404 for everyone. It is not the public posture: the service's
own `run.app` URL stays unreachable from the internet, so IAP cannot be walked
around by dialling the origin — which is the failure this posture exists to
prevent and the reason `INGRESS_TRAFFIC_ALL` is not used with an `allUsers`
invoker. `console_route.rs` admitted two values, `INTERNAL_ONLY` for
everything and `ALL` for OpenObserve alone; it now admits three, each pinned
to a named service, and the test asserts equality rather than the absence of
`ALL` for the same reason it always did — a deleted line is the public
address.

**6. IAP forwards as its own service agent, and that is the one new invoker
grant.** The portal's service requires authentication; IAP presents the
`service-<project-number>@gcp-sa-iap.iam.gserviceaccount.com` agent, which
`modules/gitops-gateway` already forces into existence with
`google_project_service_identity`. The grant is an `IAMPolicyMember` in
`gitops/envs/dev/invokers.yaml`, beside the console's grant on the API,
because the service it binds to is Config Connector's. `roles/run.invoker` on
one service, to one agent: not `allUsers`, and not a project-level binding.

**7. The browser gets two secrets and no more.** `ALGORIK_SESSION_SECRET_FILE`
— without which the process refuses to start under `NODE_ENV=production`,
rather than inventing a key no other replica could verify — and
`QIP_API_TOKEN_FILE`, the `viewer` credential its gateway proxies with. Both
as mounted files, never as environment values. No venue credential, no
capital-envelope key, no operator, analyst or monitor token, no Alpaca key.
`ALGORIK_AUTH_REQUIRED=true` is set, so the portal's own session check stands
behind IAP rather than instead of it.

**8. The image is built, scanned and attested like every other.**
`deploy.yml`'s matrix gains an entry that names `infrastructure/docker/portal.Dockerfile`
and the `frontend/` context, and goes through the same
`trivy --severity CRITICAL,HIGH --exit-code 1` gate before the push and the
same attestor afterwards. The matrix therefore stops being a list of
workspace binaries and becomes a list of images, each with the file and the
context it is built from.

## What it costs

* **A second front door, a second address, a second A record and a second
  certificate.** Somebody has to make the DNS record by hand at the
  registrar, as they did for the first two, and the managed certificate sits
  in `PROVISIONING` until they do.
* **One access list for two very different things.** A person granted
  `roles/iap.httpsResourceAccessor` to see Argo CD can also reach the
  console, and a person granted it for the console can also reach a
  controller that reconciles arbitrary manifests. This is stated in the
  tfvars and in both modules rather than left to be found. The narrowing that
  would fix it needs the Gateway's backend service to become addressable.
* **A third ingress posture to reason about.** Two values were a rule anyone
  could hold in mind; three is a table. The table is in `console_route.rs`,
  by service name, so it is checked rather than remembered.
* **Cloud Armor's rate limit is on the backend and IAP is in front of it**,
  so the limit binds a request that has already been authenticated. An
  unauthenticated flood is absorbed by Google's front end, which is where it
  should be, but it is not this policy that absorbs it.
* **The portal does not deploy on the day this lands.** No `qip-portal` image
  has ever been built, so dev's kustomization carries `TO-PIN` for it and
  Argo CD will not reconcile a service whose image is a placeholder. The
  marker goes when `deploy.yml` has run and a digest is reviewed. Wiring the
  portal into Kargo's warehouse, so a promotion keeps it pinned the way it
  keeps the three binaries pinned, is named here as the follow-on and is not
  done in this change.
* **The portal's gateway still cannot authenticate to `qip-api`.** It sends
  the platform bearer token and never a Google ID token —
  `frontend/portal/src/lib/server/google-credentials.ts` mints an *access*
  token for Identity Platform and nothing mints an *identity* token for Cloud
  Run — so the invoker grant ADR 0018 made is not yet exercised by anything.
  This record does not fix that (it is a change in `frontend/`, and a
  separate decision about whether the API's ingress or the portal's client is
  what moves); it names it so that a console that signs in and then shows
  nothing is diagnosed in one step rather than three.
* **`scripts/deploy-frontends.sh` still exists** and still deploys a second,
  `--allow-unauthenticated` portal called `algorik-portal` beside the
  GitOps one. It is the out-of-band path that predates ADR 0036 and it also
  deploys the landing, which has no manifest. Retiring its portal half is
  follow-on work; until then, the two paths are named differently and share
  one session secret so they cannot sign cookies with different keys.

## What would make this wrong

* **GKE's Gateway controller gaining a serverless-NEG backend kind.** If an
  `HTTPRoute` can one day name a Cloud Run service, this module is a second
  load balancer doing what the first could do, and the portal moves onto the
  Gateway and this record is superseded.
* **The Gateway's backend service becoming addressable in Terraform.** Then
  the project-level `iap.httpsResourceAccessor` grant narrows to two
  per-resource bindings, the two doors get two access lists, and decision 3
  is reversed on its own terms.
* **A customer surface actually shipping.** If `frontend/landing` deploys and
  `modules/public-edge` is switched on, the question of whether an
  IAP-protected console belongs behind the same Cloud Armor policy is worth
  re-asking — with the answer probably still no, because CDN and IAP want
  opposite things from a cache.
* **A second Cloud Run service needing IAP.** Two instantiations of this
  module are fine; three suggest the access list, the certificate and the
  address should be shared across them, which is a different module and a
  different record.
* **The portal ever needing a secret it does not need today.** Decision 7 is
  a list, and a list that grows without a reason in the diff is the failure
  `modules/cloudrun`'s missing `additional_roles` parameter exists to
  prevent.

## What was rejected

* **Put the portal on the GitOps Gateway.** It cannot be done; see above.
  Rejected on a fact, not a preference.
* **Run the portal as a Pod on the control-plane cluster so the Gateway
  *can* route to it.** ADR 0024 retired the runtime that scheduled this
  platform's binaries as Pods, and the acceptance suite refuses a `qip-*`
  image in any Pod spec. This would have been the easy route to "the same
  front door" and it is the one the rules exist to stop.
* **Extend `modules/public-edge` with an IAP mode.** Argued above: it turns
  the anonymous customer edge into two modules wearing one name, and lights
  up five resources in dev to use one.
* **`INGRESS_TRAFFIC_ALL` with an `allUsers` invoker and IAP in front.** This
  is what a lot of documentation shows. It leaves the service's own `run.app`
  URL answering the internet anonymously, so IAP guards the front door of a
  building with an open side entrance.
* **A per-resource IAP members list on the portal's backend service.** It
  could only widen the project-level grant, never narrow it, so it would read
  in the console as the portal's own access list while being unable to
  exclude anybody. A control that cannot fire is not a control.
* **A new `qip-portal` service account from `modules/cloudrun`.** Argued in
  decision 4: it would have required a second `roles/run.invoker` on the API.
* **Adding `portal` to `local.cloud_run_catalogue`.** Every entry there is a
  Rust binary under `backend/crates/apps/`, with an autonomy ceiling, a
  universe mount and internal ingress, and three acceptance walks assert the
  map holds exactly ADR 0010's three. The portal is none of those things, and
  making the catalogue admit it would weaken every check that reads it.
