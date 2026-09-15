# envs/

One directory per environment, one `RunService` per catalogue workload,
concrete in every field. Four copies rather than a base with replacements,
deliberately: the acceptance suite's parity test reads these files with no
kustomize binary, and a reader diffing `dev` against `test` should see the
project, the region, the identity and the subnet change and nothing else.

`kustomization.yaml` is the one file with a digest in it. Every `image:` in
a manifest is a logical name — `qip-api`, `qip-fastbrain`, `qip-deepbrain`,
`openobserve`, `envoy`, `google-cloud-cli` — and the `images` block maps
each to the environment's registry at a digest. That is the file Kargo's
promotion edits, the record `images.tfvars` used to be, and the line the
parity test reads for "by digest, never a tag". `TO-PIN` marks an
environment nothing has ever built an image for; it is not a digest and
the test refuses it as one, which is the intended reading until the first
promotion into that environment replaces it.

## How each value was derived

Every value is the one `modules/cloudrun` computed for the running service,
so Config Connector's first reconcile acquires and changes nothing:

- `metadata.name` is `qip-<env>-<name>`, the module's `local.name`, which is
  how acquisition by name works.
- `env` is the module's merged map — the catalogue's `env`, the `_FILE` path
  of each secret mount, the `_PATH` of each configuration file — **sorted by
  name**, because Terraform iterates a map sorted and Cloud Run compares the
  list in order; a reordering is a new revision.
- secret volumes mount at `/var/run/secrets/qip/<key>` and project
  `<file_name>` at mode 0400 (`256`); the secret id is `<name>-<env>` as
  `modules/secrets` names it.
- `/etc/qip/7ec25303c3f3ecac/universe.json` is the hash-named directory the
  module publishes the committed `data/datasets/universe.json` under
  (`substr(sha256(sha256(content)), 0, 16)`); a change to that file is a new
  directory in the bucket and a new path here, in one commit, and the parity
  test compares this path to the module's `config_file_paths` output.
- the egress sidecar's `-c /etc/envoy/envoy-fc9574203f973d2d.yaml` is the
  object `modules/egress-proxy` publishes from the committed bootstrap,
  named by its hash; port 9900 is its health listener.
- `cpuIdle` is `min_instances == 0`, `startupCpuBoost` is on, the probes are
  the module's, `timeout` is 300s, gen2, `ALL_TRAFFIC` through the zone's
  subnet with the zone's tag.

## `gcs` volumes — read before the first sync

The egress bootstrap and `universe.json` reach the running services as Cloud
Storage volumes (`gcs { bucket, read_only }` on the Terraform resource). The
Config Connector `RunService` reference lists no `gcs` volume — and that is
now a schema reading rather than a documentation reading. The CRD the
vendored operator version installs was fetched and its volume schema
enumerated on 2026-09-06:

```
curl -sS https://raw.githubusercontent.com/GoogleCloudPlatform/k8s-config-connector/v1.156.0/config/crds/resources/apiextensions.k8s.io_v1_customresourcedefinition_runservices.run.cnrm.cloud.google.com.yaml
sha256  e698dadd47be2595a9f6395b796abc670317dc16fdc56a97b31471363ab0d081
  (byte-identical at the `master` branch, so this is not a stale tag)
grep -ci gcs  →  0
spec.template.volumes items: cloudSqlInstance, emptyDir, name, secret
```

Four properties, and `gcs` is not one of them; nor is `nfs`. What was not
checked, because this environment's proxy refuses the GitHub contents API,
is whether a second CRD elsewhere in that directory serves the same kind —
so read this as "the CRD that generates `run.cnrm.cloud.google.com/v1beta1
RunService` from the Terraform provider carries no `gcs` volume", which is
the one these manifests are validated against.

These manifests carry them anyway, as the services run, and the Application
syncs with `Validate=true`. Three outcomes are possible on the first sync of
`dev`, and only the third is acceptable:

1. The schema does not admit `gcs` and the sync is refused with a
   validation error. Nothing changes in Cloud Run. ADR 0036 decision 4
   cannot be applied to these four workloads until the addon carries the
   field, and the ADR's rejected alternative (Terraform keeps the service)
   is the honest fallback for them.
2. The schema admits it but reconciles it differently from the provider —
   a new revision appears. The remedy is the manifest, never a manual
   `gcloud run services update`.
3. The schema admits it and the revision count does not move. That is the
   evidence the ADR asks for; record it in
   [`docs/DELIVERY-STATUS.md`](../../../docs/DELIVERY-STATUS.md), which
   absorbed the missing-infrastructure register on 2026-09-07.

What must not happen is a schema that prunes the unknown field and a
reconcile that removes the mount: a process that starts with no
`universe.json` at the path it was told, and a proxy with no bootstrap.
`Validate=true` is the line between outcome 1 and that.

## The metrics collector, when a digest is finally pinned

No manifest here carries one, and none may until an environment names
`metrics_collector_image_digest`: the parity test in `gitops.rs` counts
`qip-metrics-collector` containers and asserts the count equals
"the catalogue entry asks for one **and** the environment names a digest".
A sidecar written in ahead of the digest is a container Binary
Authorization refuses at admission, which reads as a broken deploy rather
than as a missing collector. The digest is refused upstream today —
`modules/observability/NOT-SCRAPED.md` has the finding and the commands.

What is already correct, so that the day it is pinned this is a short edit
rather than a design: `modules/cloudrun` publishes the `RunMonitoring`
document as `config.yaml` at the **root** of `qip-metrics-<env>-<name>-<project>`,
and exports `collector_mount_path` (`/etc/rungmp`) and
`collector_config_path` (`/etc/rungmp/config.yaml`). The second is not a
setting: it is the only path the sidecar's entrypoint opens. The sidecar
carries no environment, no secret and no identity of its own. The fragments
the manifests for `fastbrain` and `deepbrain` then need:

```yaml
    - name: qip-metrics-collector
      image: cloud-run-gmp-sidecar        # a logical name; kustomization.yaml pins the digest
      dependsOn:
      - fastbrain                         # `deepbrain` in deepbrain.yaml. The collector starts
                                          # after the workload and is stopped before it
      volumeMounts:
      - name: metrics-collector-config
        mountPath: /etc/rungmp            # modules/cloudrun's collector_mount_path
      resources:
        limits:
          cpu: '1'
          memory: 256Mi
    volumes:
    - name: metrics-collector-config
      gcs:
        bucket: qip-metrics-<env>-<name>-<project>
        readOnly: true
```

Three things this section had wrong or left unsaid until 2026-09-15, found by
reading Google's own instructions for the sidecar and the tests that would
check the paste. None of them is a reason to write the fragment early — the
parity test refuses a collector container while the environment names no
digest, and it is right to: a container pulling an image nothing has mirrored
is a revision Binary Authorization refuses, which reads as a broken deploy
rather than as a missing collector. They are the reasons the day it is pinned
is not only a paste.

- **`dependsOn` was missing from the fragment, and it is the half that is
  not decoration.** Google's instructions for adding the sidecar say to add
  a container-dependency annotation —
  `run.googleapis.com/container-dependencies: '{"collector":["app"]}'` — so
  that the collector "starts after and shuts down before the application
  container", and the same page says the collector "performs a start-up
  scrape after 10 seconds and performs a shut-down scrape, no matter how
  short-lived the instance is"
  (<https://cloud.google.com/stackdriver/docs/managed-prometheus/cloudrun-sidecar>,
  read 2026-09-15; it redirects to `docs.cloud.google.com` and answers 200).
  A shut-down scrape taken after the workload has already gone is the last
  scrape before a revision disappears, which is the one an operator most
  wants and the one nothing else will ever produce again. In this CRD shape
  that annotation is the container's `dependsOn`, exactly as the egress
  sidecar is already ordered — with the direction reversed, because the
  workload waits for the proxy and the collector waits for the workload.
- **The only thing checked about any of this is the container count.**
  `gitops.rs`'s parity test counts containers named `qip-metrics-collector`
  and asserts the count equals "the catalogue entry asks **and** the
  environment names a digest". Nothing reads the mount: on 2026-09-15
  `grep -rn rungmp backend/crates/tests/qip-acceptance/tests/` returned four
  lines, all in `infrastructure.rs` and all about `modules/cloudrun`'s own
  locals. So a fragment pasted with the mount one directory out, or with
  another workload's bucket, passes every gate in this repository. And it
  fails **invisibly**, in a way worse than the object-path bug it would
  reproduce: the sidecar's built-in default scrapes port 8080 at `/metrics`
  every 30 seconds, which for these two workloads is the same target the
  published document names, so a document that never arrived produces
  metrics that look right and a `RunMonitoring` nobody is reading. The
  assertion that would close this belongs beside the count — the mount equals
  the module's `collector_mount_path`, the volume names that workload's own
  `qip-metrics-<env>-<name>-<project>` — and it does not exist yet. Write it
  in the same commit as the fragment, not after.
- **Whether the document should arrive from a bucket at all is an open
  question, and it is the one decision the day cannot paste its way past.**
  Google documents the custom configuration arriving as a Secret Manager
  volume mounted at `/etc/rungmp`, with the secret version projected as
  `config.yaml`. This repository publishes it to a bucket and would mount a
  `gcs` volume — and the vendored CRD's volume schema, enumerated in the
  section above, lists `cloudSqlInstance`, `emptyDir`, `name` and `secret`
  and no `gcs`. Of the three `gcs` volumes these manifests carry, the
  collector's is therefore the only one with a documented alternative that
  the schema is known to admit. Moving it is not a manifest edit: the
  bucket, the `objectViewer` grant and the two outputs in `modules/cloudrun`
  would become a secret, a version and a `secretAccessor` grant, and that is
  an ADR, taken by a person, on the day. Recorded here so the next reader
  meets it before writing the mount rather than after a sync prunes it.

That volume is a `gcs` volume, so it is subject in full to the section
above: if the schema prunes `gcs`, the collector mounts nothing, reads its
built-in default and — per the paragraph above — charts something that looks
like a scrape. Prove the mount before believing a scrape, and do not flip
`workload_metrics_exist` on a manifest that merely applied.

## Other fields whose accepted form was not confirmable offline

- `versionRef.external: latest` on a secret volume item. The reference
  describes `versionRef` as "'latest' or an integer" and its `external`
  as "the `version` field of a `SecretManagerSecretVersion`"; the literal
  is what the provider takes. If the first sync refuses it, the full form
  `projects/<project>/secrets/<secret>/versions/latest` is the alternative.
- `IAMPolicyMember` on a `RunService` (`invokers.yaml`): Config Connector
  supports IAM on Cloud Run services; the first sync is the proof.
- The `managed_by` label is `config-connector` where the module wrote
  `terraform`. A service-level label is not part of the revision template,
  so the acquisition reconcile rewrites the label and creates no revision;
  a revision count that moved on the first sync is not explained by this.

## The proving hook

`prove-serving.yaml` is ADR 0036 decision 7: a post-sync `Job` running the
vendored `google-cloud-cli` as the `qip-prove-serving` service account —
bound through Workload Identity to `qip-<env>-argocd`, which holds
`roles/run.viewer` — that reads every `RunService` in the namespace from the
Kubernetes API, asks Cloud Run which revision each routes traffic to, and
fails the sync unless that revision's workload container carries the digest
the manifest names. Read by container name and condition type, never by
position, for the reason `deploy.yml`'s `prove-serving.py` did. It is the
one `Job` the Argo CD project admits and its image is not a `qip-*` binary.
