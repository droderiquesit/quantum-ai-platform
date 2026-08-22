variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "github_repository" {
  description = <<-EOT
    The repository permitted to impersonate the pipeline account, as
    `owner/name`.

    There is no default. A default here would be a repository somebody else
    could be running, and the failure mode of getting it wrong is that their
    pipeline can deploy to this project.
  EOT

  type = string

  validation {
    condition     = can(regex("^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$", var.github_repository))
    error_message = "The repository is owner/name, with no scheme and no trailing path."
  }
}
