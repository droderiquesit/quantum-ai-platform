# R12-GOV-10004 protected part (owner applies)

Protected path: infrastructure/terraform/variables.tf. Add:

    variable "allowed_resource_locations" {
      type        = list(string)
      default     = ["in:us-locations", "in:eu-locations"]
      description = "Organization Policy gcp.resourceLocations allowed values at project scope (owner decision 36): US and EU only."
      validation {
        condition     = length(var.allowed_resource_locations) > 0 && alltrue([for v in var.allowed_resource_locations : can(regex("^in:[a-z0-9-]+-locations$", v))])
        error_message = "Name at least one value group such as `in:us-locations`."
      }
    }

Then in infrastructure/terraform/main.tf beside require_shielded_vm:

    resource "google_org_policy_policy" "resource_locations" {
      count  = var.project_key_policies_enabled ? 1 : 0
      parent = "projects/${var.project_id}"
      name   = "projects/${var.project_id}/policies/gcp.resourceLocations"
      spec {
        rules {
          values {
            allowed_values = var.allowed_resource_locations
          }
        }
      }
      depends_on = [module.services]
    }

Extend the acceptance test with a project-scope check of this body once applied.
Folder-scope policies are built in infrastructure/foundation (owner applies that root).
