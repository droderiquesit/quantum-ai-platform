output "folder_id" {
  description = "The platform folder's numeric id, or null in no-org mode."
  value       = local.folder_id
}

output "seed_project_id" {
  description = "The seed project, which holds this root's state and nothing that runs."
  value       = google_project.seed.project_id
}

output "state_bucket" {
  description = "This root's state bucket, in the seed project."
  value       = local.state_bucket
}

# What `scripts/bootstrap-foundation.sh` records into
# `infrastructure/environments/<env>/terraform.tfvars` and the environment's
# GitOps manifests. The number is the one value the repository cannot know
# before the project exists, and the workflows cannot authenticate without it.
output "projects" {
  description = "Each enabled environment's project id and number."
  value = {
    for name, project in google_project.environment : name => {
      project_id     = project.project_id
      project_number = project.number
    }
  }
}

output "budgets" {
  description = "The monthly budget amount held on each project, keyed by environment (and `seed`)."
  value       = { for name, budget in local.budgeted : budget.project_id => budget.amount }
}

# Said rather than inferred: in no-org mode, or without the policy-admin role,
# these controls are simply absent, and a reader of the apply should see the
# list rather than assume the folder's guarantees hold.
output "controls_not_applied" {
  description = "Organisation-level controls this apply did not set, and why."
  value = concat(
    local.org_mode ? [] : ["platform folder: no organisation (no-org mode); projects have no parent"],
    local.policies_applied ? [] : [
      "iam.disableServiceAccountKeyCreation (the platform root still sets it per project)",
      "iam.disableServiceAccountKeyUpload (the platform root still sets it per project)",
      "storage.uniformBucketLevelAccess",
      "compute.skipDefaultNetworkCreation (projects are still created with auto_create_network = false)",
      "compute.vmExternalIpAccess",
    ],
    local.policies_applied && length(var.allowed_policy_member_customer_ids) > 0 ? [] : ["iam.allowedPolicyMemberDomains (optional; no customer ids given)"],
    local.firewall_applied ? [] : ["hierarchical firewall policy (the platform root's network firewall policy still denies ingress per network)"],
  )
}
