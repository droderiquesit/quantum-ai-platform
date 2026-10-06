# The networks.
#
# Three separate VPCs per environment (GCP-009): Reflex for execution nodes,
# Fabric for control-plane and event-fabric workloads, Service for all others.
# Each has its own routing and firewall boundary. §45 asks for regional subnets
# with no inter-region peering; a Google Cloud VPC is global and subnets in
# different regions share it natively. Three VPCs partition this at the network
# level rather than relying on firewall rules alone.
#
# This module owns three network resources, the deny-all ingress on each, the
# private Google APIs zone (which serves all three), the console egress subnet
# on the Service VPC (ADR 0018), and the egress firewall rules for the console.
#
# What this module is *not*, deliberately:
#
#   * It is not where workloads live. Every Cloud Run workload attaches to
#     its trust zone's subnet on the appropriate network; every execution node
#     to its own on the Reflex network. A range and the rules that bound it are
#     one declaration per zone.
#   * It has no NAT. Cloud NAT is created per network by the zone or node
#     module that needs egress.
#   * It does not create a Private Service Connect endpoint for Google APIs.
#     `modules/connectivity` already has one, gated off.

locals {
  networks = {
    reflex  = "qip-${var.environment}-reflex"
    fabric  = "qip-${var.environment}-fabric"
    service = "qip-${var.environment}-service"
  }
}

resource "google_compute_network" "vpc" {
  for_each = local.networks

  project = var.project_id
  name    = each.value

  auto_create_subnetworks = false
  routing_mode            = "REGIONAL"
}

# Deny everything inbound that is not explicitly permitted on each network.
resource "google_compute_firewall" "deny_ingress" {
  for_each = local.networks

  project = var.project_id
  name    = "${each.value}-deny-ingress"
  network = google_compute_network.vpc[each.key].id

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

# --- Data and Engineering VPCs (GCP-051) ----------------------------------------
#
# Two separate VPCs for the Data and Engineering planes, as required by GCP-051.
# Each is structurally identical to the main VPC: deny-all ingress, private
# Google APIs zone, but no regional subnets or console egress — those stay in
# the shared VPC, `modules/trust-zones` and `modules/execution-node` are its
# tenants. These networks join others only through NCC spokes.

resource "google_compute_network" "data_vpc" {
  project = var.project_id
  name    = "qip-${var.environment}-data"

  auto_create_subnetworks = false
  routing_mode            = "REGIONAL"
}

resource "google_compute_firewall" "data_vpc_deny_ingress" {
  project = var.project_id
  name    = "qip-${var.environment}-data-deny-ingress"
  network = google_compute_network.data_vpc.id

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

resource "google_dns_managed_zone" "data_googleapis" {
  project     = var.project_id
  name        = "qip-${var.environment}-data-googleapis"
  dns_name    = "googleapis.com."
  description = "Sends every Google API to the restricted VIP in the Data VPC."
  visibility  = "private"

  private_visibility_config {
    networks {
      network_url = google_compute_network.data_vpc.id
    }
  }

  labels = var.labels
}

resource "google_dns_record_set" "data_restricted_vip" {
  project      = var.project_id
  managed_zone = google_dns_managed_zone.data_googleapis.name
  name         = "restricted.googleapis.com."
  type         = "A"
  ttl          = 300
  rrdatas      = ["199.36.153.8", "199.36.153.9", "199.36.153.10", "199.36.153.11"]
}

resource "google_dns_record_set" "data_googleapis_wildcard" {
  project      = var.project_id
  managed_zone = google_dns_managed_zone.data_googleapis.name
  name         = "*.googleapis.com."
  type         = "CNAME"
  ttl          = 300
  rrdatas      = ["restricted.googleapis.com."]
}

resource "google_compute_network" "engineering_vpc" {
  project = var.project_id
  name    = "qip-${var.environment}-engineering"

  auto_create_subnetworks = false
  routing_mode            = "REGIONAL"
}

resource "google_compute_firewall" "engineering_vpc_deny_ingress" {
  project = var.project_id
  name    = "qip-${var.environment}-engineering-deny-ingress"
  network = google_compute_network.engineering_vpc.id

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

resource "google_dns_managed_zone" "engineering_googleapis" {
  project     = var.project_id
  name        = "qip-${var.environment}-engineering-googleapis"
  dns_name    = "googleapis.com."
  description = "Sends every Google API to the restricted VIP in the Engineering VPC."
  visibility  = "private"

  private_visibility_config {
    networks {
      network_url = google_compute_network.engineering_vpc.id
    }
  }

  labels = var.labels
}

resource "google_dns_record_set" "engineering_restricted_vip" {
  project      = var.project_id
  managed_zone = google_dns_managed_zone.engineering_googleapis.name
  name         = "restricted.googleapis.com."
  type         = "A"
  ttl          = 300
  rrdatas      = ["199.36.153.8", "199.36.153.9", "199.36.153.10", "199.36.153.11"]
}

resource "google_dns_record_set" "engineering_googleapis_wildcard" {
  project      = var.project_id
  managed_zone = google_dns_managed_zone.engineering_googleapis.name
  name         = "*.googleapis.com."
  type         = "CNAME"
  ttl          = 300
  rrdatas      = ["restricted.googleapis.com."]
}

# --- Google APIs without an external address ---------------------------------
#
# Every subnet on this platform has private Google access and no external
# address, and every egress firewall names `199.36.153.4/30` as the one range
# a workload may reach Google APIs on. That range answers only if the
# workload resolves `storage.googleapis.com` *to* it — otherwise the name
# resolves to a public address, the egress deny drops the packet, and the
# failure reads as the vendor being down. This zone is the resolver's half
# of that arrangement: `*.googleapis.com` is a CNAME to
# `restricted.googleapis.com`, and that name is the four restricted-VIP
# addresses. Cloud Run's direct VPC egress and Compute Engine instances both
# resolve through the VPC, so one zone serves every tier.
#
# `restricted` (199.36.153.4/30) rather than `private` (199.36.153.8/30): the
# restricted VIP carries only the APIs a VPC Service Controls perimeter can
# protect, which is the set this platform uses, and it refuses the ones a
# perimeter cannot — so a workload that reaches for an API outside that set
# fails to resolve it rather than quietly reaching it.
resource "google_dns_managed_zone" "googleapis" {
  project     = var.project_id
  name        = "qip-${var.environment}-googleapis"
  dns_name    = "googleapis.com."
  description = "Sends every Google API to the restricted VIP across all three networks (Reflex, Fabric, Service)."
  visibility  = "private"

  private_visibility_config {
    dynamic "networks" {
      for_each = local.networks
      content {
        network_url = google_compute_network.vpc[networks.key].id
      }
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
  rrdatas      = ["199.36.153.4", "199.36.153.5", "199.36.153.6", "199.36.153.7"]
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
  network = google_compute_network.vpc["service"].id

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

# --- what the console may reach ----------------------------------------------
#
# The subnet above is the one subnet on this platform that `modules/trust-zones`
# does not cover: it is not a zone, so no zone tag is on its interface and no
# per-zone `deny_egress` targets it, and what applied to it was Terraform's
# implied allow-all egress at priority 65535 — the exact thing the zone deny
# exists to sit above (missing-infrastructure-register §3). These two rules are
# the zone module's pair, copied in shape and priority rather than invented,
# so that a reader comparing the console to a zone finds the same posture.
locals {
  # The tag both rules target, named after the subnet so the operator adding
  # it to the console sees which subnet it belongs with. A Cloud Run interface
  # carries a tag only if the deploy passes `--network-tags`, and
  # `scripts/deploy-frontends.sh` does not yet: until it does, these rules
  # bind no instance and — as the zone module says of its own tags — do
  # nothing, silently. That gap is written here rather than discovered from a
  # flow log, and the script is the file that closes it.
  console_egress_tag = "qip-${var.environment}-console-egress"

  # The restricted VIP: the /30 the A record above resolves every
  # `*.googleapis.com` to, and the default `modules/trust-zones` and
  # `modules/execution-node` take for `google_apis_range`. One literal, kept
  # beside the record set that makes it mean something.
  restricted_vip_range = "199.36.153.4/30"
}

# Priority 65000, `0.0.0.0/0`, all protocols — the zone module's deny. The
# failure this rule prevents is the register's: a console whose egress was
# never argued through could reach any private range in the VPC, and a
# compromised portal is then a foothold in every zone's subnet rather than a
# broken web page. The failure it must not cause is the opposite one, and it
# is the reason the allow below exists: a deny that breaks the console is not
# found by a test — `terraform validate` cannot see a dropped packet — it is
# found by an operator whose portal answers 500 on every gateway call, and
# that reads as a platform fault rather than a firewall one.
resource "google_compute_firewall" "console_egress_deny_egress" {
  count   = var.console_egress_cidr == null ? 0 : 1
  project = var.project_id
  name    = "qip-${var.environment}-console-egress-deny-egress"
  network = google_compute_network.vpc["service"].id

  direction = "EGRESS"
  priority  = 65000

  deny {
    protocol = "all"
  }

  destination_ranges = ["0.0.0.0/0"]
  target_tags        = [local.console_egress_tag]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# The one allow, and it is the zone module's `google_apis` rule: TCP 443 to
# the restricted VIP, priority 1000, nothing else. Everything the console
# reaches over this interface is a Google-fronted address — Secret Manager for
# its token, Identity Platform for sign-in, and `qip-api` at its own Cloud Run
# URL (ADR 0018; `api_internal_base_url` in the root outputs, which replaced
# the internal load-balancer address the GKE runtime reserved). There is no
# private address of the API's for this rule to name, so a rule naming a zone
# subnet would admit traffic the console never sends. That is the reason
# the destination is this /30 rather than anything wider: an allow to
# `10.0.0.0/8` or to `0.0.0.0/0` would be the original gap with a rule's name
# on it, and would pass every text-level check this repository has.
#
# What this rule does not decide, said plainly: whether a request to the API's
# `run.app` URL crosses this interface at all is the deploy's egress setting
# (`--vpc-egress private-ranges-only`) and the VPC's resolution of `run.app`,
# neither of which this module holds. A request that leaves by Cloud Run's own
# egress instead is not subject to either rule here.
resource "google_compute_firewall" "console_egress_google_apis" {
  count   = var.console_egress_cidr == null ? 0 : 1
  project = var.project_id
  name    = "qip-${var.environment}-console-egress-google-apis"
  network = google_compute_network.vpc["service"].id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = ["443"]
  }

  destination_ranges = [local.restricted_vip_range]
  target_tags        = [local.console_egress_tag]
}
