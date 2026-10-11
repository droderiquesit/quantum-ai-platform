# ADR 0110: the four artefact-class signing keys, who may sign under each,
# the publisher identity the model-pack workflow federates to, and the trust
# bundle the fleet verifies against (R12 SEC-9109, SEC-9137, SEC-8100's KMS
# clause, MODEL-4023).
#
# Off unless an environment opts in, the way security.tf gates its controls:
# `artefact_signing = null` everywhere, so every environment's plan is
# unchanged by this file. Opting in is two reviewed acts, in this order, and
# the order is the point:
#
#   1. `artefact_signing = { … }` creates the ring, the four keys, their
#      signer bindings and the publisher's identity. Nothing reads a key yet.
#   2. After the apply, the bundle the `artefact_trust_bundle` output renders
#      is committed under `infrastructure/trust/` and the environment names it
#      in `artefact_trust_bundle_file`. Only then does the API sign under KMS
#      and a node verify against the bundle — the bundle the image carries,
#      because `infrastructure/docker/Dockerfile` copies that directory into
#      every image and `.github/workflows/image.yml` carries it into the
#      node's boot image. ADR 0110: the bundle is baked into the attested
#      image, never fetched at run time.
#
# A key that verifies a fleet's policy frames cannot be destroyed by a flag:
# every key declares `prevent_destroy`, so setting `artefact_signing` back to
# null plans a destroy Terraform refuses. That is deliberate (the shape ADR
# 0093 records for the etcd key) — a destroyed key is every signature ever
# made under it unverifiable at once.

variable "artefact_signing" {
  description = <<-EOT
    ADR 0110's artefact-signing keys, or null for none (every environment).

      * `protection_level` — HSM, and nothing else: ADR 0110 decision 1 and
        its "What would make this wrong" both name a signing key at any other
        level. Deliberately not `kms_protection_level`, the root's SOFTWARE
        default for encryption keys.
      * `bundle_not_after` — the RFC 3339 instant after which the rendered
        bundle verifies nothing. A review horizon a person chooses; no default.
      * `key_versions` — per class, the key version the signers use and the
        bundle publishes. 1 until a rotation adds a version (asymmetric keys
        have no rotation_period; a rotation is a new version, shipped in the
        bundle before anything signs with it).
      * `publish_environment` and `publish_ref` — the GitHub protected
        environment and branch the model-pack publish workflow must run in
        for its token to be exchanged at all.
  EOT

  type = object({
    protection_level    = optional(string, "HSM")
    bundle_not_after    = string
    key_versions        = optional(map(number), {})
    publish_environment = optional(string, "model-pack-publish")
    publish_ref         = optional(string, "refs/heads/main")
  })
  default = null

  validation {
    condition     = var.artefact_signing == null || try(var.artefact_signing.protection_level == "HSM", false)
    error_message = "artefact_signing.protection_level must be exactly \"HSM\". ADR 0110 decision 1: the policy, halt, envelope and model-pack keys are HSM-protected, and a signing key created at any other level is the first thing its \"What would make this wrong\" names."
  }

  validation {
    condition     = var.artefact_signing == null || can(formatdate("YYYY", var.artefact_signing.bundle_not_after))
    error_message = "artefact_signing.bundle_not_after is an RFC 3339 instant such as 2027-10-11T00:00:00Z."
  }

  validation {
    condition = var.artefact_signing == null || alltrue([
      for class, version in try(var.artefact_signing.key_versions, {}) :
      contains(["policy", "halt", "envelope", "model-pack"], class) && version >= 1 && floor(version) == version
    ])
    error_message = "artefact_signing.key_versions maps policy, halt, envelope or model-pack to a whole version number of 1 or more."
  }

  validation {
    condition     = var.artefact_signing == null || can(regex("^[A-Za-z0-9_-]{1,64}$", var.artefact_signing.publish_environment))
    error_message = "artefact_signing.publish_environment is a GitHub environment name of letters, digits, '_' and '-'; it is pasted into a workload identity condition."
  }

  validation {
    condition     = var.artefact_signing == null || can(regex("^refs/heads/[A-Za-z0-9._/-]+$", var.artefact_signing.publish_ref))
    error_message = "artefact_signing.publish_ref is a branch ref such as refs/heads/main. A pull request's merge ref or a tag is something anyone who can open one controls."
  }
}

variable "artefact_trust_bundle_file" {
  description = <<-EOT
    The committed trust bundle this environment's images carry, as a
    repository path under `infrastructure/trust/`, or null (every
    environment). Set only after `artefact_signing`'s keys are applied and the
    `artefact_trust_bundle` output has been committed at this path. Set, the
    API signs through KMS (`QIP_ARTEFACT_SIGNER=kms`) and verifies against
    `/etc/qip/trust/<file>`, and the plan refuses a bundle that does not
    publish the key versions the signers use.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.artefact_trust_bundle_file == null || can(regex("^infrastructure/(trust|terraform/tests/fixtures)/[a-z0-9-]+\\.json$", var.artefact_trust_bundle_file))
    error_message = "artefact_trust_bundle_file is a path such as infrastructure/trust/dev.json. Only that directory is copied into the image, so a bundle anywhere else is one no process could read."
  }
  # `infrastructure/terraform/tests/fixtures/` is admitted for the plan
  # harness alone and is not a way round the rule above: a fixture is in no
  # image, so a process pointed at it stops at start-up, and its keys are
  # throwaway ones no KMS key version can match, so the precondition below
  # refuses it in any real environment. What this variable needs beside it is
  # checked by `terraform_data.artefact_trust_bundle_is_current`, one
  # precondition per need, so each refusal is separately provable.

  # The committed bundle publishes, for every class, the version the signers
  # use. A bundle that does not is a fleet refusing every frame the API sends
  # (ADR 0110, "What it costs"), so it is refused here, before anything is
  # planned. The names are configuration and are written here as
  # `local.artefact_key_version_name` writes them — a validation may read
  # variables and not locals; the data source's postcondition holds both to
  # Google's answer. Skipped (not admitted) when the file or the keys are
  # absent: those are refused by name by the preconditions.
  validation {
    condition = (
      var.artefact_trust_bundle_file == null
      || var.artefact_signing == null
      || !try(fileexists("${path.module}/../../${var.artefact_trust_bundle_file}"), false)
      || try(alltrue([
        for class in ["policy", "halt", "envelope", "model-pack"] : anytrue([
          for key in jsondecode(file("${path.module}/../../${var.artefact_trust_bundle_file}")).keys :
          key.class == class
          && key.key_version == "projects/${var.project_id}/locations/${var.region}/keyRings/qip-${var.environment}-artefacts/cryptoKeys/qip-${var.environment}-${class}/cryptoKeyVersions/${lookup(var.artefact_signing.key_versions, class, 1)}"
        ])
      ]), false)
    )
    error_message = "The committed trust bundle does not publish, for every class, the key version the signers use. A rotation ships the new version in the bundle before key_versions moves to it; re-render the bundle from the `artefact_trust_bundle` output."
  }
}

variable "google_tls_roots_file" {
  description = <<-EOT
    The pinned Google root certificates a process authenticates a Google API
    against (ADR 0112 decision 1, third row), as a committed path under
    `infrastructure/trust/`, or null (every environment). Baked into the image
    at `/etc/qip/trust/<file>` like the bundle; read by qip-api for KMS and by
    the Compute Engine binaries for CA Service.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.google_tls_roots_file == null || can(regex("^infrastructure/(trust|terraform/tests/fixtures)/[a-z0-9-]+\\.pem$", var.google_tls_roots_file))
    error_message = "google_tls_roots_file is a path such as infrastructure/trust/google-roots.pem."
  }
}

locals {
  artefact_signing_on = var.artefact_signing != null

  # The four classes, and which identity signs under each. Two disjoint sets
  # by construction, so no expression below can give one identity both: the
  # API signs what it dispatches per cycle, and model packs are signed by the
  # publish workflow and never by a running process
  # (`qip-api/src/signing.rs`, SIGNED_HERE).
  artefact_classes_signed_by_the_api       = toset(["policy", "halt", "envelope"])
  artefact_classes_signed_by_the_publisher = toset(["model-pack"])
  artefact_classes = setunion(
    local.artefact_classes_signed_by_the_api,
    local.artefact_classes_signed_by_the_publisher,
  )

  artefact_key_versions = {
    for class in local.artefact_classes :
    class => local.artefact_signing_on ? lookup(var.artefact_signing.key_versions, class, 1) : 1
  }

  # Where the image carries the committed trust files. The Dockerfile copies
  # `infrastructure/trust/` to `/etc/qip/trust/`, so the path inside the image
  # is the basename of the committed one.
  trust_bundle_image_path = var.artefact_trust_bundle_file == null ? null : "/etc/qip/trust/${basename(var.artefact_trust_bundle_file)}"
  google_roots_image_path = var.google_tls_roots_file == null ? null : "/etc/qip/trust/${basename(var.google_tls_roots_file)}"

  # Every name below is written out from configured values rather than read
  # off a resource's computed attribute, so the plan a person reviews shows
  # the exact version each signer uses and the exact member of each binding
  # — not "(known after apply)". Each is checked against what Google returned
  # at apply by a precondition on the resource that uses it, so a name that
  # drifted from Google's form stops the apply rather than binding nobody.
  artefact_key_ring_id = "projects/${var.project_id}/locations/${var.region}/keyRings/qip-${var.environment}-artefacts"
  artefact_key_version_name = {
    for class in local.artefact_classes :
    class => "${local.artefact_key_ring_id}/cryptoKeys/qip-${var.environment}-${class}/cryptoKeyVersions/${local.artefact_key_versions[class]}"
  }

  # The two signers, by email. A user-managed service account's email is
  # `<account id>@<project>.iam.gserviceaccount.com`, and both account ids
  # are configuration (the API's comes out of `modules/cloudrun`).
  artefact_api_signer       = "serviceAccount:${module.cloud_run["api"].service_account_id}@${var.project_id}.iam.gserviceaccount.com"
  artefact_publisher_signer = "serviceAccount:qip-${var.environment}-pack-publisher@${var.project_id}.iam.gserviceaccount.com"
  artefact_signer_of = {
    for class in local.artefact_classes :
    class => contains(local.artefact_classes_signed_by_the_api, class) ? local.artefact_api_signer : local.artefact_publisher_signer
  }

  model_pack_publish_pool = "projects/${local.project_number}/locations/global/workloadIdentityPools/qip-publish-${var.environment}"

  # The bundle, in `qip_contracts::trust::TrustBundle`'s committed shape.
  # The public key is the one thing only KMS can say, so it is read from the
  # version and converted by `modules/spki-hex`, whose harness holds the
  # conversion to a fixed vector.
  artefact_trust_bundle = local.artefact_signing_on ? {
    keys = [
      for class in sort(tolist(local.artefact_classes)) : {
        class       = class
        key_version = local.artefact_key_version_name[class]
        public_key  = module.artefact_public_key[class].hex
        not_after   = var.artefact_signing.bundle_not_after
      }
    ]
    revoked = []
  } : null

  # `qip-api`'s QIP_KMS_KEY_VERSIONS_PATH document: the three classes it
  # signs, each a full cryptoKeyVersions resource name.
  artefact_api_key_versions = local.artefact_signing_on ? {
    for class in local.artefact_classes_signed_by_the_api :
    class => local.artefact_key_version_name[class]
  } : null
}

module "artefact_public_key" {
  source   = "./modules/spki-hex"
  for_each = data.google_kms_crypto_key_version.artefact

  pem = each.value.public_key[0].pem
}

resource "google_kms_key_ring" "artefacts" {
  count = local.artefact_signing_on ? 1 : 0

  project  = var.project_id
  name     = "qip-${var.environment}-artefacts"
  location = var.region

  depends_on = [module.services]
}

resource "google_kms_crypto_key" "artefact" {
  for_each = local.artefact_signing_on ? local.artefact_classes : toset([])

  name     = "qip-${var.environment}-${each.key}"
  key_ring = google_kms_key_ring.artefacts[0].id
  purpose  = "ASYMMETRIC_SIGN"

  version_template {
    algorithm        = "EC_SIGN_P256_SHA256"
    protection_level = var.artefact_signing.protection_level
  }

  # No `rotation_period`: Cloud KMS rotates symmetric encryption keys only and
  # refuses a period on an asymmetric key at apply. A rotation is a new key
  # version, published in the committed bundle and shipped in an image
  # *before* `key_versions` moves the signers to it — a signer that moves
  # first produces frames every cell refuses.
  lifecycle {
    prevent_destroy = true
  }

  labels = local.labels
}

data "google_kms_crypto_key_version" "artefact" {
  for_each = google_kms_crypto_key.artefact

  crypto_key = each.value.id
  version    = local.artefact_key_versions[each.key]

}

# What Google says each version is, checked once it has said it: the name the
# signers and the bundle were planned with, and a key still HSM and still
# P-256 — a version template edited out of band would otherwise reach the
# bundle unnoticed. On a `terraform_data` rather than as postconditions on the
# data source because Terraform 1.9's test runner aborts a plan whose
# deferred data source carries a postcondition ("Missing pending object in
# plan"), and a check that kills the harness is a check nobody runs.
resource "terraform_data" "artefact_key_versions_are_what_kms_says" {
  for_each = data.google_kms_crypto_key_version.artefact

  input = each.value.name

  lifecycle {
    precondition {
      condition     = each.value.name == local.artefact_key_version_name[each.key]
      error_message = "Cloud KMS names the ${each.key} key version differently from the name the signers and the bundle were planned with; the API would sign under a version no cell trusts."
    }
    precondition {
      condition     = each.value.protection_level == "HSM" && each.value.algorithm == "EC_SIGN_P256_SHA256"
      error_message = "The ${each.key} key version is not an HSM EC_SIGN_P256_SHA256 version (ADR 0110 decision 1); it is not published."
    }
  }
}

# --- who may sign -------------------------------------------------------------
#
# Authoritative for `roles/cloudkms.signer` on each key: one member, and an
# apply removes any other a person added by hand. The API's runtime identity
# holds it on policy, halt and envelope and on nothing else; the publisher
# holds it on model-pack and on nothing else (R12-SEC-9137). Nobody holds a
# viewer role here: the trust bundle is rendered at plan by the infrastructure
# identity's existing project-level `cloudkms.publicKeyViewer`, and neither
# signer needs to read a public key to sign.

resource "google_kms_crypto_key_iam_binding" "artefact_signer" {
  for_each = google_kms_crypto_key.artefact

  crypto_key_id = each.value.id
  role          = "roles/cloudkms.signer"
  members       = [local.artefact_signer_of[each.key]]

  # The members are written from account ids so the plan shows them; this
  # holds them to the accounts Google actually created.
  lifecycle {
    precondition {
      condition = (
        local.artefact_api_signer == "serviceAccount:${module.cloud_run["api"].service_account_email}"
        && local.artefact_publisher_signer == "serviceAccount:${google_service_account.model_pack_publisher[0].email}"
      )
      error_message = "A signer's planned email is not the email of the account Google created; the binding would name an identity nothing runs as."
    }
  }

  depends_on = [module.cloud_run, google_service_account.model_pack_publisher]
}

# The publisher: an identity with no key, impersonable only by the publish
# workflow, through a workload identity pool of its own.
resource "google_service_account" "model_pack_publisher" {
  count = local.artefact_signing_on ? 1 : 0

  project      = var.project_id
  account_id   = "qip-${var.environment}-pack-publisher"
  display_name = "qip ${var.environment} model-pack publisher"
  description  = "Signs reviewed model packs under the model-pack key (ADR 0110 decision 4). Impersonated only by .github/workflows/model-pack-publish.yml in its protected environment; it has no key."

  depends_on = [module.services]
}

# A pool of its own, not a provider in `modules/cicd`'s. A principal is
# pool-wide: the pipeline's provider maps `google.subject` from any workflow
# in this repository, so a binding on the publisher's subject in that pool
# would admit every workflow that names the same environment — and the
# pipeline's provider checks no workflow at all. Here the one provider refuses
# every token that is not this repository's publish workflow, on the
# configured branch, in the configured environment.
resource "google_iam_workload_identity_pool" "model_pack_publish" {
  count = local.artefact_signing_on ? 1 : 0

  project                   = var.project_id
  workload_identity_pool_id = "qip-publish-${var.environment}"
  display_name              = "Model-pack publish (${var.environment})"
  description               = "Federates the model-pack publish workflow's GitHub OIDC token, and nothing else."

  depends_on = [module.services]
}

resource "google_iam_workload_identity_pool_provider" "model_pack_publish" {
  count = local.artefact_signing_on ? 1 : 0

  project                            = var.project_id
  workload_identity_pool_id          = google_iam_workload_identity_pool.model_pack_publish[0].workload_identity_pool_id
  workload_identity_pool_provider_id = "github"
  display_name                       = "GitHub OIDC, publish workflow"

  attribute_condition = join(" && ", [
    "attribute.repository == '${var.github_repository}'",
    "attribute.environment == '${var.artefact_signing.publish_environment}'",
    "attribute.ref == '${var.artefact_signing.publish_ref}'",
    "attribute.workflow_ref == '${var.github_repository}/.github/workflows/model-pack-publish.yml@${var.artefact_signing.publish_ref}'",
  ])

  attribute_mapping = {
    "google.subject"         = "assertion.sub"
    "attribute.repository"   = "assertion.repository"
    "attribute.environment"  = "assertion.environment"
    "attribute.ref"          = "assertion.ref"
    "attribute.workflow_ref" = "assertion.workflow_ref"
  }

  oidc {
    issuer_uri = "https://token.actions.githubusercontent.com"
  }
}

# One principal: the subject GitHub mints for a job in that environment of
# this repository (`repo:<owner>/<name>:environment:<environment>`).
resource "google_service_account_iam_member" "model_pack_publisher_impersonation" {
  count = local.artefact_signing_on ? 1 : 0

  service_account_id = google_service_account.model_pack_publisher[0].name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principal://iam.googleapis.com/${local.model_pack_publish_pool}/subject/repo:${var.github_repository}:environment:${var.artefact_signing.publish_environment}"

  lifecycle {
    precondition {
      condition     = google_iam_workload_identity_pool.model_pack_publish[0].name == local.model_pack_publish_pool
      error_message = "The publish pool's resource name is not the one the impersonation binding was planned with."
    }
  }
}

# --- the committed bundle agrees with the keys --------------------------------
#
# A bundle that does not publish the version a signer uses is a fleet that
# refuses every frame the API sends (ADR 0110 "What it costs"). That is caught
# here, at plan, rather than at the first cycle.
resource "terraform_data" "artefact_trust_bundle_is_current" {
  count = var.artefact_trust_bundle_file == null ? 0 : 1
  input = var.artefact_trust_bundle_file

  lifecycle {
    precondition {
      condition     = var.artefact_signing != null
      error_message = "artefact_trust_bundle_file is set and artefact_signing is null: a bundle published for keys this environment does not have verifies nothing it signs."
    }

    precondition {
      condition     = var.google_tls_roots_file != null
      error_message = "artefact_trust_bundle_file is set and google_tls_roots_file is null. Signing through KMS is a TLS call to Google, and qip-api refuses QIP_ARTEFACT_SIGNER=kms without the pinned roots it authenticates that call against."
    }

    precondition {
      condition     = fileexists("${path.module}/../../${var.artefact_trust_bundle_file}")
      error_message = "artefact_trust_bundle_file names a file that is not committed. Render it from the `artefact_trust_bundle` output after the keys are applied, commit it at that path, and only then name it."
    }

    # The names were checked by the variable's validation. The public keys are
    # Cloud KMS's answer and are checked once it has given one — at plan
    # against an applied environment, and at apply on the first.
    precondition {
      condition = alltrue([
        for class in local.artefact_classes : anytrue([
          for key in try(jsondecode(file("${path.module}/../../${var.artefact_trust_bundle_file}")).keys, []) :
          try(key.class == class && key.key_version == local.artefact_key_version_name[class], false)
          ? try(lower(key.public_key) == module.artefact_public_key[class].hex, false)
          : false
        ])
      ])
      error_message = "The committed trust bundle publishes a public key for a signing version that is not the key Cloud KMS holds; every artefact signed under it would be refused. Re-render the bundle from the `artefact_trust_bundle` output."
    }

    precondition {
      condition = alltrue([
        for key in try(jsondecode(file("${path.module}/../../${var.artefact_trust_bundle_file}")).keys, []) :
        try(timecmp(key.not_after, plantimestamp()) > 0, false)
      ])
      error_message = "The committed trust bundle carries a key whose not_after has passed; every artefact under it is refused. Re-render it with a later bundle_not_after."
    }
  }
}

resource "terraform_data" "google_tls_roots_are_committed" {
  count = var.google_tls_roots_file == null ? 0 : 1
  input = var.google_tls_roots_file

  lifecycle {
    precondition {
      condition     = fileexists("${path.module}/../../${var.google_tls_roots_file}") && strcontains(file("${path.module}/../../${var.google_tls_roots_file}"), "-----BEGIN CERTIFICATE-----")
      error_message = "google_tls_roots_file names no committed PEM certificate file."
    }
  }
}
