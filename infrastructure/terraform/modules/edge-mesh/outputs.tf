output "psc_endpoint_ids" {
  description = "The Private Service Connect endpoint global addresses, keyed by region."
  value = {
    for region, endpoint in google_compute_global_address.cell_psc_endpoint :
    region => endpoint.id
  }
}

output "psc_endpoint_addresses" {
  description = "The private internal addresses of the PSC endpoints, keyed by region."
  value = {
    for region, endpoint in google_compute_global_address.cell_psc_endpoint :
    region => endpoint.address
  }
}

output "cell_ingress_firewall_ids" {
  description = "The firewall rules that permit ingress to cells from the central plane and other cells."
  value = {
    for node_id, rule in google_compute_firewall.cell_ingress_health :
    node_id => rule.id
  }
}

output "mesh_connectivity" {
  description = "Summary of mesh connectivity for reference."
  value = {
    deployed_regions              = distinct([for config in var.execution_nodes : config.region])
    has_cross_region_mirrors      = local.has_cross_region_mirror
    central_plane_reachable_count = length([for node_config in var.execution_nodes : node_config if !node_config.shadow_mode])
    psc_endpoints_created         = length(google_compute_global_address.cell_psc_endpoint)
  }
}
