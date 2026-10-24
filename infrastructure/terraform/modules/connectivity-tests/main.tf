# Network Intelligence Center Connectivity Tests (R12-GCP-10060).
#
# A firewall rule or a route is a claim about which paths exist; a
# connectivity test is Google's own analysis of whether a packet would get
# through. The platform's intended paths (a workload reaching what it needs)
# and its forbidden ones (an execution node reaching the internet except
# through its venue path, anything reaching the ledger it should not) are
# declared together, and each carries its intent as a label. Nothing is
# declared unless `tests` names some; no environment does yet.
resource "google_network_management_connectivity_test" "this" {
  for_each = var.tests

  project  = var.project_id
  name     = "qip-${var.environment}-${each.key}"
  protocol = each.value.protocol

  source {
    instance   = each.value.source_instance
    ip_address = each.value.source_ip
  }

  destination {
    instance   = each.value.destination_instance
    ip_address = each.value.destination_ip
    port       = each.value.port
  }

  labels = merge(var.labels, { "expect" = each.value.expect })
}
