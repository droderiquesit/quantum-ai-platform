# Every value below that reaches the startup script is substituted by
# `templatefile`, which escapes nothing, into a script that runs as root
# (infrastructure/CLAUDE.md). Each such variable carries a validation that
# confines it to a character set no shell reads as syntax; adding a
# substitution without one reopens that hole.

variable "project_id" {
  type        = string
  description = "GCP project ID."
  nullable    = false
}

variable "environment" {
  type        = string
  description = "Environment name; prefixes every resource."
  nullable    = false

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,15}$", var.environment))
    error_message = "environment is lowercase letters, digits and '-', starting with a letter."
  }
}

variable "labels" {
  type        = map(string)
  description = "Labels every resource carries."
  default     = {}
}

variable "zone" {
  type        = string
  description = "Compute Engine zone the ledger machine and its archive disk live in."
  nullable    = false

  validation {
    condition     = can(regex("^[a-z]+-[a-z]+[0-9]+-[a-z]$", var.zone))
    error_message = "zone is a Compute Engine zone such as us-east4-a."
  }
}

variable "network_id" {
  type        = string
  description = "The network the ledger zone's subnet is in."
  nullable    = false
}

variable "subnet_id" {
  type        = string
  description = "The ledger trust zone's subnet."
  nullable    = false
}

variable "network_tags" {
  type        = list(string)
  description = "Network tags, first among them the ledger trust zone's, so the zone's default-deny and its declared paths apply to the machine."
  nullable    = false

  validation {
    condition     = length(var.network_tags) > 0
    error_message = "The ledger carries at least its trust zone's tag; a machine with none is outside every boundary modules/trust-zones draws."
  }
}

variable "internal_address" {
  type        = string
  description = "The stable internal IPv4 address the read API answers on."
  nullable    = false

  validation {
    condition     = can(regex("^([0-9]{1,3}\\.){3}[0-9]{1,3}$", var.internal_address))
    error_message = "internal_address is an IPv4 address such as 10.0.21.10."
  }
}

variable "boot_image" {
  type        = string
  description = "Boot image carrying qip-ledgerd and its systemd unit, as a self-link to one image, never a family."
  nullable    = false

  validation {
    condition     = can(regex("^projects/[a-z0-9-]+/global/images/[a-z0-9-]+$", var.boot_image))
    error_message = "boot_image is a self-link to one image (projects/<p>/global/images/<name>). A family moves under the machine after the review that approved it."
  }
}

variable "machine_type" {
  type        = string
  description = "Machine type for the ledger."
  default     = "e2-standard-2"
  nullable    = false
}

variable "archive_disk_size_gb" {
  type        = number
  description = "Size of the ledger archive disk in GB."
  default     = 50
  nullable    = false

  validation {
    condition     = var.archive_disk_size_gb >= 10
    error_message = "archive_disk_size_gb must be at least 10."
  }
}

variable "account_id" {
  type        = string
  description = "The ledger's book identifier (QIP_LEDGER_ACCOUNT_ID): an internal name such as `paper-book`, never a venue or bank account number."
  nullable    = false

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,62}$", var.account_id))
    error_message = "account_id is an internal book name of lowercase letters, digits and '-', starting with a letter. A venue or bank account number does not belong in committed configuration."
  }
}

variable "fabric_consumer_group" {
  type        = string
  description = "The event-fabric consumer group the ledger reads P1 outcomes as (QIP_LEDGER_FABRIC_CONSUMER_GROUP)."
  default     = "qip-ledgerd"
  nullable    = false

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,62}$", var.fabric_consumer_group))
    error_message = "fabric_consumer_group is lowercase letters, digits and '-', starting with a letter."
  }
}

variable "listen_port" {
  type        = number
  description = "Port the read-side API listens on, on the internal address."
  default     = 8090
  nullable    = false

  validation {
    condition     = var.listen_port > 1024 && var.listen_port < 65536 && floor(var.listen_port) == var.listen_port
    error_message = "listen_port is a whole number between 1025 and 65535."
  }
}
