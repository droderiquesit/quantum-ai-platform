# The security services of blueprint §15's control table (W4.6): Privileged
# Access Manager, Certificate Authority Service, VPC Service Controls and
# Sensitive Data Protection (R12 SEC-9123, 9128, 9129, 9134).
#
# Each is behind its own variable and off in every environment, and each
# refuses at plan the configuration that would make it weaker than it reads:
# an entitlement nobody approves, a CA that issues long-lived or CA-capable
# certificates, a perimeter that protects nothing, a discovery scan that
# inspects for nothing. Off is honest — the control does not exist and the
# assessment says so. On and hollow is the state this file refuses.
#
# Enabling an API here is per control and only when it is on, for the
# reason `modules/services` gives: a plan against an environment that has
# not opted in creates nothing.

locals {
  pam_on = length(var.pam_entitlements) > 0
  cas_on = var.certificate_authority != null
  vpc_on = var.vpc_service_controls != null
  sdp_on = var.sensitive_data_protection != null

  security_apis = merge(
    local.pam_on ? { "privilegedaccessmanager.googleapis.com" = true } : {},
    local.cas_on ? { "privateca.googleapis.com" = true } : {},
    local.vpc_on ? { "accesscontextmanager.googleapis.com" = true } : {},
    local.sdp_on ? { "dlp.googleapis.com" = true } : {},
  )

  project_resource = "//cloudresourcemanager.googleapis.com/projects/${var.project_id}"
}

resource "google_project_service" "security" {
  for_each = local.security_apis

  project = var.project_id
  service = each.key

  # Disabling an API deletes what lives under it — the CA's keys, the
  # perimeter's enforcement, the entitlements' audit trail. A person
  # decides that with a plan in front of them, not a flag flip.
  disable_on_destroy = false

  depends_on = [module.services]
}

# --- Privileged Access Manager (SEC-9123) ------------------------------------
#
# Sensitive operations by time-bounded elevation with approval, instead of a
# standing grant. Each entitlement names the roles it grants, who may ask,
# who must approve, and for how long at most. The variable refuses an
# entitlement with no approver, a duration over eight hours, a basic role,
# and a requester who is also an approver of the same entitlement — each of
# which turns "elevation with approval" back into a standing grant with
# extra steps.
resource "google_privileged_access_manager_entitlement" "this" {
  for_each = var.pam_entitlements

  entitlement_id       = "qip-${var.environment}-${each.key}"
  location             = "global"
  parent               = "projects/${var.project_id}"
  max_request_duration = each.value.max_request_duration

  requester_justification_config {
    unstructured {}
  }

  eligible_users {
    principals = sort(tolist(each.value.requesters))
  }

  privileged_access {
    gcp_iam_access {
      resource      = local.project_resource
      resource_type = "cloudresourcemanager.googleapis.com/Project"

      dynamic "role_bindings" {
        for_each = sort(tolist(each.value.roles))
        content {
          role = role_bindings.value
        }
      }
    }
  }

  approval_workflow {
    manual_approvals {
      require_approver_justification = true
      steps {
        approvals_needed = 1
        approvers {
          principals = sort(tolist(each.value.approvers))
        }
      }
    }
  }

  depends_on = [google_project_service.security]
}

# --- Certificate Authority Service (SEC-9128) -------------------------------
#
# A private CA for the identities that live outside a mesh — the fabric
# broker, the ledger, the execution node. The pool's issuance policy is the
# control: leaf certificates only (never a CA), client and server auth only,
# at most thirty days, EC P-256 or RSA 3072+. The CA's key is HSM-backed.
#
# The transport side is built: since ADR 0112 `qip-transport::tls` speaks
# TLS 1.3 through rustls, its server *requires* a client certificate (there
# is no optional-client-auth path), its client presents one, and a link is
# mutual TLS by default everywhere but a `local` deployment — plaintext
# outside `local` stops the process. `qip-fabricd`, `qip-ledgerd` and
# `qip-edge-node` load their workload identity from files. This comment said
# until 2026-10-11 that the transport could not do mTLS, which had stopped
# being true. What remains is issuance: nothing in this root yet requests a
# leaf from this pool and delivers it to those files, so no deployed link
# has presented one — nothing those binaries run on is applied.
resource "google_privateca_ca_pool" "internal" {
  count = local.cas_on ? 1 : 0

  project  = var.project_id
  name     = "qip-${var.environment}-internal"
  location = var.certificate_authority.location
  tier     = "ENTERPRISE"

  publishing_options {
    publish_ca_cert = true
    publish_crl     = true
  }

  issuance_policy {
    maximum_lifetime = "2592000s"

    allowed_key_types {
      elliptic_curve {
        signature_algorithm = "ECDSA_P256"
      }
    }
    allowed_key_types {
      rsa {
        min_modulus_size = 3072
      }
    }

    baseline_values {
      ca_options {
        is_ca = false
      }
      key_usage {
        base_key_usage {
          digital_signature = true
          key_encipherment  = true
        }
        extended_key_usage {
          server_auth = true
          client_auth = true
        }
      }
    }
  }

  labels = local.labels

  depends_on = [google_project_service.security]
}

resource "google_privateca_certificate_authority" "internal" {
  count = local.cas_on ? 1 : 0

  project                  = var.project_id
  pool                     = google_privateca_ca_pool.internal[0].name
  certificate_authority_id = "qip-${var.environment}-root"
  location                 = var.certificate_authority.location
  type                     = "SELF_SIGNED"

  # Ten years for the root; the leaves it signs live thirty days at most.
  lifetime = "315360000s"

  # Deleting a CA that still has live certificates is refused, and a
  # deleted CA keeps its thirty-day grace period: a mistaken destroy is
  # recoverable. Not `deletion_protection`, which would make the gate
  # unable to close at plan — the shape infrastructure/CLAUDE.md records.
  deletion_protection                    = false
  skip_grace_period                      = false
  ignore_active_certificates_on_deletion = false

  key_spec {
    algorithm = "EC_P256_SHA256"
  }

  config {
    subject_config {
      subject {
        organization = var.certificate_authority.organization
        common_name  = "qip ${var.environment} internal root"
      }
    }
    x509_config {
      ca_options {
        is_ca                  = true
        max_issuer_path_length = 0
      }
      key_usage {
        base_key_usage {
          cert_sign = true
          crl_sign  = true
        }
        extended_key_usage {}
      }
    }
  }

  labels = local.labels
}

# Who may request a leaf. Service accounts only; the pool's policy bounds
# what they get.
resource "google_privateca_ca_pool_iam_member" "requesters" {
  for_each = local.cas_on ? var.certificate_authority.requesters : toset([])

  project = var.project_id
  ca_pool = google_privateca_ca_pool.internal[0].id
  role    = "roles/privateca.certificateRequester"
  member  = each.value
}

# --- VPC Service Controls (SEC-9129) -----------------------------------------
#
# A perimeter around this project's data, ledger and AI services, so a
# credential that leaks cannot copy them out to a project outside it.
#
# Organisation-scoped, which this configuration otherwise is not: the
# access policy lives on the organisation and is not created here (the
# repository creates no organisation resource, and `modules/scc` records
# why). The perimeter is this repository's; the policy it sits in is named
# by number. Enforced by default — a perimeter in dry run blocks nothing —
# with dry run available for the first apply, deliberately, as a field.
#
# Artifact Registry is not in the default list, on purpose: the build
# registry is read across projects by every later environment
# (R12 CICD-9112), and restricting it here without an egress rule would
# stop every promotion. Adding it is a decision with that rule beside it.
resource "google_access_context_manager_service_perimeter" "data" {
  count = local.vpc_on ? 1 : 0

  parent         = "accessPolicies/${var.vpc_service_controls.access_policy}"
  name           = "accessPolicies/${var.vpc_service_controls.access_policy}/servicePerimeters/qip_${replace(var.environment, "-", "_")}_data"
  title          = "qip ${var.environment} data, ledger and AI"
  perimeter_type = "PERIMETER_TYPE_REGULAR"

  use_explicit_dry_run_spec = !var.vpc_service_controls.enforced

  dynamic "status" {
    for_each = var.vpc_service_controls.enforced ? [1] : []
    content {
      resources           = ["projects/${local.project_number}"]
      restricted_services = sort(tolist(var.vpc_service_controls.restricted_services))

      dynamic "ingress_policies" {
        for_each = length(var.vpc_service_controls.ingress_identities) > 0 ? [1] : []
        content {
          ingress_from {
            identities = sort(tolist(var.vpc_service_controls.ingress_identities))
            sources {
              access_level = "*"
            }
          }
          ingress_to {
            resources = ["*"]
            operations {
              service_name = "*"
            }
          }
        }
      }
    }
  }

  dynamic "spec" {
    for_each = var.vpc_service_controls.enforced ? [] : [1]
    content {
      resources           = ["projects/${local.project_number}"]
      restricted_services = sort(tolist(var.vpc_service_controls.restricted_services))
    }
  }

  depends_on = [google_project_service.security]
}

# --- Sensitive Data Protection (SEC-9134) ------------------------------------
#
# Discovery over the lake and analytics: every BigQuery table and every
# Cloud Storage bucket in the project is profiled for the info types below,
# continuously, and a de-identification template is published for the
# pipelines that must redact before analytics reads. The inspect template's
# minimum likelihood is POSSIBLE — a scanner tuned to report only certainty
# reports nothing a reviewer did not already know.
resource "google_data_loss_prevention_inspect_template" "pii" {
  count = local.sdp_on ? 1 : 0

  parent       = "projects/${var.project_id}/locations/${var.region}"
  template_id  = "qip-${var.environment}-pii"
  display_name = "qip ${var.environment} PII and financial identifiers"
  description  = "What discovery looks for in the lake and analytics (SEC-9134)."

  inspect_config {
    min_likelihood = "POSSIBLE"

    dynamic "info_types" {
      for_each = sort(tolist(var.sensitive_data_protection.info_types))
      content {
        name = info_types.value
      }
    }
  }

  depends_on = [google_project_service.security]
}

resource "google_data_loss_prevention_deidentify_template" "redact" {
  count = local.sdp_on ? 1 : 0

  parent       = "projects/${var.project_id}/locations/${var.region}"
  template_id  = "qip-${var.environment}-redact"
  display_name = "qip ${var.environment} redaction"
  description  = "Replaces every finding with its info-type name before analytics reads it (SEC-9134)."

  deidentify_config {
    info_type_transformations {
      transformations {
        primitive_transformation {
          replace_with_info_type_config = true
        }
      }
    }
  }

  depends_on = [google_project_service.security]
}

resource "google_data_loss_prevention_discovery_config" "lake" {
  count = local.sdp_on ? 1 : 0

  parent   = "projects/${var.project_id}/locations/${var.region}"
  location = var.region
  status   = "RUNNING"

  inspect_templates = [google_data_loss_prevention_inspect_template.pii[0].id]

  targets {
    big_query_target {
      filter {
        other_tables {}
      }
    }
  }

  targets {
    cloud_storage_target {
      filter {
        others {}
      }
    }
  }

  depends_on = [google_project_service.security]
}
