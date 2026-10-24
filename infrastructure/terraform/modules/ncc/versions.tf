# The provider this module is planned against.
#
# Declared here, and only in the handful of modules that carry a
# `tests/*.tftest.hcl` harness, because those are the only modules
# `terraform init` is ever run in directly. Everywhere else the root's
# constraint governs and a second copy would be a second thing to keep in step.
#
# Without this the harness resolves whatever the registry publishes: on
# 2026-09-14 that was google 8.2.0, two majors above the `~> 6.12` the root
# pins and applies with. A gate proven against a provider the platform does not
# use is a gate proven against somebody else's schema — the refusals would
# still fire, and the resources they guard might not be the same resources.
terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}
