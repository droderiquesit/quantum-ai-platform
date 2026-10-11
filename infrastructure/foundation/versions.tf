# The tenant foundation: a root of its own, with its own state, applied once
# by the owner (ADR 0118).
#
# Separate from `infrastructure/terraform` because the two are applied by
# different principals and fail in different ways. This root needs
# organisation-level rights — create a folder, create projects, link billing,
# set organisation policy — that no workflow identity holds and none should.
# The platform root runs from `infra.yml` under Workload Identity Federation as
# `qip-infra-<env>`, an identity that lives *inside* a project this root
# creates. One state for both would mean either the workflow holding the
# owner's rights or the owner applying the platform by hand; both are what
# ADR 0024 and ADR 0036 moved away from.

terraform {
  required_version = "~> 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }

  # Partial configuration: `scripts/bootstrap-foundation.sh` passes
  # `-backend-config=bucket=<prefix>-seed-foundation-tfstate`. The bucket's
  # name derives from the prefix, which is a runtime input and never
  # committed, so it cannot be written here.
  backend "gcs" {
    prefix = "foundation"
  }
}

# The owner's own credentials (Cloud Shell's application-default login), never
# a key. `user_project_override` sends quota and billing for the Billing
# Budgets and Organization Policy APIs to the seed project: both refuse a
# user credential that names no quota project, and the seed is the one
# project that exists before this root runs.
provider "google" {
  user_project_override = true
  billing_project       = local.seed_project_id
}
