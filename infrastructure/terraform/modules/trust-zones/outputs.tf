output "zone_service_accounts" {
  description = "Each zone's workload identity, keyed by zone. One per zone and never one shared by two."
  value       = { for name, account in google_service_account.zone : name => account.email }
}

output "zone_kubernetes_service_accounts" {
  description = "The Kubernetes service account each zone's workload identity binding names, keyed by zone."
  value       = { for name, zone in var.zones : name => zone.kubernetes_service_account }
}

output "zone_subnets" {
  description = "Each zone's subnet, keyed by zone."
  value       = { for name, subnet in google_compute_subnetwork.zone : name => subnet.id }
}

output "zone_network_tags" {
  description = <<-EOT
    The network tag every rule in this module targets, keyed by zone.

    Whoever creates a zone's node pool applies its tag. A firewall rule
    targeting a tag nothing carries is a rule that does nothing, and it does
    nothing silently — so this output is the contract between the boundary
    declared here and the workloads it is supposed to bound.
  EOT

  value = local.zone_tag
}

output "permitted_paths" {
  description = <<-EOT
    The zone-to-zone paths that exist, as `from->to (mode)`, sorted.

    For the Kubernetes network policy layer to mirror. Two controls rather than
    one: the firewall bounds the subnet, the network policy bounds the pod, and
    a policy written from this list cannot drift from the rules that were
    applied without the drift being visible in a diff.
  EOT

  value = sort([
    for path in values(var.permitted_paths) : "${path.from}->${path.to} (${path.mode})"
  ])
}

output "external_egress_destinations" {
  description = <<-EOT
    Every destination outside the VPC any zone may reach, as
    `zone purpose cidr:port`, sorted.

    The whole external surface of the deployment in one list, which is the
    form an auditor asks for and the form a review can actually be done on.
  EOT

  value = sort([
    for entry in values(var.external_egress) :
    "${entry.zone} ${entry.purpose} ${entry.cidr}:${entry.port}"
  ])
}

output "zones_with_external_egress" {
  description = <<-EOT
    The zones that hold any route out of the VPC at all, sorted.

    Expected to be a short list and to stay one. A zone appearing here that
    was not expected to is the finding; the ports it reaches are detail.
  EOT

  value = sort(local.egress_zones)
}

output "nat_name" {
  description = "The Cloud NAT translating for the zones above, or an empty string when no zone declared external egress and none was created."
  value       = length(google_compute_router_nat.egress) > 0 ? google_compute_router_nat.egress[0].name : ""
}
