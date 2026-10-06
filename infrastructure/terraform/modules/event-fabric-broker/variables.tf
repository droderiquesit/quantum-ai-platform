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
  description = "Block device path for event fabric journal storage"
  default     = "/dev/sdb"
  nullable    = false
}
