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

variable "network_id" {
  type = string
}

variable "subnet_id" {
  type = string
}

variable "pod_range" {
  type = string
}

variable "service_range" {
  type = string
}

variable "authorised_networks" {
  type = list(object({
    cidr_block   = string
    display_name = string
  }))
}

variable "node_count" {
  type = number
}

variable "machine_type" {
  type = string
}

variable "kms_key_id" {
  type = string
}

variable "service_account" {
  type = string
}

# --- Autoscaling ------------------------------------------------------------
#
# Both are **per zone**. This is a regional cluster, so the real range is three
# times each number, and a reader who takes them as regional totals will size
# the pool at a third of what they meant.

variable "min_node_count" {
  description = <<-EOT
    The smallest the pool may shrink to, per zone.

    Not zero, and not one. The autoscaler removes a node by draining it, so the
    floor is the amount of capacity that is never reclaimed under a quiet
    period — and a quiet period on this platform is a market that is closed,
    followed by one that opens. A floor that lets the pool collapse overnight
    buys a few hours of a smaller bill and pays for it with a cold start and a
    wave of evictions at the open.
  EOT
  type        = number
  default     = 2

  validation {
    condition     = var.min_node_count >= 1 && var.min_node_count <= 20
    error_message = "The minimum node count must be between 1 and 20, per zone."
  }
}

variable "max_node_count" {
  description = <<-EOT
    The largest the pool may grow to, per zone.

    A ceiling rather than a target: nothing scales to it unless pods are
    unschedulable. It exists because the alternative to a bound is an
    autoscaler that answers a runaway workload — or a workload wedged in a
    crash loop that requests four CPUs — by buying nodes until somebody
    notices the bill.

    Six per zone is eighteen regionally, against a committed two per zone. That
    is room for `qip-api` to reach its `maxReplicas: 6`, for the edge cells to
    be rescheduled off a lost node, and for a rolling upgrade to surge, without
    being room for an accident to run for a day.
  EOT
  type        = number
  default     = 6

  validation {
    condition     = var.max_node_count >= 1 && var.max_node_count <= 50
    error_message = "The maximum node count must be between 1 and 50, per zone."
  }
}

variable "maintenance_exclusions" {
  description = <<-EOT
    Dated periods during which no maintenance happens at all, keyed by name.

    Empty by default, and that is not a placeholder. A GKE maintenance
    exclusion is a fixed pair of timestamps; there is no recurring form, so
    "never during market hours" is not something this file can say. The weekly
    window in the maintenance policy is what covers the ordinary case — Sunday,
    when no venue this platform trades is open.

    This is for the specific dated freeze: a quarterly roll, an exchange
    migration weekend, the week either side of a go-live. Google allows at most
    three, and `NO_MINOR_OR_NODE_UPGRADES` may not exceed 180 days.

    `scope` is one of:

      * `NO_UPGRADES`                — nothing at all, including patches.
      * `NO_MINOR_UPGRADES`          — patches still land; minor versions do not.
      * `NO_MINOR_OR_NODE_UPGRADES`  — the control plane may take a patch; the
                                       nodes are left alone entirely.

    The last is usually the one wanted: it freezes the nodes, which is where
    the workload is, without leaving the control plane unpatched.
  EOT

  type = map(object({
    start_time = string
    end_time   = string
    scope      = optional(string, "NO_MINOR_OR_NODE_UPGRADES")
  }))

  default = {}

  validation {
    condition = alltrue([
      for exclusion in values(var.maintenance_exclusions) :
      contains(["NO_UPGRADES", "NO_MINOR_UPGRADES", "NO_MINOR_OR_NODE_UPGRADES"], exclusion.scope)
    ])
    error_message = "An exclusion scope must be NO_UPGRADES, NO_MINOR_UPGRADES or NO_MINOR_OR_NODE_UPGRADES."
  }

  validation {
    condition     = length(var.maintenance_exclusions) <= 3
    error_message = "Google permits at most three maintenance exclusions. A fourth is rejected at apply, after the cluster exists."
  }
}

variable "enable_confidential_nodes" {
  description = <<-EOT
    Whether the nodes run as Confidential VMs, with their memory encrypted by
    the AMD SEV key the host cannot read.

    **Off, and off is a decision rather than a default.**
    modules/data/NOT-PROVISIONED.md carries the argument; it is repeated here
    because this is where somebody will read it.

    The hardening is real. The reason it is not simply on is the name next to
    it: `backend/crates/libs/qip-confidential` is **not** confidential computing. It is
    statistical disclosure control — a k-anonymity gate, a monotone privacy
    budget and calibrated noise — and its own module documentation says in its
    first paragraph that there is no enclave, no attestation and no hardware
    isolation. A configuration with `confidential_nodes` enabled, sitting
    alongside a crate called `qip-confidential`, lets the two together be read
    as a guarantee that neither one makes. Nothing in this platform attests a
    node, and no decision anywhere is gated on a node having been attested.

    So: turn it on as defence in depth if that is what you want, and do not
    conclude from having turned it on that fabric D is now confidential
    computing.

    Two things it costs, both of which the precondition on the node pool will
    tell you about at plan time rather than at apply:

      * The machine family must be AMD — n2d, c2d or c3d. Neither the
        `n2-standard-4` default nor production's `e2-standard-16` qualifies, so
        this is never a one-line change.
      * It cannot be turned on in place. The cluster is replaced.
  EOT
  type        = bool
  default     = false
}

variable "node_disk_type" {
  description = <<-EOT
    The node boot disks' type.

    pd-ssd by default for the environments that trade, because a node's disk
    is where every image pull and every container write lands. Development
    overrides this to pd-standard: pd-ssd and pd-balanced both count against
    the region's SSD_TOTAL_GB quota, 250GB on a fresh project, and a regional
    cluster's three boot disks alone exceed it.
  EOT
  type        = string
  default     = "pd-ssd"

  validation {
    condition     = contains(["pd-standard", "pd-balanced", "pd-ssd"], var.node_disk_type)
    error_message = "The node disk type must be pd-standard, pd-balanced or pd-ssd."
  }
}

variable "node_disk_size_gb" {
  description = "The node boot disks' size. Multiplied by every node in every zone, which is what the quota sees."
  type        = number
  default     = 100

  validation {
    condition     = var.node_disk_size_gb >= 20 && var.node_disk_size_gb <= 500
    error_message = "The node disk size must be between 20 and 500 GB."
  }
}

variable "cluster_deletion_protection" {
  description = <<-EOT
    Whether Terraform (and the console) may destroy this cluster at all,
    independent of and in addition to Terraform's own `prevent_destroy`.

    True by default — deleting a cluster that could hold a live book should
    take deliberate effort. Set false in any environment `infra.yml down` is
    meant to tear down between sessions, or the teardown fails with the same
    refusal a stuck, tainted cluster does: "Cannot destroy cluster because
    deletion_protection is set to true."
  EOT
  type        = bool
  default     = true
}
