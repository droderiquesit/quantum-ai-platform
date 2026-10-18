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

# The refusing half. Both runs above admit; this one proves the module still
# refuses something on the configuration they plan. The archive holds the
# hash-chained event log for seven years, and DATA-067 puts it on replicated
# storage: `bucket_location` admits only a dual- or multi-region name, and a
# single region is refused at plan, before any lifecycle rule exists. The
# variables are the admitting runs' own, with that one value changed.
run "a_single_region_archive_is_refused_before_any_retention_class_is_planned" {
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
    bucket_location        = "us-central1"
  }

  expect_failures = [var.bucket_location]
}

# --- The regional segment store and the simulation bucket ------------------
#
#
# R12-DATA-8124 asks for a regional store for sealed tick and internal replay
# segments; R12-DATA-7013 asks for a bucket per artifact class with retention
# set, naming simulation outputs and reproducible campaign datasets. Both are
# plan checks, so the provider is mocked. `condition` and `action` are set
# blocks and cannot be indexed; the assertions iterate them, as the runs
# above do and for the reason the header gives.
#
# The file-level variables below apply to every run in the file, and a run's
# own variables override them; the runs above set every variable they read,
# so only the runs below take these values.

variables {
  project_id           = "test-project"
  environment          = "test"
  region               = "us-central1"
  enable_cloud_storage = true
  enable_bigquery      = false
  enable_alloydb       = false
  enable_memorystore   = false
  key_ring_id          = "/projects/test-project/locations/us-central1/keyRings/test"
  network_id           = "projects/test-project/global/networks/default"
  labels               = {}
}

run "the_segment_store_is_regional_keyed_and_expires_what_it_holds" {
  command = plan

  assert {
    condition     = upper(google_storage_bucket.segments[0].location) == upper(var.region)
    error_message = "The segment store must sit in the deployment's region, not a multi-region (R12-DATA-8124)."
  }

  # The key reference is unknown under a mocked provider, so the CMEK half is
  # held by qip-acceptance's security_controls suite, which reads the file.

  assert {
    condition     = google_storage_bucket.segments[0].retention_policy[0].retention_period == 604800
    error_message = "The segment store must hold each segment at least seven days, so compaction reads it before anyone can delete it."
  }

  assert {
    condition = length([
      for rule in google_storage_bucket.segments[0].lifecycle_rule : rule
      if anytrue([for c in rule.condition : c.age == 30]) && anytrue([for a in rule.action : a.type == "Delete"])
    ]) == 1
    error_message = "The segment store must expire segments at 30 days; it is a regional copy, not the record."
  }
}

run "the_simulation_bucket_bounds_outputs_and_keeps_campaigns_and_manifests" {
  command = plan

  assert {
    condition     = google_storage_bucket.simulation[0].retention_policy[0].retention_period == 2592000
    error_message = "The simulation bucket must carry a thirty-day retention policy (R12-DATA-7013)."
  }

  assert {
    condition = length([
      for rule in google_storage_bucket.simulation[0].lifecycle_rule : rule
      if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "outputs/") && c.age == 90]) &&
      anytrue([for a in rule.action : a.type == "Delete"])
    ]) == 1
    error_message = "Simulation outputs must be deleted at 90 days (R12-DATA-7013)."
  }

  # A campaign dataset is what makes a result reproducible: no rule naming
  # its prefix may delete it, and one must move it to a colder class.
  assert {
    condition = length([
      for rule in google_storage_bucket.simulation[0].lifecycle_rule : rule
      if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "campaigns/")]) &&
      anytrue([for a in rule.action : a.type == "Delete"])
    ]) == 0
    error_message = "No lifecycle rule may delete a campaign dataset (R12-DATA-7013)."
  }

  assert {
    condition = length([
      for rule in google_storage_bucket.simulation[0].lifecycle_rule : rule
      if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "campaigns/")]) &&
      anytrue([for a in rule.action : a.type == "SetStorageClass"])
    ]) == 1
    error_message = "Campaign datasets must carry a storage-class rule, so retention is set rather than absent."
  }

  assert {
    condition = length([
      for rule in google_storage_bucket.simulation[0].lifecycle_rule : rule
      if anytrue([for c in rule.condition : contains(coalesce(c.matches_prefix, []), "manifests/")]) &&
      anytrue([for a in rule.action : a.type == "Delete"])
    ]) == 0
    error_message = "No lifecycle rule may delete a manifest; it names exactly what ran."
  }
}

run "cloud_storage_disabled_plans_neither_bucket" {
  command = plan

  variables {
    enable_cloud_storage = false
  }

  assert {
    condition     = length(google_storage_bucket.segments) == 0 && length(google_storage_bucket.simulation) == 0
    error_message = "Neither bucket may be planned with Cloud Storage disabled."
  }
}
