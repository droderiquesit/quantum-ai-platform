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

variable "key_ring_id" {
  description = "The platform's existing KMS key ring. The evidence key is created in it."
  type        = string
}

variable "retention_days" {
  description = <<-EOT
    How long an object cannot be deleted for.

    Seven years, because that is the longest of the record-retention periods
    the platform's compliance surface has to satisfy and a retention policy can
    only ever be lengthened once locked. Shortening it later is impossible, so
    the default is the long one.
  EOT

  type    = number
  default = 2557

  validation {
    condition     = var.retention_days >= 1
    error_message = "A retention policy of zero days is not a retention policy."
  }
}

variable "retention_locked" {
  description = <<-EOT
    Whether the retention policy is locked.

    True by default, which is irreversible: a locked policy survives every
    later change to this configuration and cannot be shortened or removed for
    the life of the bucket. That is what makes the store immutable to the
    people who run the platform, and it is the only version of the guarantee
    worth having.

    Set it to false only in an environment whose evidence is disposable, and
    know that doing so means the store is append-only by convention.
  EOT

  type    = bool
  default = true
}

variable "writer_service_accounts" {
  description = <<-EOT
    Accounts granted object *creation* and nothing else.

    An empty list is no writers, not all of them.
  EOT

  type    = list(string)
  default = []
}

variable "reader_service_accounts" {
  description = "Accounts granted object read. Empty by default."
  type        = list(string)
  default     = []
}

variable "kms_protection_level" {
  description = "Protection level for the evidence key. Set from the root's single value so the platform cannot be HSM for one key and software for another."
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
