# What scrapes what, and what does not yet

Read this before flipping `workload_metrics_exist` to `true` anywhere. The
alert policies in `main.tf` name descriptors the binaries register, and Cloud
Monitoring refuses a policy naming a descriptor it has never ingested — so the
gate stays `false` until a scrape has been *observed*, not until one has been
declared. Two things in this file are declared. Nothing in it is observed.

## The execution node: scraped, once it exists

`modules/execution-node/templates/startup.sh.tftpl` writes an Ops Agent
configuration with a Prometheus receiver on `localhost:<health_port>/metrics`
and refuses to start the unit if the agent is not on the image. Every series
`qip_edge::CellMetrics` records — the halt gauge, the refusals, the
reconciliation breaks, the mesh circuit — reaches Cloud Monitoring through
that receiver, as `prometheus.googleapis.com/qip_edge_*/gauge` and
`/counter` descriptors, which is what the PromQL conditions here query.

**No node exists.** `execution_nodes` is empty in every environment's tfvars,
so nothing has been scraped and the three edge policies cannot be created.
The receiver is declared; ingestion is not a fact.

## The Cloud Run services: a collector is declared, and no digest is pinned

`qip-fastbrain` and `qip-deepbrain` serve a Prometheus exposition on
`/metrics` from the registry the kernel writes to, and `qip-api` serves its
own behind `Role::Monitor`. On GKE a `PodMonitoring` resource collected the
two brains; that resource left with the cluster (ADR 0024).

The Cloud Run equivalent is Google's managed-Prometheus sidecar
(`cloud-run-gmp-sidecar`), a container that scrapes the workload on loopback
and writes to Cloud Monitoring. What is now in the Terraform:

- `modules/cloudrun` takes `collector_image_digest`, null by default. Set,
  it must be a full `repository@sha256:<64 hex>` and is refused otherwise;
  null is no sidecar, no configuration bucket, no grant, and the module's
  `metrics_collected` output is `false`. There is no second switch.
- Under a digest the module renders the sidecar beside the workload, started
  after the workload container is ready, with a `RunMonitoring` document
  scraping `/metrics` on the workload's own port every 30 seconds with a
  10-second timeout — the same cadence as the node's receiver. The document
  is published to a bucket and mounted read-only at `/etc/rungmp`, so the
  target and the interval are in a diff. The sidecar carries no secret, no
  environment and no identity; it writes on the `metricWriter` grant every
  workload already holds, and nothing was widened for it.
- `catalogue.tf` attaches it to both brains and deliberately not to the API,
  whose `/metrics` sits behind `Role::Monitor` and would answer a tokenless
  sidecar 401 every thirty seconds. The image is composed from the
  environment's registry prefix and the root's
  `metrics_collector_image_digest`, so only a mirrored, attested copy can
  reach a plan.

What is not:

- **No digest is pinned.** `metrics_collector_image_digest` is null in every
  environment. The reason is the one the Envoy proxy had to satisfy first:
  Binary Authorization admits only what the platform's attestor signed, so
  the sidecar has to be mirrored by digest through
  `infrastructure/egress/vendored-images.txt` and `vendor.yml` before any
  revision carrying it can be admitted. A candidate now sits in that file —
  `cloud-run-gmp-sidecar` at the digest tag `1.9.2` resolved to, confirmed
  against the manifest bytes it names — but it is **commented out**, so
  `vendor.yml` parses past it, no copy exists in any environment's registry,
  and nothing is attested. Resolving bytes is not reviewing them: the review
  is the commit that uncomments the line. Attaching an unattested image would
  produce a revision Binary Authorization refuses, which reads as a broken
  deploy rather than as a missing collector.
- **The document does not yet land where the collector reads.** The sidecar
  reads exactly one path, `/etc/rungmp/config.yaml` — the only `/etc/rungmp*`
  literal in its entrypoint binary, and its `Cmd` names
  `/etc/rungmpcol/config.yaml`, which is the OpenTelemetry configuration it
  *generates* from ours rather than one it reads. `modules/cloudrun` mounts
  the whole bucket, because the GA provider has no `mount_options` on a Cloud
  Run GCS volume and so `only-dir` is unavailable, and it names the object
  `${local.collector_prefix}/config.yaml` — a content hash used as a
  directory, on the reasoning that a changed configuration should sit beside
  the old one rather than overwrite it. Under this mount that document lands
  at `/etc/rungmp/<hash>/config.yaml`, where nothing looks. Pin the digest on
  top of that layout and the collector starts, finds no document, falls back
  to its own built-in default and scrapes a target nobody chose — with every
  alert policy still gated off, so nobody would see it. The object must be
  named `config.yaml` at the bucket root before a digest is pinned, and the
  fixed name costs one bucket-scoped `storage.objects.delete` because an
  overwrite needs it. That change is to `modules/cloudrun`, not to this
  module, and has not been made.
- **Nothing has been applied.** ADR 0024 records that no plan has been
  produced on any environment; a declared sidecar is a statement about a
  configuration.
- **Nothing has been observed.** No `prometheus.googleapis.com/qip_*`
  descriptor exists in any project.

So today the five central-plane policies — kill switch, live fill, persistent
breach, permission violation, central reconciliation break — still name
series nothing carries to Cloud Monitoring. That is the honest state:
emitted, scrapable, collector declared, not scraped.

## What would change this file

- `modules/cloudrun` publishing the collector's document as `config.yaml` at
  the root of its bucket, which is the one path the sidecar reads.
- A `cloud-run-gmp-sidecar` digest reviewed — the candidate line in the
  vendored-images list uncommented — mirrored and attested by `vendor.yml` as
  `vendor/cloud-run-gmp-sidecar`, and recorded as
  `metrics_collector_image_digest` in an environment's tfvars.
- A plan read and applied by a person, and both brains' revisions admitted
  carrying the sidecar.
- A node applied from a non-empty `execution_nodes`, and a
  `prometheus.googleapis.com/qip_edge_halted/gauge` descriptor visible in the
  project's metric explorer.
- Only then, `workload_metrics_exist = true` in that environment's tfvars.
