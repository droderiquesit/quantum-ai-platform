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
    condition     = can(regex("^[a-z]+-[a-z]+[0-9]+$", var.region))
    error_message = "region is a Google Cloud region such as us-east4; it is written into the broker's environment file and its lease key."
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
  # ADR 0111 (accepted): two to three instances per region, the leader chosen
  # by the Spanner lease on the managed P0/P1 path (FABRIC-8096). Never more:
  # the ADR names three as the ceiling a region's lease is sized for. And
  # never two or more on the local single-broker fabric, where two machines
  # are two writers of one chain with nothing to choose between them — the
  # refusal ADR 0100 §3 made, kept for the configuration it still describes.
  validation {
    condition     = length(var.broker_internal_addresses) <= 3
    error_message = "broker_internal_addresses names more than three brokers. ADR 0111 runs two to three instances per region under one Spanner lease; a fourth is not a configuration the lease was sized for."
  }

  validation {
    condition     = length(var.broker_internal_addresses) <= 1 || var.managed_log != null
    error_message = "broker_internal_addresses names more than one broker and managed_log is null. Without ADR 0111's Spanner lease two brokers are two writers of one chain (ADR 0100 §3); name the managed log's database, or one address."
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

# --- ADR 0110 / 0111 / 0112: what the broker must be told to start ----------
#
# `qip-fabricd` refuses to start outside a `local` deployment without its
# deployment named, without mutual TLS on its listener, and without the
# workloads it admits. Each is a variable here, validated to the character
# set it is written into the environment file with, because `templatefile`
# escapes nothing.

variable "deployment" {
  type        = string
  description = "The deployment the broker runs in (QIP_DEPLOYMENT): the environment's name. Never `local` on a machine."
  nullable    = false

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,30}$", var.deployment)) && var.deployment != "local"
    error_message = "deployment is the environment's name, lower-case letters, digits and '-'; `local` is a developer's machine and admits plaintext, which a deployed broker must not."
  }
}

variable "workload_certificates" {
  description = <<-EOT
    The CA Service pool and template the broker requests its mTLS workload
    certificate from (ADR 0112 §2), the lifetime it asks for, and the pool's
    CA certificates it verifies clients against. Full resource names by
    project id, the form the requester grant's IAM condition matches.
  EOT

  type = object({
    ca_pool              = string
    certificate_template = string
    lifetime_secs        = number
    trust_roots_pem      = string
  })
  nullable = false

  validation {
    condition = (
      can(regex("^projects/[a-z][a-z0-9-]{4,28}[a-z0-9]/locations/[a-z0-9-]+/caPools/[A-Za-z0-9_-]+$", var.workload_certificates.ca_pool))
      && can(regex("^projects/[a-z][a-z0-9-]{4,28}[a-z0-9]/locations/[a-z0-9-]+/certificateTemplates/[A-Za-z0-9_-]+$", var.workload_certificates.certificate_template))
    )
    error_message = "workload_certificates names the CA pool and the template by full resource name with a project id; both are written into the broker's environment file."
  }

  validation {
    condition     = var.workload_certificates.lifetime_secs >= 3600 && var.workload_certificates.lifetime_secs <= 86400 && floor(var.workload_certificates.lifetime_secs) == var.workload_certificates.lifetime_secs
    error_message = "workload_certificates.lifetime_secs is a whole number of seconds from an hour to a day (ADR 0112 §2)."
  }

  validation {
    condition     = strcontains(var.workload_certificates.trust_roots_pem, "-----BEGIN CERTIFICATE-----")
    error_message = "workload_certificates.trust_roots_pem is the CA pool's PEM certificates."
  }
}

variable "google_tls_roots_path" {
  type        = string
  description = "Where the boot image carries the pinned Google roots the broker authenticates CA Service (and the managed log's Spanner) against: /etc/qip/trust/<file>.pem."
  nullable    = false

  validation {
    condition     = can(regex("^/etc/qip/trust/[a-z0-9-]+\\.pem$", var.google_tls_roots_path))
    error_message = "google_tls_roots_path is /etc/qip/trust/<name>.pem."
  }
}

variable "admitted_peers" {
  type        = list(string)
  description = <<-EOT
    The workloads the broker's listener admits (QIP_EVENT_FABRIC_ADMITTED_PEERS),
    as the DNS SANs their CA Service certificates carry: each client's
    service-account email with `@` read as `.`, the one name the workload
    template lets that client obtain. Derived at the root from the same
    identities granted certificate requests, so the two cannot disagree.
  EOT
  nullable    = false

  # An empty list is "any certificate the pool issued", which on a pool every
  # node can request from admits a node to a ledger's stream. qip-fabricd
  # refuses it outside local; so does this.
  validation {
    condition     = length(var.admitted_peers) > 0
    error_message = "admitted_peers is empty. A broker admitting every certificate its pool issues admits every workload in the environment; name the clients (the edge nodes and qip-ledgerd)."
  }

  validation {
    condition = alltrue([
      for peer in var.admitted_peers :
      can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]\\.[a-z][a-z0-9-]{4,28}[a-z0-9]\\.iam\\.gserviceaccount\\.com$", peer))
    ])
    error_message = "Every admitted peer is a service account's SAN, <account>.<project>.iam.gserviceaccount.com — the only name the workload template issues."
  }
}

variable "managed_log" {
  description = <<-EOT
    ADR 0111's managed P0/P1 commit path, or null for the local single-broker
    fabric. `spanner_database` is the database holding the region's leader
    lease (projects/<p>/instances/<i>/databases/<d>); `lease_ttl_ms` the
    lease's life, which the broker bounds to 1.5–120 s. Required for more than
    one broker: it is what makes two brokers one leader and a follower rather
    than two writers.
  EOT

  type = object({
    spanner_database = string
    lease_ttl_ms     = optional(number, 10000)
  })
  default = null

  validation {
    condition     = var.managed_log == null || can(regex("^projects/[a-z][a-z0-9-]{4,28}[a-z0-9]/instances/[a-z][a-z0-9-]{1,62}/databases/[a-z][a-z0-9_-]{1,29}$", var.managed_log.spanner_database))
    error_message = "managed_log.spanner_database is projects/<project>/instances/<instance>/databases/<database>."
  }

  validation {
    condition     = var.managed_log == null || try(var.managed_log.lease_ttl_ms >= 1500 && var.managed_log.lease_ttl_ms <= 120000 && floor(var.managed_log.lease_ttl_ms) == var.managed_log.lease_ttl_ms, false)
    error_message = "managed_log.lease_ttl_ms is a whole number of milliseconds from 1500 to 120000, the bounds qip-fabricd enforces."
  }
}
