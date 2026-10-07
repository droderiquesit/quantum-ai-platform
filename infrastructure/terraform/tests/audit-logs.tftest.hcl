# Cloud Audit Logs ensure every runtime change is visible, CICD-037.
#
# CICD-037 requires "No autonomous change is invisible" — runtime mutations
# must be audited so that an autonomous agent's changes can be attributed,
# reviewed and reverted. This test verifies that audit logs are configured
# for the services that make runtime changes: Cloud Run, GKE, and Compute Engine.
#
# It needs no credential and reaches no real project. The google provider
# is mocked, so nothing is created or modified; only the configuration is
# planned and verified.

mock_provider "google" {}
mock_provider "google-beta" {}
mock_provider "github" {}

variables {
  project_id        = "audit-logs-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_owner      = "test-owner"
  github_repository = "test-repo"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

run "cloud_run_audit_logs_configured" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/audit-logs-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/audit-logs-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/audit-logs-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/audit-logs-harness/harness"
    }
  }

  # Cloud Run audit config exists and covers ADMIN_WRITE and DATA_WRITE
  # operations, so that gcloud run deploy, service updates and deletions
  # are all recorded in the audit log.
  assert {
    condition     = google_project_iam_audit_config.cloud_run.service == "run.googleapis.com"
    error_message = "cloud_run audit config service is not run.googleapis.com"
  }

  # Verify both log types are configured: ADMIN_WRITE for deployments,
  # DATA_WRITE for access and modifications.
  assert {
    condition     = length([for log in google_project_iam_audit_config.cloud_run.audit_log_config : log.log_type if log.log_type == "ADMIN_WRITE"]) > 0
    error_message = "cloud_run audit config does not include ADMIN_WRITE"
  }

  assert {
    condition     = length([for log in google_project_iam_audit_config.cloud_run.audit_log_config : log.log_type if log.log_type == "DATA_WRITE"]) > 0
    error_message = "cloud_run audit config does not include DATA_WRITE"
  }
}

run "gke_audit_logs_configured" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/audit-logs-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/audit-logs-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/audit-logs-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/audit-logs-harness/harness"
    }
  }

  # GKE audit config exists for Kubernetes Engine so that kubectl apply,
  # edit and delete operations are audited.
  assert {
    condition     = google_project_iam_audit_config.gke.service == "container.googleapis.com"
    error_message = "gke audit config service is not container.googleapis.com"
  }

  assert {
    condition     = length([for log in google_project_iam_audit_config.gke.audit_log_config : log.log_type if log.log_type == "ADMIN_WRITE"]) > 0
    error_message = "gke audit config does not include ADMIN_WRITE"
  }

  assert {
    condition     = length([for log in google_project_iam_audit_config.gke.audit_log_config : log.log_type if log.log_type == "DATA_WRITE"]) > 0
    error_message = "gke audit config does not include DATA_WRITE"
  }
}

run "compute_audit_logs_configured" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/audit-logs-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/audit-logs-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/audit-logs-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/audit-logs-harness/harness"
    }
  }

  # Compute audit config exists for Compute Engine so that instance creation,
  # deletion and updates (including execution nodes) are audited.
  assert {
    condition     = google_project_iam_audit_config.infrastructure.service == "compute.googleapis.com"
    error_message = "infrastructure audit config service is not compute.googleapis.com"
  }

  assert {
    condition     = length([for log in google_project_iam_audit_config.infrastructure.audit_log_config : log.log_type if log.log_type == "ADMIN_WRITE"]) > 0
    error_message = "infrastructure audit config does not include ADMIN_WRITE"
  }

  assert {
    condition     = length([for log in google_project_iam_audit_config.infrastructure.audit_log_config : log.log_type if log.log_type == "DATA_WRITE"]) > 0
    error_message = "infrastructure audit config does not include DATA_WRITE"
  }
}
