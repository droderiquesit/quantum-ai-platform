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

variable "secret_names" {
  description = "The secrets that exist. Values are written out of band."
  type        = list(string)
}

variable "venue_credential_reader" {
  description = <<-EOT
    The identity the venue credential is granted to, when it is granted at
    all: the fast brain's Cloud Run service account, from the catalogue.

    Required rather than defaulted so the root has to say which workload
    could ever hold the credential. It is read only inside the conditional
    binding below, so in every environment a plan can carry — where
    `venue_credential_readable` is false — the value names nothing that is
    granted anything.
  EOT
  type        = string
}

variable "venue_credential_readable" {
  description = <<-EOT
    Whether the venue credential may be read in this environment.

    False in any environment whose autonomy ceiling is paper trading. A
    credential that cannot be read cannot be misused by a misconfigured
    application, which is a stronger guarantee than one the application makes
    about itself.
  EOT
  type        = bool
  default     = false
}

variable "project_number" {
  description = <<-EOT
    The project's numeric id, which is not the project id and cannot be derived
    from it. Google service agents are named by number — Secret Manager
    publishes rotation notices as
    `service-<number>@gcp-sa-secretmanager.iam.gserviceaccount.com` — so the
    grant that lets rotation work at all needs this value.
  EOT
  type        = number
}

variable "console_enabled" {
  type = bool
  # The console's own identity exists only where the console has a platform to
  # read. An environment that has not opted into ADR 0018's route gets no
  # account and no grant, because an identity that can read a credential it
  # has no way to use is a standing grant with no purpose to justify it.
  default     = false
  description = "Create the console's service account and let it read the viewer token."
}

variable "kms_protection_level" {
  description = "Protection level for the key ring's secrets key. Set from the root's single value so the platform cannot be HSM for one key and software for another."
  type        = string
  default     = "SOFTWARE"

  validation {
    # Defended here as well as at the root, because this module is callable on
    # its own and an input nobody checks is a gap wherever it is reached from.
    # EXTERNAL and EXTERNAL_VPC need an EKM connection this configuration does
    # not declare; the root variable of the same name carries the argument.
    condition     = contains(["SOFTWARE", "HSM"], var.kms_protection_level)
    error_message = "kms_protection_level must be exactly \"SOFTWARE\" or \"HSM\"."
  }
}
