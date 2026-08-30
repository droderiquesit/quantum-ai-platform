# Google Cloud Identity Platform for Algorik customer sign-in.
#
# Customer identity only. The administrative surface deliberately does NOT use
# this: workforce access goes through IAP and workforce identity, because a
# customer database and an operator directory are different trust models and
# an operator account in the customer pool would be a privilege escalation
# waiting for a password reset.
#
# What is managed here and what is not:
#
# - Managed: the Identity Platform configuration itself — email/password
#   sign-in, email verification requirement, MFA posture, authorized domains,
#   and the quota guard on sign-ups.
# - NOT managed: the Google sign-in IdP configuration. Attaching it requires
#   the OAuth client secret, and any Terraform-managed path lands that secret
#   in state. The runbook (docs/operations/algorik-domain-migration.md, §OAuth)
#   configures it out-of-band instead, and the application reads only the
#   public client id. A secret in state is a secret in every state backup.
#
# Enabling the service is a project-level act with an explicit variable, so a
# plan against an environment that has not opted in stays a no-op.

resource "google_project_service" "identitytoolkit" {
  count   = var.enabled ? 1 : 0
  project = var.project_id
  service = "identitytoolkit.googleapis.com"

  # Identity holds the customer directory; tearing the API down with the
  # module would orphan it. Disabling is a decision for a human with a plan.
  disable_on_destroy = false
}

resource "google_identity_platform_config" "algorik" {
  count   = var.enabled ? 1 : 0
  project = var.project_id

  # Every domain a browser may be redirected back to after authentication.
  # Google-issued URLs first (Cloud Run's *.run.app hostnames arrive as
  # deployment outputs), algorik.ai domains added at migration. Localhost
  # stays for development against the real project.
  authorized_domains = var.authorized_domains

  sign_in {
    allow_duplicate_emails = false

    email {
      enabled = var.enable_email_password
      # A password account whose mailbox was never proven belongs to whoever
      # typed the address first, which may not be its owner.
      password_required = true
    }

    anonymous {
      # An anonymous session on a trading platform is an audit hole with a
      # cookie. Refused structurally rather than left to a checkbox.
      enabled = false
    }
  }

  mfa {
    # OPTIONAL at launch: enrolment is offered and step-up honours it.
    # MANDATORY is a product decision that locks out every unenrolled account
    # at the moment it flips, so it is a variable, not a default.
    state = var.mfa_state
  }

  quota {
    sign_up_quota_config {
      quota          = var.sign_up_quota_per_hour
      quota_duration = "3600s"
      # start_time is omitted: the quota applies from creation. A dated
      # window is for planned campaigns, not a standing guard.
    }
  }

  depends_on = [google_project_service.identitytoolkit]
}
