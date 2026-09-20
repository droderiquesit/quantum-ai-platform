variable "project_id" {
  type        = string
  description = "The environment's own project. Never one another environment uses."
}

variable "project_number" {
  type        = string
  description = "The project's numeric id. The IAP brand is addressed by number, not by id, and a brand created under the wrong number belongs to somebody else's project."
}

variable "environment" {
  type        = string
  description = "dev, test, stage or prod. Names the address, the certificate and the OAuth client."
}

variable "argocd_hostname" {
  type        = string
  description = "The public name Argo CD answers on, e.g. argocd.algorik.ai. Its A record is created at the registrar by hand and must resolve to this module's address before the managed certificate can provision."

  validation {
    # A hostname reaches a certificate's SAN list and an OAuth redirect URI.
    # Both are places where an unvalidated value is somebody else's name.
    condition     = can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$", var.argocd_hostname))
    error_message = "argocd_hostname must be a dotted lowercase DNS name, with no scheme, port or path."
  }
}

variable "kargo_hostname" {
  type        = string
  description = "The public name Kargo answers on, e.g. kargo.algorik.ai. Same contract as argocd_hostname."

  validation {
    condition     = can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$", var.kargo_hostname))
    error_message = "kargo_hostname must be a dotted lowercase DNS name, with no scheme, port or path."
  }
}

variable "iap_members" {
  type        = list(string)
  description = <<-EOT
    Exactly who may pass Identity-Aware Proxy and reach the GitOps control
    plane, as IAM members (`user:someone@example.com`, `group:...`).

    This is the entire access list for a controller that can reconcile
    arbitrary manifests into the cluster, so it is enumerated rather than
    defaulted. An empty list is a valid and deliberate state: the front door
    exists and admits nobody, which is the correct posture for an environment
    whose operators have not been named yet.

    `allUsers` and `allAuthenticatedUsers` are refused below. Neither is ever
    the right answer here: the first is the public internet and the second is
    every Google account in existence.
  EOT
  default     = []

  validation {
    condition = alltrue([
      for member in var.iap_members :
      !contains(["allUsers", "allAuthenticatedUsers"], member)
    ])
    error_message = "iap_members may not contain allUsers or allAuthenticatedUsers; that admits the internet, or every Google account, to a controller that can reconcile arbitrary manifests into the cluster."
  }
}
