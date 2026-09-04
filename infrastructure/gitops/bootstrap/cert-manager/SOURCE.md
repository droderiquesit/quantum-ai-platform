# cert-manager v1.21.1 — provenance

`upstream/cert-manager.yaml` is the static install manifest cert-manager
publishes as a release asset, copied byte for byte:

```
https://github.com/cert-manager/cert-manager/releases/download/v1.21.1/cert-manager.yaml
sha256  5f6a499b8c1857d57f560f536e0dcc830914b45c420899fe7ad0692c8624e408
13960 lines; 6 CRDs, 3 Deployments, 1 Namespace; no Helm hook, no Job
```

v1.21.1 was the newest tag with a published manifest on 2026-09-04, found by
probing `releases/download/<tag>/cert-manager.yaml` from v1.18.0 upward
(GitHub's releases page and API are unreachable through this environment's
proxy, so the release list itself could not be read).

`upstream/kustomization.yaml` is the one file beside it that is not
upstream's: a kustomization naming `cert-manager.yaml`, because kustomize
refuses a base that references a file outside its own directory and admits
a directory. Recorded here so this note covers every byte under `upstream/`:

```
sha256  6c073d4a58a16c9c9db708b061615df59038bbbebc1100ec94da96eb9a123416  upstream/kustomization.yaml
```

The three images it names, resolved from `quay.io`'s registry v2 API by the
`docker-content-digest` header of each tag's manifest list (every one is a
multi-arch index, so `vendor.yml` signs the index and each platform
manifest under it):

| upstream | digest |
|---|---|
| `quay.io/jetstack/cert-manager-controller:v1.21.1` | `sha256:416a2d76870d996460e62bd7f521bf14fa017be9e3e904aab92163a331fcb61a` |
| `quay.io/jetstack/cert-manager-webhook:v1.21.1` | `sha256:d8b3961b51c8c7320633f8208dc46bf88aa13804d0f7cbe48a096b2c523cee42` |
| `quay.io/jetstack/cert-manager-cainjector:v1.21.1` | `sha256:ccf6b919ec0500745a47a910118f834f9636d0aac1ff221245cd2557ed8c7c98` |

The same three lines are in `infrastructure/egress/vendored-images.txt`, and
`overlays/<env>/kustomization.yaml` moves each `image:` in the upstream file
to the environment's own registry at that digest. The upstream file is not
edited: what a reviewer diffs against the next release is the bytes above.
