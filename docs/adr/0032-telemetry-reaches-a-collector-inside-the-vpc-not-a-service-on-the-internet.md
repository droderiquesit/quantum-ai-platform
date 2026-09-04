# ADR 0032 — Telemetry reaches a collector inside the VPC, not a service on the internet

- **Status:** accepted
- **Date:** 2026-09-04
- **Decides:** the fast-brain telemetry route, and A6's collector image
- **Relates to:** ADR 0024 (no proxy beside the fast brain), ADR 0008 (nothing
  on the fast path consults a model), ADR 0026 (the export path), ADR 0030

## The decision

A collector runs **inside the VPC, speaking plaintext OTLP on a private
address**. All three central roots drain to it. It forwards to OpenObserve,
and later to anything else, on its own TLS.

The collector is the vendored, digest-pinned, Binary-Authorization-attested
image A6 has been blocked on. It is one image and it closes three gaps at
once, which is why this is one decision rather than three.

No root drains to a public URL. `QIP_OPENOBSERVE_URL` on a workload names the
collector, never OpenObserve itself.

## Why, and what the alternatives cost

The forcing constraint is not preference. `qip_transport::Url::parse` refuses
every scheme but plaintext `http` **by name**, because the HTTP client speaks
plaintext HTTP/1.1 and expects a TLS-terminating hop in front of it
(ADR 0024). OpenObserve as deployed is HTTPS-only. A root handed its public
URL exits `EX_CONFIG` at start-up. So a route must be found, and there were
three.

**The fast brain is what decides between them.** It deliberately has no
egress proxy: port 9102 on that proxy routes to a language model API, and
nothing on the fast path may consult a model (ADR 0008). `catalogue.tf`
refuses at plan time to give it one. So the fast brain has no
TLS-terminating hop at all, and any answer that routes telemetry through the
existing proxy simply does not serve it.

- **Route everything through the existing egress proxy on a new port.**
  Works for the API and the deep brain. Does not work for the fast brain
  without giving the fast path a sidecar that also carries a model route —
  eroding ADR 0008 by configuration, which is exactly how a structural
  guarantee becomes a policy one. Rejected.
- **A second, narrower proxy beside the fast brain,** carrying only
  OpenObserve. Keeps ADR 0008 intact by construction. A legitimate answer,
  and the fallback if the collector image cannot be vendored. Rejected only
  because it solves one gap where the collector solves three, and because it
  still sends operational telemetry across the public internet.
- **Do not drain the fast brain.** Leaves the latency-sensitive plane the
  least observed, which is the plane where an unseen problem is most
  expensive. Rejected.

The chosen route is also the one the drain's own module documentation
anticipated: *"A plaintext collector inside the VPC would be reachable by
this client and is the shape a deployment would have to provide."* This
record is that provision.

Three things fall out that were not the goal but decide the matter on their
own:

1. **The fast brain is served with no new sidecar and no model route
   anywhere near it.** ADR 0008 is untouched.
2. **A6 closes.** The same attested image is the scrape target the seven
   alert policies need, so `workload_metrics_exist` becomes flippable on
   evidence rather than aspiration.
3. **Operational telemetry stops crossing the public internet.** Under any
   proxy answer, the platform's cycle counts, refusals, limit breaches and
   reconciliation breaks would be POSTed over the internet to an endpoint
   that accepts anonymous connections. That is a worse property than the
   observability it buys.

## What it costs

- **An image this platform did not write, running in its network.** It is
  vendored, pinned by digest, mirrored and attested like every other
  upstream image — the same discipline as the Envoy proxy and OpenObserve —
  but it is a third-party binary with a supply chain, and ADR 0002's argument
  about dependencies applies to containers too.
- **A component that can be down.** A collector between the emitters and the
  store is a place telemetry can be lost. The drain already tolerates a
  refused POST without stopping the process, and must continue to; a
  collector outage must degrade to "no telemetry" and never to "no trading
  decisions".
- **A second hop to reason about.** "The metric is missing" now has two
  candidate causes rather than one.
- **It is not free at rest.** Unlike a Cloud Run service at zero instances, a
  collector that is scraping is a collector that is running.

## What would make this wrong

**If the collector image cannot be vendored and attested.** That is the whole
premise. If Binary Authorization cannot admit it, fall back to the narrow
second proxy — the alternative that was rejected on breadth, not on
correctness — and accept the public-internet hop with ADR 0033's mitigation.

**If a root is ever pointed at a public URL directly.** The refusal is
currently a property of `Url::parse` rather than of anything in this record.
If that client ever learns TLS, this decision must be re-argued rather than
quietly bypassed, because the reason was never only "the client cannot".

**If the collector grows a route to anything on the fast path.** The whole
argument for reaching the fast brain rests on this component carrying
telemetry and nothing else. A collector that also proxies a vendor call is a
sidecar with a model route in it by another name, and ADR 0008 falls to it.
