# The resource hierarchy as code (R12-GCP-10001, 8043-8050, 8052).
#
# One folder per environment under a platform folder, and inside each the
# same planes as projects, so a failure or a policy change in one plane's
# project cannot reconfigure another's:
#
#   net (network-core)      NCC hubs, shared DNS, hybrid connectivity,
#                           firewall policy, PSC endpoints     (8043)
#   exec-<region>           the Reflex VPC and Reflex VMs only (8044)
#   fabric-<region>         Fabric VPC, brokers, journal disks,
#                           archiver identities                (8045)
#   svc-<region>            regional services, cache, sinks    (8046)
#   ledger                  the ledger databases and their keys (8047)
#   world                   world-model stores                 (8048)
#   data                    archive, packs, Bigtable, BigQuery,
#                           catalogue                          (8049)
#   ai                      training, registry, agent runtime  (8050)
#
# Declaring the projects is the first half. Moving each resource into its
# plane's project is the second, and is not done here: today every
# environment is one project holding every plane.
#
# The hierarchy is never a runtime dependency: no workload is granted
# anything on a folder, and nothing here outputs a value a binary reads.

locals {
  # Short plane tokens, because a project id is at most 30 characters and
  # the region name is most of it.
  regional_planes = ["exec", "fabric", "svc"]
  global_planes   = ["net", "ledger", "world", "data", "ai"]

  # environment/plane[-region] => the project's id, name and folder.
  projects = merge(
    { for pair in setproduct(keys(var.environments), local.global_planes) :
      "${pair[0]}/${pair[1]}" => {
        environment = pair[0]
        plane       = pair[1]
        region      = null
        project_id  = "${var.prefix}-${pair[0]}-${pair[1]}"
      }
    },
    merge([for environment, regions in var.environments : {
      for pair in setproduct(local.regional_planes, regions) :
      "${environment}/${pair[0]}-${pair[1]}" => {
        environment = environment
        plane       = pair[0]
        region      = pair[1]
        project_id  = "${var.prefix}-${environment}-${pair[0]}-${pair[1]}"
      }
    }]...)
  )
}

resource "google_folder" "platform" {
  display_name = "qip-${var.prefix}"
  parent       = var.parent
}

resource "google_folder" "environment" {
  for_each = var.environments

  display_name = each.key
  parent       = google_folder.platform.name
}

resource "google_project" "plane" {
  for_each = local.projects

  project_id      = each.value.project_id
  name            = each.value.project_id
  folder_id       = google_folder.environment[each.value.environment].name
  billing_account = var.billing_account
  labels = merge(var.labels, {
    environment = each.value.environment
    plane       = each.value.plane
  })

  # A project is never created with the default network: every VPC here is
  # declared, and a default network opens SSH and RDP to the internet.
  auto_create_network = false
  deletion_policy     = "PREVENT"
}

# Refused at plan, not warned: two planes sharing an id would be one project
# holding both — the blast radius this hierarchy exists to split. (The
# 30-character limit is refused on var.environments, before the provider's
# own check can fail half a plan.)
resource "terraform_data" "project_ids" {
  input = [for p in values(local.projects) : p.project_id]

  lifecycle {
    precondition {
      condition     = length(distinct([for p in values(local.projects) : p.project_id])) == length(local.projects)
      error_message = "Two planes share a project id."
    }
  }
}
