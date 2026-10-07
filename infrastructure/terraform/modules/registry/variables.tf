variable "project_id" {
  type = string
}

variable "region" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "ci_service_account" {
  description = "The pipeline's service account. Pushes; cannot delete."
  type        = string
}

variable "pull_service_accounts" {
  description = <<-EOT
    The accounts permitted to pull, keyed by the workload they belong to.
    The node service account belongs here, because the kubelet pulls as the
    node rather than as the pod.

    Empty by default: a registry nothing can read is useless and obvious, and a
    registry everything can read is useful and invisible.

    A map and not a list, and the keys are the point. These emails are
    produced by another module, so a command with no plan behind it —
    `terraform import`, which the reclaim step in `infra.yml` runs against an
    environment being rebuilt from nothing — cannot know them, and a
    `for_each` over a set of unknown strings is a refusal rather than an
    empty grant. Keys fixed here, values arriving at apply, is Terraform's
    own remedy and the shape the evidence module now uses for the same
    reason.
  EOT

  type    = map(string)
  default = {}
}

# The project number, for the Cloud Run service agent's own name.
#
# Not the project id: a service agent is named by number, and the two are
# different strings for the same project. Passed in rather than looked up
# here so this module makes no API call of its own.
variable "project_number" {
  description = "The numeric project id, used to name the Cloud Run service agent that pulls images."
  type        = number
}

# The environment's key ring, for the key the two GENERIC repositories are
# encrypted with (SEC-046). The ring is `modules/secrets`', as every other
# keyed store's is; the key itself is this module's own.
variable "key_ring_id" {
  description = "The environment's KMS key ring, as projects/<project>/locations/<region>/keyRings/<ring>. The registry's key is created in it."
  type        = string
  nullable    = false

  # Artifact Registry encrypts a repository only with a key in the
  # repository's own location, and says so at apply — after the plan has been
  # read and half the environment built. Every repository here is in
  # `var.region`, so the ring has to be there too, and the plan says so first.
  validation {
    condition     = can(regex("^projects/[^/]+/locations/${var.region}/keyRings/[^/]+$", var.key_ring_id))
    error_message = "key_ring_id must be a key ring in ${var.region} (projects/<project>/locations/${var.region}/keyRings/<ring>). Artifact Registry refuses a key from any other location, and refuses it only at apply."
  }
}

variable "kms_protection_level" {
  description = "Protection level for the registry key. Set from the root's single value so the platform cannot be HSM for one key and software for another."
  type        = string
  default     = "SOFTWARE"

  validation {
    # Defended here as well as at the root, as `modules/evidence` does: this
    # module is callable on its own. EXTERNAL and EXTERNAL_VPC need an EKM
    # connection this configuration does not declare.
    condition     = contains(["SOFTWARE", "HSM"], var.kms_protection_level)
    error_message = "kms_protection_level must be exactly \"SOFTWARE\" or \"HSM\"."
  }
}
