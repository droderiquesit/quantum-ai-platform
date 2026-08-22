# Inputs.
#
# Every variable that could make the deployment less safe has a restrictive
# default and a validation rule. A variable whose default is dangerous is a
# variable someone will forget to set.

variable "project_id" {
  description = "The Google Cloud project."
  type        = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.project_id))
    error_message = "The project id must be a valid Google Cloud project identifier."
  }
}

variable "region" {
  description = "The region everything is deployed to."
  type        = string
  default     = "europe-west2"
}

variable "environment" {
  description = "Which environment this is: development, staging or production."
  type        = string

  validation {
    condition     = contains(["development", "staging", "production"], var.environment)
    error_message = "The environment must be development, staging or production."
  }
}

variable "autonomy_ceiling" {
  description = <<-EOT
    The highest autonomy level this environment's platform may reach.

    Paper trading by default, and deliberately an input rather than something
    derived from the environment name: an environment called "production" that
    trades on paper is a perfectly reasonable thing to want, and inferring the
    ceiling would take that choice away.

    Setting this above paper_trading does not enable live trading. It permits
    two authenticated operators to enable it, which is a separate act.
  EOT
  type        = string
  default     = "paper_trading"

  validation {
    condition = contains([
      "observation",
      "advisory",
      "paper_trading",
      "supervised_live",
      "limited_autonomous_live",
      "autonomous_live",
    ], var.autonomy_ceiling)
    error_message = "The autonomy ceiling must be one of the six declared levels."
  }
}

variable "subnet_cidr" {
  description = "The primary subnet range for nodes."
  type        = string
  default     = "10.0.0.0/20"
}

variable "pod_cidr" {
  description = "The secondary range for pods."
  type        = string
  default     = "10.4.0.0/14"
}

variable "service_cidr" {
  description = "The secondary range for services."
  type        = string
  default     = "10.8.0.0/20"
}

variable "authorised_networks" {
  description = <<-EOT
    Which CIDR blocks may reach the Kubernetes control plane.

    Empty by default, which means nobody: a cluster reachable from anywhere is
    not a private cluster, and defaulting to the whole internet so that the
    first apply succeeds is how that happens.
  EOT
  type = list(object({
    cidr_block   = string
    display_name = string
  }))
  default = []

  validation {
    condition = alltrue([
      for network in var.authorised_networks :
      network.cidr_block != "0.0.0.0/0"
    ])
    error_message = "0.0.0.0/0 is not an authorised network. Name the ranges that need access."
  }
}

variable "node_count" {
  description = "Nodes per zone in the default pool."
  type        = number
  default     = 2

  validation {
    condition     = var.node_count >= 1 && var.node_count <= 20
    error_message = "The node count must be between 1 and 20."
  }
}

variable "machine_type" {
  description = "The node machine type."
  type        = string
  default     = "n2-standard-4"
}

variable "notification_channels" {
  description = "Where alerts are sent. An alert with nowhere to go is not an alert."
  type        = list(string)
  default     = []
}

variable "github_repository" {
  description = <<-EOT
    The GitHub repository permitted to deploy, as `owner/name`.

    No default. A default here would be a repository somebody else could be
    running, and the consequence of getting it wrong is that their pipeline can
    push images and apply manifests in this project.
  EOT

  type = string

  validation {
    condition     = can(regex("^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$", var.github_repository))
    error_message = "The repository is owner/name, with no scheme and no trailing path."
  }
}

variable "edge_cells" {
  description = <<-EOT
    The edge cells this environment runs, keyed by cell id.

    ADR 0008 calls for seven, next to the venues they trade. This is a map so
    that the eighth is an entry rather than a directory: seven copies of a
    network policy is seven places for one of them to be wrong, and the wrong
    one is the one nobody reads.

    Empty by default. A cell that has not been asked for is not created, and an
    environment with no cells is a central plane on its own, which is a working
    configuration rather than a broken one.

    The seven planned locations, their cell ids and the regions they would run
    in are in docs/operations/deploying-an-edge-cell.md, together with the two
    of them that have no Google Cloud region in the right metropolitan area and
    what that costs.

    Every field is deliberate:

      * `region` is chosen for its distance to the venues, not for convenience.
      * The three CIDRs must not overlap another cell's. Overlapping ranges
        route to whichever subnet was created first, silently.
      * `venues` is empty until somebody has the venue's published address
        ranges in front of them. An empty map is no venues, not all of them.
  EOT

  type = map(object({
    region       = string
    subnet_cidr  = string
    pod_cidr     = string
    service_cidr = string
    venues = map(object({
      cidr = string
      port = number
    }))
  }))

  default = {}

  validation {
    condition = alltrue([
      for cell in values(var.edge_cells) : alltrue([
        for venue in values(cell.venues) : venue.cidr != "0.0.0.0/0" && venue.cidr != "::/0"
      ])
    ])
    error_message = "A venue range of the whole internet is not a venue range. Name the ranges the venue publishes."
  }

  validation {
    condition     = length(distinct([for cell in values(var.edge_cells) : cell.subnet_cidr])) == length(var.edge_cells)
    error_message = "Two cells share a subnet range. Overlapping ranges route to whichever subnet was created first."
  }
}
