# 0018 — The console reaches the platform over the VPC

**Status:** accepted

## Context

The portal is a Next.js server on Cloud Run. The platform it exists to
display — `qip-api` — is a `ClusterIP` Service inside a private GKE cluster:
private nodes, a private control-plane endpoint, no Ingress, no external
address. There has never been a route between the two.

The consequence was visible and, to its credit, honest. `upstream()` refuses
when `QIP_API_BASE_URL` is unset, so `/api/gateway/health` answered **500**
with "QIP_API_BASE_URL is not set, so this console has no platform to read"
rather than inventing a number. The console was unavailable, not lying. But
unavailable is what it was, on every page, since it was first deployed.

The variable could not simply be set. There was no value it could take: no
address existed that Cloud Run could reach.

Two facts found while measuring this, which shape the decision:

- **`qip-api` requires a bearer token.** Every `/api/v1/*` route answers
  `401 {"error":"no credential was presented"}` without one. A network path
  alone converts a 500 into a 401. The console needs a credential as well as
  a route, and the credential already exists as `qip-token-viewer-dev` in
  Secret Manager.
- **The one NetworkPolicy admitting traffic to `qip-api` names a namespace
  that does not exist.** `allow-api-ingress` permits ingress from
  `ingress-nginx`; there is no `ingress-nginx` namespace in the cluster.
  Under `default-deny` that means nothing outside the pod network can reach
  the API at all today. A route to the Service would have been necessary and
  still not sufficient.

## Decision

Cloud Run reaches `qip-api` over the VPC, and only over the VPC.

- **Direct VPC egress**, not a serverless VPC connector. The portal revision
  gets an interface in a subnet of the `qip-dev` network dedicated to it
  (`qip-<env>-console-egress`, a `/26`), with egress restricted to private
  ranges. A connector would be a second managed instance group to size, pay
  for and patch, for the same reachability.
- **An internal passthrough Network Load Balancer** in front of `qip-api`,
  on a reserved internal address. Internal, so the API gains no public
  surface: the address is RFC1918 and routable only from inside the VPC. A
  passthrough LB rather than an internal Application LB because the latter
  needs a proxy-only subnet and buys nothing here — there is one backend, one
  port, and no host or path routing to do.
- **The address is reserved and committed**, not allocated. A load balancer
  address that changed on recreation would silently break the console's
  configuration, and the failure would appear as a timeout rather than as the
  configuration change it was.
- **A NetworkPolicy admitting exactly the console subnet** to `app=qip-api`
  on 8080. Not `0.0.0.0/0`, not the whole VPC: the console's `/26` and
  nothing else. The existing `allow-api-ingress` rule is left alone rather
  than widened — it describes an ingress controller that may yet exist, and
  making an unrelated rule broader to solve this problem is how a policy stops
  describing anything.
- **The console authenticates as `viewer`.** The token is projected from
  Secret Manager as a file and read through a `_FILE` indirection, matching
  what `qip_core::secret` does for every other credential in this platform.
  Viewer is the whole entitlement: it reads. `POST /api/v1/cycle` is
  `analyst` and the kill switch is `operator`, and the console holds neither.

## Why the alternatives lose

**A serverless VPC connector.** Works, and is the older mechanism. It costs a
dedicated subnet *and* a managed instance group of connector VMs that must be
sized and upgraded. Direct VPC egress is the same reachability with no
instances to own.

**Moving the portal into GKE.** The largest change, and the one with the best
end state — the console would sit behind the same admission control,
NetworkPolicy and Binary Authorization as everything else, and this ADR would
not need to exist. It is not this change because it replaces a working
deployment path (Cloud Build → Cloud Run, which the desk uses) with an
Ingress, a certificate, and a DNS name, and does so before the console has
ever shown a real number. Worth revisiting once it has.

**Exposing `qip-api` publicly with authentication in front.** Refused. The
bearer token would become the only thing between the internet and the
platform's operator interface, and `POST /api/v1/kill-switch` is on that
interface. The paper-trading boundary is not weakened by this — no live path
exists to expose — but the kill switch reaching the internet is a different
failure and a sufficient one.

## What it costs

A subnet, a reserved address, a forwarding rule, and a firewall rule per
environment that opts in. The console's availability now depends on the
cluster's, which it did not before — though what it depended on before was
nothing, and it displayed nothing.

The console holds a platform credential. It is `viewer`, it is a file rather
than an environment variable, and it never crosses to the browser: `upstream`
is imported only by route handlers. But it is a credential in a process that
serves the public internet, and that is a real change in posture from a
process that held none.

The reserved address appears in three places — the Terraform that reserves
it, the Helm value the Service is given, and the deployment that points the
console at it. Three copies of one fact disagree eventually, so an acceptance
test asserts they agree, in the manner `manifest_wiring.rs` already asserts
the mesh ports do.

## What would make this wrong

If the portal moves into the cluster, all of this becomes dead weight and
should be deleted rather than kept "in case". If `qip-api` ever grows a
second consumer outside the VPC, an internal passthrough LB is the wrong
shape and a proper internal Application LB with host routing is the right
one.
