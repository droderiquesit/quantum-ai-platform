output "central_plane_ranges" {
  description = "The CIDR ranges the central plane reaches a cell's health port from: every trust-zone subnet, then the Google APIs range."
  value       = local.central_plane_ranges
}

output "psc_addresses" {
  description = "One PSC endpoint address per region a cell is deployed in, keyed by region."
  value       = local.psc_addresses
}

output "deployment_summary" {
  description = "Summary of the validated edge cell topology."
  value = {
    total_nodes = length(var.execution_nodes)
    # Shadow mode is about venue paths, not about live trading: a cell out of
    # shadow mode reaches its configured (simulated) venues and is exactly as
    # paper-only as one inside it (ADR 0003).
    nodes_in_shadow_mode            = length([for config in values(var.execution_nodes) : config if config.shadow_mode])
    nodes_with_venue_paths          = length([for config in values(var.execution_nodes) : config if !config.shadow_mode])
    regions_deployed                = local.regions
    cross_region_mirrors_configured = length(var.cross_region_mirrors)
    total_capital_allocation        = sum(concat([0], [for config in values(var.execution_nodes) : tonumber(config.region_allocation)]))
  }
}
