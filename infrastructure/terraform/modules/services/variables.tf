variable "project_id" {
  type = string
}

# --- What is switched on elsewhere ------------------------------------------
#
# These mirror the root's flags rather than reading them, because a module that
# reached into the root's variables would be a module that could not be
# instantiated twice. Each one enables exactly the API the resource that flag
# creates needs, and nothing else.
#
# All default false, matching the flags they mirror: an API enabled for a
# service that has no instance is a quota surface, an audit-log stream and a
# line in a security review, bought for nothing.

variable "enable_bigquery" {
  description = "Mirrors the root's enable_bigquery. Enables bigquery.googleapis.com."
  type        = bool
  default     = false
}

variable "enable_alloydb" {
  description = "Mirrors the root's enable_alloydb. Enables alloydb.googleapis.com and the Service Networking peering it needs."
  type        = bool
  default     = false
}

variable "enable_bigtable" {
  description = "Mirrors the root's enable_bigtable. Enables bigtableadmin.googleapis.com."
  type        = bool
  default     = false
}

variable "enable_memorystore" {
  description = "Mirrors the root's enable_memorystore. Enables redis.googleapis.com and the Service Networking peering it needs."
  type        = bool
  default     = false
}

variable "enable_spanner" {
  description = "Mirrors the root's enable_spanner. Enables spanner.googleapis.com."
  type        = bool
  default     = false
}

variable "enable_vertex_ai" {
  description = "Mirrors the root's enable_vertex_ai. Enables aiplatform.googleapis.com."
  type        = bool
  default     = false
}

variable "enable_security_command_center" {
  description = "Mirrors the root's enable_security_command_center. Enables securitycenter.googleapis.com."
  type        = bool
  default     = false
}

variable "enable_gitops" {
  description = "Mirrors the root's gitops_enabled. Enables container.googleapis.com, gkehub.googleapis.com and connectgateway.googleapis.com, which the control-plane cluster and the bootstrap that reaches it need."
  type        = bool
  default     = false
}

variable "disable_services_on_destroy" {
  description = <<-EOT
    Whether `terraform destroy` turns these APIs back off.

    False, in every environment, and this is the one setting in this module
    worth arguing about.

    Disabling a Google API is not a permissions change. Disabling
    `compute.googleapis.com` deletes every Compute resource in the project —
    instances, disks, networks, firewall rules — whether or not this
    configuration created them. In a project that holds anything else, a
    destroy aimed at this platform takes those with it, and the plan gives no
    hint: it shows one API being disabled, not the resources that vanish with
    it.

    Leaving an API enabled after a destroy costs nothing. Google does not bill
    for an enabled API with no resources under it, and the next apply adopts
    it. The asymmetry is total, which is why the default is not a judgement
    call.

    Set it true only for a project that exists for one change and is deleted
    whole afterwards — where the destroy is the project going away and there is
    nothing else in it to damage.
  EOT
  type        = bool
  default     = false
}
