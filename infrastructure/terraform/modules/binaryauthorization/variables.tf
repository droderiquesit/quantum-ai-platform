variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "key_ring_id" {
  description = <<-EOT
    The platform's existing KMS key ring, from `modules/secrets`.

    The signing key is created in it rather than in a ring of this module's
    own, for the reason the evidence and data modules give: a second key ring
    is a second thing nobody rotates and a second place an auditor has to be
    told about.
  EOT
  type        = string
}

variable "ci_service_account" {
  description = <<-EOT
    The pipeline's service account, which is the identity that signs.

    This is the honest shape of the control and its main limitation: whoever
    controls the pipeline controls the signature. See OUT-OF-BAND.md for what a
    stronger arrangement would be and why this repository cannot contain it.
  EOT
  type        = string
}

variable "signing_algorithm" {
  description = <<-EOT
    The KMS algorithm the attestor's key uses.

    RSA with SHA-512 by default. Binary Authorization accepts a PKIX key of
    several algorithms; the constraint that matters is that the attestor's
    declared algorithm and the key's own must agree, which is why this is read
    back from KMS rather than written twice.
  EOT
  type        = string
  default     = "RSA_SIGN_PKCS1_4096_SHA512"

  validation {
    condition = contains([
      "RSA_SIGN_PKCS1_2048_SHA256",
      "RSA_SIGN_PKCS1_3072_SHA256",
      "RSA_SIGN_PKCS1_4096_SHA256",
      "RSA_SIGN_PKCS1_4096_SHA512",
      "EC_SIGN_P256_SHA256",
      "EC_SIGN_P384_SHA384",
    ], var.signing_algorithm)
    error_message = "The algorithm must be one Binary Authorization can verify a PKIX signature with."
  }
}

variable "exempt_image_patterns" {
  description = <<-EOT
    Image paths admitted without an attestation.

    Empty by default, and every entry added is an image that runs unsigned. The
    common reason to want one — GKE's own system images — is already handled by
    `global_policy_evaluation_mode = "ENABLE"`, which lets Google's policy admit
    the images Google maintains. An entry here is for a third-party image this
    platform chose to run and cannot sign, and it should name the exact
    repository rather than a prefix that would cover the next thing published
    under it.

    A pattern of `*` or `*/*` is refused below. It would admit everything while
    leaving a policy in place that reads as though it denies, which is the
    failure this whole module exists to end.
  EOT
  type        = list(string)
  default     = []

  validation {
    condition = alltrue([
      for pattern in var.exempt_image_patterns :
      !contains(["*", "**", "*/*", "*/**"], trimspace(pattern))
    ])
    error_message = "A pattern matching every image is not an exemption; it is the policy turned off. Name the repository."
  }
}
