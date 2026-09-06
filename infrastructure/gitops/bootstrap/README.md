# bootstrap/

What runs on the control-plane cluster, as vendored manifests, and the order
`infra.yml`'s `bootstrap the GitOps controllers` step applies them in.

| Order | Directory | What | Why in this position |
|---|---|---|---|
| 1 | `cert-manager/` | cert-manager v1.21.1, three images | Kargo's webhook certificate is a cert-manager `Certificate`; the CRDs must exist before the render that names one is applied |
| 2 | `argocd/` | Argo CD v3.5.2, two images (Dex deleted) | Independent of the others; before Kargo because Kargo's `argocd-update` step needs the `Application` CRD |
| 3 | `kargo/` | Kargo v1.11.4 rendered from its chart, one image | After cert-manager and Argo CD |
| 4 | `config-connector-operator/` | The Config Connector operator 1.156.0, Autopilot variant, one image | ADR 0036 had this as the GKE addon; the API refused the addon on an Autopilot cluster (infra.yml runs 34 and 35, 2026-09-05), so the operator is vendored like the three above. The step waits for its StatefulSet and for the `ConfigConnector` CRD to be established, because the next row names that kind |
| 5 | `config-connector/` | The `ConfigConnector` object and the `qip-run` namespace | After the operator's CRD exists; the object names the identity, and the step waits for the controller manager the operator installs |
| 6 | the two App-key Secrets | Projected from Secret Manager by the step, never from this tree | After the namespaces they land in exist |
| 7 | `../argocd/overlays/<env>` | The project and the Application | After Argo CD's CRDs |
| 8 | `../kargo/overlays/<env>` | The project, then everything in it | Applied twice with a wait between: Kargo creates the project's namespace |

One thing the operator row does not vendor, so a reader does not assume it
did: the operator installs the `cnrm-system` controllers itself from four
`gcr.io/gke-release/cnrm/*` images pinned inside its own image, which no
overlay can move. `config-connector-operator/SOURCE.md` names them and their
digests, and says what admits them — Google's global policy, which
`modules/binaryauthorization` enables and which has not been observed to
admit that registry. The wait in row 5 is where that is found out.

Each `upstream/` file is the bytes the project published, unedited, with a
`SOURCE.md` beside it recording the URL, the sha256, the version resolution
and every image digest. Each `base/` is the posture; each `overlays/<env>/`
moves every image to that environment's registry at the digest
`infrastructure/egress/vendored-images.txt` reviewed and `vendor.yml`
mirrored and attested. The cluster's Binary Authorization policy is the
project's — one rule, no exemptions — so an image not on that list does not
start, which is the property that makes the digest pins load-bearing rather
than tidy.

## Reaching the cluster

The endpoint is private and its only authorised network is the management
zone's range. A GitHub runner reaches it through the fleet's Connect
gateway — `gcloud container fleet memberships get-credentials` — which is a
Google API call authorised by `roles/gkehub.gatewayEditor` and the custom
role `modules/gitops-control-plane` grants the infrastructure account,
carrying exactly the `container.*` permissions the manifests above need
and not `roles/container.admin`. ADR 0036 decision 2 says
`gcloud container clusters get-credentials`; that command yields an
endpoint the runner cannot route to, and the gateway is the same
credential reaching the same API server by the one path that exists. This
is stated rather than proven: no bootstrap has run.

## The two credentials

ADR 0036 decision 3. Two GitHub App installations on this repository —
`qip-argocd-<env>` with `contents: read`, `qip-kargo-<env>` with
`contents: write` — each seeded out of band into the Secret Manager secret
`modules/secrets` creates empty (`qip-github-app-argocd-<env>`,
`qip-github-app-kargo-<env>`) as one JSON document:

```json
{"app_id": "<number>", "installation_id": "<number>", "private_key": "-----BEGIN RSA PRIVATE KEY-----\n..."}
```

The bootstrap reads each with `gcloud secrets versions access` into a
mode-600 file in a private directory removed on every exit path, writes a
Kubernetes `Secret` from it — `argocd/qip-repository` in Argo CD's
repository shape, `qip/qip-repository` in Kargo's `cred-type: git` shape —
applies it, and removes the file. Nothing is echoed, nothing reaches a
workflow output or a step summary, and nothing in this tree ever holds a
value. etcd is encrypted with `qip-<env>-control-plane-etcd` in the
environment's key ring. The private key is the one long-lived third-party
credential this platform holds; its rotation is a person's schedule, and
ADR 0036 names that cost rather than assuming a rotation exists.

## cert-manager, and why it is vendored rather than avoided

Kargo's admission webhooks serve TLS. The chart provisions the certificate
as a cert-manager `Certificate` from a self-signed `Issuer` and injects the
CA into four webhook configurations with the `cert-manager.io/inject-ca-from`
annotation. Its only other option is `webhooksServer.tls.selfSignedCert=false`
with an operator-supplied certificate `Secret` and a `caBundle` value
spliced into those four objects — a keypair generated in a workflow,
written into manifests by substitution, and rotated by nobody. That is the
sed-rendered-manifest shape ADR 0017 recorded and ADR 0024 retired. Three
digest-pinned images, vendored, scanned and attested like every other
foreign image, and used for nothing else, is the smaller cost; ADR 0024's
own note that cert-manager existed only for the webhooks is exactly why it
returns with them.

## Identity, without a login

Argo CD's admin account is off and no SSO is configured; Dex is deleted
from the install. Kargo's admin account is off and OIDC is off. An operator
acts as their GKE identity: `argocd --core` against the kubeconfig the
gateway wrote, and `kargo login --kubeconfig`. There is no password in this
design and no bearer token at rest that is not a Kubernetes service
account's; what a person can do is what their IAM role lets the Kubernetes
API do, which is the same audit trail everything else on the platform has.
