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
| `project-d3f96b6b-852b-4460-b6d` | GCP project id | `infrastructure/environments/{dev,test,stage,prod}/terraform.tfvars`, in version control |
| `claude-builder@project-d3f96b6b-852b-4460-b6d.iam.gserviceaccount.com` | Bootstrap service account | This document. Not referenced by Terraform — see below |

Neither is secret. Both are recorded so the next person does not have to ask.

### The bootstrap account is not the pipeline account

These are deliberately different identities and must not be merged.

**`claude-builder` is the bootstrap identity.** It is what *applies* Terraform:
it creates the network, the trust zones, the KMS keys, the service accounts,
the Cloud Run services and any execution node group. It therefore holds
project-level administrative roles, and it is used by a person or a privileged
automation run, rarely.

**The pipeline identity is created by Terraform**, in
`infrastructure/terraform/modules/cicd`. It can push an image and move a Cloud
Run service to a digest. It cannot create a service, read a secret's payload,
or grant itself anything.

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

### Before `bootstrap-deploy.sh` can run

The script does everything it can do without authority it does not have. When
you run it as the project's owner — the ordinary case for a personal project —
only the first two rows below are yours: the project and its billing. The rest
it creates or grants itself, and it fails naming the missing thing rather than
part-way through.

| Requirement | How to check it | Why the script cannot do it |
|---|---|---|
| The project exists | `gcloud projects describe project-d3f96b6b-852b-4460-b6d` | A project is created in a folder, under an organisation, with org policies — a landing-zone decision this repository does not make |
| Billing is enabled on it | `gcloud beta billing projects describe <project>` | `run`, `compute`, `cloudkms` and `artifactregistry` refuse to enable without it, and the error names billing rather than the API |
| Service Usage and Cloud Resource Manager are on | `gcloud services list --enabled \| grep -E 'serviceusage\|cloudresourcemanager'` | Service Usage cannot enable itself. See `modules/services/BOOTSTRAP.md` |
| `claude-builder@…` exists, with project admin | `gcloud iam service-accounts describe claude-builder@<project>.iam.gserviceaccount.com` | The script creates it when you have the authority to create service accounts; a project owner creates it otherwise |
| You can impersonate it | The script grants this and says who to ask if it cannot | Needs `roles/iam.serviceAccountTokenCreator`, which only a project owner can give |

The identity running the first apply also needs
`roles/serviceusage.serviceUsageAdmin`. The pipeline account deliberately does
not hold it — enabling an API widens the project's attack surface, and a
pipeline that can do that unreviewed is what several other decisions here exist
to prevent. So the **first** apply is done by a person, and every later apply
by the pipeline is a no-op against that module.

### Tools

| Requirement | Notes |
|---|---|
| `terraform` ≥ 1.9 | v1.9.8, installed in this container. `validate` and `plan` run here; `apply` cannot, for want of credentials |
| `gcloud` CLI | Not installed here, and not useful without credentials |
| Credentials for `claude-builder` | See §3 — via impersonation, never a key file |
| A GCS bucket for Terraform state | `bootstrap-deploy.sh` creates `<project>-qip-tfstate`, versioned. Terraform cannot: `backend "gcs"` is read before any resource is planned, so state cannot bootstrap itself |

Cloud Shell (<https://shell.cloud.google.com>) has `gcloud`, `terraform` and
`gh` preinstalled and is already authenticated as you, which is why it is the
recommended place to run the script.

### For the deploy pipeline

**The pipeline reads no repository variable.** This section used to list six
GitHub Actions variables the pipeline needed and `bootstrap-deploy.sh` set;
that is no longer how `deploy.yml` authenticates, and the earlier text sent a
maintainer to set values nothing reads.

Every value authentication and attestation need — the workload-identity
provider audience, the CI service account, the Binary Authorization attestor
and the key version that signs for it — is derived in the workflow itself from
the environment's committed tfvars. `.github/workflows/deploy.yml:22-36` states
the rule and the failure that produced it; the `derive the identity from the
tfvars` step (`:191-224` in the `images` job, repeated at `:394` in `deploy`)
reads `project_id`, `project_number` and `region` from
`infrastructure/environments/<env>/terraform.tfvars` and constructs the rest
from the names the `cicd` and `binaryauthorization` modules fix
(`qip-github-<env>`, `qip-ci-<env>`, `qip-<env>-build`, version 1 of
`qip-<env>-attestor`). `infra.yml` does the same. The acceptance test
`no_workflow_depends_on_a_repository_variable`
(`backend/crates/tests/qip-acceptance/tests/infrastructure.rs:1765`) refuses
any `${{ vars.` in either workflow, so the variables cannot come back without
that test being changed.

Why: repository variables were the one input nothing reviewed. They were
written by the bootstrap's last step, and once a bootstrap that reached that
step ran against Cloud Shell's `terraform` stub and captured several lines of
apt install advice into the workload-identity variable — non-empty, so every
check that only asked whether it was set waved it through, and every run
afterwards failed on an audience nobody could explain. A value nothing
re-derives is a value that stays wrong.

What a deployment therefore needs from a person is the tfvars: a real
`project_id` and `project_number` in the environment's file, in place of the
`unprovisioned` marker the derivation step refuses before authenticating. The
values that used to be pasted are now a pure function of those two.

**A finding for the owner of `scripts/bootstrap-deploy.sh`, not fixed here.**
The script's step 6 (`scripts/bootstrap-deploy.sh:288-326`) still builds
seven values — `GCP_PROJECT`, `GCP_REGION`, `GCP_WORKLOAD_IDENTITY_PROVIDER`,
`GCP_DEPLOY_SERVICE_ACCOUNT`, `GCP_BINAUTHZ_ATTESTOR`,
`GCP_BINAUTHZ_KEY_VERSION`, `GCP_INFRA_SERVICE_ACCOUNT` — shape-checks them,
and sets them with `gh variable set` when `gh` is authenticated, or prints
the commands for a person to run when it is not. Its header (`:16`, `:23`)
and the comment above step 6 (`:258-268`) still say the pipeline reads them.
Nothing does: neither workflow contains `${{ vars.`. The step is harmless
today and misleading, and a bootstrap that fails inside it fails for no
reason a deployment cares about. The repair belongs to the script's owner —
either delete step 6 or restate it as the record it actually is.

There is deliberately **no** GitHub secret holding a key. The pipeline uses
workload identity federation: GitHub mints a short-lived OIDC token, GCP
exchanges it for a credential scoped to one job. An `attribute_condition` pins
the repository, so a token from any other repository is refused. And the
account it exchanges for is `qip-ci-<env>`, the narrow one Terraform creates
— never `claude-builder`, for the reasons in §1.

### For the platform at runtime

| Secret | Consumer | Injection |
|---|---|---|
| `qip-token-{operator,approver,analyst,viewer,monitor}` | `qip-api` — the five bearer tokens | Secret Manager → a volume `modules/cloudrun` mounts as a file, named by `secret_mounts` in `catalogue.tf`; the process reads the `_FILE` variable |
| `qip-capital-envelope-key` | `qip-api`, `qip-fastbrain` and `qip-deepbrain` mount it (`catalogue.tf`); `qip-edge-node` verifies signed capital envelopes against it | The same volume mount on Cloud Run; on the execution node, fetched at boot by the startup script into the unit's run directory and named by `QIP_CAPITAL_ENVELOPE_KEY_FILE` |
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

**The whole flow below is scripted.** From Cloud Shell, where every tool is
preinstalled and you are already authenticated:

```sh
git clone https://github.com/droderiquesit/quantum-ai-platform
cd quantum-ai-platform
./scripts/bootstrap-deploy.sh          # dev; pass test|stage|prod to target another
```

It enables the APIs, grants you impersonation, creates the versioned state
bucket, runs `terraform init` and an **interactive** apply — it never
auto-approves — and sets six GitHub variables that, since the workflows began deriving
their identity from the tfvars, nothing reads (see §2). It never creates,
downloads or reads a key. The rest of this section is the same flow by hand,
kept because a script you cannot check against its documentation is a script
you have to trust.

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
  -var-file=../environments/dev/terraform.tfvars
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
roles/run.admin                    the Cloud Run services in catalogue.tf
roles/compute.instanceAdmin.v1     the execution node's template and group
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

The infrastructure is **specified and structurally tested**: the
`infrastructure` acceptance suite reads the Terraform and asserts properties a
plan would not catch, such as the execution node's template carrying no
external address and no workload identity holding delete on the evidence
bucket. It has never been validated against the provider schema, because that
needs a provider download, and never applied.
