# The network.
#
# One global VPC, and almost nothing else here — which is the blueprint's §45
# shape and is worth stating because the gap used to read larger than it was.
# §45 asks for one global VPC, regional subnets, no inter-region peering and no
# overlay. A Google Cloud VPC is a global resource and subnets in different
# regions share it natively, so there has never been a peering layer to
# remove. `routing_mode = "REGIONAL"` is not a second VPC and does not
# partition it; it scopes which Cloud Router BGP routes propagate between
# regions, which matters only to the interconnect in `modules/connectivity`.
#
# What this module is *not*, deliberately:
#
#   * It is not where workloads live. Every Cloud Run workload attaches to
#     its trust zone's subnet and every execution node to its own; both are
#     created by the module that owns the zone or the node, so a range and
#     the rules that bound it are one declaration. A general-purpose subnet
#     here would be a range a workload could land in with no zone and no
#     rule, which is the state the zone model exists to make impossible.
#   * It has no NAT. A Cloud NAT is regional and `ALL_SUBNETWORKS_ALL_IP_RANGES`
#     gives a way out to whatever subnet is added next; `modules/trust-zones`
#     creates one that lists exactly the zones that declared external
#     egress, and `modules/execution-node` creates one per node that needs
#     it. Two NATs covering one subnet in one region is a conflict the API
#     refuses, so this module must not create a third.
#   * It does not create a Private Service Connect endpoint for Google APIs.
#     `modules/connectivity` already has one, gated off.
#
# What it does hold: the VPC, the deny-all ingress that makes every allow
# rule elsewhere mean something, the console's subnet (ADR 0018), and the
# private DNS zone that sends `*.googleapis.com` to the restricted VIP — the
# one piece every subnet with private Google access depends on and none of
# them can own.

resource "google_compute_network" "vpc" {
  project = var.project_id
  name    = "qip-${var.environment}"

  # Subnets are declared, not created automatically. An automatic subnet in
  # every region is a lot of network nobody asked for.
  auto_create_subnetworks = false

  # Regional routing keeps a failure in one region from affecting another's
  # routing table.
  routing_mode = "REGIONAL"
}

# Deny everything inbound that is not explicitly permitted. Declared even
# though it is the default, because a reviewer should be able to see the
# posture rather than infer it.
resource "google_compute_firewall" "deny_ingress" {
  project = var.project_id
  name    = "qip-${var.environment}-deny-ingress"
  network = google_compute_network.vpc.id

  direction = "INGRESS"
  priority  = 65534

  deny {
    protocol = "all"
  }

  source_ranges = ["0.0.0.0/0"]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# --- Google APIs without an external address ---------------------------------
#
# Every subnet on this platform has private Google access and no external
# address, and every egress firewall names `199.36.153.8/30` as the one range
# a workload may reach Google APIs on. That range answers only if the
# workload resolves `storage.googleapis.com` *to* it — otherwise the name
# resolves to a public address, the egress deny drops the packet, and the
# failure reads as the vendor being down. This zone is the resolver's half
# of that arrangement: `*.googleapis.com` is a CNAME to
# `restricted.googleapis.com`, and that name is the four restricted-VIP
# addresses. Cloud Run's direct VPC egress and Compute Engine instances both
# resolve through the VPC, so one zone serves every tier.
#
# `restricted` rather than `private`: the restricted VIP carries only the
# APIs a VPC Service Controls perimeter can protect, which is the set this
# platform uses, and it refuses the ones a perimeter cannot — so a workload
# that reaches for an API outside that set fails to resolve it rather than
# quietly reaching it.
resource "google_dns_managed_zone" "googleapis" {
  project     = var.project_id
  name        = "qip-${var.environment}-googleapis"
  dns_name    = "googleapis.com."
  description = "Sends every Google API to the restricted VIP, so no subnet needs a route to the internet to reach one."
  visibility  = "private"

  private_visibility_config {
    networks {
      network_url = google_compute_network.vpc.id
    }
  }

  labels = var.labels
}

resource "google_dns_record_set" "restricted_vip" {
  project      = var.project_id
  managed_zone = google_dns_managed_zone.googleapis.name
  name         = "restricted.googleapis.com."
  type         = "A"
  ttl          = 300
  rrdatas      = ["199.36.153.8", "199.36.153.9", "199.36.153.10", "199.36.153.11"]
}

resource "google_dns_record_set" "googleapis_wildcard" {
  project      = var.project_id
  managed_zone = google_dns_managed_zone.googleapis.name
  name         = "*.googleapis.com."
  type         = "CNAME"
  ttl          = 300
  rrdatas      = ["restricted.googleapis.com."]
}

# --- The console's route to the platform (ADR 0018) --------------------------
#
# The portal runs on Cloud Run outside the catalogue — `scripts/deploy-frontends.sh`
# deploys it — and reaches `qip-api` at the API's own Cloud Run URL, as an
# invoker the catalogue names. This subnet is the interface the portal's
# direct VPC egress attaches to; its own range rather than a share of a
# zone's, because the console is not a trust zone and a range it drew from
# would be a zone with a second tenant.
resource "google_compute_subnetwork" "console_egress" {
  count   = var.console_egress_cidr == null ? 0 : 1
  project = var.project_id
  name    = "qip-${var.environment}-console-egress"
  region  = var.region
  network = google_compute_network.vpc.id

  ip_cidr_range = var.console_egress_cidr

  # The portal reads Secret Manager and Identity Platform. Private Google
  # access is how it does that without the egress leaving the VPC.
  private_ip_google_access = true

  log_config {
    aggregation_interval = "INTERVAL_5_SEC"
    flow_sampling        = 0.5
    metadata             = "INCLUDE_ALL_METADATA"
  }
}
