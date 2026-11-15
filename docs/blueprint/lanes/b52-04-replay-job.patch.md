# Patch for a person: the replay Cloud Run Job (R12-TICK-8110)

Outcome `NEEDS_PERSON`: this needs `infrastructure/terraform/variables.tf` and
the dev tfvars, both protected. **Not validated, not planned, not applied.**

## variables.tf (add)

```hcl
variable "enable_replay_pool" {
  description = "Create the replay shard Cloud Run Job (R12-TICK-8110, ADR 0609 part 3). Off by default; provisional cost about 105 USD a month, the owner's figure."
  type        = bool
  default     = false
}
```

## environments/dev/terraform.tfvars (add, commented until funded)

```hcl
# enable_replay_pool = true
```

## A new file, infrastructure/terraform/replay-job.tf

```hcl
resource "google_cloud_run_v2_job" "replay" {
  count = var.enable_replay_pool ? 1 : 0

  name                = "qip-replay"
  location            = var.region
  deletion_protection = false

  binary_authorization {
    use_default = true
  }

  template {
    parallelism = 2
    task_count  = 2 # one shard per task; CLOUD_RUN_TASK_INDEX is the shard
    template {
      # Its own identity: no service or identity shared with serving.
      service_account = google_service_account.replay[0].email
      max_retries     = 0
      timeout         = "3600s"
      containers {
        image = var.replay_image # a digest, never a tag
      }
    }
  }
}

resource "google_service_account" "replay" {
  count        = var.enable_replay_pool ? 1 : 0
  account_id   = "qip-replay"
  display_name = "Replay shard job (reads archived segments only)"
}
```

`var.replay_image` and the read-only grant on the archive bucket (needs
`enable_cloud_storage`, R12-DATA-8125) are not written here. A `.tftest.hcl`
proving the gate admits false and plans the job on true is also needed.
