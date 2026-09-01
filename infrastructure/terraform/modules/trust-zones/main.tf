# The trust zones.
#
# Thirteen zones, default deny between them, and only the paths somebody wrote
# down. The zone model is blueprint §46.1; this module is the part of it a
# network and an IAM policy can hold, and NOT-ENFORCED-HERE.md is the part they
# cannot — read that file before believing anything here is a complete control.
#
# The shape of the thing:
#
#   * a zone gets one subnet, one service account, one network tag, and a
#     deny-everything rule in each direction. That is the whole default. A zone
#     that is declared and given nothing else can reach nothing and be reached
#     by nothing, which is the correct posture for a zone whose paths have not
#     been argued through.
#   * an internal path exists only if `permitted_paths` names it, and only if
#     the pair and the mode appear in `local.sanctioned_paths` below. A caller
#     cannot invent a path this file does not sanction; changing what is
#     sanctioned is an edit to this file and therefore a review.
#   * external egress exists only if `external_egress` names it, and only for
#     the purposes `local.sanctioned_egress_purposes` gives that zone. Only
#     optimisation may name `ibm-quantum`, so IBM egress from anywhere else is
#     refused at plan time rather than caught in a NAT log afterwards.
#   * the internet reaches `public_ingress` zones and nothing else, from
#     Google's load-balancer ranges only.
#
# Nothing here is wired into the root module yet. An unwired module changes no
# plan, which is deliberate: the wiring pass carries its own plan as evidence.
# NOT-ENFORCED-HERE.md, §Not wired, lists exactly what that pass has to do.

locals {
  # --- the zone model ------------------------------------------------------
  #
  # The thirteen names, in blueprint order. This list is the module's whole
  # vocabulary: a name outside it is refused by `var.zones`, and every other
  # map here is keyed by these names, so a typo becomes an undeclared zone
  # rather than a zone with no rules.
  zone_names = [
    "public-edge",
    "application-identity",
    "ingestion-discovery",
    "cognition",
    "valuation",
    "intelligence",
    "optimisation",
    "control-fabric",
    "execution",
    "ledger",
    "wallet-read",
    "treasury-write",
    "management",
  ]

  # A service account id is 6 to 30 characters, and two zone names are long
  # enough to overrun that once the prefix and the environment are on them.
  # Shortening happens here, once, rather than at each call site.
  zone_code = {
    "public-edge"          = "public-edge"
    "application-identity" = "app-identity"
    "ingestion-discovery"  = "ingestion"
    "cognition"            = "cognition"
    "valuation"            = "valuation"
    "intelligence"         = "intelligence"
    "optimisation"         = "optimisation"
    "control-fabric"       = "fabric"
    "execution"            = "execution"
    "ledger"               = "ledger"
    "wallet-read"          = "wallet-read"
    "treasury-write"       = "treasury"
    "management"           = "management"
  }

  # Every internal path the blueprint sanctions, and the modes each may carry.
  #
  # Read it as the adjacency list of §46.1. Absence is the point: there is no
  # `ledger->` key, because the ledger sends nothing outbound; there is no
  # `wallet-read->` key, because the read path talks to the outside world and
  # to nothing inside; and there is no key joining `wallet-read` and
  # `treasury-write` in either direction, which is what keeps the read path and
  # the signing path two boundaries rather than one with a wall drawn on it.
  core_paths = {
    "public-edge->application-identity"    = ["request"]
    "application-identity->ledger"         = ["read"]
    "application-identity->intelligence"   = ["intent"]
    "application-identity->treasury-write" = ["intent"]
    "ingestion-discovery->cognition"       = ["publish"]
    "cognition->ledger"                    = ["read"]
    "cognition->control-fabric"            = ["publish"]
    "valuation->ledger"                    = ["read"]
    "valuation->cognition"                 = ["read"]
    "intelligence->ledger"                 = ["read", "append"]
    "intelligence->control-fabric"         = ["publish"]
    "optimisation->ledger"                 = ["read"]
    "control-fabric->execution"            = ["publish"]
    "execution->control-fabric"            = ["publish"]
    "execution->ledger"                    = ["append"]
    "treasury-write->ledger"               = ["append"]
  }

  # Management reaches everything, audited. Generated rather than typed, so a
  # fourteenth zone cannot be added with management access silently missing —
  # break-glass that does not reach the thing on fire is not break-glass.
  management_paths = {
    for name in local.zone_names :
    "management->${name}" => ["audited"]
    if name != "management"
  }

  sanctioned_paths = merge(local.core_paths, local.management_paths)

  # External egress, exhaustively. A zone absent from this map may reach
  # nothing outside the VPC at all, and most zones are absent.
  #
  # `ibm-quantum` appears exactly once. That single occurrence is the whole of
  # "only optimisation may reach IBM": there is no other key it could be
  # declared under, and `var.external_egress` refuses a purpose its zone does
  # not hold.
  sanctioned_egress_purposes = {
    "ingestion-discovery" = ["information-source"]
    "execution"           = ["venue"]
    "wallet-read"         = ["venue-read", "chain", "custodian-read"]
    "treasury-write"      = ["custodian", "withdrawal-api"]
    "optimisation"        = ["ibm-quantum"]
  }

  # The only zones a client may reach. Customer traffic terminates in front of
  # these two and nowhere else; a load balancer in front of a trading zone is
  # refused rather than reviewed, because the two kinds of traffic sharing one
  # front door is how a customer session ends up holding a trading route.
  client_reachable_zones = ["public-edge", "application-identity"]

  # The ranges Google's own load balancing and health checking originate from.
  # A backend reachable from anywhere else is reachable from the internet.
  load_balancer_ranges = ["35.191.0.0/16", "130.211.0.0/22"]

  # The tag every rule below targets, per zone. Exported: a rule targeting a
  # tag no instance carries is a rule that does nothing, and it does nothing
  # silently.
  zone_tag = {
    for name, _ in var.zones : name => "qip-${var.environment}-tz-${name}"
  }

  # The range each zone answers on, flattened out of `var.zones` so a path
  # rule naming a zone that was never declared falls back to a documentation
  # address instead of failing on an index — the precondition below is the
  # error the reader should see, not a lookup trace.
  zone_cidr = {
    for name, zone in var.zones : name => zone.subnet_cidr
  }

  # Which zones may use the NAT, derived from what was declared rather than
  # from a second list that can disagree with it.
  egress_zones = distinct([for entry in values(var.external_egress) : entry.zone])

  nat_zones = [
    for name in local.egress_zones : name
    if contains(keys(var.zones), name) && var.zones[name].region == var.region
  ]

  # Ledger access, split by the mode the path was declared with. Filtered to
  # declared zones so that an undeclared one produces the path rule's
  # precondition failure rather than an index error nobody can read.
  ledger_readers = distinct([
    for path in values(var.permitted_paths) : path.from
    if path.to == "ledger" && path.mode == "read" && contains(keys(var.zones), path.from)
  ])

  ledger_appenders = distinct([
    for path in values(var.permitted_paths) : path.from
    if path.to == "ledger" && path.mode == "append" && contains(keys(var.zones), path.from)
  ])

  fabric_publishers = distinct([
    for path in values(var.permitted_paths) : path.from
    if path.to == "control-fabric" && path.mode == "publish" && contains(keys(var.zones), path.from)
  ])

  fabric_subscribers = distinct([
    for path in values(var.permitted_paths) : path.to
    if path.from == "control-fabric" && path.mode == "publish" && contains(keys(var.zones), path.to)
  ])
}

# --- the zones themselves ----------------------------------------------------

# One subnet per zone. Separate ranges rather than a shared one with rules
# drawn across it: a firewall rule can name a range, and a range that belongs
# to one zone is a range that cannot quietly acquire a second tenant.
resource "google_compute_subnetwork" "zone" {
  for_each = var.zones

  project = var.project_id
  name    = "qip-${var.environment}-tz-${each.key}"
  region  = each.value.region
  network = var.network_id

  ip_cidr_range = each.value.subnet_cidr

  # Google APIs without a public address, which is what makes the
  # no-external-address posture survivable rather than merely stated.
  private_ip_google_access = true

  dynamic "secondary_ip_range" {
    for_each = each.value.pod_cidr == null ? [] : [each.value.pod_cidr]
    content {
      range_name    = "pods"
      ip_cidr_range = secondary_ip_range.value
    }
  }

  dynamic "secondary_ip_range" {
    for_each = each.value.service_cidr == null ? [] : [each.value.service_cidr]
    content {
      range_name    = "services"
      ip_cidr_range = secondary_ip_range.value
    }
  }

  # "What talked to what" is the question asked after an incident, and half a
  # percent is enough to answer it.
  log_config {
    aggregation_interval = "INTERVAL_5_SEC"
    flow_sampling        = 0.5
    metadata             = "INCLUDE_ALL_METADATA"
  }
}

# One identity per zone, and therefore no identity shared by two.
#
# This is the structural half of the wallet/treasury separation: the read path
# and the signing path authenticate as different principals because there is no
# expression in this module that could give them the same one. Workload
# Identity Federation throughout — a downloaded key would make the whole
# arrangement a formality, since a key copied out of one zone works from any.
resource "google_service_account" "zone" {
  for_each = var.zones

  project    = var.project_id
  account_id = "qip-tz-${local.zone_code[each.key]}-${var.environment}"

  lifecycle {
    precondition {
      condition     = length("qip-tz-${local.zone_code[each.key]}-${var.environment}") <= 30
      error_message = "The derived service account id qip-tz-${local.zone_code[each.key]}-${var.environment} is ${length("qip-tz-${local.zone_code[each.key]}-${var.environment}")} characters; Google allows 30. Shorten the code for this zone in local.zone_code."
    }
  }

  display_name = "qip trust zone ${each.key} (${var.environment})"
  description  = "Workload identity for the ${each.key} trust zone. Blueprint §46.1."
}

resource "google_service_account_iam_member" "workload_identity" {
  for_each = var.zones

  service_account_id = google_service_account.zone[each.key].name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.project_id}.svc.id.goog[${var.kubernetes_namespace}/${each.value.kubernetes_service_account}]"
}

# The baseline every zone gets and no zone gets more of: it may say what it did
# and how it is. A zone that cannot write a log is a zone whose breach has no
# record, and that is the one grant worth having before any other.
resource "google_project_iam_member" "telemetry" {
  for_each = var.zones

  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.zone[each.key].email}"
}

resource "google_project_iam_member" "logging" {
  for_each = var.zones

  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.zone[each.key].email}"
}

# --- default deny, in both directions ---------------------------------------

# Priority 65000 is below every allow this module writes and above Terraform's
# implied allow-all egress at 65535. Delete a path rule and the zone stops
# talking; delete this one and the zone talks to everything, which is why it is
# declared per zone rather than assumed from the platform default.
resource "google_compute_firewall" "deny_egress" {
  for_each = var.zones

  project = var.project_id
  name    = "qip-${var.environment}-tz-${each.key}-deny-egress"
  network = var.network_id

  direction = "EGRESS"
  priority  = 65000

  deny {
    protocol = "all"
  }

  destination_ranges = ["0.0.0.0/0"]
  target_tags        = [local.zone_tag[each.key]]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

resource "google_compute_firewall" "deny_ingress" {
  for_each = var.zones

  project = var.project_id
  name    = "qip-${var.environment}-tz-${each.key}-deny-ingress"
  network = var.network_id

  direction = "INGRESS"
  priority  = 65000

  deny {
    protocol = "all"
  }

  source_ranges = ["0.0.0.0/0"]
  target_tags   = [local.zone_tag[each.key]]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# --- the permitted paths ------------------------------------------------------

# A path is two rules: the source may leave for the destination's range, and
# the destination may be entered from the source's. Both, because the deny
# above is in both directions, and one without the other is a path that half
# exists — traffic that leaves and is dropped on arrival, diagnosed for a day.
#
# The preconditions are where the zone model stops being a comment. Each fires
# at plan time, before anything is created, and each names the declaration to
# change rather than the rule to widen.
resource "google_compute_firewall" "path_egress" {
  for_each = var.permitted_paths

  project = var.project_id
  name    = "qip-${var.environment}-tzp-${each.key}-out"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [for port in each.value.ports : tostring(port)]
  }

  destination_ranges = [lookup(local.zone_cidr, each.value.to, "192.0.2.0/32")]
  target_tags        = [lookup(local.zone_tag, each.value.from, "qip-${var.environment}-tz-undeclared")]

  lifecycle {
    precondition {
      condition     = contains(keys(var.zones), each.value.from) && contains(keys(var.zones), each.value.to)
      error_message = "Path ${each.key} joins ${each.value.from} to ${each.value.to}, and at least one of those zones is not declared in var.zones. An undeclared zone is refused rather than created empty: a path to a zone that does not exist reads as a boundary and is a rule targeting nothing."
    }

    precondition {
      condition     = contains(keys(local.sanctioned_paths), "${each.value.from}->${each.value.to}")
      error_message = "Path ${each.key} joins ${each.value.from} to ${each.value.to}, which blueprint §46.1 does not sanction. Zone pairs with no declared path are denied. If this path is genuinely required, add it to local.sanctioned_paths in this module with the argument for it — that edit is the review."
    }

    precondition {
      condition     = contains(lookup(local.sanctioned_paths, "${each.value.from}->${each.value.to}", []), each.value.mode)
      error_message = "Path ${each.key} is declared ${each.value.mode}, but ${each.value.from} to ${each.value.to} carries ${join(" or ", lookup(local.sanctioned_paths, "${each.value.from}->${each.value.to}", []))}. The mode is not decoration: it decides the ledger and fabric grants below."
    }
  }
}

resource "google_compute_firewall" "path_ingress" {
  for_each = var.permitted_paths

  project = var.project_id
  name    = "qip-${var.environment}-tzp-${each.key}-in"
  network = var.network_id

  direction = "INGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [for port in each.value.ports : tostring(port)]
  }

  source_ranges = [lookup(local.zone_cidr, each.value.from, "192.0.2.0/32")]
  target_tags   = [lookup(local.zone_tag, each.value.to, "qip-${var.environment}-tz-undeclared")]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# --- what reaches the outside world ------------------------------------------

# One rule per allowlist entry, named after the entry. A single rule listing
# every destination and every port would permit each port to each address —
# a cross product nobody asked for and nobody would notice, which is the same
# mistake the edge-cell module avoids one venue at a time.
resource "google_compute_firewall" "external_egress" {
  for_each = var.external_egress

  project = var.project_id
  name    = "qip-${var.environment}-tzx-${each.key}"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [tostring(each.value.port)]
  }

  destination_ranges = [each.value.cidr]
  target_tags        = [lookup(local.zone_tag, each.value.zone, "qip-${var.environment}-tz-undeclared")]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }

  lifecycle {
    precondition {
      condition     = contains(keys(var.zones), each.value.zone)
      error_message = "Egress entry ${each.key} is declared for zone ${each.value.zone}, which is not in var.zones. Declare the zone or delete the entry; an allowlist entry for a zone that does not exist is an allowlist that grows without anything to grant."
    }

    precondition {
      condition     = contains(lookup(local.sanctioned_egress_purposes, each.value.zone, []), each.value.purpose)
      error_message = "Egress entry ${each.key} gives ${each.value.zone} a ${each.value.purpose} destination. That zone may reach ${join(", ", lookup(local.sanctioned_egress_purposes, each.value.zone, ["nothing outside the VPC"]))}. Only optimisation reaches IBM; ingestion reaches sources and nothing that moves money; everything else reaches nothing."
    }
  }
}

# The internet's only door, and it opens on two zones.
#
# Source ranges are Google's load balancer and health checker, never the whole
# internet: a backend reachable from anywhere is a backend the load balancer's
# policies are optional in front of.
resource "google_compute_firewall" "public_ingress" {
  for_each = var.public_ingress

  project = var.project_id
  name    = "qip-${var.environment}-tzi-${each.key}"
  network = var.network_id

  direction = "INGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [tostring(each.value.port)]
  }

  source_ranges = local.load_balancer_ranges
  target_tags   = [lookup(local.zone_tag, each.value.zone, "qip-${var.environment}-tz-undeclared")]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }

  lifecycle {
    precondition {
      condition     = contains(keys(var.zones), each.value.zone)
      error_message = "Public ingress ${each.key} names zone ${each.value.zone}, which is not declared in var.zones."
    }

    precondition {
      condition     = contains(local.client_reachable_zones, each.value.zone)
      error_message = "Public ingress ${each.key} would put a load balancer in front of ${each.value.zone}. A client reaches the public edge, the static shell and the authenticated application APIs — never Spanner, Pub/Sub, an execution node, a venue, IBM, custody or signing material. Customer traffic and trading traffic share no load balancer, so this is refused rather than scoped."
    }
  }
}

# --- controlled egress --------------------------------------------------------

# The NAT the allowlisted destinations are reached through, listing only the
# zones that declared an allowlist entry. A zone absent from this list has no
# translation and therefore no route out at all — which is a second, separate
# statement from the firewall rules above, and the one that still holds if a
# rule is widened by hand.
resource "google_compute_router" "egress" {
  count = length(local.nat_zones) > 0 ? 1 : 0

  project = var.project_id
  name    = "qip-${var.environment}-tz-router"
  region  = var.region
  network = var.network_id
}

resource "google_compute_router_nat" "egress" {
  count = length(local.nat_zones) > 0 ? 1 : 0

  project = var.project_id
  name    = "qip-${var.environment}-tz-nat"
  router  = google_compute_router.egress[0].name
  region  = var.region

  nat_ip_allocate_option = "AUTO_ONLY"

  # Never ALL_SUBNETWORKS_ALL_IP_RANGES. That option is how a zone created
  # later acquires internet egress by existing, and the zone created later is
  # the one whose paths nobody argued through.
  source_subnetwork_ip_ranges_to_nat = "LIST_OF_SUBNETWORKS"

  dynamic "subnetwork" {
    for_each = toset(local.nat_zones)
    content {
      name                    = google_compute_subnetwork.zone[subnetwork.value].id
      source_ip_ranges_to_nat = ["PRIMARY_IP_RANGE"]
    }
  }

  log_config {
    enable = true
    filter = "ALL"
  }

  lifecycle {
    precondition {
      condition     = length(local.nat_zones) == length(local.egress_zones)
      error_message = "One of the zones declaring external egress (${join(", ", local.egress_zones)}) is not in ${var.region}, and a Cloud NAT is regional. Give that region its own instance of this module rather than moving the zone: a zone NAT'd through another region's router is a zone whose egress leaves from an address the venue allowlist has never seen."
    }
  }
}

# --- the ledger ---------------------------------------------------------------

# Read and append are different grants because they are different paths in
# §46.1, and the module reads the mode off the declaration rather than asking
# for a second list that can disagree with it.
#
# Honesty about the append side: Spanner has no append-only role. A zone
# declared `append` gets `databaseUser`, which can also update and delete.
# Append-only is held by the schema and by the application, not by this
# binding — see NOT-ENFORCED-HERE.md rather than reading the mode as a control.
resource "google_spanner_database_iam_member" "ledger_read" {
  for_each = var.ledger_database == null ? toset([]) : toset(local.ledger_readers)

  project  = var.project_id
  instance = var.ledger_database.instance
  database = var.ledger_database.database
  role     = "roles/spanner.databaseReader"
  member   = "serviceAccount:${google_service_account.zone[each.value].email}"
}

resource "google_spanner_database_iam_member" "ledger_append" {
  for_each = var.ledger_database == null ? toset([]) : toset(local.ledger_appenders)

  project  = var.project_id
  instance = var.ledger_database.instance
  database = var.ledger_database.database
  role     = "roles/spanner.databaseUser"
  member   = "serviceAccount:${google_service_account.zone[each.value].email}"
}

# --- the control fabric -------------------------------------------------------

# Publish down, outcomes up, and never the two grants on one identity for one
# direction. A publisher that can also subscribe can read every other zone's
# payload, which turns a fabric into a shared bus.
resource "google_pubsub_topic_iam_member" "fabric_publish" {
  for_each = var.control_fabric_topic == null ? toset([]) : toset(local.fabric_publishers)

  project = var.project_id
  topic   = var.control_fabric_topic
  role    = "roles/pubsub.publisher"
  member  = "serviceAccount:${google_service_account.zone[each.value].email}"
}

# The attach side, and only the attach side. `subscriber` on a topic grants
# the right to attach a subscription to it; consuming from that subscription
# needs a binding on the subscription itself, which this module does not create
# because it does not create the subscription. Said plainly rather than left to
# be inferred: a reader who takes this for the whole grant will look for a
# consumer permission that is not here.
resource "google_pubsub_topic_iam_member" "fabric_attach" {
  for_each = var.control_fabric_topic == null ? toset([]) : toset(local.fabric_subscribers)

  project = var.project_id
  topic   = var.control_fabric_topic
  role    = "roles/pubsub.subscriber"
  member  = "serviceAccount:${google_service_account.zone[each.value].email}"
}
