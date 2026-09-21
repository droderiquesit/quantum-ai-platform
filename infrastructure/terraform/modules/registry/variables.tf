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

variable "ci_service_account" {
  description = "The pipeline's service account. Pushes; cannot delete."
  type        = string
}

variable "pull_service_accounts" {
  description = <<-EOT
    The accounts permitted to pull, keyed by the workload they belong to.
    The node service account belongs here, because the kubelet pulls as the
    node rather than as the pod.

    Empty by default: a registry nothing can read is useless and obvious, and a
    registry everything can read is useful and invisible.

    A map and not a list, and the keys are the point. These emails are
    produced by another module, so a command with no plan behind it —
    `terraform import`, which the reclaim step in `infra.yml` runs against an
    environment being rebuilt from nothing — cannot know them, and a
    `for_each` over a set of unknown strings is a refusal rather than an
    empty grant. Keys fixed here, values arriving at apply, is Terraform's
    own remedy and the shape the evidence module now uses for the same
    reason.
  EOT

  type    = map(string)
  default = {}
}

# The project number, for the Cloud Run service agent's own name.
#
# Not the project id: a service agent is named by number, and the two are
# different strings for the same project. Passed in rather than looked up
# here so this module makes no API call of its own.
variable "project_number" {
  description = "The numeric project id, used to name the Cloud Run service agent that pulls images."
  type        = number
}
