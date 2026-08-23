# Credentials

What the platform needs to reach anything real, where each value belongs, and
what is already known.

The organising rule: **an identifier is not a credential.** A project id, a
service-account email and a workload-identity provider path all appear in
resource names and pipeline configuration; hiding them buys nothing and costs
reproducibility. A key, a token or a session password is a different thing
entirely and never enters this repository.

---

## 1. Supplied

| Value | What it is | Where it lives |
|---|---|---|
| `project-d3f96b6b-852b-4460-b6d` | GCP project id | `infrastructure/environments/*/terraform.tfvars`, in version control |
| `claude-builder@project-d3f96b6b-852b-4460-b6d.iam.gserviceaccount.com` | Bootstrap service account | This document. Not referenced by Terraform — see below |

Neither is secret. Both are recorded so the next person does not have to ask.

### The bootstrap account is not the pipeline account

These are deliberately different identities and must not be merged.

**`claude-builder` is the bootstrap identity.** It is what *applies* Terraform:
it creates the cluster, the network, the KMS keys, the service accounts. It
therefore holds project-level administrative roles, and it is used by a person
or a privileged automation run, rarely.

**The pipeline identity is created by Terraform**, in
`infrastructure/terraform/modules/cicd`. It can push an image and apply a
manifest. It cannot create a cluster, read a secret's payload, or grant itself
anything.

Pointing the deploy pipeline at `claude-builder` would be the single most
damaging shortcut available here: every CI run would hold permanent project
admin, and a compromised workflow file would own the project. The pipeline gets
the narrow account Terraform makes for it.

`claude-builder` is not named in any `.tf` file for the same reason — Terraform
does not need to know which identity is applying it, and hard-coding one would
make the configuration harder to run from a different account later.

---

## 2. Still required

Nothing below exists in this environment. This is the checklist.

### To apply the infrastructure at all

| Requirement | Notes |
|---|---|
| `terraform` ≥ 1.9 | Not installed here |
| `gcloud` CLI | Not installed here |
| Credentials for `claude-builder` | See §3 — via impersonation, not a key file |
| A GCS bucket for Terraform state | `backend "gcs"` is configured with prefix `qip/state`; the bucket is not created by this configuration, because state cannot bootstrap itself |
| Project APIs enabled | container, compute, artifactregistry, secretmanager, cloudkms, iam, monitoring, logging, storage |

### For the deploy pipeline

Set as GitHub Actions **variables** (not secrets — all three are identifiers):

| Variable | Source |
|---|---|
| `GCP_PROJECT` | `project-d3f96b6b-852b-4460-b6d` |
| `GCP_WORKLOAD_IDENTITY_PROVIDER` | `terraform output workload_identity_provider` |
| `GCP_DEPLOY_SERVICE_ACCOUNT` | `terraform output deploy_service_account` — the Terraform-created one |
| `GCP_REGION` | The Artifact Registry region |

There is deliberately **no** GitHub secret holding a key. The pipeline uses
workload identity federation: GitHub mints a short-lived OIDC token, GCP
exchanges it for a credential scoped to one job. An `attribute_condition` pins
the repository, so a token from any other repository is refused.

### Setting the pipeline variables

All four are **variables, not secrets** — every one is an identifier that
appears in resource names anyway, and marking them secret only makes them
harder to debug. Run these once you have applied the infrastructure:

```sh
gh variable set GCP_PROJECT --repo droderiquesit/quantum-ai-platform \
  --body "project-d3f96b6b-852b-4460-b6d"

gh variable set GCP_REGION --repo droderiquesit/quantum-ai-platform \
  --body "europe-west2"

# Both of these are Terraform outputs. They do not exist until apply.
gh variable set GCP_WORKLOAD_IDENTITY_PROVIDER --repo droderiquesit/quantum-ai-platform \
  --body "$(terraform -chdir=infrastructure/terraform output -raw workload_identity_provider)"

gh variable set GCP_DEPLOY_SERVICE_ACCOUNT --repo droderiquesit/quantum-ai-platform \
  --body "$(terraform -chdir=infrastructure/terraform output -raw deploy_service_account)"
```

Only `GCP_PROJECT` and `GCP_REGION` can be set today. The other two are
Terraform outputs and do not exist until the infrastructure has been applied
once — which is the correct ordering, not an inconvenience: the provider and
the pipeline account are created by the apply.

**`GCP_DEPLOY_SERVICE_ACCOUNT` must not be `claude-builder`.** That is the
bootstrap identity and it holds project admin. The value belongs to the narrow
account Terraform creates, which can push an image and apply a manifest and
nothing else. Setting it to the bootstrap account would give every CI run
permanent project admin and make one compromised workflow file enough to own
the project — see the section above.

**No GitHub secret is needed at all.** There is deliberately no key to store:
the pipeline exchanges a GitHub OIDC token for a short-lived credential, and
`attribute_condition` pins the repository so a token minted anywhere else is
refused.

### For the platform at runtime

| Secret | Consumer | Injection |
|---|---|---|
| `qip-tokens` | `qip-api` — operator, approver, analyst, viewer, monitor bearer tokens | Secret Manager → CSI mount |
| `qip-capital-envelope-key` | `qip-edge-node` — verifies signed capital envelopes | Secret Manager → CSI mount |
| `qip-quantum-token` | Quantum provider | Secret Manager. **No IAM reader binding exists yet** — the audit records this |
| Venue feed endpoint + session credential | Edge cell market data | Not yet modelled. Per venue |
| Venue gateway endpoint + order-entry credential | Edge cell execution | Not yet modelled. Per venue |
| Drop-copy endpoint | Independent fill channel | Not yet modelled |

The three venue values are what `qip-edge-node` prints on start-up as
"awaiting". It serves its health surface without them and trades nothing,
which is the correct degraded state.

### Not yet reachable, per the audit

Each names its exact requirement in
`docs/operations/external-dependencies.md`: BigLake/Iceberg, BigQuery,
Bigtable, Spanner, Spanner Graph, AlloyDB, Memorystore, Vertex AI, IBM Quantum,
and any hosted language model. The last two need more than a credential — IBM
Quantum and a hosted model both require an HTTP client with TLS, which this
build does not have and which ADR 0009 now permits at the I/O edge.

---

## 3. How to authenticate `claude-builder`

**Do not create a service-account key.** A downloaded JSON key is a
long-lived, exfiltratable, non-rotating credential, and it is the root cause of
a large share of cloud compromises. Every path below avoids one.

**Preferred — impersonation from a human identity:**

```sh
gcloud auth application-default login
gcloud config set project project-d3f96b6b-852b-4460-b6d
export GOOGLE_IMPERSONATE_SERVICE_ACCOUNT=\
claude-builder@project-d3f96b6b-852b-4460-b6d.iam.gserviceaccount.com
terraform -chdir=infrastructure/terraform apply \
  -var-file=../environments/development/terraform.tfvars
```

The human needs `roles/iam.serviceAccountTokenCreator` on `claude-builder`.
Credentials are short-lived and tied to a person, so the audit log names who
applied what.

**Alternative — workload identity federation**, if Terraform runs in CI. Same
mechanism as the deploy pipeline, no key.

**If a key already exists**, it should be deleted after impersonation is set
up. `gcloud iam service-accounts keys list` will show any.

### Roles `claude-builder` needs

Narrower than Owner, which is the point:

```
roles/container.admin              GKE
roles/compute.networkAdmin         VPC, subnets, firewall, NAT
roles/iam.serviceAccountAdmin      the accounts Terraform creates
roles/iam.serviceAccountUser       attaching them to workloads
roles/resourcemanager.projectIamAdmin   IAM bindings
roles/secretmanager.admin          secret containers, not payloads
roles/cloudkms.admin               keyring and keys
roles/artifactregistry.admin       the image repository
roles/storage.admin                the evidence bucket and state bucket
roles/monitoring.editor            alert policies
```

Note `secretmanager.admin` creates secret *containers*. Payloads are written
separately by a person, so no automation identity has to hold them.

---

## 4. Rules

1. **No credential in the repository.** Not in source, Terraform, manifests,
   images, logs, commit messages or this file. `scripts/check-secrets.sh`
   enforces it in CI, and `no_secret_value_appears_in_the_terraform` and
   `no_credential_appears_in_a_kubernetes_manifest` enforce it in the test
   suite.
2. **No service-account keys** where impersonation or federation will do.
3. **Identifiers are configuration.** Project ids and service-account emails
   belong in version control. Treating them as secrets produces the worse
   outcome where nobody can reproduce a deployment and real secrets get the
   same casual handling as fake ones.
4. **Secret Manager holds payloads; Terraform holds containers.** The
   configuration creates the secret and its IAM; a person writes the value.
   That way `terraform apply` never needs the plaintext.
5. **Live trading is not a credential problem.** Every environment ships with
   `autonomy_ceiling = "paper_trading"`, and the venue credential's IAM binding
   does not exist where that is true. Supplying a broker credential does not
   enable live trading; raising the ceiling is a separate, reviewed change.

---

## 5. What this environment can and cannot do

It has no `gcloud`, no `terraform`, no credentials and no application-default
credentials. It therefore cannot authenticate, plan, apply or deploy anything,
and a service-account email does not change that — an identifier is not a key.

The infrastructure is **specified and structurally tested**: 54 tests read the
Terraform and manifests and assert properties a plan would not catch, such as
the node pool having no public addresses and no workload identity holding
delete on the evidence bucket. It has never been validated against the provider
schema, because that needs a provider download, and never applied.
