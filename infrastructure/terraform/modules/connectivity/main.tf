# Private connectivity: partner interconnect, and private access to Google APIs.
#
# The platform's deployment model names "Private Links & Direct Peering" and
# nothing in this configuration created either. This module is that Terraform,
# and it is also the honest answer to a problem the platform already writes
# down rather than a capability added for its own sake.
#
# `environments/prod/terraform.tfvars` and `docs/operations/deploying-an-edge-cell.md` both record
# that three of the nine cells are not in their metropolitan area: Google Cloud
# has no region in Chicago, none in NY/NJ and none in Dubai, so `chicago-1`,
# `newyork-1` and `dubai-1` run roughly 400, 300 and 380 kilometres from the
# venues whose adjacency is the entire argument for a cell existing. The
# runbook names three honest answers, and the first is colocation with a
# partner interconnect back to the VPC. This is the VPC half of that: the
# attachments a colocated cage terminates on, the Cloud Router they need, and a
# private path from that circuit to Google APIs.
#
# # Everything here is off by default
#
# The same rule `modules/data` applies to every managed store, for a reason
# that is sharper here. A VLAN attachment with no partner circuit sits in
# `PENDING_PARTNER` waiting for a cross-connect that nobody has ordered, and
# once a partner does activate it, it bills hourly whether or not a single
# packet crosses it. Worse than the bill: an attachment in a diagram reads as a
# private path that exists. It does not exist until a partner, a circuit, a
# pairing key and a VLAN attachment on their side all exist, and Terraform can
# create exactly none of those four. See NOT-ORDERED.md.
#
# # What this module deliberately does not create
#
# No `google_compute_router_interface` and no `google_compute_router_peer`. For
# a PARTNER attachment Google creates both itself when the partner activates
# the circuit, and a hand-written pair fights it: Terraform proposes an
# interface the API already made, and the attachment ends up with two.
#
# No `google_compute_service_attachment` either. That is the producer side of
# Private Service Connect — publishing a service in this VPC for someone else
# to consume — and nothing in this platform is published to anyone. An
# attachment nobody consumes is an endpoint waiting to be found.

locals {
  prefix = "qip-${var.environment}"

  # Nothing is created when the flag is off, whatever the map says — so the
  # intended attachments can be written down, reviewed and committed while the
  # circuits are still being negotiated, and the day they arrive is a one-line
  # change rather than a design discussion. Until then this is empty and the
  # plan contains no attachment at all, which is the honest state.
  attachments = var.enable_partner_interconnect ? var.partner_interconnects : {}

  # One Cloud Router per region an attachment lands in. A router is regional:
  # two attachments in one region share one, and an attachment in a region with
  # no router cannot be created at all.
  router_regions = toset([for attachment in values(local.attachments) : attachment.region])

  # HA VPN gateways are created only when explicitly enabled AND the gateway's
  # region has an Interconnect attachment to share the Cloud Router with. Like
  # Interconnect, this is off by default — a gateway without a configured peer
  # reads as a working backup path when it is not. The peer's configuration
  # lives outside this repository, and an operator must complete it before the
  # path carries traffic.
  vpn_enabled = var.enable_ha_vpn
  vpn_configs_input = var.enable_ha_vpn ? var.ha_vpn_peer_gateways : {}
  # Only create VPN in regions that have Interconnect attachments (to share routers)
  vpn_configs = {
    for config_key, config in local.vpn_configs_input :
    config_key => config
    if contains(local.router_regions, config.region)
  }

  # Flatten VPN configs into tunnel specifications: one entry per tunnel,
  # with config key, tunnel index, and derived link-local IP range.
  vpn_tunnels = merge([
    for config_key, config in local.vpn_configs : {
      "${config_key}-1" = {
        config_key      = config_key
        tunnel_index    = 1
        name            = "${local.prefix}-${config_key}-tunnel-1"
        region          = config.region
        peer_ip         = config.peer_gateway_addresses[0]
        shared_secret   = config.tunnel_1_shared_secret
        peer_asn        = config.peer_asn
        link_local_base = 10 + (index(keys(local.vpn_configs), config_key) * 4)
      }
      "${config_key}-2" = {
        config_key      = config_key
        tunnel_index    = 2
        name            = "${local.prefix}-${config_key}-tunnel-2"
        region          = config.region
        peer_ip         = length(config.peer_gateway_addresses) > 1 ? config.peer_gateway_addresses[1] : config.peer_gateway_addresses[0]
        shared_secret   = config.tunnel_2_shared_secret
        peer_asn        = config.peer_asn
        link_local_base = 10 + (index(keys(local.vpn_configs), config_key) * 4) + 2
      }
    }
  ]...)
}

# --- The Cloud Router -------------------------------------------------------

resource "google_compute_router" "interconnect" {
  for_each = local.router_regions

  project = var.project_id
  name    = "${local.prefix}-interconnect-${each.value}"
  region  = each.value
  network = var.network_id

  description = "BGP for the partner interconnect attachments terminating in ${each.value}."

  bgp {
    # A private ASN. This side of the session is the VPC's; the colocated
    # equipment uses its own, and the two must differ or the session never
    # establishes — a failure that shows up as an attachment stuck in
    # `PENDING_PARTNER` long after the partner says they are done.
    asn = var.cloud_router_asn

    # `DEFAULT` advertises the VPC's own subnets and nothing else. Custom
    # advertisement is how a router ends up announcing a range it does not own
    # to a partner network, which is a routing incident in somebody else's
    # network rather than in this one.
    advertise_mode = "DEFAULT"
  }
}

# --- The attachments --------------------------------------------------------
#
# `type = "PARTNER"` means Google allocates a pairing key and waits. The
# deployment gives that key to the partner, the partner provisions the circuit
# on their side, and only then does the attachment carry traffic. Terraform
# creates the near half and can neither order the cross-connect nor confirm one
# arrived.

resource "google_compute_interconnect_attachment" "partner" {
  for_each = local.attachments

  project = var.project_id
  name    = "${local.prefix}-${each.key}"
  region  = each.value.region
  router  = google_compute_router.interconnect[each.value.region].id

  type = "PARTNER"

  # Which of the region's two edge availability domains this attachment lands
  # in. Two attachments in the same domain are not redundant: one metro
  # maintenance window takes both, and the deployment discovers that its pair
  # of circuits was one circuit twice.
  edge_availability_domain = each.value.edge_availability_domain

  # Off unless the entry says otherwise, which is not the Google default.
  #
  # An attachment that is enabled when the partner activates it starts
  # accepting and advertising routes the moment the far end comes up — before
  # anybody has looked at what the far end is. Enabling it is the deliberate
  # act that admits a route from a network this project does not control.
  admin_enabled = each.value.admin_enabled

  description = each.value.description
  labels      = var.labels
}

# --- HA VPN fallback for Interconnect ----------------------------------------
#
# A backup path when partner interconnect is unavailable or being maintained.
# The gateway and tunnels are created when the flag is on, but traffic flows
# only when a peer VPN endpoint outside this repository has been configured
# and the shared secrets transmitted to it by hand. An unconfigured gateway
# wastes nothing and blocks nothing; it reads, to an operator reviewing the
# infrastructure, as a defined fallback that awaits connection.

resource "google_compute_ha_vpn_gateway" "backup" {
  for_each = local.vpn_configs

  project = var.project_id
  name    = "${local.prefix}-vpn-${each.key}"
  region  = each.value.region
  network = var.network_id
  type    = "IPV4"

  labels = var.labels
}

# HA VPN requires two tunnels for redundancy and HA failover. Each tunnel
# connects to an external IP on the peer's side. The first tunnel uses the
# first peer address (or the first address if only one is configured), and
# the second tunnel uses the second address (or the first address again).
resource "google_compute_vpn_tunnel" "backup" {
  for_each = local.vpn_tunnels

  project       = var.project_id
  name          = each.value.name
  region        = each.value.region
  vpn_gateway   = google_compute_ha_vpn_gateway.backup[each.value.config_key].id
  peer_ip       = each.value.peer_ip
  shared_secret = each.value.shared_secret
  ike_version   = 2

  labels = var.labels

  depends_on = [google_compute_ha_vpn_gateway.backup]
}

# Each router interface is tied to a single VPN tunnel. The interface gets
# a link-local address for the BGP session; by convention, Google Cloud uses
# 169.254.x.y for these. Each config gets a /24 block; within it, tunnel-1
# gets .0/30 and tunnel-2 gets .4/30.
resource "google_compute_router_interface" "backup_tunnel" {
  for_each = google_compute_vpn_tunnel.backup

  name       = "${each.value.name}-iface"
  router     = google_compute_router.interconnect[local.vpn_tunnels[each.key].region].name
  region     = local.vpn_tunnels[each.key].region
  ip_range   = "169.254.${local.vpn_tunnels[each.key].link_local_base}.0/30"
  vpn_tunnel = each.value.name

  depends_on = [google_compute_ha_vpn_gateway.backup, google_compute_vpn_tunnel.backup]
}

# BGP sessions on the tunnels allow dynamic routing. Each tunnel gets its own
# session, and both carry the same routes. If one tunnel fails, traffic
# reroutes to the other without reconfiguration.
resource "google_compute_router_peer" "backup_tunnel" {
  for_each = google_compute_router_interface.backup_tunnel

  name            = "${local.vpn_tunnels[each.key].name}-peer"
  router          = google_compute_router.interconnect[local.vpn_tunnels[each.key].region].name
  region          = local.vpn_tunnels[each.key].region
  peer_asn        = local.vpn_tunnels[each.key].peer_asn
  interface_name  = each.value.name
  peer_ip_address = "169.254.${local.vpn_tunnels[each.key].link_local_base + 1}.1"

  depends_on = [google_compute_router_interface.backup_tunnel]
}

# --- Private Service Connect for Google APIs --------------------------------
#
# An internal address in this VPC that answers for Google APIs, so a request
# reaches them without a route to the internet. The VPC already has private
# Google access through the restricted range `199.36.153.8/30`; this endpoint
# is what makes the same thing reachable *from the far end of the
# interconnect*, where that range is not routable without one.
#
# Which is the point of it here: a colocated cell in Chicago or NY/NJ needs to
# read a secret, pull an image and write evidence, and doing that over the
# public internet would give back the isolation the interconnect was ordered
# for.

resource "google_compute_global_address" "google_apis" {
  count = var.enable_private_service_connect ? 1 : 0

  project      = var.project_id
  name         = "${local.prefix}-google-apis"
  purpose      = "PRIVATE_SERVICE_CONNECT"
  address_type = "INTERNAL"
  network      = var.network_id

  # Named explicitly rather than allocated. The address has to be one the far
  # end's own DNS can be pointed at, and an address Google picked at apply time
  # is one somebody has to go and read before anything can resolve to it.
  address = var.private_service_connect_address
  labels  = var.labels
}

resource "google_compute_global_forwarding_rule" "google_apis" {
  count = var.enable_private_service_connect ? 1 : 0

  project = var.project_id
  name    = "${local.prefix}-google-apis"
  network = var.network_id

  ip_address = google_compute_global_address.google_apis[0].id

  # `vpc-sc` by default rather than `all-apis`: the restricted bundle, the same
  # set the VPC's existing `199.36.153.8/30` route reaches, and the set a VPC
  # Service Controls perimeter can actually protect. `all-apis` includes APIs
  # that no perimeter covers, which would widen the egress surface at exactly
  # the point the platform documents as its narrowest.
  target = var.private_service_connect_target

  # A PSC endpoint is not a load balancer, and the empty scheme is how the API
  # is told so. Any other value is rejected at apply.
  load_balancing_scheme = ""
  labels                = var.labels
}
