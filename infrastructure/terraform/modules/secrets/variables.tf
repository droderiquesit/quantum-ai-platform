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

variable "service_accounts" {
  description = "Deployable name to service-account name."
  type        = map(string)
}

variable "secret_names" {
  description = "The secrets that exist. Values are written out of band."
  type        = list(string)
}

variable "venue_credential_readable" {
  description = <<-EOT
    Whether the venue credential may be read in this environment.

    False in any environment whose autonomy ceiling is paper trading. A
    credential that cannot be read cannot be misused by a misconfigured
    application, which is a stronger guarantee than one the application makes
    about itself.
  EOT
  type    = bool
  default = false
}
