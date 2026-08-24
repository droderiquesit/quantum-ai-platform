# Binary Authorization: only an image this pipeline signed may run.
#
# `docs/operations/external-dependencies.md` named this gap precisely. The
# cluster has always carried `binary_authorization =
# PROJECT_SINGLETON_POLICY_ENFORCE`, which reads like a control and was not
# one: with no policy resource in the project, Google evaluates the *implicit*
# policy, whose default rule is `ALWAYS_ALLOW`. Enforcement was on and it
# refused nothing. A switch that reports "on" and admits every image is worse
# than an absent control, because a reviewer reads the line and stops looking.
#
# The chain is five links and four of them are here:
#
#   1. an asymmetric signing key, in the platform's existing key ring;
#   2. a Container Analysis note, which is where an attestation is recorded;
#   3. an attestor, which is that note plus the *public* half of the key;
#   4. a policy whose default rule requires an attestation by that attestor.
#
# The fifth is something that signs, and that is the step added to
# `.github/workflows/deploy.yml` after the push. What a deployment must still
# supply, and what this chain does and does not prove, is in OUT-OF-BAND.md.
# Read it before applying: a deny-by-default policy with no working signer
# produces a cluster that refuses every image, which is a safe failure and a
# total one.
#
# # There is no enable flag here, and that is deliberate
#
# Every optional service in this configuration defaults to off. This is not
# optional. The cluster already enforces the project's singleton policy, so the
# only two states available are "a policy that denies by default" and "the
# implicit policy that allows everything". An off switch here would be a switch
# whose off position is the gap.

locals {
  prefix = "qip-${var.environment}"
}

# The signing key.
#
# Asymmetric, so the half that verifies can be published in the attestor and
# the half that signs never leaves Cloud KMS. The pipeline signs by *calling*
# KMS, which is why there is no signing key on a runner, in a repository
# secret, or in this state file.
resource "google_kms_crypto_key" "attestor" {
  name     = "${local.prefix}-attestor"
  key_ring = var.key_ring_id
  purpose  = "ASYMMETRIC_SIGN"

  version_template {
    algorithm        = var.signing_algorithm
    protection_level = "SOFTWARE"
  }

  # No `rotation_period`, and its absence is a fact about Cloud KMS rather than
  # an oversight: automatic rotation exists only for symmetric encryption keys,
  # and setting a period on an asymmetric key is refused at apply. Rotating
  # this one is a deliberate sequence — add a version, let the attestor carry
  # both public keys while images signed by the old one are still being
  # scheduled, and disable the old version only when nothing needs it. A
  # rotation that skips the overlap refuses every running image at its next
  # reschedule.
  #
  # Destroying the key is worse than losing it: every attestation ever made
  # with it becomes unverifiable at once. Nothing already admitted stops, so
  # the failure appears later, as the first pod that happens to be rescheduled.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# The public half, read back from KMS.
#
# Read rather than configured, because the two must be the same key. An
# attestor holding a public key that was pasted in is an attestor that verifies
# signatures from whatever made that key, which is not necessarily this one.
data "google_kms_crypto_key_version" "attestor" {
  crypto_key = google_kms_crypto_key.attestor.id
}

# Where an attestation is recorded. One note, referenced by the attestor; an
# occurrence attached to it is the statement "this pipeline produced these
# bytes".
resource "google_container_analysis_note" "attestation" {
  project = var.project_id
  name    = "${local.prefix}-build-attestation"

  attestation_authority {
    hint {
      human_readable_name = "qip ${var.environment} build"
    }
  }
}

resource "google_binary_authorization_attestor" "build" {
  project     = var.project_id
  name        = "${local.prefix}-build"
  description = "Verifies that an image was pushed by the qip pipeline for ${var.environment}."

  attestation_authority_note {
    note_reference = google_container_analysis_note.attestation.name

    public_keys {
      # Keyed by the KMS version's own resource name, so an attestation carries
      # which version signed it and a rotation is visible in the policy rather
      # than only in the key.
      id = data.google_kms_crypto_key_version.attestor.id

      pkix_public_key {
        public_key_pem      = data.google_kms_crypto_key_version.attestor.public_key[0].pem
        signature_algorithm = data.google_kms_crypto_key_version.attestor.public_key[0].algorithm
      }
    }
  }
}

# --- What the pipeline may do -----------------------------------------------
#
# Three grants, each the narrowest that performs one step of signing. None of
# them lets the pipeline change the policy: an identity that can both sign an
# image and rewrite the rule that requires signing is not constrained by the
# rule.

# Sign with the key, and read its public half. Not `roles/cloudkms.admin`,
# which could destroy the key, and not a grant on the key ring, which would
# reach the etcd and secrets keys the same ring holds.
resource "google_kms_crypto_key_iam_member" "ci_signs" {
  crypto_key_id = google_kms_crypto_key.attestor.id
  role          = "roles/cloudkms.signerVerifier"
  member        = "serviceAccount:${var.ci_service_account}"
}

# Attach an occurrence to this note, and only this note.
resource "google_container_analysis_note_iam_member" "ci_attaches" {
  project = var.project_id
  note    = google_container_analysis_note.attestation.name
  role    = "roles/containeranalysis.notes.attacher"
  member  = "serviceAccount:${var.ci_service_account}"
}

# Create the occurrence itself. Occurrences are project-scoped, so this grant
# cannot be narrowed to the note the way the one above is.
#
# It also permits deleting an attestation, which is worth saying rather than
# leaving for someone to find. Deleting one can only make the policy stricter —
# the image stops being admissible — so it is not a route to running unsigned
# code. It is a route to a self-inflicted outage, which is why the identity
# holding it is the pipeline and not a workload.
resource "google_project_iam_member" "ci_creates_occurrences" {
  project = var.project_id
  role    = "roles/containeranalysis.occurrences.editor"
  member  = "serviceAccount:${var.ci_service_account}"
}

# Read the attestor, which `gcloud container binauthz attestations
# sign-and-create` does before it signs. Viewer, not editor: the pipeline may
# not add a public key to the attestor it is signing for.
resource "google_binary_authorization_attestor_iam_member" "ci_reads_attestor" {
  project  = var.project_id
  attestor = google_binary_authorization_attestor.build.name
  role     = "roles/binaryauthorization.attestorsViewer"
  member   = "serviceAccount:${var.ci_service_account}"
}

# --- The policy -------------------------------------------------------------

resource "google_binary_authorization_policy" "platform" {
  project = var.project_id

  # Google's own policy, evaluated first, and the reason the cluster still
  # starts at all. GKE's system workloads — kube-dns, the metrics server, the
  # Calico agents that enforce every NetworkPolicy in this platform — run
  # images Google builds and this pipeline has never signed. With this
  # `DISABLE`, the default rule below denies them: the cluster comes up with no
  # DNS and no network policy enforcement, and it reads as broken nodes rather
  # than as a policy decision somebody made here.
  global_policy_evaluation_mode = "ENABLE"

  # Exemptions, by image path prefix. Empty by default: every pattern here is
  # an image that runs without an attestation, which is the hole this module
  # exists to close. The variable's validation refuses the patterns that would
  # exempt everything.
  dynamic "admission_whitelist_patterns" {
    for_each = toset(var.exempt_image_patterns)

    content {
      name_pattern = admission_whitelist_patterns.value
    }
  }

  # Deny, then allow by name.
  #
  # `REQUIRE_ATTESTATION` with a named attestor is the whole control. The
  # alternative the implicit policy uses — `ALWAYS_ALLOW` — is what this
  # configuration looked like it had and did not. `ENFORCED_BLOCK_AND_AUDIT_LOG`
  # rather than `DRYRUN_AUDIT_LOG_ONLY`, because a dry run is a policy that
  # writes a log line while the unsigned image runs anyway.
  default_admission_rule {
    evaluation_mode         = "REQUIRE_ATTESTATION"
    enforcement_mode        = "ENFORCED_BLOCK_AND_AUDIT_LOG"
    require_attestations_by = [google_binary_authorization_attestor.build.name]
  }

  # The same rule again, pinned to the cluster that trades.
  #
  # Deliberately a duplicate of the default. A cluster rule outranks the
  # default, so the day somebody loosens the default to admit a vendor image,
  # this cluster keeps requiring an attestation instead of quietly inheriting
  # the wider rule. The key is `<location>.<cluster name>` — GKE matches on
  # exactly that string, and a rule naming a cluster that does not exist is
  # silently never evaluated, so it comes from the cluster module's own output
  # rather than being spelled again here.
  cluster_admission_rules {
    cluster                 = var.cluster_id
    evaluation_mode         = "REQUIRE_ATTESTATION"
    enforcement_mode        = "ENFORCED_BLOCK_AND_AUDIT_LOG"
    require_attestations_by = [google_binary_authorization_attestor.build.name]
  }
}
