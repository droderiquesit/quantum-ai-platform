# Declared here because this module carries its own `tests/` harness and
# `ci.yml` runs `terraform init` in it directly: without a pin the harness
# would resolve whatever major the registry publishes, and a gate proven
# against another major's schema is proven against somebody else's
# resources (the reason `modules/registry/versions.tf` gives at length).
terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}
