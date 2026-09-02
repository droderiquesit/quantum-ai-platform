# Binary Authorization: only an image this pipeline signed may run.
#
# `docs/operations/external-dependencies.md` named this gap precisely, in the
# GKE runtime: enforcement was switched on against no policy, so Google
# evaluated the *implicit* policy — default rule `ALWAYS_ALLOW` — and refused
# nothing while reading as though it did. A switch that reports "on" and
# admits every image is worse than an absent control, because a reviewer
# reads the line and stops looking.
#
# The same shape holds on Cloud Run. Every service and job in the catalogue
# carries `binary_authorization { use_default = true }`, which evaluates the
# project's default policy — this one — on every revision. With no policy
# resource that would again be the implicit one. So this is the policy, and
# it is one rule with no holes: the images the pipeline attests, and the one
# vendored image `vendor.yml` mirrors and attests with the same attestor.
#
# The chain is five links and four of them are here:
#
#   1. an asymmetric signing key, in the platform's existing key ring;
#   2. a Container Analysis note, which is where an attestation is recorded;
#   3. an attestor, which is that note plus the *public* half of the key;
#   4. a policy whose default rule requires an attestation by that attestor.
#
# The fifth is something that signs, and that is the step in
# `.github/workflows/deploy.yml` after the push. What a deployment must still
# supply, and what this chain does and does not prove, is in OUT-OF-BAND.md.
# Read it before applying: a deny-by-default policy with no working signer
# produces a catalogue in which every revision is refused, which is a safe
# failure and a total one.
#
# The execution node has no admission controller at all — §41.4's point is
# that nothing sits between the binary and the kernel — so this policy does
# not reach it. `modules/execution-node/README.md` says what stands in for
# it there, and that it is a contract on the image rather than a control.
#
# # There is no enable flag here, and that is deliberate
#
# Every optional service in this configuration defaults to off. This is not
# optional. `use_default = true` is set in the Cloud Run module without a
# variable, so the only two states available are "a policy that denies by
# default" and "the implicit policy that allows everything". An off switch
# here would be a switch whose off position is the gap.

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

# List the occurrences already attached to this note, and only this note.
#
# `attestations list` is the pipeline's idempotency check: `sign-and-create`
# refuses to write an attestation that already exists, and re-running a deploy
# for an unchanged commit is a normal thing to do, so the step asks first. That
# question is `containeranalysis.notes.listOccurrences`, which no role above
# grants — `notes.attacher` grants `attachOccurrence`, a neighbouring verb on
# the same resource, and the first real attestation run died on the difference
# after the images had already been built and pushed.
#
# `notes.occurrences.viewer` carries that one permission and nothing else,
# which is why it is here rather than `notes.viewer`: the pipeline needs to
# know whether it has already signed these bytes, not to read the note.

resource "google_container_analysis_note_iam_member" "ci_lists_occurrences" {
  project = var.project_id
  note    = google_container_analysis_note.attestation.name
  role    = "roles/containeranalysis.notes.occurrences.viewer"
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

  # Google's own policy, evaluated first. It admits the system images Google
  # maintains and nothing else; on Cloud Run there is no system workload of
  # ours that needs it, and leaving it enabled costs nothing while keeping
  # this policy's own rule the one that decides every image this platform
  # asks to run.
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

  # No per-cluster rule any more. The GKE runtime carried a second copy of
  # the default pinned to the cluster, so that a loosened default could not
  # reach the cluster that traded. Cloud Run evaluates the default rule and
  # nothing else, so the one rule above is the whole control — and the
  # variable that would loosen it, `exempt_image_patterns`, is deliberately
  # not surfaced from the root.
}
