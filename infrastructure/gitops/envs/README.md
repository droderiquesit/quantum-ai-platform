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
Config Connector `RunService` reference lists no `gcs` volume. These
manifests carry them anyway, as the services run, and the Application syncs
with `Validate=true`. Three outcomes are possible on the first sync of
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
   `docs/ops/missing-infrastructure-register.md`.

What must not happen is a schema that prunes the unknown field and a
reconcile that removes the mount: a process that starts with no
`universe.json` at the path it was told, and a proxy with no bootstrap.
`Validate=true` is the line between outcome 1 and that.

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
