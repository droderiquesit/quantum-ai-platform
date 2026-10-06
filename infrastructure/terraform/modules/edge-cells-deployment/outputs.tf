output "nodes" {
  description = "Outputs from each execution node module."
  value = {
    for node_id, node_module in module.execution_node :
    node_id => {
      instance_group_name   = node_module.instance_group_name
      instance_group_id     = node_module.instance_group_id
      service_account_email = node_module.service_account_email
      subnetwork_id         = node_module.subnetwork_id
      health_check_id       = node_module.health_check_id
      node_tag              = node_module.node_tag
    }
  }
}

output "mesh_connectivity" {
  description = "Edge mesh connectivity summary."
  value = {
    psc_endpoints        = module.edge_mesh.psc_endpoint_addresses
    regions              = distinct([for config in var.execution_nodes : config.region])
    cross_region_mirrors = length(var.cross_region_mirrors)
  }
}

output "deployment_summary" {
  description = "Summary of the deployed edge cell topology."
  value = {
    total_nodes                     = length(var.execution_nodes)
    nodes_in_shadow_mode            = length([for config in var.execution_nodes : config if config.shadow_mode])
    nodes_in_live_mode              = length([for config in var.execution_nodes : config if !config.shadow_mode])
    regions_deployed                = distinct([for config in var.execution_nodes : config.region])
    cross_region_mirrors_configured = length(var.cross_region_mirrors)
    total_capital_allocation        = sum([for config in var.execution_nodes : tonumber(config.region_allocation)])
  }
}
