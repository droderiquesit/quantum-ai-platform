# Network Connectivity Center hubs (R12-GCP-8057, R12-GCP-8058).
#
# Two hubs because the two kinds of traffic want opposite things. Reflex and
# Fabric mirror traffic between regions is latency-bound and must be narrow:
# a mesh hub joining only the peer endpoints and broker ports, nothing else
# of each VPC. Service, data, AI and management traffic is not latency-bound
# and must be inspected: a star hub whose edge spokes reach one another only
# through the centre. Each hub is declared only when spokes are named for it;
# no environment names any yet.

resource "google_network_connectivity_hub" "latency" {
  count = length(var.latency_spokes) > 0 ? 1 : 0

  project         = var.project_id
  name            = "qip-${var.environment}-latency"
  description     = "Reflex/Fabric peer endpoints and broker ports only, mesh topology."
  preset_topology = "MESH"
  labels          = var.labels
}

resource "google_network_connectivity_spoke" "latency" {
  for_each = var.latency_spokes

  project  = var.project_id
  name     = "qip-${var.environment}-latency-${each.key}"
  location = "global"
  hub      = google_network_connectivity_hub.latency[0].id
  labels   = var.labels

  linked_vpc_network {
    uri                   = each.value.network_uri
    include_export_ranges = each.value.export
  }
}

resource "google_network_connectivity_hub" "services" {
  count = length(var.star_spokes) > 0 ? 1 : 0

  project         = var.project_id
  name            = "qip-${var.environment}-services"
  description     = "Service, data, AI and management networks, star topology with central inspection."
  preset_topology = "STAR"
  labels          = var.labels
}

resource "google_network_connectivity_spoke" "services" {
  for_each = var.star_spokes

  project  = var.project_id
  name     = "qip-${var.environment}-services-${each.key}"
  location = "global"
  hub      = google_network_connectivity_hub.services[0].id
  # The hub's path is known before it exists, so the group is named from it
  # rather than from the hub's computed id.
  group  = "projects/${var.project_id}/locations/global/hubs/qip-${var.environment}-services/groups/${each.value.group}"
  labels = var.labels

  linked_vpc_network {
    uri = each.value.network_uri
  }
}
