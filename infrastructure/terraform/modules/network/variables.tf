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

# --- The console's route to the platform (ADR 0018) --------------------------

variable "console_egress_cidr" {
  type = string
  # Null in an environment whose console does not reach the platform. Not a
  # default range: a subnet created because a variable had a default is a
  # subnet nobody decided to create.
  default     = null
  description = "CIDR for the console's Cloud Run direct-VPC-egress subnet, or null for none."

  validation {
    # A /26 is the smallest Google accepts for direct VPC egress. Refusing a
    # smaller one here rather than at apply time keeps the failure attached to
    # the value that caused it.
    condition     = var.console_egress_cidr == null || can(cidrnetmask(var.console_egress_cidr))
    error_message = "console_egress_cidr must be a CIDR block, for example 10.0.16.0/26."
  }
}
