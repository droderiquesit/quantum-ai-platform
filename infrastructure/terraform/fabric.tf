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

    # ADR 0112: the broker's listener is mutual TLS on a CA Service workload
    # certificate and qip-fabricd refuses plaintext outside `local`; it also
    # refuses to start admitting nobody by name. So a broker is refused at
    # plan where the environment has no CA, no pinned Google roots, or no
    # client the listener could name.
    precondition {
      condition     = var.certificate_authority != null && var.google_tls_roots_file != null
      error_message = "event_fabric_broker is set and the environment has no certificate_authority or no google_tls_roots_file. The broker's listener is mutual TLS on a CA Service certificate (ADR 0112) and there is no plaintext mode outside a developer's machine."
    }

    precondition {
      # Read from configuration, not from `local.fabric_admitted_peers`: the
      # peers come out of the clients' modules, and a precondition that
      # waited on them would never be evaluated when a client is itself
      # refused — hiding this refusal behind that one.
      condition     = var.ledgerd != null || (var.execution_nodes_enabled && length(var.execution_nodes) > 0)
      error_message = "event_fabric_broker is set and no client is declared beside it — no qip-ledgerd and no execution node — so its listener would admit nobody by name, which qip-fabricd refuses."
    }

    precondition {
      condition     = length(var.event_fabric_broker.additional_zones) == length(var.event_fabric_broker.additional_addresses)
      error_message = "event_fabric_broker names a different number of additional_zones than additional_addresses. Each additional broker is placed in the zone at its position, distinct from `zone` and from each other (FABRIC-008)."
    }
  }
}

# The workloads the broker admits: every CA Service requester but the broker
# itself, by the one SAN the workload template lets each obtain. The same map
# the requester grants are made from (security.tf), so a client that can get
# a certificate is exactly a client the broker names (FABRIC, item 8 of the
# round-2 review: QIP_EVENT_FABRIC_ADMITTED_PEERS).
locals {
  fabric_admitted_peers = sort([for key, san in local.cas_san_of : san if key != "fabricd"])
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

  journal_provisioned_iops             = var.event_fabric_broker.journal_provisioned_iops
  journal_provisioned_throughput_mibps = var.event_fabric_broker.journal_provisioned_throughput_mibps

  service_account_email = google_service_account.fabricd[0].email

  dns_zone_name             = var.event_fabric_broker.dns_domain
  managed_zone              = google_dns_managed_zone.fabric[0].name
  broker_internal_addresses = concat([var.event_fabric_broker.internal_address], var.event_fabric_broker.additional_addresses)
  # FABRIC-008: the first broker in `zone`, each additional one in the zone
  # listed at its position. A length mismatch is refused by the placement
  # check above; the empty map then leaves every broker on `zone`, which the
  # module refuses for more than one.
  broker_zones = length(var.event_fabric_broker.additional_zones) == length(var.event_fabric_broker.additional_addresses) ? zipmap(
    concat([var.event_fabric_broker.internal_address], var.event_fabric_broker.additional_addresses),
    concat([var.event_fabric_broker.zone], var.event_fabric_broker.additional_zones),
  ) : {}

  # The committed stream catalogue (ADR 0100 §5, §7) — the single source of
  # seal age and peak byte rate — rather than a second copy in the tfvars.
  qip_events_config  = file("${path.module}/../event-fabric/streams.local.json")
  qip_fabricd_config = jsonencode({ environment = var.environment, region = var.region })

  # ADR 0110/0111/0112: what qip-fabricd refuses to start without outside
  # `local`. The broker's preconditions above make every one non-null.
  deployment            = var.environment
  google_tls_roots_path = local.google_roots_image_path
  admitted_peers        = local.fabric_admitted_peers
  managed_log           = var.event_fabric_broker.managed_log
  workload_certificates = {
    ca_pool              = local.cas_runtime.ca_pool
    certificate_template = local.cas_runtime.template
    lifetime_secs        = local.cas_runtime.lifetime_secs
    trust_roots_pem      = local.cas_runtime.trust_roots
  }
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

    precondition {
      condition     = var.certificate_authority != null && var.google_tls_roots_file != null
      error_message = "ledgerd is set and the environment has no certificate_authority or no google_tls_roots_file. qip-ledgerd's fabric client is mutual TLS on a CA Service certificate (ADR 0112) and refuses plaintext outside a developer's machine."
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

  # ADR 0112: the ledger's fabric client is mutual TLS on a CA Service
  # workload certificate; `ledgerd_is_placed` makes these non-null.
  google_tls_roots_path = local.google_roots_image_path
  workload_certificates = {
    ca_pool              = local.cas_runtime.ca_pool
    certificate_template = local.cas_runtime.template
    lifetime_secs        = local.cas_runtime.lifetime_secs
    trust_roots_pem      = local.cas_runtime.trust_roots
  }
}
