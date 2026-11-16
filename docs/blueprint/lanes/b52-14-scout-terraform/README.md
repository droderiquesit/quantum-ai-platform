# Scout job, identity, bucket and schedule (ADR 0144, R12-DATA-8112): a patch, not applied

Written 2026-11-15 by lane 14 of wave b52. `scout/` is the Terraform module for
the scout Cloud Run job. It is **not** under `infrastructure/terraform/modules/`
because placing it there fails two existing guard suites, and a lane does not
edit a guard to make its own addition pass. `terraform` is not installed in the
lane's environment: nothing here was formatted, validated or planned.

## Why it cannot simply be added

1. `qip-acceptance`'s `infrastructure.rs::every_service_account_terraform_creates_runs_something_or_signs_something`
   lists every service account Terraform creates. The module adds two
   (`scout` and `trigger`), each needing a line there saying what runs as it.
2. `data_infrastructure_guards.rs::no_bucket_or_dataset_lands_external_world_content`
   refuses a bucket whose name or label has the token `scout` (R12-DATA-8012:
   source bytes stream through memory and are never landed) and refuses a bucket
   missing from `BUCKET_CLASSES`. ADR 0144 decision 4 accepts a scout bucket of
   immutable *decoded records and their manifests* (never bodies). The two
   statements need reconciling by a person: either the token check gets a
   narrow named exception for this bucket, or the bucket is named for what it
   holds. Renaming it only to slip past the token would be evading the guard,
   so the lane did not.
3. A new storage class is also needed (`qip_storage::classes::DECLARED`, with a
   `Holds` variant, which `data_infrastructure_guards.rs` matches exhaustively)
   before `StorageSettings::blobs("scout-records")` can open the store.
4. The root must instantiate the module behind a switch; the switch is a
   variable in the protected `variables.tf`.

## To adopt

Move `scout/` to `infrastructure/terraform/modules/scout/`, point
`qip-acceptance/tests/scout_identity.rs::module_path` at it, make the two guard
edits in 1 and 2, add the class in 3, and instantiate it per 4. The scout's
grants are exactly `roles/storage.objectCreator` on its bucket and
`roles/logging.logWriter`; the deep brain gets `roles/storage.objectViewer`
on the bucket. `scout_identity.rs` holds the module to that list now, over this
directory.
