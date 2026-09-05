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
so nothing has been scraped and the two edge policies cannot be created.
The receiver is declared; ingestion is not a fact.

Two, not three — this file said three until someone counted. Exactly two
policies in `main.tf` query a `qip_edge_*` series, and
`grep -n 'query *= *"[^"]*qip_edge' main.tf` returns
`edge_halted` and `edge_reconciliation_break` and nothing else;
`central_reconciliation_break` reads the centre's own counter and is one of
the five below. Three edge plus five central is eight, and
`grep -c '^resource "google_monitoring_alert_policy"' main.tf` says seven.

The receiver labels each sample `cell: <node_id>` and `region: <region>`
from the template's `static_configs`, which is what
`max by (cell, source) (qip_edge_halted)` groups on, so the halt policy's
query resolves against what this receiver produces. Note that
`CellMetrics::new` already puts `cell` and `region` in the exposition
(`qip-edge/src/telemetry.rs:170-171`) from `QIP_CELL_ID` and
`QIP_CELL_REGION`, which `startup.sh.tftpl:165-166` sets to the same
`${node_id}` and `${region}` the receiver uses. The values therefore agree;
the labels still collide at scrape time, and a Prometheus receiver at the
default `honor_labels: false` keeps the target's copy and renames the
exposed one to `exported_cell` and `exported_region`. The alert is
unaffected — `cell` survives carrying the right value — but the target
labels are redundant with the exposition and the `exported_*` pair is
duplicate cardinality nobody asked for. Dropping the two `labels:` lines
from the receiver would be the smaller configuration; that is a change to
`modules/execution-node`, not to this module, and has not been made.

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

- **No digest is pinned, and the adoption is REFUSED rather than pending.**
  `metrics_collector_image_digest` is null in every environment
  (`environments/dev/terraform.tfvars:142` is the only mention and it is
  commented out). The gate is the one the Envoy proxy had to satisfy first:
  Binary Authorization admits only what the platform's attestor signed, so
  the sidecar must be mirrored by digest through
  `infrastructure/egress/vendored-images.txt` and `vendor.yml` before any
  revision carrying it can be admitted.

  This file used to say the candidate line was merely "commented out" and
  that "the review is the commit that uncomments the line". That is no
  longer the state and has not been for some time. The review happened.
  `vendor.yml` run 11 mirrored `cloud-run-gmp-sidecar` 1.9.2 and Trivy —
  the same gate the platform's own images pass — failed it on
  `CVE-2026-56854`, an unfixed CRITICAL in `golang.org/x/crypto` v0.54.0,
  fixed upstream in 0.55.0. `vendored-images.txt:78-100` carries the finding.
  The line stays commented **for a reason**, and "resolved but nobody
  looked" and "looked at and refused" are different states to hand the next
  person.

  Re-checked on 2026-09-05, and the position has not improved — it has got
  slightly worse:

  - The newest published version is still 1.9.2. The registry's own tag list
    (`curl -sS https://us-docker.pkg.dev/v2/cloud-ops-agents-artifacts/cloud-run-gmp-sidecar/cloud-run-gmp-sidecar/tags/list`,
    plain semver tags only) returns `1.0.0 1.1.0 1.1.1 1.2.0 1.3.0 1.4.0
    1.6.0 1.7.0 1.8.0 1.9.1 1.9.2` and nothing above it. There is no 1.9.3
    or 1.10.0 to move to.
  - The bytes have not been rebuilt. `docker-content-digest` for tag 1.9.2 is
    still `sha256:ff1fc68871118f1032a3ce17e2b0abd703292e883989d220244330ebdf522fd1`,
    identical to the digest the commented line names and identical to what
    run 11's Trivy scanned. Same digest means the same scan result; nothing
    needs re-scanning to know it still fails.
  - Two further advisories have landed against the same dependency since the
    refusal was written, both in `golang.org/x/crypto/ssh`, both fixed in
    0.56.0 and both published 2026-09-02
    (`curl -sS https://vuln.go.dev/ID/GO-2026-6354.json`, and `-6355`):
    `CVE-2026-78662` and `CVE-2026-56855`, denial of service on a deadlocked
    channel. The image pins v0.54.0, so it is now behind three advisories
    rather than one.

  The temptation to wave it through is the same one `vendored-images.txt`
  already names and refuses: the findings are all in `x/crypto/ssh` and a
  metrics collector opens no SSH server, so they are very likely
  unreachable. That is how a scanner exception gets written, and an
  exception outlives the release that needed it. The fix is Google's to
  ship. Attaching an unattested image anyway would produce a revision Binary
  Authorization refuses, which reads as a broken deploy rather than as a
  missing collector.
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
- **The sidecar has never been applied, though this module has.** This entry
  used to read "Nothing has been applied. ADR 0024 records that no plan has
  been produced on any environment." That sentence has outlived its truth in
  the same way `infrastructure/CLAUDE.md`'s did — row 10 of
  `docs/ops/missing-infrastructure-register.md` records that one being
  rewritten for exactly this reason — and ADR 0024's closing paragraph is
  itself listed as stale in that register. `dev` has been applied by
  `infra.yml`'s `up`, dispatched by a person, and `module.observability` is
  instantiated unconditionally at `terraform/main.tf:400`, so it was in that
  apply.

  What it produced is the useful part: with `workload_metrics_exist` false,
  `count = 0` on all seven policies, so the apply created no alert policy at
  all. The gate has therefore actually run rather than merely being
  declared. What has never been applied is a *sidecar* — no environment
  pins a collector digest, so no revision has ever carried one.

  Recorded, not observed by me: I read this from the tree and the register,
  not from a plan or an apply log.
- **Nothing has been observed.** No `prometheus.googleapis.com/qip_*`
  descriptor exists in any project.

So today the five central-plane policies — kill switch, live fill, persistent
breach, permission violation, central reconciliation break — still name
series nothing carries to Cloud Monitoring. That is the honest state:
emitted, scrapable, collector declared, not scraped.

## What would change this file

- `modules/cloudrun` publishing the collector's document as `config.yaml` at
  the root of its bucket, which is the one path the sidecar reads.
- **Google publishing a `cloud-run-gmp-sidecar` above 1.9.2 built on
  `golang.org/x/crypto` >= 0.56.0.** This is the blocker, it is upstream, and
  nothing in this repository can clear it. Re-run the tag-list command in the
  refusal entry above when observability is next picked up; a release
  carrying the fixed dependency makes the rest a one-line change. Do not
  clear it with a scanner exception.
- Then that digest reviewed and the candidate line in the vendored-images
  list uncommented, mirrored and attested by `vendor.yml` as
  `vendor/cloud-run-gmp-sidecar`, and recorded as
  `metrics_collector_image_digest` in an environment's tfvars.
- A plan read and applied by a person, and both brains' revisions admitted
  carrying the sidecar.
- A node applied from a non-empty `execution_nodes`, and a
  `prometheus.googleapis.com/qip_edge_halted/gauge` descriptor visible in the
  project's metric explorer.
- Only then, `workload_metrics_exist = true` in that environment's tfvars.
