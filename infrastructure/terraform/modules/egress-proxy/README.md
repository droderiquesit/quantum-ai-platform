# The egress proxy

`qip_transport::http` speaks plaintext HTTP/1.1 and refuses the `https` scheme
by name (`backend/crates/libs/qip-transport/src/http.rs`, "Refuses `https` by
name"). Every outbound adapter in the workspace therefore needs an
`http://host:port` beside it that terminates TLS onward, and it must be a
*reverse* proxy: the client emits origin-form request lines and never
`CONNECT`, so the destination is chosen by which listener it connects to and
cannot be named in the request. This module is that proxy, for the Cloud Run
runtime.

## This is the first proxy that has existed

ADR 0020's corrections record the fact plainly: the Kubernetes chart's
`egress.yaml` described an Envoy Deployment and committed it commented out, so
the Argo CD of that time rendered the chart and no proxy pod ever ran. (That
chart and that controller are gone under ADR 0024; this paragraph is history,
not a description of anything running.) The `qip-egress` Service
had no endpoints, the NetworkPolicies naming it selected nothing, and every
adapter configured through it was inert. There is therefore **no "same request
served by both" evidence to produce** for ADR 0020's step 2 — there was never
a GKE path to compare against — and this module is not a port of a working
control. It is the control being built for the first time, off Kubernetes,
carrying the bootstrap the chart reviewed and never applied.

## Shape

| | |
|---|---|
| Bootstrap | `infrastructure/egress/envoy.yaml`, read with `file()`. The one copy |
| Published as | A versioned bucket object per environment, mounted read-only into every rendering |
| Cloud Run | An Envoy sidecar in the service that needs it (`modules/cloudrun`, `egress_sidecar`), sharing the workload's loopback. The workload container is held back until the sidecar's health listener answers |
| Execution node | The same bootstrap as a systemd unit beside the binary — image contract, see `modules/execution-node/README.md` |
| Image | `docker.io/envoyproxy/envoy` at the digest in `infrastructure/egress/vendored-images.txt`, mirrored and attested by `vendor.yml` into the environment's registry. No Binary Authorization exemption |
| Address | `http://127.0.0.1:910x`; every listener and the admin interface bind loopback |

The listener and port scheme is unchanged from the chart: 9101 Cloud Storage
and BigQuery, 9102 Vertex, 9103 IBM Quantum Runtime, 9104 IBM legacy, 9105
Frankfurter (ECB reference rates), 9900 health. The port is the destination selector, and that is the whole security
argument for the design: a process permitted to reach 9101 can reach Cloud
Storage and BigQuery and has no way to express a wish to reach anything else.

## Why not a Cloud Run service of its own

A Cloud Run service is reachable only at an HTTPS `run.app` name — internal
ingress changes who may reach it, not what scheme it answers on — and HTTPS is
the one scheme the client cannot speak. Reaching it in plaintext would need an
internal Application Load Balancer with an HTTP frontend and a serverless NEG,
and preserving port-as-selector would need one forwarding rule, target proxy,
URL map, backend service, NEG and Cloud Run service *per listener*. The cheap
version of that collapses the four listeners onto one address and routes on
`Host`, which hands destination choice back to a header the client controls.
Co-location deletes the addressing problem instead of solving it, and removes
the proxy from the availability and latency budget: a loopback connection is
not a hop, a scale-to-zero decision, or a rollout that can fail independently.

## What the plan refuses

The bucket object carries the gate, and it runs at plan time against the
committed file:

- the bootstrap dials a host `allowed_upstreams` does not name, or fails to
  dial one it does — the two must be the same set;
- a listener or the admin interface binds anything but `127.0.0.1`;
- there is no `health` listener for the startup probe to hit;
- `vendored-images.txt` carries anything other than exactly one `vendor/envoy`
  entry pinned by `sha256` digest.

**These preconditions have not been exercised against a real provider.** No
`terraform` binary exists in the environment this module was written in, so
`terraform fmt -check`, `terraform validate` and `terraform plan` have not
run. The first person with a binary runs a plan with a deliberately widened
bootstrap to see the second precondition fire, then restores it to see the
plan admit the good file — a gate proven only to refuse is indistinguishable
from one that refuses everything.

## What this does not do

- **It does not decide who may reach IBM.** Every rendering carries the same
  bootstrap, so a cognition-zone sidecar declares the IBM listeners too. What
  stops it reaching IBM is the trust zone's egress firewall
  (`modules/trust-zones`: only `optimisation` may hold an `ibm-quantum`
  destination). Two controls; the network one binds.
- **It does not give the execution node its proxy.** The node's unit is an
  image contract: the image ships `/usr/local/bin/envoy` and the startup
  script installs the unit. `modules/execution-node/README.md` carries it.
- **It reaches no venue and no broker, and one market-data vendor.** The
  variable's validation refuses a host whose name contains `venue`, `broker`
  or `exchange`, and the acceptance suite refuses a bootstrap that dials one.
  `api.frankfurter.app` is a republisher of the ECB's daily reference rates,
  polled hourly for a table with a sixteen-hour dissemination delay; it is on
  the list because a manifest in this workspace names it and its licence was
  evaluated before it could be opened, not because a market-data path was
  wanted. Nothing reads it yet — see the fast brain's missing sidecar in
  `infrastructure/terraform/catalogue.tf`.
