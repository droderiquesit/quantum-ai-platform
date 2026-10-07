# The provider this module is planned against.
#
# Declared here, as in `modules/network` and `modules/dns-zone` and for the
# same reason: this module carries a `tests/*.tftest.hcl` harness, so it is
# one of the modules `terraform init` is run in directly, by `ci.yml`'s
# "plan the gates" step. Without this the harness resolved whatever the
# registry published — google 8.6.0 on 2026-10-07, two majors above the
# `~> 6.12` the root pins and applies with — and a gate proven against
# another major's schema is a gate proven against somebody else's resources.
# It was not hypothetical: `retention_policy.retention_period` is a number in
# 6.x and a string in 8.x, so `modules/data`'s retention assertion failed on
# a type the module never sees when the root plans it.
terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
    # `google_project_service_identity` only, which is beta-only: it forces
    # Artifact Registry's service agent into existence before the key grant
    # names it, the race `modules/secrets` records losing twice.
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 6.12"
    }
  }
}
