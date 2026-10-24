output "hub_id" {
  description = "The hub's resource ID, or null while the module is off."
  value       = one(google_network_connectivity_hub.this[*].id)
}

output "spokes" {
  description = "Spoke name to its kind; empty while the module is off."
  value = merge(
    { for name, _ in google_network_connectivity_spoke.vpc : name => "vpc" },
    { for name, _ in google_network_connectivity_spoke.interconnect : name => "interconnect" },
  )
}
