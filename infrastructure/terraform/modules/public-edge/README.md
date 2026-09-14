# The public edge

Blueprint §40.5 and §40.14: Cloud Armor, the global HTTPS load balancer, and
Cloud CDN for the static shell. §45.1 lists the three together and adds the
constraint the whole module is shaped by — **web and mobile only, never in
front of venue connectivity**.

## Nothing here exists anywhere

`hostnames` is empty in all four environments, so this module plans to zero
resources in all four. That is deliberate and it is the honest state: no
customer surface is deployed. `frontend/landing` and `frontend/portal` are in
the tree, in no catalogue, no image matrix and no manifest. An edge in front of
nothing is a public address on the internet that somebody would have to decide
to close again.

It is declared rather than absent for the reason `modules/execution-node` is
declared while `execution_nodes = {}` everywhere: a module written on the day
a surface first ships is a module written under deadline, and the refusals
below are the ones nobody argues for at that moment.

## What it refuses, and where

| Refusal | Where |
|---|---|
| A backend in any zone but `public-edge` or `application-identity` | `lifecycle.precondition` on `google_compute_backend_service.application` |
| A wildcard or malformed hostname | `var.hostnames` |
| A duplicate hostname | `var.hostnames` |
| A rate limit of zero, or one too large to bind | `var.rate_limit_requests_per_minute` |
| A lower-case or three-letter country code | `var.permitted_regions` |
| An application backend with no hostnames | `var.public_edge` in the root |

Each has a paired admission in `tests/public-edge.tftest.hcl`, which is a real
plan against a mocked provider: no credential, no project, nothing created.
`terraform test` from this directory runs it. A gate proven only to refuse is
a gate that may refuse everything, and the pair is what tells the two apart.

The zone refusal is the load-bearing one. §40.5 says customer traffic and
trading traffic never share a load balancer, an identity, a credential or a
route; this module holds the first clause, and it holds it as a plan that
stops rather than as a review comment. `execution`, `ledger`,
`treasury-write`, `control-fabric`, `optimisation` and `wallet-read` are
refused by name, not narrowed.

## What it does not hold

- **The ports a load balancer answers on are not the only way in.** This
  module creates no HTTP listener — 443 or nothing, because a redirect from
  port 80 is still an unencrypted endpoint and the request that reaches it has
  already travelled in the clear. It cannot stop another module creating one.
- **Cloud Armor's rules are about addresses, rates and country codes.** They
  cannot tell an authenticated request from an unauthenticated one. The
  session, the passkey, the device binding and the step-up §40.14 describes
  are the application's, and none of them is in this file.
- **CSRF protection and secure headers.** §40.14 says these are applied "at
  the edge and in the application". Here they are applied in the application:
  `frontend/portal/next.config.ts` sets them. A response-header policy on the
  load balancer would be a second place the same fact is written, and the two
  would disagree.
- **Private Service Connect.** §40.14 puts the application services behind it.
  `modules/connectivity`'s `enable_private_service_connect` is false in every
  environment and this module does not turn it on.
- **A certificate that has issued.** Google's managed certificates provision
  only after each domain resolves to the address this module allocates. Until
  DNS exists the edge serves nothing on 443, and no plan can tell you whether
  the record was made.
- **Anything an apply would find.** Nothing here has ever been applied.
