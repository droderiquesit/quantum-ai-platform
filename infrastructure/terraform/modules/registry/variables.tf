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
    The accounts permitted to pull. The node service account belongs here,
    because the kubelet pulls as the node rather than as the pod.

    Empty by default: a registry nothing can read is useless and obvious, and a
    registry everything can read is useful and invisible.
  EOT

  type    = list(string)
  default = []
}
