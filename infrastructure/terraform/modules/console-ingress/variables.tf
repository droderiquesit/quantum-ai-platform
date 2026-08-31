variable "enabled" {
  description = "Publish the delivery consoles. Off everywhere it is not deliberately turned on: this is the one route into a cluster built to have none."
  type        = bool
  default     = false
}

variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "operators" {
  description = <<-EOT
    The identities that may pass Identity-Aware Proxy, as full IAM members
    (`user:someone@example.com`, `group:desk@example.com`).

    This is the access decision itself, not a convenience: an identity absent
    from this list is refused at Google's edge and never reaches Argo CD's own
    login. A group is the better shape once there is more than one operator —
    membership then changes without a Terraform apply, which is the point of
    a group.
  EOT
  type        = list(string)
  default     = []

  validation {
    condition = alltrue([
      for member in var.operators :
      can(regex("^(user|group|serviceAccount|domain):", member))
    ])
    error_message = "Each operator is a full IAM member: user:…, group:…, serviceAccount:… or domain:…."
  }

  validation {
    condition     = !contains(var.operators, "allAuthenticatedUsers") && !contains(var.operators, "allUsers")
    error_message = "allUsers and allAuthenticatedUsers admit the entire internet or every Google account; name the desk instead."
  }
}
