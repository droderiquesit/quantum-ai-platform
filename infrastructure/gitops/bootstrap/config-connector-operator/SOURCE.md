# Config Connector operator 1.156.0 — provenance

`upstream/autopilot-configconnector-operator.yaml` is the Autopilot variant
of the operator manifest Google publishes in its release bundle, copied
byte for byte from the versioned bundle, not from `latest`:

```
https://storage.googleapis.com/configconnector-operator/1.156.0/release-bundle.tar.gz
sha256  1ccad0867c46d965d0dc6f2c73ff65e9970e46459494c62ff009c4485b6ecbbc  release-bundle.tar.gz
  (identical bytes to latest/release-bundle.tar.gz on 2026-09-05)
operator-system/autopilot-configconnector-operator.yaml, from that archive:
sha256  f93ad679788a94361d6e9a3e7a605e34351c68115223bdc2ed585767edddcdb6
3503 lines; 8 CRDs, 1 Namespace, 1 ServiceAccount, 2 ClusterRoles, 2 ClusterRoleBindings, 1 Service, 1 StatefulSet; no Job, no webhook
```

1.156.0 was the newest versioned prefix in the bucket's listing on
2026-09-05 (`?max-keys=1000`, sorted by version: `1.154.1`, `1.155.1`,
`1.156.0`, then `latest`), and the `latest/` archive hashed to the same
bytes. The Autopilot variant differs from `configconnector-operator.yaml`
in two lines: `--local-repo=/configconnector-operator/autopilot-channels`
instead of `channels`, and no `GOMEMLIMIT` environment value. It is the
variant Google's manual-install path names for Autopilot clusters; the
addon path this replaced was refused by the API (`base/kustomization.yaml`).

`upstream/kustomization.yaml` is the one file beside it that is not
upstream's: a kustomization naming the manifest, because kustomize refuses
a base that references a file outside its own directory and admits a
directory. Recorded here so this note covers every byte under `upstream/`:

```
sha256  d0751aef57d46a2b40f3b3312bec8d36d8e2142821eaa99b09cfeed1e1ea9526  upstream/kustomization.yaml
```

## The one image the manifest names

Resolved from `gcr.io`'s registry v2 API on 2026-09-05, two ways that
agree — the `docker-content-digest` header for the tag, and the sha256 of
the manifest bytes that header names:

| upstream | digest | in the bootstrap? |
|---|---|---|
| `gcr.io/gke-release/cnrm/operator:1.156.0` | `sha256:ed4bd32055c435af80a33131ff8a92c9d75b22c29bf9de5e9c8c3a677db2156c` | yes |

It is a single `application/vnd.docker.distribution.manifest.v2+json`
manifest, not a multi-arch index (the registry answers 404 to an index
`Accept`), so `vendor.yml`'s child-manifest loop signs exactly this one
digest. The same line is in `infrastructure/egress/vendored-images.txt`,
and `overlays/<env>/kustomization.yaml` moves the `image:` in the upstream
file to the environment's own registry at that digest.

## The four images the operator installs, which nothing here can move

The operator does not run Config Connector; it installs it. In cluster mode
under Workload Identity it applies
`configconnector-operator/autopilot-channels/packages/configconnector/1.156.0/cluster/workload-identity/0-cnrm-system.yaml`
from inside its own image — read on 2026-09-05 out of the 54,780,211-byte
layer `sha256:8c42193ba806c06b570a25420f87d1a5510fa535f5d9d39dd5cde1f97cc16e32`;
the channel's `stable` file names `1.156.0` as its one version — and that
file runs:

| image the operator applies | digest (`docker-content-digest`, 2026-09-05) |
|---|---|
| `gcr.io/gke-release/cnrm/controller:1.156.0` | `sha256:4c5c85c0d47a6f0d38da5bbd03b555567c2626bedc0e8de04b0aa7d6e0445255` |
| `gcr.io/gke-release/cnrm/webhook:1.156.0` | `sha256:47121f1cb8e5561cc638b795c40351b835fcdedccac624ae8054dc23f36fd45f` |
| `gcr.io/gke-release/cnrm/deletiondefender:1.156.0` | `sha256:86e4567182389151a4875120e54d47ea01e001c036ec7f1074ebe35d0b9eafe3` |
| `gcr.io/gke-release/cnrm/recorder:1.156.0` | `sha256:c4a04abd1cec4dc0af61bb3f2c5b3dc33b77dfe595db50e1edc9adcc2a972c53` |

These are not in `vendored-images.txt` because no manifest in this tree
names them and no overlay can redirect them: the operator writes them from
its bundled channel, and the `ControllerResource` customisation it offers
changes resource limits, not images. They are admitted to the cluster only
if Google's global policy — enabled by `modules/binaryauthorization`'s
`global_policy_evaluation_mode = "ENABLE"`, evaluated before the project's
one rule — admits Google's own release registry. **That was not observed.**
The public documentation names examples under `gcr.io/gke-release/` without
publishing the system policy's allowlist, and the policy itself
(`locations/<region>/policy`) answers 401 to an anonymous read. As the GKE
addon these controllers were Google-managed system workloads and the
question did not arise; as a manual install it does, and the first
bootstrap's `kubectl -n cnrm-system wait` is where it is answered. A denial
there names the image and the policy; the answer to it is a decision about
`exempt_image_patterns` (`infrastructure/terraform/main.tf` argues against
one by name) or a request to Google, and not a pattern added in passing.
