# 0028 — OpenObserve is adopted as a deliberate deviation from §2.1, over OTLP, on ephemeral storage

**Status:** accepted, by explicit owner instruction given after the conflict
below was raised and understood.
**Amends:** ADR 0022 in one narrow place — §2.1's "every managed service is
Google Cloud or IBM" no longer holds for the observability backend.
**Supersedes:** ADR 0026's recommendation. ADR 0026 remains in the tree as
the record of the road not taken and the reasoning that weighed it; nothing
in it was wrong, the owner simply chose the other one.
**Does not touch:** the paper-trading boundary, the two-dependency policy
for Rust crates (ADR 0002, ADR 0009), or the async-runtime refusal (ADR
0001, ADR 0012, ADR 0026's Option (c)).

## Context

ADR 0026 already existed, as a proposed (not decided) record, for exactly
the question this one answers: what backs the platform's dashboards. It
quotes the blueprint's own §2.1 directly — "managed services are Google
Cloud or IBM only... 'external observability' is excluded" — and, on that
basis, recommends Google's Managed Prometheus for metrics and hand-rolled
OTLP spans to Cloud Trace: no third-party platform, no new dependency, no
new deployed image beyond a Google-authored sidecar.

The owner was told this directly, in these terms, before deciding: that
OpenObserve is not Google Cloud or IBM, that adopting it means building
something the blueprint's own architecture-of-record excludes by name, and
that the compliant path (ADR 0026 as written) gets a real, remotely-viewable
link for free through the Cloud Console with no new public-ingress surface,
where OpenObserve would need one built from nothing since this platform has
no public-facing ingress anywhere today (confirmed: every workload in
`infrastructure/terraform/catalogue.tf` uses `ingress_posture = "internal"`;
the `"public-edge"` value the module accepts has never been exercised and no
load balancer resource exists to back it — checked across
`infrastructure/terraform/modules/connectivity/`, the only VPC-connectivity
module in the tree, which is Private Google Access and interconnect routing,
not a customer-facing edge).

The owner chose OpenObserve anyway, over OTLP (not Prometheus remote-write),
on ephemeral storage. This record exists so that choice is written down
with its cost, not defaulted into silently.

## Decision

1. **OpenObserve is adopted as the platform's metrics, logs and traces
   backend**, in place of ADR 0026's Google-Managed-Prometheus-plus-Cloud-
   Trace recommendation. This is a knowing exception to blueprint §2.1, not
   a reinterpretation of it: the blueprint still says Google Cloud or IBM
   only, and this platform now runs one third-party observability service
   anyway, by explicit instruction.

2. **The wire protocol is OTLP, not Prometheus remote-write, and not the
   Prometheus-format `/metrics` scrape this platform's processes already
   serve.** `qip-observability` already emits Prometheus text exposition
   (`Snapshot::to_prometheus`) and an OTLP-shaped span export
   (`Tracer::export`, per ADR 0026's Option (b) research). Metrics gain a
   second, OTLP-JSON encoder alongside the Prometheus one — the exposition
   endpoint is not removed, since nothing says a metric may only be
   represented one way, and removing it would cost every existing test and
   runbook that reads it for no benefit. A composition-root drain thread (the
   same shape ADR 0026 already designed for spans: blocking, on an interval,
   with an explicit timeout, through the egress proxy where one exists)
   posts both encodings' output to OpenObserve's ingestion endpoints —
   `POST /api/{org}/v1/logs`, `/api/{org}/v1/metrics`, `/api/{org}/traces`,
   confirmed against OpenObserve's own published API reference. No async
   runtime, no new Rust dependency: `serde_json` only, matching ADR 0026's
   Option (b) exactly except for the destination.

3. **OpenObserve is a new *kind* of workload, sourced differently from every
   other service `infrastructure/terraform/modules/cloudrun` deploys.**
   Every workload today gets its image digest from this platform's own
   build→sign→attest pipeline, written into
   `infrastructure/environments/<env>/images.tfvars` by `deploy.yml`.
   OpenObserve's digest instead comes from
   `infrastructure/egress/vendored-images.txt`, mirrored and attested by
   `vendor.yml` — a different lifecycle the module has no input for today.
   The module gains a `source = "built" | "vendored"` input (default
   `"built"`, so every existing workload is unaffected) that, when
   `"vendored"`, reads the digest from a `vendored_image_digest` variable
   instead of `images.tfvars`, and skips the `ignore_changes` lifecycle rule
   `deploy.yml`'s update path relies on for built images (a vendored image
   has no such pipeline to fight with). This is the minimal shape: one new
   input, one new branch in the digest lookup, nothing else about the
   module's secret-file-mounting, network-tag, or ingress-posture behaviour
   changes for a vendored workload.

4. **Storage is ephemeral, on purpose, as instructed.** OpenObserve's only
   persistent-storage backend is S3-compatible (`ZO_S3_*`; confirmed against
   its own published environment-variable reference — no native
   GCP-workload-identity storage path exists). Durable storage would mean a
   GCS HMAC access/secret key pair: a static, long-lived credential of
   exactly the kind `.claude/rules/01-security-and-safety.md` forbids
   ("Workload Identity Federation only... No downloaded service-account
   keys, ever"). Ephemeral local storage avoids that credential entirely, at
   the cost named below. This record does not authorise a future move to
   durable storage; that is a separate decision, when it is wanted, that
   will have to say explicitly how it gets a credential without violating
   that rule (a KMS-wrapped, rotated, narrowly-scoped HMAC pair mounted the
   same way every other secret in this platform is mounted — as a file,
   never an environment value — is the shape such a record would need to
   defend, not assume).

5. **Ingress stays internal for this first deployment.** Building a public-
   facing edge (external HTTPS load balancer, backend service, URL map,
   certificate, and a real access-control decision — IAP, or OpenObserve's
   own auth behind it, or both) is a materially separate project: it would
   be this platform's first-ever public network surface, for any workload.
   That is not a decision this record makes as a side effect of adopting a
   dashboard tool. OpenObserve is reached, for now, the same way every other
   internal-only Cloud Run service in this platform already is — through the
   VPC, by an operator with the right IAM binding — and its own URL is
   real and stated plainly as internal-only when it exists, not oversold as
   a public link it is not.

## What it costs

- **A third-party service inside the trust boundary that is not Google Cloud
  or IBM**, contradicting the blueprint's own §2.1 in the one place this
  record carves out. Every other "managed services are GCP/IBM only" claim
  in this repository's documentation continues to hold; this is a named,
  singular exception, not a precedent for the next one to cite without its
  own record.
- **A second wire encoding to maintain**: the OTLP-JSON metrics encoder is
  new code, alongside the Prometheus encoder that stays. A mistake in it is
  refused loudly by OpenObserve's ingestion endpoint (a 4xx on a malformed
  batch) rather than silently misread, the same loud-failure property ADR
  0026 required of Option (b) — but it is still surface area ADR 0026's
  recommended path would not have added.
- **No durable metrics history until a separate, harder decision is made.**
  A Cloud Run cold start loses everything OpenObserve was holding. This is
  named, not hidden: `docs/architecture/algorik-blueprint-traceability.md`'s
  observability rows should say so once this ships, the same way
  `NOT-SCRAPED.md` says plainly that nothing scrapes any process today.
- **One more vendored third-party image reviewed by digest** (in addition to
  Envoy), and a small, real extension to `modules/cloudrun` to carry a
  second image-sourcing lifecycle — tested the same way every other module
  change in this tree is: `terraform fmt -check`, `terraform validate`, and
  the `infrastructure` acceptance suite, plus a real plan showing the gate
  admits this one new workload and nothing else changes for the workloads
  that already exist.
- **No public link, this pass.** A person outside the VPC cannot open a
  browser and see this. That is stated here so nobody is surprised by it
  later, not proposed as acceptable forever — a follow-up record can make
  the public-edge decision explicitly, when it is wanted, on its own
  reasoning about the attack surface it opens.

## What would make this wrong

- OpenObserve's OTLP ingestion endpoints turning out not to accept the
  encodings `qip-observability` actually produces (a version mismatch, a
  stricter schema than the published reference describes) — closes only by
  a real deployment and a real POST, which ADR 0024's standing limit already
  names: nothing here can be observed from this environment; the record
  states a shape, an apply and an ingested event decide whether it works.
- A reviewer finding the ephemeral-storage cost is not actually acceptable
  in practice (an operator who needs last week's graph and does not have
  it) — the remedy is the durable-storage follow-up record named above, not
  silently mounting a static key to route around this one.

## What this does not do

This record does not authorise granting OpenObserve, or anything that
proxies to it, any credential this platform's paper-trading or capital-
movement controls guard. It observes; it does not decide, size, route, or
sign anything. Nothing about `AutonomyController`, `qip-execution-engine`,
or the three paper-trading layers changes here or may be justified by this
record.
