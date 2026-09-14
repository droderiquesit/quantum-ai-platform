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
    condition     = var.console_egress_cidr == null || can(cidrnetmask(var.console_egress_cidr))
    error_message = "console_egress_cidr must be a CIDR block, for example 10.0.16.0/26."
  }

  validation {
    # A /26 is the smallest Google accepts for direct VPC egress. Refusing a
    # smaller one here rather than at apply time keeps the failure attached to
    # the value that caused it. Split into its own validation because the
    # prefix cannot be read out of a value the syntax check above has not
    # already proven well-formed — `split` on a malformed string would be the
    # error the reader sees instead of the one they caused.
    #
    # The `try` is the whole reason this line is worth reading twice. Terraform
    # evaluates *both* operands of `||`, so the guard on the left does not stop
    # `split` being handed the null on the right, and `split` refuses a null
    # argument with an error the validation cannot catch: the plan dies on
    # "Invalid value for \"str\" parameter: argument must not be null" before
    # any error_message here is reached. This variable is null by default and
    # null in three of the four environments, so the shape below made
    # `terraform plan` impossible in test, stage and prod — a gate that refused
    # every good value and could never have reported the bad one it was written
    # for. It went unseen because the module had never been planned; the first
    # plan ever run against it found this in its first second. `try` returning
    # false for a value the syntax check above has already refused keeps the
    # malformed case failing on the message it earned.
    condition     = var.console_egress_cidr == null || try(tonumber(split("/", var.console_egress_cidr)[1]) <= 26, false)
    error_message = "console_egress_cidr must be a /26 or larger (a prefix of /26 or lower). Google refuses direct VPC egress on anything smaller, and reserves addresses in it as instances scale."
  }
}
