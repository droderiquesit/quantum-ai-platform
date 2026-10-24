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

  # ADR 0112 §2's lifetime, in the duration form CA Service writes.
  cas_maximum_lifetime = local.cas_on ? "${var.certificate_authority.max_certificate_lifetime_hours * 3600}s" : null

  # The SAN every workload certificate carries: its service account's email
  # with `@` read as `.` — a DNS name `webpki` can match and `PeerNames`
  # admits, and one a reader maps back to the account by eye.
  cas_san_shape = "subject_alt_names.size() == 1 && subject_alt_names.all(san, san.type == DNS && san.value.endsWith('.iam.gserviceaccount.com'))"

  # The identities that may request one, keyed by a name known at plan so the
  # grant's `for_each` never waits on an email — and written from account ids,
  # which are configuration, so the plan shows each grant's member and SAN
  # exactly. `cas_requester_emails` is what Google returned, and the grant's
  # precondition holds the two together at apply.
  cas_requesters = merge(
    var.event_fabric_broker == null ? {} : { fabricd = "${google_service_account.fabricd[0].account_id}@${var.project_id}.iam.gserviceaccount.com" },
    var.ledgerd == null ? {} : { ledgerd = "${module.ledgerd[0].service_account_id}@${var.project_id}.iam.gserviceaccount.com" },
    { for id, node in module.execution_node : "edge-node-${id}" => "${node.service_account_id}@${var.project_id}.iam.gserviceaccount.com" },
  )
  cas_requester_emails = merge(
    var.event_fabric_broker == null ? {} : { fabricd = google_service_account.fabricd[0].email },
    var.ledgerd == null ? {} : { ledgerd = module.ledgerd[0].service_account_email },
    { for id, node in module.execution_node : "edge-node-${id}" => node.service_account_email },
  )
  cas_san_of = { for key, email in local.cas_requesters : key => replace(email, "@", ".") }

  # What each Compute Engine binary is told when CA Service is on: the pool
  # and template by full resource name (project id, not number — the IAM
  # condition above matches that form), the lifetime it asks for, and the
  # pool's CA certificates it verifies peers against. Empty strings when off,
  # which every module reads as "no CA Service in this environment".
  cas_runtime = {
    # Written from configuration so a plan shows them; the same shape as the
    # resources' ids, which Google assigns as exactly this.
    ca_pool       = local.cas_on ? "projects/${var.project_id}/locations/${var.certificate_authority.location}/caPools/${google_privateca_ca_pool.internal[0].name}" : ""
    template      = local.cas_on ? "projects/${var.project_id}/locations/${var.certificate_authority.location}/certificateTemplates/${google_privateca_certificate_template.workload[0].name}" : ""
    lifetime_secs = local.cas_on ? var.certificate_authority.max_certificate_lifetime_hours * 3600 : 0
    trust_roots   = local.cas_on ? join("", google_privateca_certificate_authority.internal[0].pem_ca_certificates) : ""
  }
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
# extra steps. Since 2026-10-24 (R12-GCP-8114) it also refuses:
# - a service-account requester or approver: entitlements are for people;
# - any Secret Manager role, since a project-scope one reads the venue
#   credential;
# - impersonation and IAM-admin roles;
# - custom roles, whose permissions a plan cannot read.
# `tests/pam.tftest.hcl` plans each refusal and the admitting case.
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

# --- Certificate Authority Service (SEC-9128, ADR 0112 §2) ------------------
#
# A private CA for the identities that own their sockets — the fabric broker,
# the ledger, the execution node — whose links are mutual TLS (ADR 0112
# decision 1). The pool's issuance policy is the outer control: leaf
# certificates only (never a CA), client and server auth only, EC P-256 or
# RSA 3072+, for at most `max_certificate_lifetime_hours` (24 or fewer:
# hours, not months). The CA's key is HSM-backed.
#
# The transport side is built: since ADR 0112 `qip-tls` (formerly
# `qip-transport::tls`) speaks TLS 1.3 through rustls, its server *requires* a
# client certificate (there is no optional-client-auth path), its client
# presents one, and a link is mutual TLS by default everywhere but a `local`
# deployment — plaintext outside `local` stops the process. This comment said
# until 2026-10-11 that the transport could not do mTLS, which had stopped
# being true.
# The inner control is the workload template and the conditioned grants
# below it: a workload may request a certificate only through the template,
# and only for the one SAN that names its own service account — which is what
# "a node can only obtain a certificate naming itself" (ADR 0112 §2) is, here.
# The three binaries are told the pool and the template in their startup
# configuration (QIP_TLS_MODE=cas, QIP_CAS_CA_POOL, …) when this is on.
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
    maximum_lifetime = local.cas_maximum_lifetime

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

  # Ten years for the root; the leaves it signs live hours (ADR 0112 §2).
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

# The one template a workload certificate is issued through.
#
# `allow_subject_passthrough = false`: the subject is not an identity here and
# a requester may not choose one. `allow_subject_alt_names_passthrough = true`
# is not the loosening it reads as: with it false CA Service discards every
# requested SAN, and a certificate with no SAN carries no identity for a peer
# to check (`qip_transport::tls::PeerNames` matches DNS SANs). So the SAN
# passes through and two things bound it — this expression, which admits
# exactly one DNS SAN shaped as a service account's (the account's email
# with `@` read as `.`, injective because an account id cannot contain a
# dot), and the conditioned grant below, which admits only the requester's
# own. The template's CEL cannot see the caller; the grant's condition can
# see the request, and is per caller — so the pair is what binds a SAN to an
# identity.
resource "google_privateca_certificate_template" "workload" {
  count = local.cas_on ? 1 : 0

  project          = var.project_id
  name             = "qip-${var.environment}-workload"
  location         = var.certificate_authority.location
  description      = "mTLS workload certificates for qip Compute Engine binaries: one DNS SAN naming the requester's own service account (ADR 0112 §2)."
  maximum_lifetime = local.cas_maximum_lifetime

  identity_constraints {
    allow_subject_passthrough           = false
    allow_subject_alt_names_passthrough = true

    cel_expression {
      title       = "one SAN shaped as a service account"
      description = "Exactly one DNS SAN, a service account email with '@' as '.'. Each requester's IAM condition narrows it to its own."
      expression  = local.cas_san_shape
    }
  }

  predefined_values {
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

  labels = local.labels

  depends_on = [google_project_service.security]
}

# Who may request a leaf: exactly the three binaries' identities, each only
# through the template and only for its own SAN. Derived, never listed —
# `certificate_authority.requesters` is refused — so the set follows the
# machines this environment declares and nothing else can be in it. The
# template is written `<project id>/-/<id>` because that is the form CA
# Service evaluates, and a requester must therefore name the template by
# project id, never number (QIP_CAS_CERTIFICATE_TEMPLATE below is that form).
resource "google_privateca_ca_pool_iam_member" "requesters" {
  for_each = local.cas_on ? local.cas_requesters : {}

  project = var.project_id
  ca_pool = google_privateca_ca_pool.internal[0].id
  role    = "roles/privateca.certificateRequester"
  member  = "serviceAccount:${each.value}"

  condition {
    title       = "own-san-via-workload-template"
    description = "Only through the workload template, and only for the SAN naming this service account."
    expression  = "api.getAttribute('privateca.googleapis.com/template', '') == '${var.project_id}/-/${google_privateca_certificate_template.workload[0].name}' && api.getAttribute('privateca.googleapis.com/subject_alt_names', []).hasOnly(['dns:${local.cas_san_of[each.key]}'])"
  }

  lifecycle {
    precondition {
      condition     = each.value == local.cas_requester_emails[each.key]
      error_message = "The planned email for ${each.key} is not the email of the account Google created; the grant and its SAN would name an identity nothing runs as."
    }
  }
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
