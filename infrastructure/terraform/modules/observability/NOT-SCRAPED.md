# What scrapes what, and what does not yet

Read this before flipping `workload_metrics_exist` to `true` anywhere. The
alert policies in `main.tf` name descriptors the binaries register, and Cloud
Monitoring refuses a policy naming a descriptor it has never ingested — so the
gate stays `false` until a scrape has been *observed*, not until one has been
declared.

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

## The Cloud Run services: emitting, not yet scraped

`qip-fastbrain` and `qip-deepbrain` serve a Prometheus exposition on
`/metrics` from the registry the kernel writes to, and `qip-api` serves its
own behind `Role::Monitor`. On GKE a `PodMonitoring` resource collected the
two brains; that resource left with the cluster.

The Cloud Run equivalent is Google's managed-Prometheus sidecar
(`cloud-run-gmp-sidecar`), a container that scrapes the workload on loopback
and writes to Cloud Monitoring. It is **not attached**, and the reason is the
same rule the Envoy proxy had to satisfy first: Binary Authorization admits
only what the platform's attestor signed, so the sidecar has to be mirrored by
digest through `infrastructure/egress/vendored-images.txt` and `vendor.yml`
before any revision carrying it can be admitted. Nobody has pinned that digest
yet. Attaching an unattested image would produce a revision Binary
Authorization refuses, which reads as a broken deploy rather than as a
missing collector.

Until it is vendored and attached, the four central-plane policies —
kill switch, live fill, persistent breach, permission violation — and the
central reconciliation-break policy name series nothing carries to Cloud
Monitoring. That is the honest state: emitted, scrapable, not scraped.

## What would change this file

- A `cloud-run-gmp-sidecar` digest reviewed and added to the vendored-images
  list, a `metrics_sidecar` input on `modules/cloudrun` shaped like
  `egress_sidecar`, and the two brains carrying it.
- A node applied from a non-empty `execution_nodes`, and a
  `prometheus.googleapis.com/qip_edge_halted/gauge` descriptor visible in the
  project's metric explorer.
- Only then, `workload_metrics_exist = true` in that environment's tfvars.
