# The provider this module is planned against.
#
# Declared here, as in `modules/iap-edge` and for the same reason: this module
# carries a `tests/*.tftest.hcl` harness, so it is one of the handful
# `terraform init` is ever run in directly. Without this the harness resolves
# whatever the registry publishes rather than the `~> 6.12` the root pins and
# applies with, and a gate proven against another major's schema is a gate
# proven against somebody else's resources.
#
# `google` and not `google-beta`, and the distinction is the whole shape of
# ADR 0095. `google_iap_web_cloud_run_service_iam_member` — the access list
# below — is in the GA provider. `iap_enabled` on `google_cloud_run_v2_service`
# is in `google-beta` only, which is why the enable bit is not here and why
# Config Connector's `RunService`, generated from the GA provider, has no
# `iapEnabled` field for the manifest to carry.
terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
    # For `google_project_service_identity` only, which is beta-only. The
    # access list below stays on GA `google`.
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 6.12"
    }
  }
}
