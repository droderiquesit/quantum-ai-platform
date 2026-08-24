# What the pipeline needs to sign with, and nothing secret.
#
# The private half of the signing key never leaves Cloud KMS and appears in no
# output here. What these carry is the *name* of the key version to sign with
# and the attestor to sign for, which is what
# `gcloud container binauthz attestations sign-and-create` takes.

output "attestor_name" {
  description = <<-EOT
    The attestor's short name.

    Set this as the GitHub repository variable `GCP_BINAUTHZ_ATTESTOR`. Until
    it is set, `deploy.yml` refuses to build rather than pushing an image
    nothing will sign — see OUT-OF-BAND.md.
  EOT
  value       = google_binary_authorization_attestor.build.name
}

output "attestor_key_version" {
  description = <<-EOT
    The fully qualified KMS key version that signs, for the GitHub repository
    variable `GCP_BINAUTHZ_KEY_VERSION`.

    Fully qualified rather than the bare version number, because the pipeline
    should not be reconstructing a key ring name it would then be a second
    place to get wrong.
  EOT
  value       = data.google_kms_crypto_key_version.attestor.id
}

output "attestation_note" {
  description = "The Container Analysis note attestations are attached to."
  value       = google_container_analysis_note.attestation.name
}

output "signing_key_id" {
  description = "The attestor's signing key, for an operator granting a second signer."
  value       = google_kms_crypto_key.attestor.id
}

# The gap, as data — the same idea as `enabled_without_an_adapter` in
# `modules/data`. A deployment renders this at plan time rather than
# discovering at admission that the policy denies everything.
output "admits_without_an_attestation" {
  description = "Image patterns this policy admits unsigned. Empty is the intended state."
  value       = var.exempt_image_patterns
}
