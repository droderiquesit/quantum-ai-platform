# The network.
#
# One VPC, one subnet with secondary ranges for pods and services, and no
# route to the internet except through a NAT that logs. Nodes have no external
# addresses at all: a node that cannot be reached from the internet cannot be
# reached from the internet, which is a stronger statement than any firewall
# rule.
#
# # Against the blueprint's target (ADR 0022, §45)
#
# The VPC is the part the blueprint and the running platform already agree
# about, and the agreement is worth writing down because the gap reads larger
# than it is. §45 asks for one global VPC, regional subnets, no inter-region
# peering and no overlay. A Google Cloud VPC is a global resource and subnets
# in different regions share it natively, so there is no peering layer here to
# remove — there has never been one. `routing_mode = "REGIONAL"` above is not a
# second VPC and does not partition it; it scopes which Cloud Router BGP routes
# propagate between regions, which matters only to the interconnect in
# `modules/connectivity`.
#
# What this module is *not*, deliberately:
#
#   * It is not where the trust zones live. §45's thirteen zones, their
#     default-deny egress and the rule that only Optimisation may reach IBM are
#     `modules/trust-zones`, which owns a zone's subnet, tag and rules together.
#     A second zone implementation here would be a second set of subnets in the
#     same VPC and a second answer to the same question, and the wiring pass
#     would have to pick one.
#   * It does not create a Private Service Connect endpoint for Google APIs.
#     `modules/connectivity` already has one, gated off. Two endpoints
#     answering for the same APIs is two things to keep a resolver pointed at.
#
# Two things here are still the transitional shape rather than the target, and
# both are cutover work rather than an omission: the primary subnet carries
# secondary ranges for pods and services, which exist for a scheduler the
# target does not have, and the NAT above is
# `ALL_SUBNETWORKS_ALL_IP_RANGES`, which gives a way out to whatever subnet is
# added next. Narrowing either one changes the path the GKE cluster of ADR 0011
# is serving traffic on today, and ADR 0020 fixes the order: not before the
# evidence in its step 5.

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

resource "google_compute_subnetwork" "primary" {
  project = var.project_id
  name    = "qip-${var.environment}-primary"
  region  = var.region
  network = google_compute_network.vpc.id

  ip_cidr_range = var.subnet_cidr

  # Private Google access lets nodes reach Google APIs without a public
  # address, which is what makes the no-external-address rule survivable.
  private_ip_google_access = true

  secondary_ip_range {
    range_name    = "pods"
    ip_cidr_range = var.pod_cidr
  }

  secondary_ip_range {
    range_name    = "services"
    ip_cidr_range = var.service_cidr
  }

  # Flow logs at a sampled rate. Full sampling is expensive and half a percent
  # is enough to answer "what talked to what" after an incident.
  log_config {
    aggregation_interval = "INTERVAL_5_SEC"
    flow_sampling        = 0.5
    metadata             = "INCLUDE_ALL_METADATA"
  }
}

# Egress through a NAT with logging, so a component reaching somewhere
# unexpected is visible rather than silent.
resource "google_compute_router" "router" {
  project = var.project_id
  name    = "qip-${var.environment}-router"
  region  = var.region
  network = google_compute_network.vpc.id
}

resource "google_compute_router_nat" "nat" {
  project = var.project_id
  name    = "qip-${var.environment}-nat"
  router  = google_compute_router.router.name
  region  = var.region

  nat_ip_allocate_option             = "AUTO_ONLY"
  source_subnetwork_ip_ranges_to_nat = "ALL_SUBNETWORKS_ALL_IP_RANGES"

  log_config {
    enable = true
    filter = "ALL"
  }
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

# Health checks from Google's own ranges, which load balancing needs.
resource "google_compute_firewall" "allow_health_checks" {
  project = var.project_id
  name    = "qip-${var.environment}-allow-health-checks"
  network = google_compute_network.vpc.id

  direction = "INGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = ["8080"]
  }

  # The documented ranges Google health checks originate from.
  source_ranges = ["35.191.0.0/16", "130.211.0.0/22"]
  target_tags   = ["qip-node"]
}

# Traffic between nodes in the cluster.
resource "google_compute_firewall" "allow_internal" {
  project = var.project_id
  name    = "qip-${var.environment}-allow-internal"
  network = google_compute_network.vpc.id

  direction = "INGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
  }
  allow {
    protocol = "udp"
  }
  allow {
    protocol = "icmp"
  }

  source_ranges = [var.subnet_cidr, var.pod_cidr]
  target_tags   = ["qip-node"]
}

# --- The console's route to the platform (ADR 0018) --------------------------
#
# The portal runs on Cloud Run and the platform runs in this VPC behind private
# nodes. These three resources are the whole route, and each is scoped to the
# one thing that needs it.

# The subnet Cloud Run puts the portal's network interface in.
#
# Its own subnet rather than a share of the primary: the primary is where GKE
# allocates node addresses, and a pool that grows into a range Cloud Run is
# also drawing from produces an allocation failure in whichever of the two asks
# second. Separate ranges make that impossible rather than unlikely.
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

# The address qip-api's internal load balancer answers on.
#
# Reserved here so that the value the console is configured with, the value the
# Helm chart gives the Service, and the value that actually exists are the same
# value. An acceptance test asserts the first two agree; this resource is what
# makes the third one true.
resource "google_compute_address" "api_internal" {
  count        = var.api_internal_address == null ? 0 : 1
  project      = var.project_id
  name         = "qip-${var.environment}-api-internal"
  region       = var.region
  subnetwork   = google_compute_subnetwork.primary.id
  address_type = "INTERNAL"
  address      = var.api_internal_address

  # `SHARED_LOADBALANCER_VIP` rather than `GCE_ENDPOINT`: the forwarding rule
  # is created by GKE from the Service, not by Terraform, and reserving the
  # address for an endpoint Terraform does not own leaves the two arguing over
  # who allocated it.
  purpose = "SHARED_LOADBALANCER_VIP"

  labels = var.labels
}

# The console's subnet may reach the API's port. Nothing else new may.
#
# Deliberately not a widening of `allow_internal` above: that rule describes
# traffic between nodes and pods, and adding a serverless range to it would
# make a rule about the cluster's interior also a rule about who may enter it.
# Two rules that each say one thing beat one rule that says two.
resource "google_compute_firewall" "allow_console_to_api" {
  count     = var.console_egress_cidr == null ? 0 : 1
  project   = var.project_id
  name      = "qip-${var.environment}-allow-console-to-api"
  network   = google_compute_network.vpc.id
  direction = "INGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = ["8080"]
  }

  source_ranges = [var.console_egress_cidr]
  target_tags   = ["qip-node"]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}
