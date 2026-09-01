output "service_account_email" {
  description = "The node's identity. Federated through the metadata server; no key exists for it and none may be created."
  value       = google_service_account.node.email
}

output "subnet_id" {
  description = "The node's subnet."
  value       = google_compute_subnetwork.node.id
}

output "node_tag" {
  description = <<-EOT
    The network tag every rule in this module targets and the instance template
    applies.

    Exported so nothing else has to guess it. A firewall rule targeting a tag
    nothing carries is a rule that does nothing, and it does nothing silently.
  EOT

  value = local.node_tag
}

output "instance_group" {
  description = "The managed instance group, for a load balancer or a deployment that needs to name it."
  value       = google_compute_instance_group_manager.node.instance_group
}

output "isolated_cpus" {
  description = <<-EOT
    The `isolcpus` range this node's image must carry, derived from the machine
    shape.

    §41.3 assigns threads to cores 2–15, which needs a sixteen-vCPU machine. On
    an eight-vCPU shape this reads `2-7` and the assignment does not fit: the
    quote engine, the risk gate and the leg coordinator end up sharing cores
    that §41.3 gives each its own. That is a legitimate configuration for a node
    with few venues and it is not the blueprint's core assignment, so the value
    is exported rather than buried — a deployment should be able to see which of
    the two it got.
  EOT

  value = local.isolated_cpus
}

output "shadow_mode" {
  description = <<-EOT
    Whether this node is in shadow mode.

    True means no venue egress rule and no venue-credential binding exist, so
    the node cannot open a venue session. ADR 0020 step 3 closes on "a node
    holding sessions, quoting nothing, matching the pod's decisions", and this
    output is what a report of that state can cite rather than assert.
  EOT

  value = var.shadow_mode
}

output "venue_credential_bound" {
  description = <<-EOT
    Whether the venue credential is readable by this node's identity.

    False unless the environment's ceiling permits live trading *and* the node
    is out of shadow mode *and* a secret was named. Exported so that "the node
    cannot authenticate to a venue" is a value somebody can read in an output
    rather than a claim about a conditional they would have to go and check.
  EOT

  value = local.venue_credential_bound
}
