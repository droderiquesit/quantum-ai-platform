# The Cloud Build private pool images are built in (R12 CICD-9097).
#
# Off by default and in every environment. Turning it on is the owner's act:
# set `cloud_build_private_pool_enabled = true` in the build environment's
# tfvars, read the plan, apply, and from the next `deploy.yml` run on, the
# `images` job submits every build — and its reproducibility rebuild — to the
# pool instead of running `docker buildx` on the GitHub runner. The workflow
# reads the same line from the same committed file, so the two cannot
# disagree about where builds run; there is no repository variable.
#
# Only the build environment may hold one. `deploy.yml` builds in exactly one
# environment (`BUILD_ENVIRONMENT: dev`) and every later environment is
# promoted that digest and never builds, so a pool anywhere else would be a
# worker billing for builds that never come. The literal duplicates the
# workflow's; change both together.
variable "cloud_build_private_pool_enabled" {
  description = "Whether the build environment's images are built in a Cloud Build private pool peered into the Engineering VPC. False everywhere until the owner turns it on."
  type        = bool
  default     = false
  nullable    = false

  validation {
    condition     = !var.cloud_build_private_pool_enabled || var.environment == "dev"
    error_message = "Only the build environment (dev — deploy.yml's BUILD_ENVIRONMENT) builds images; a private pool anywhere else would never receive a build."
  }
}

module "build_pool" {
  source = "./modules/build-pool"
  count  = var.cloud_build_private_pool_enabled ? 1 : 0

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  # The Engineering/build plane network (GCP-051), which hosts nothing else.
  # Built from the network's *name* rather than its `id` because the name is
  # known at plan time and the id is not, so the module's own validation and
  # the harness can both read the value the pool will be peered into before
  # anything exists. It still depends on `module.network`.
  peered_network_id = "projects/${var.project_id}/global/networks/${module.network.engineering_vpc_name}"

  ci_service_account = module.cicd.service_account_email
}

output "build_pool" {
  description = "The private build pool, or null where none is enabled. deploy.yml derives the same names from the tfvars; this is for a person reading the apply."
  value = var.cloud_build_private_pool_enabled ? {
    worker_pool_id        = module.build_pool[0].worker_pool_id
    build_service_account = module.build_pool[0].build_service_account
    staging_bucket        = module.build_pool[0].staging_bucket
  } : null
}
