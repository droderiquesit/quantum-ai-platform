# The event fabric's two stateful processes (ADR 0100): the broker,
# `qip-fabricd`, and the ledger writer, `qip-ledgerd`. Provisionable from
# committed configuration and off in every environment (W4.2).
#
# Each is behind its own object variable, null by default, the way
# `image_bake_subnet_cidr` gates the bake: an environment opts in by naming
# what the machine needs, and a null is "this environment runs none". Both
# are null everywhere today for the same reason the execution node is: no
# boot image carrying either binary has been baked (`image.yml` bakes the
# execution node's), and both module validations refuse anything but a
# self-link to one image.
#
# Neither touches the paper-trading boundary. The broker carries records;
# the ledger parks any fill whose `simulated` flag is not true (ADR 0106),
# a refusal in the binary with no setting here to relax it.

# --- the broker --------------------------------------------------------------

# Where the broker may sit, refused by name rather than by a lookup trace:
# the control-fabric zone's subnet and tag are what bound it, so the zone
# has to be declared.
resource "terraform_data" "event_fabric_broker_is_placed" {
  count = var.event_fabric_broker == null ? 0 : 1
  input = "event-fabric-broker"

  lifecycle {
    precondition {
      condition     = contains(keys(var.trust_zones), "control-fabric")
      error_message = "event_fabric_broker is set and trust_zones declares no `control-fabric` zone. The broker lives in that zone's subnet on the Fabric network (GCP-009) and carries its tag, which is what every firewall rule for the fabric targets; declare the zone, with a /24 of its own, in the same change."
    }
  }
}

# The broker's own identity: logs and metrics, nothing else. It reads no
# Google API and holds no secret; its journal is its own disk.
resource "google_service_account" "fabricd" {
  count = var.event_fabric_broker == null ? 0 : 1

  project      = var.project_id
  account_id   = "qip-${var.environment}-fabricd"
  display_name = "qip ${var.environment} event-fabric broker"
  description  = "qip-fabricd's machine identity: writes its own logs and metrics and nothing else (ADR 0100 §2)."

  depends_on = [module.services]
}

resource "google_project_iam_member" "fabricd_telemetry" {
  for_each = var.event_fabric_broker == null ? toset([]) : toset(["roles/logging.logWriter", "roles/monitoring.metricWriter"])

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.fabricd[0].email}"
}

# The private zone the SRV record lives in (FABRIC-087), visible on the
# Fabric network alone. `modules/dns-zone` is the public `algorik.ai` zone
# and must never carry an internal address.
resource "google_dns_managed_zone" "fabric" {
  count = var.event_fabric_broker == null ? 0 : 1

  project     = var.project_id
  name        = "qip-${var.environment}-fabric"
  dns_name    = "${var.event_fabric_broker.dns_domain}."
  description = "Event-fabric broker discovery (FABRIC-087), private to the Fabric network."
  visibility  = "private"

  private_visibility_config {
    networks {
      network_url = module.network.fabric_network_id
    }
  }

  labels = local.labels

  depends_on = [module.services]
}

module "event_fabric_broker" {
  source = "./modules/event-fabric-broker"
  count  = var.event_fabric_broker == null ? 0 : 1

  depends_on = [module.services, terraform_data.event_fabric_broker_is_placed]

  enabled    = true
  project_id = var.project_id
  region     = var.region
  zone       = var.event_fabric_broker.zone

  network_id   = module.network.fabric_network_id
  subnet_id    = module.trust_zones.zone_subnets["control-fabric"]
  network_tags = compact([lookup(module.trust_zones.zone_network_tags, "control-fabric", "")])

  machine_type         = var.event_fabric_broker.machine_type
  boot_image           = var.event_fabric_broker.boot_image
  journal_disk_size_gb = var.event_fabric_broker.journal_disk_size_gb

  service_account_email = google_service_account.fabricd[0].email

  dns_zone_name             = var.event_fabric_broker.dns_domain
  managed_zone              = google_dns_managed_zone.fabric[0].name
  broker_internal_addresses = [var.event_fabric_broker.internal_address]

  # The committed stream catalogue (ADR 0100 §5, §7) — the single source of
  # seal age and peak byte rate — rather than a second copy in the tfvars.
  qip_events_config  = file("${path.module}/../event-fabric/streams.local.json")
  qip_fabricd_config = jsonencode({ environment = var.environment, region = var.region })
}

# --- the ledger --------------------------------------------------------------

resource "terraform_data" "ledgerd_is_placed" {
  count = var.ledgerd == null ? 0 : 1
  input = "ledgerd"

  lifecycle {
    precondition {
      condition     = contains(keys(var.trust_zones), "ledger")
      error_message = "ledgerd is set and trust_zones declares no `ledger` zone. The ledger writer lives in that zone's subnet and carries its tag; declare the zone, with a /24 of its own, in the same change."
    }
  }
}

module "ledgerd" {
  source = "./modules/ledgerd"
  count  = var.ledgerd == null ? 0 : 1

  depends_on = [module.services, terraform_data.ledgerd_is_placed]

  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels
  zone        = var.ledgerd.zone

  network_id   = module.network.service_network_id
  subnet_id    = module.trust_zones.zone_subnets["ledger"]
  network_tags = compact([lookup(module.trust_zones.zone_network_tags, "ledger", "")])

  internal_address      = var.ledgerd.internal_address
  boot_image            = var.ledgerd.boot_image
  machine_type          = var.ledgerd.machine_type
  archive_disk_size_gb  = var.ledgerd.archive_disk_size_gb
  account_id            = var.ledgerd.account_id
  fabric_consumer_group = var.ledgerd.fabric_consumer_group
}
