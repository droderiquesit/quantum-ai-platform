# Argo CD v3.5.2 — provenance

`upstream/install.yaml` is the non-HA install manifest at the tag the
project's `stable` branch named on 2026-09-04, copied byte for byte:

```
https://raw.githubusercontent.com/argoproj/argo-cd/v3.5.2/manifests/install.yaml
(version read from https://raw.githubusercontent.com/argoproj/argo-cd/stable/VERSION → 3.5.2)
sha256  9a87f2b3e14c278f12501eb0ef5c3955b27cf05370ca425381c6a908cf85a5c5
34050 lines; 3 CRDs, 6 Deployments, 1 StatefulSet, 8 Services, 7 NetworkPolicies, 2 empty Secrets; no Job
```

Images it names, and their index digests as resolved from each registry's
v2 API (`docker-content-digest` of the tag's manifest list):

| upstream | digest | in the bootstrap? |
|---|---|---|
| `quay.io/argoproj/argocd:v3.5.2` (8 containers) | `sha256:e2aadfae709d904e87f46ba4aa49601d827b3022db22cd4d03aae816a2e7097b` | yes |
| `public.ecr.aws/docker/library/redis:8.2.3-alpine` | `sha256:08ad0b1d280850169a790dba1393ff7a90aef951fc19632cf4d3ce4f78e679ba` | yes |
| `ghcr.io/dexidp/dex:v2.45.1` | `sha256:8499afd690c437f52301efd2b05b2455da5bd2dfc20332cd697dc9937f808462` | **no** — Dex is deleted by `base/kustomization.yaml`; the digest is recorded so a reviewer who re-enables SSO knows what the upstream file names |

`upstream/kustomization.yaml` is the one file beside it that is not
upstream's: a kustomization naming `install.yaml`, because kustomize refuses
a base that references a file outside its own directory and admits a
directory. Recorded here so this note covers every byte under `upstream/`:

```
sha256  b748b7715b56a19f4b19c07cf31b98dfee4380d6533b6c12ca1bd15392aeaa3b  upstream/kustomization.yaml
```

The two images that run are in `infrastructure/egress/vendored-images.txt`;
`overlays/<env>/kustomization.yaml` moves them to the environment's registry
at those digests. Dex is not vendored because nothing configures it: the
admin account is off and there is no SSO connector, so the identity that
operates Argo CD is the Kubernetes one.
