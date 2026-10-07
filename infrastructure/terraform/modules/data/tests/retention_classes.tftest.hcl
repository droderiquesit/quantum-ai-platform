# Terraform test: TICK-037 retention classes
#
# Validates that the data module enforces separate lifecycle/retention policies
# for market history, internal financial history, and world-derived knowledge.
# The test verifies that a plan over test objects of each class expires or
# retains each according to its own policy only.

# A plan needs no credentials, so the provider is mocked.
#
# `condition` and `action` are set-nested blocks in the provider schema, and a
# set has no index: `rule.condition[0]` cannot be read. Under the mock,
# `condition.with_state` (optional and computed) is unknown at plan, so the
# assertions below iterate each set rather than index it. The original
# `rule.condition.prefix_match` named neither the block shape nor the
# attribute the provider has (`matches_prefix`).
#
# Nor is `lifecycle_rule` wrapped in `try()` any more: `try` returns unknown
# for a value that is not wholly known, which is every rule here, and both
# runs enable the bucket, so a bucket missing from the plan should be an
# error rather than an empty list.
mock_provider "google" {}

run "market_data_lifecycle_plan" {
  command = plan

  variables {
    project_id             = "test-project"
    environment            = "test"
    region                 = "us-central1"
    enable_cloud_storage   = true
    enable_bigquery        = false
    enable_alloydb         = false
    enable_memorystore     = false
    archive_retention_days = 2555 # 7 years for the hash-chained event log
    key_ring_id            = "/projects/test-project/locations/us-central1/keyRings/test"
    network_id             = "projects/test-project/global/networks/default"
    labels                 = {}
  }

  # The archive bucket is created with the differentiated lifecycle rules.
  # Market data (class=market) is transient: expires after 90 days.
  assert {
    condition = (
      length([
        for rule in google_storage_bucket.archive[0].lifecycle_rule
        : rule
        if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "lake/class=market/") && c.age == 90]) &&
        anytrue([for a in rule.action : a.type == "Delete"])
      ]) > 0
    )
    error_message = "Market data must have a lifecycle rule that deletes after 90 days (TICK-037)"
  }

  # Internal data (class=internal) is permanent: transitions to cold storage
  # but never deletes.
  assert {
    condition = (
      length([
        for rule in google_storage_bucket.archive[0].lifecycle_rule
        : rule
        if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "lake/class=internal/")]) &&
        anytrue([for a in rule.action : a.type == "SetStorageClass"])
      ]) > 0
    )
    error_message = "Internal data must have a SetStorageClass rule and never Delete (TICK-037)"
  }

  # No internal data should have a Delete action.
  assert {
    condition = (
      length([
        for rule in google_storage_bucket.archive[0].lifecycle_rule
        : rule
        if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "lake/class=internal/")]) &&
        anytrue([for a in rule.action : a.type == "Delete"])
      ]) == 0
    )
    error_message = "Internal data must never be deleted; only storage class transitions are permitted (TICK-037)"
  }

  # The bucket has a retention policy that protects the event log
  # (hash-chained; no record should be deleted before its investigation window).
  assert {
    condition = (
      try(google_storage_bucket.archive[0].retention_policy[0].retention_period, 0) ==
      (var.archive_retention_days * 24 * 60 * 60)
    )
    error_message = "Archive bucket must have a retention policy matching archive_retention_days"
  }
}

run "retention_classes_integrated" {
  command = plan

  variables {
    project_id             = "test-project"
    environment            = "test"
    region                 = "us-central1"
    enable_cloud_storage   = true
    enable_bigquery        = false
    enable_alloydb         = false
    enable_memorystore     = false
    archive_retention_days = 2555
    key_ring_id            = "/projects/test-project/locations/us-central1/keyRings/test"
    network_id             = "projects/test-project/global/networks/default"
    labels                 = {}
  }

  # The three retention classes are structurally separated and governed:
  # 1. Market history (Transient): Expires after 90 days
  # 2. Internal history (Irreplaceable/Permanent): Never expires
  # 3. World-derived knowledge: Bounded pass-through (not stored in archive)

  # Market data transitions STANDARD -> COLDLINE -> Delete
  assert {
    condition = (
      length([
        for rule in google_storage_bucket.archive[0].lifecycle_rule
        : rule
        if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "lake/class=market/") && c.age == 30]) &&
        anytrue([for a in rule.action : a.type == "SetStorageClass" && a.storage_class == "COLDLINE"])
      ]) > 0
    )
    error_message = "Market data must transition to COLDLINE at 30 days (TICK-037)"
  }

  # Internal data transitions STANDARD -> COLDLINE at 180 days and stops
  assert {
    condition = (
      length([
        for rule in google_storage_bucket.archive[0].lifecycle_rule
        : rule
        if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "lake/class=internal/") && c.age == 180]) &&
        anytrue([for a in rule.action : a.type == "SetStorageClass"])
      ]) > 0
    )
    error_message = "Internal data must transition to COLDLINE at 180 days and never delete (TICK-037)"
  }
}
