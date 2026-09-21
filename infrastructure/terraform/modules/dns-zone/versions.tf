# The provider this module is planned against.
#
# Declared here, as in `modules/iap-edge` and `modules/public-edge` and for the
# same reason: this is one of the handful of modules carrying a
# `tests/*.tftest.hcl` harness, so it is one of the handful `terraform init` is
# ever run in directly. Without this the harness resolves whatever the registry
# publishes rather than the `~> 6.12` the root pins and applies with, and a gate
# proven against another major's schema is a gate proven against somebody
# else's resources.
terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}
