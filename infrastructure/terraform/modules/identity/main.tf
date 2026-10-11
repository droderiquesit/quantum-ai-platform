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

  lifecycle {
    # The API materializes a default multi_tenant block (allow_tenants =
    # false) on every read; the provider then plans its removal on every
    # run, applies it, and the API adds it back — a diff that can never
    # converge and says nothing. Multi-tenancy stays off either way, and
    # the same applies to the phone_number sign-in block inside sign_in:
    # materialized disabled, planned away, added back.
    ignore_changes = [multi_tenant, sign_in[0].phone_number]
  }

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
    # MANDATORY, and the variable admits nothing else (W4.6). This was
    # ENABLED — optional — by default "at launch", which on a platform whose
    # every decision has to be attributable to a person meant an account
    # protected by a password alone was a normal account. A posture that can
    # be lowered by a tfvars line is one a tfvars line will lower, so the
    # refusal lives in the variable's validation and lowering it is an edit
    # a reviewer sees. What it costs is stated plainly: the apply that flips
    # an environment from ENABLED locks out every account not yet enrolled,
    # which in dev — the one environment with identity on — is operators
    # testing, not customers.
    state = var.mfa_state

    # A second factor that is not a text message, beside the one that is.
    # TOTP is the factor a SIM swap does not reach; SMS stays enabled so an
    # account can enrol at all before an authenticator app exists for it.
    enabled_providers = ["PHONE_SMS"]

    provider_configs {
      state = "ENABLED"
      totp_provider_config {
        adjacent_intervals = 1
      }
    }
  }

  quota {
    sign_up_quota_config {
      quota          = var.sign_up_quota_per_hour
      quota_duration = "3600s"
      # The provider requires all three fields together; the comment that
      # previously omitted this one produced "Missing required argument" at
      # plan time. A committed static instant in the past means the quota
      # has simply always applied, and the plan stays deterministic — a
      # timestamp() here would diff on every run.
      start_time = "2026-09-01T00:00:00Z"
    }
  }

  depends_on = [google_project_service.identitytoolkit]
}
