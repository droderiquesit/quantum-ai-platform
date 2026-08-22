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
