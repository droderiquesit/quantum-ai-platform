# Kargo v1.11.4 — provenance

Kargo publishes no static manifest; its install is a Helm chart. ADR 0036
rejects a Helm provider and Helm in the workflow, so the chart was rendered
once, offline, and the rendered bytes are what is vendored. `upstream/install.yaml`
is the output of:

```
helm pull oci://ghcr.io/akuity/kargo-charts/kargo --version 1.11.4
  # Pulled: ghcr.io/akuity/kargo-charts/kargo:1.11.4
  # Digest: sha256:0a0cb3b7a4d6b35aa37bc0971857a6420ebc569bf73d4cae8728b7d06a8211de
  # sha256 of kargo-1.11.4.tgz: d655415d53a115101676e93f0489f7786f444155069fd3c199dccda1c77edf06
helm template kargo ./kargo --namespace kargo \
  --set api.adminAccount.enabled=false \
  --set api.oidc.enabled=false \
  --set api.tls.enabled=false \
  --set api.ingress.enabled=false \
  --set api.service.type=ClusterIP \
  --set externalWebhooksServer.enabled=false
  # helm v3.16.4+g7877b45, from get.helm.sh, sha256 fc307327959aa38ed8f9f7e66d45492bb022a66c3e5da6063958254b9767d179
```

```
sha256  8187049ec5948a7669e35c9044d8cbe9cb0119341d493b6d2a82ab8b8eae2056
9031 lines; 9 CRDs, 4 Deployments, 1 CronJob, 3 Namespaces, 1 Certificate + 1 Issuer (cert-manager),
1 MutatingWebhookConfiguration, 1 ValidatingWebhookConfiguration, 1 Secret (`kargo-api`, empty); no Helm hook
```

1.11.4 was the newest plain release tag in the chart repository's tag list
on 2026-09-04 (`ghcr.io/v2/akuity/kargo-charts/kargo/tags/list`; GitHub's
release API is unreachable through this environment's proxy).

`upstream/kustomization.yaml` is the one file beside it that is not the
render's: a kustomization naming `install.yaml`, because kustomize refuses a
base that references a file outside its own directory and admits a
directory. Recorded here so this note covers every byte under `upstream/`:

```
sha256  b748b7715b56a19f4b19c07cf31b98dfee4380d6533b6c12ca1bd15392aeaa3b  upstream/kustomization.yaml
```

The one image the render names, in five containers, and its index digest
from `ghcr.io`'s registry v2 API:

| upstream | digest |
|---|---|
| `ghcr.io/akuity/kargo:v1.11.4` | `sha256:1413bdb63b1ad409c0a38a0ae5d6f4080e1fd3226aaf8344dfe0e5552921f533` |

The same line is in `infrastructure/egress/vendored-images.txt`;
`overlays/<env>/kustomization.yaml` moves it to the environment's registry
at that digest. Re-rendering with the same chart digest and the same values
reproduces the same bytes; a new Kargo version is a new pull, a new render,
a new sha256 here and a new line in the vendored list, in one commit.
