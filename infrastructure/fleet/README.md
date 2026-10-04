# infrastructure/fleet

The development and research fleet's own Terraform root (ADR 0102, first
slice). Separate from `../terraform/` on purpose: its own state, so a plan on
the platform's root cannot propose to change the fleet and the reverse.

## What it creates

| Resource | Why |
|---|---|
| `google_service_account.worker` (`fleet-worker`) | The one identity the Job runs as. No key resource exists; the credential is the metadata server's short-lived token. |
| `google_project_iam_member.worker` | `roles/aiplatform.user` on the project. Nothing else at project level. `worker_may_call_models = false` removes it: the harder stop. |
| `google_storage_bucket.fleet` + `_iam_member.worker` | `<project>-fleet`, uniform access, public access prevented, object access for the worker on this bucket only. Packets, outputs and ledger objects age out; `HALT` does not. |
| `google_cloud_run_v2_job.fleet` | One Job, one image (by digest), `max_retries = 0`, `parallelism` default 8, refused above 40. |
| `google_project_service.api` | Only when `enable_apis = true`. Off by default. |

Every resource that takes labels carries `env`, `service`, `owner`, `cost-center`.

## What the worker is not given

No Secret Manager, KMS, Artifact Registry write, Binary Authorization, Cloud Run
(so a worker cannot start a worker), git, or other bucket. `output.worker_roles`
is the full list and `tests/fleet.tftest.hcl` fails if it grows.

## The kill switch

An object named `HALT` in the bucket (`output.halt_object_uri`). The dispatcher
refuses to start a packet while it exists and a worker checks before each call.
Terraform never creates it and the lifecycle rule never ages it out.
`gcloud storage cp /dev/null gs://<project>-fleet/HALT` halts; deleting it
resumes. The harder stop is `worker_may_call_models = false` and an apply.

## What it does not do

- No Artifact Registry repository, no dispatch identity, no billing budget (a
  monthly budget already exists, ADR 0102 decision 5), no image. The image is a
  required variable, pinned by digest, because the Job cannot be created
  without one that exists.
- Cost ceilings are passed to the dispatcher as environment values. Google
  does not enforce them.

## Tests

```
cd infrastructure/fleet
terraform fmt -check -recursive .
terraform init -backend=false
terraform validate
terraform test
```

Providers are mocked and every run is a plan; no credential is needed.

## Applying (a person, after reading the plan)

An agent does not apply. Nothing here has been applied.

1. Enable the APIs (human step; `enable_apis` stays false):
   ```
   gcloud services enable aiplatform.googleapis.com run.googleapis.com \
     storage.googleapis.com iam.googleapis.com --project algorik-platform-dev
   ```
2. State bucket: this root needs its own backend prefix. Using the platform's
   state bucket with a different prefix keeps the states separate:
   ```
   terraform init -backend-config="bucket=<state-bucket>" -backend-config="prefix=fleet"
   ```
3. Plan, and read it:
   ```
   terraform plan -out fleet.plan \
     -var project_id=algorik-platform-dev \
     -var image=<region>-docker.pkg.dev/algorik-platform-dev/<repo>/worker@sha256:<digest>
   ```
4. Apply only the plan you read: `terraform apply fleet.plan`.
