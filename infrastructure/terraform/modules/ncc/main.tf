# Network Connectivity Center (R12 GCP-8056, GCP-10009): one hub, a spoke per
# VPC and a spoke per set of hybrid attachments.
#
# Off unless `enabled`, and off in every environment: a VPC spoke is billed for
# the data it moves and joins planes the trust-zone map keeps apart. Nothing
# here has been planned against a project.
resource "google_network_connectivity_hub" "this" {
  count = var.enabled ? 1 : 0

  project     = var.project_id
  name        = "qip-${var.environment}"
  description = "Hub joining the platform's VPC planes and hybrid attachments."
  labels      = var.labels
}

resource "google_network_connectivity_spoke" "vpc" {
  for_each = var.enabled ? var.vpc_spokes : {}

  project  = var.project_id
  name     = "qip-${var.environment}-${each.key}"
  location = "global"
  hub      = google_network_connectivity_hub.this[0].id
  labels   = var.labels

  linked_vpc_network {
    uri = each.value
  }
}

resource "google_network_connectivity_spoke" "interconnect" {
  for_each = var.enabled ? var.interconnect_spokes : {}

  project  = var.project_id
  name     = "qip-${var.environment}-${each.key}"
  location = each.value.location
  hub      = google_network_connectivity_hub.this[0].id
  labels   = var.labels

  linked_interconnect_attachments {
    uris                       = each.value.uris
    site_to_site_data_transfer = false
  }
}
