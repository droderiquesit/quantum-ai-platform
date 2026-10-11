variable "enabled" {
  type        = bool
  description = "Whether to provision event fabric brokers (false until FABRIC-086 consensus record exists)"
  default     = false
  nullable    = false

  validation {
    condition     = var.enabled ? true : true # Always admits false; forward-compatible
    error_message = "Event fabric broker provisioning must align with ADR 0100 §3 consensus requirements."
  }
}

variable "project_id" {
  type        = string
  description = "GCP project ID"
  nullable    = false

  validation {
    condition     = length(var.project_id) > 0
    error_message = "project_id must not be empty."
  }
}

variable "region" {
  type        = string
  description = "GCP region for event fabric deployment"
  nullable    = false

  validation {
    condition     = length(var.region) > 0
    error_message = "region must not be empty."
  }
}

variable "network_id" {
  type        = string
  description = "VPC network ID where brokers run (internal only, FABRIC-089)"
  nullable    = false

  validation {
    condition     = length(var.network_id) > 0
    error_message = "network_id must not be empty."
  }
}

variable "subnet_id" {
  type        = string
  description = "VPC subnet ID for broker instances"
  nullable    = false

  validation {
    condition     = length(var.subnet_id) > 0
    error_message = "subnet_id must not be empty."
  }
}

variable "machine_type" {
  type        = string
  description = "Machine type for event fabric brokers"
  default     = "c3-standard-4"
  nullable    = false
}

variable "boot_image" {
  type        = string
  description = "Boot image for broker instances (self-link, not family)"
  nullable    = false

  validation {
    condition     = can(regex("^projects/.+/global/images/.+$", var.boot_image))
    error_message = "boot_image must be a self-link to a specific image, not a family."
  }
}

variable "disk_type" {
  type        = string
  description = "Disk type for broker journal (P2 class Hyperdisk, FABRIC-085)"
  default     = "hyperdisk-balanced"
  nullable    = false
}

variable "disk_size_gb" {
  type        = number
  description = "Journal disk size in GB"
  default     = 100
  nullable    = false

  validation {
    condition     = var.disk_size_gb >= 10
    error_message = "disk_size_gb must be at least 10."
  }
}

variable "service_account_email" {
  type        = string
  description = "Service account email for broker instances"
  nullable    = false

  validation {
    condition     = can(regex("^.+@.+\\.iam\\.gserviceaccount\\.com$", var.service_account_email))
    error_message = "service_account_email must be a valid GCP service account email."
  }
}

variable "dns_zone_name" {
  type        = string
  description = "DNS zone name for SRV record registration (FABRIC-087)"
  nullable    = false

  validation {
    condition     = length(var.dns_zone_name) > 0
    error_message = "dns_zone_name must not be empty."
  }
}

variable "managed_zone" {
  type        = string
  description = "Managed DNS zone resource name for SRV records"
  nullable    = false

  validation {
    condition     = length(var.managed_zone) > 0
    error_message = "managed_zone must not be empty."
  }
}

variable "broker_internal_addresses" {
  type        = list(string)
  description = "Internal IP addresses for brokers (no external IPs, FABRIC-089)"
  nullable    = false

  validation {
    condition     = length(var.broker_internal_addresses) > 0
    error_message = "broker_internal_addresses must not be empty."
  }

  # RF1 (ADR 0100 §3): one broker per region, the sole writer of every
  # partition's chain (§2). A second address would start a second broker
  # with no consensus between them — two writers of one chain, each
  # acknowledging records the other never saw. Refused until FABRIC-086's
  # replication exists, rather than admitted and discovered in a fork.
  validation {
    condition     = length(var.broker_internal_addresses) <= 1
    error_message = "broker_internal_addresses names more than one broker. The first build is RF1 with no consensus (ADR 0100 §3); two brokers are two writers of one chain. One address until FABRIC-086 lands."
  }

  # Every address is a dotted IPv4 literal. It is written into a DNS record
  # and into a resource name; anything else is a record nobody can reach.
  validation {
    condition = alltrue([
      for address in var.broker_internal_addresses :
      can(regex("^([0-9]{1,3}\\.){3}[0-9]{1,3}$", address))
    ])
    error_message = "Every broker address is an IPv4 address such as 10.0.20.10."
  }
}

variable "zone" {
  type        = string
  description = "Compute Engine zone the broker machine runs in, e.g. us-east4-a. Empty creates the template and the DNS records and no machine, which is what this module did before W4.2."
  default     = ""
  nullable    = false

  validation {
    condition     = var.zone == "" || can(regex("^[a-z]+-[a-z]+[0-9]+-[a-z]$", var.zone))
    error_message = "zone is a Compute Engine zone such as us-east4-a, or empty."
  }
}

variable "network_tags" {
  type        = list(string)
  description = "Network tags the broker carries, first among them the control-fabric trust zone's tag, so the zone's firewall rules apply to it."
  default     = []
  nullable    = false
}

variable "journal_disk_size_gb" {
  type        = number
  description = "Size of the broker's journal disk in GB — its own disk, attached as `qip-journal`."
  default     = 100
  nullable    = false

  validation {
    condition     = var.journal_disk_size_gb >= 10
    error_message = "journal_disk_size_gb must be at least 10."
  }
}

variable "broker_port" {
  type        = number
  description = "Port for event fabric broker protocol (TCP, ADR 0100 §1)"
  default     = 9200
  nullable    = false

  validation {
    condition     = var.broker_port > 1024 && var.broker_port < 65536
    error_message = "broker_port must be between 1024 and 65535."
  }
}

variable "qip_events_config" {
  type        = string
  description = "Base64-encoded qip-events configuration JSON"
  nullable    = false

  validation {
    condition     = length(var.qip_events_config) > 0
    error_message = "qip_events_config must not be empty."
  }
}

variable "qip_fabricd_config" {
  type        = string
  description = "Base64-encoded qip-fabricd configuration JSON"
  nullable    = false

  validation {
    condition     = length(var.qip_fabricd_config) > 0
    error_message = "qip_fabricd_config must not be empty."
  }
}

variable "qip_storage_journal_device" {
  type        = string
  description = "Block device path for event fabric journal storage. The default is the stable name Google gives the template's `qip-journal` disk; `/dev/sdb` was the default, and named a disk the template did not declare."
  default     = "/dev/disk/by-id/google-qip-journal"
  nullable    = false

  # Substituted into a root-run startup script by `templatefile`, which
  # escapes nothing (infrastructure/CLAUDE.md). A path of word characters
  # and slashes under /dev, and nothing a shell would read as syntax.
  validation {
    condition     = can(regex("^/dev/[A-Za-z0-9/_.-]+$", var.qip_storage_journal_device))
    error_message = "qip_storage_journal_device is a device path under /dev made of letters, digits, '/', '_', '.' and '-'."
  }
}
