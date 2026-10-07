# Event Fabric Broker module.
#
# ADR 0100 §2 assigns qip-fabricd the sole writer role of every partition's
# batch chain. ADR 0100 §3 specifies RF1 with fsync before acknowledgement,
# static leader epoch, and no consensus replication (blocked on C2).
# FABRIC-008 requires brokers on dedicated Compute Engine VMs spread across
# three zones in a region. FABRIC-089 requires stable internal addresses and
# no public endpoint. FABRIC-087 requires bootstrap through Cloud DNS SRV records.

# The provider constraint is in versions.tf and the outputs in outputs.tf.
# This file carried a second copy of each, which Terraform refuses outright
# ("Duplicate required providers configuration", "Duplicate output
# definition"), so the module could not be initialised, validated or planned.

locals {
  broker_count = var.enabled ? 1 : 0 # FABRIC-086 blocked(C2) = RF1 for now
}

# FABRIC-089: Brokers have stable internal addresses and no public endpoint.
# Instance template for event-fabric brokers with no external IP.
resource "google_compute_instance_template" "event_fabric_broker" {
  count       = local.broker_count
  name_prefix = "qip-event-fabric-broker-"
  description = "Event fabric broker instance template, no external IP"

  machine_type = var.machine_type

  service_account {
    email  = var.service_account_email
    scopes = ["https://www.googleapis.com/auth/cloud-platform"]
  }

  # An instance template has no `boot_disk` block — that is
  # `google_compute_instance`'s — and requires at least one `disk`. This is
  # the same boot disk the block described, in the shape the provider
  # accepts: the same image, type, size and label.
  disk {
    boot         = true
    auto_delete  = true
    source_image = var.boot_image
    disk_type    = var.disk_type
    disk_size_gb = var.disk_size_gb

    labels = {
      component = "event-fabric-broker"
    }
  }

  # No external IP (FABRIC-089): an external address exists only where an
  # `access_config` block is declared, and none is. `access_config = []` was
  # not valid configuration — it is a block, not an argument — and the
  # attribute is `subnetwork`, not `subnet`.
  network_interface {
    network    = var.network_id
    subnetwork = var.subnet_id
    stack_type = "IPV4_ONLY"
  }

  metadata = {
    user-data = base64encode(templatefile("${path.module}/startup.sh.tftpl", {
      qip_events_config          = base64encode(var.qip_events_config)
      qip_fabricd_config         = base64encode(var.qip_fabricd_config)
      qip_storage_journal_device = var.qip_storage_journal_device
      broker_instance_name       = "qip-fabricd"
    }))
  }

  tags = ["event-fabric-broker", "internal-only"]

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
  }

  lifecycle {
    create_before_destroy = true
  }
}

# FABRIC-087: Clients bootstrap from Cloud DNS SRV records.
# Create internal DNS name for broker discovery via SRV records.
resource "google_dns_record_set" "event_fabric_broker_srv" {
  count = local.broker_count

  # Fully qualified, as Cloud DNS requires and the provider refuses at plan
  # without: `dns_zone_name` is the domain without its trailing dot, as
  # `modules/dns-zone`'s `domain` is, and the dot is added here.
  name         = "qip-event-fabric-broker.${var.dns_zone_name}."
  type         = "A"
  ttl          = 300
  managed_zone = var.managed_zone
  rrdatas      = var.broker_internal_addresses
}

# FABRIC-087: SRV record for broker discovery.
resource "google_dns_record_set" "event_fabric_broker_srv_tcp" {
  count = local.broker_count

  name         = "_qip-event-fabric._tcp.${var.dns_zone_name}."
  type         = "SRV"
  ttl          = 300
  managed_zone = var.managed_zone
  rrdatas = [
    # The target is the A record above, by its fully qualified name.
    "0 10 ${var.broker_port} qip-event-fabric-broker.${var.dns_zone_name}."
  ]
}
