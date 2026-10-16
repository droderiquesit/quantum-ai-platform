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
- credential secret volumes mount at `/var/run/secrets/qip/<key>` and
  project `<file_name>` at mode 0400 (`256`); the secret id is
  `<name>-<env>` as `modules/secrets` names it. The two committed
  configuration files are secret volumes too, at the paths the next two
  bullets give and at mode 0444 (`292`); the section below on
  `config-files` and `egress-bootstrap` says why the mode differs.
- `QIP_UNIVERSE_PATH` is `/etc/qip/<prefix>/universe.json`: the hash-named
  directory the module publishes the committed `data/datasets/universe.json`
  under, `substr(sha256(join("\n", [sha256(content)])), 0, 16)` — the first
  sixteen hex characters of the SHA-256 of the file's own lowercase-hex
  SHA-256 digest, the join being that one digest while no tfvars names a
  second file. **No value is written here, and this bullet is why.** It
  carried a literal prefix and said "the parity test compares this path to
  the module's `config_file_paths` output"; no such test existed. So when
  `universe.json` changed in `4a571de6` (2026-09-21) the bucket got a new
  directory, the twelve manifests kept the old one, and `infra.yml` run
  114's diagnose (2026-10-13) found every `fastbrain` exiting 78 unable to
  read a path the bucket no longer held. A change to that file is a new
  directory in the bucket, a new path and mount point in every manifest,
  and a new secret version, in one commit.
  Recompute with
  `sha256sum data/datasets/universe.json | cut -c1-64 | tr -d '\n' | sha256sum | cut -c1-16`;
  `gitops.rs`'s
  `every_hash_named_path_a_manifest_mounts_is_the_name_terraform_publishes_the_committed_bytes_under`
  recomputes it from the bytes and refuses a manifest whose literal differs.
- the egress sidecar's `-c /etc/envoy/envoy-<hash>.yaml` is the object
  `modules/egress-proxy` publishes from the committed bootstrap,
  `envoy-${substr(sha256(file), 0, 16)}.yaml` — the first sixteen hex
  characters of the SHA-256 of `infrastructure/egress/envoy.yaml`'s bytes.
  Same rule, same test, same history: the bootstrap changed in `349638db`
  (2026-10-05), eight manifests kept the old name, and run 114 found
  `deepbrain`'s Envoy dying on `Invalid path`. Recompute with
  `sha256sum infrastructure/egress/envoy.yaml | cut -c1-16`. The same
  name is the one item the `egress-bootstrap` secret volume projects, so
  a stale hash is caught in the argument and in the item by the same
  test. Port 9900 is its health listener.
- `cpuIdle` is `min_instances == 0`, `startupCpuBoost` is on, the probes are
  the module's, `timeout` is 300s, gen2, `ALL_TRAFFIC` through the zone's
  subnet with the zone's tag. Every probe's `timeoutSeconds` is strictly
  below its `periodSeconds`, because Cloud Run refuses the template
  otherwise and the `gcloud run services update` behind the IAP gate was
  refused on exactly that in run 119; the `gitops` suite holds it.

## `config-files` and `egress-bootstrap` are secret volumes, because the CRD has no `gcs`

The egress bootstrap and `universe.json` reach a revision as Secret Manager
secret versions — `qip-egress-bootstrap-<env>` and
`qip-universe-catalogue-<env>`, mounted as `secret` volumes — and not as
the `gcs { bucket, read_only }` volumes `modules/cloudrun` rendered on the
Terraform resource before ADR 0036 moved the service into a manifest. This
section used to pose a question about those `gcs` volumes, with three
outcomes a first sync could answer and a fourth it named as what must not
happen. The question is answered, and the answer is the fourth (ADR 0036,
amendment of 2026-10-13).

The schema reading has not changed. The CRD the vendored operator installs
was fetched and its volume schema enumerated on 2026-09-06, and it was
byte-identical at upstream `master` on 2026-10-13:

```
curl -sS https://raw.githubusercontent.com/GoogleCloudPlatform/k8s-config-connector/v1.156.0/config/crds/resources/apiextensions.k8s.io_v1_customresourcedefinition_runservices.run.cnrm.cloud.google.com.yaml
sha256  e698dadd47be2595a9f6395b796abc670317dc16fdc56a97b31471363ab0d081
  (byte-identical at the `master` branch, so this is not a stale tag)
grep -ci gcs  →  0
spec.template.volumes items: cloudSqlInstance, emptyDir, name, secret
```

Four properties, three of them volume kinds, and `gcs` is not among them;
nor is `nfs`. What the earlier version of this section hoped was that
`Validate=true` on the Application would turn that into a refused sync — a
validation error, nothing changed in Cloud Run. It did not. The API server
pruned the unknown key rather than rejecting the object, Config Connector
reconciled a service with no such mount, and Argo CD reported exactly these
three `RunService`s OutOfSync: the target carries a field the live object
can never carry, so no sync converges and the diff never closes.
`infra.yml` run 116 (`apps`, dev, 2026-10-13) failed in its bootstrap step
with the Application still reading `OutOfSync Healthy` when the wait gave
up and all three revisions failing their startup probes, and run 117
(`diagnose`, dev, the same day) read the consequence off the containers — the API's and the deep
brain's Envoy sidecar dying on `Invalid path: /etc/envoy/envoy-<hash>.yaml`,
and the fast brain exiting 78 because `QIP_UNIVERSE_PATH` named
`/etc/qip/<hash>/universe.json`, "which cannot be read". A process that
starts with no `universe.json` at the path it was told, and a proxy with no
bootstrap: the sentence this section wrote as a warning is the one it now
records as a finding. Why `Validate=true` did not refuse the object was not
established and this file does not guess at it; what is established is
that it did not, and a gate that is believed to fire and does not is worse
than no gate, because nobody looks behind it.

**What travels now, and what did not move.** Terraform declares the two
containers beside every other slot `modules/secrets` names; `infra.yml`'s
seed step writes a version from the checkout's bytes when the file's digest
differs from the latest enabled version's; each manifest mounts the
container as a `secret` volume; and `apps` stamps the version onto the
revision exactly as it stamps every mounted credential. The paths the
processes read did not move — `QIP_UNIVERSE_PATH` is still
`/etc/qip/<prefix>/universe.json` and the sidecar still reads
`-c /etc/envoy/envoy-<hash>.yaml` — because a path a process opens is the
contract and the volume behind it is not. What moved is one mount point: a
secret volume projects its items flat, so `config-files` mounts at the hash
directory itself rather than at `/etc/qip` with the hash as a subdirectory
the bucket used to supply. `egress-bootstrap` still mounts at `/etc/envoy`
and projects the bootstrap under its hashed name.

Both items are mode `292` (0444) where every credential mount is `256`
(0400), and each manifest says why beside the value: the bytes are a
committed, reviewed file, not a credential, and both the Envoy image and
the workload image run as a non-root user. The parity test keeps asserting
`256` on every volume the catalogue entry names as a secret and says
nothing about these two, which is the right split — a mode that is correct
for a file anyone can read in this repository is the wrong one for a key.

**What is kept, on purpose.** The buckets and hash-named objects
`modules/cloudrun` and `modules/egress-proxy` publish are still created and
still Terraform's record of the bytes, and the hashed names — `<prefix>`
and `envoy-<hash>` — are still what `gitops.rs`'s
`every_hash_named_path_a_manifest_mounts_is_the_name_terraform_publishes_the_committed_bytes_under`
recomputes from the committed files and refuses a manifest for disagreeing
with. So the rule in the section above holds, with one more clause: a
change to either file is a new hashed name, a new literal in every
manifest, **and a new secret version**, in one commit. The parity test
catches a stale literal. A stale version is caught by the seed step
comparing digests before it writes — the workflow's own comments are the
record of that rule — and by nothing in this repository's gates, which is
named here rather than closed. Secret Manager caps a version at 64 KiB;
`wc -c infrastructure/egress/envoy.yaml data/datasets/universe.json` is
the check, no figure is written here because the bootstrap grows, and a
file that outgrows the cap needs a different road.

**What refuses the pruned mount coming back by habit** is
`grep -rnE 'gcs[:]' infrastructure/gitops/envs` printing nothing — the
key, not the word, because every manifest's comment names the word, and
the brackets so that this sentence does not match the check it cites: a
check whose own citation matches it reads itself — and the parity
assertion in `gitops.rs` on the `config-files` volume, the one that
required a read-only `gcs` mount at `/etc/qip` until this change and the
place the secret-volume assertion belongs. The day a `RunService` CRD
carries `gcs`, the module's design returns and the two containers retire;
the ADR amendment says so. Until then a `gcs` key in any manifest here is a
mount nobody will ever receive, reported as OutOfSync forever.

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
```

plus a `volumes` entry named `metrics-collector-config` of the shape
`config-files` takes in every manifest here — a `secret` volume projecting
one item, `config.yaml`, at mode `292` — naming a container nothing
declares today. Not a `gcs` volume of `qip-metrics-<env>-<name>-<project>`,
which is what this fragment showed until 2026-10-13: the section above
records what the API server does with one.

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
- **Whether the document arrives from a bucket is no longer open, and
  it is still the one decision the day cannot paste its way past.** Google
  documents the custom configuration arriving as a Secret Manager volume
  mounted at `/etc/rungmp`, with the secret version projected as
  `config.yaml`. This repository publishes it to a bucket, and a `gcs`
  volume is what the section above records the API server pruning from
  the other two files — so the document arrives as a secret version or
  it does not arrive. Moving it is not a manifest edit: the bucket, the
  `objectViewer` grant and the two outputs in `modules/cloudrun` become a
  container, a version the seed step writes from the rendered document
  and a `secretAccessor` grant — the road ADR 0036's amendment of
  2026-10-13 built for the universe and the bootstrap — and that is a
  Terraform change taken by a person, on the day. Recorded here so the
  next reader meets it before writing the mount rather than after a sync
  prunes it.

A collector whose document never arrived mounts nothing, reads its
built-in default and — per the paragraph above — charts something that
looks like a scrape; the section above records that a `gcs` volume is
exactly how a document never arrives, silently. Prove the mount before
believing a scrape, and do not flip `workload_metrics_exist` on a manifest
that merely applied.

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
