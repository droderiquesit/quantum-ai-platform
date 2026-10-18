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

  # One machine per stable address, and only where a zone is named. The
  # template alone started nothing: this module declared a template and two
  # DNS records and no instance, so `enabled = true` produced a name that
  # resolved to an address no machine held (W4.2). `broker_internal_addresses`
  # admits one broker on the local fabric and two or three only with ADR
  # 0111's managed log (variables.tf), whose Spanner lease makes them one
  # leader and its followers rather than several writers of one chain.
  brokers = var.enabled && var.zone != "" ? toset(var.broker_internal_addresses) : toset([])
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

  # The journal (ADR 0100 §3: fsync before acknowledgement). Its own disk,
  # because the startup script used to name `/dev/sdb` while the template
  # declared a boot disk and nothing else, so the journal device the broker
  # was told to use did not exist. Named by `device_name`, so the guest sees
  # it at `/dev/disk/by-id/google-qip-journal` whatever order the kernel
  # enumerates disks in. Not auto-deleted: a machine replaced is not a
  # journal discarded. Labelled `qip_journal` so the backup module's
  # snapshot schedule is the one the execution node's journal uses.
  disk {
    boot         = false
    auto_delete  = false
    device_name  = "qip-journal"
    disk_type    = var.disk_type
    disk_size_gb = var.journal_disk_size_gb

    # Provisioned rather than left to the type's default, because every
    # acknowledgement waits on this disk's fsync (ADR 0100 §3): a journal
    # sized by capacity alone inherits whatever performance Google derives
    # from its size, and an append rate the catalogue promises is then a
    # number nothing provisioned (FABRIC-085).
    provisioned_iops       = var.journal_provisioned_iops
    provisioned_throughput = var.journal_provisioned_throughput_mibps

    labels = {
      component   = "event-fabric-broker"
      qip_journal = "true"
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

  # `startup-script`, which the guest agent on every Google image runs. This
  # was `user-data`, which only a cloud-init image reads, so on the images
  # this platform bakes the script never ran at all.
  metadata = {
    startup-script = templatefile("${path.module}/startup.sh.tftpl", {
      qip_events_config          = base64encode(var.qip_events_config)
      qip_fabricd_config         = base64encode(var.qip_fabricd_config)
      qip_storage_journal_device = var.qip_storage_journal_device
      broker_port                = var.broker_port
      deployment                 = var.deployment
      region                     = var.region
      cas_ca_pool                = var.workload_certificates.ca_pool
      cas_certificate_template   = var.workload_certificates.certificate_template
      cas_lifetime_secs          = var.workload_certificates.lifetime_secs
      cas_trust_roots_base64     = base64encode(var.workload_certificates.trust_roots_pem)
      google_tls_roots_path      = var.google_tls_roots_path
      admitted_peers             = join(",", var.admitted_peers)
      managed_log_database       = var.managed_log == null ? "" : var.managed_log.spanner_database
      managed_log_lease_ttl_ms   = var.managed_log == null ? 0 : var.managed_log.lease_ttl_ms
    })
    block-project-ssh-keys = "true"
  }

  # The zone's tag first: `modules/trust-zones` writes every allow and
  # deny rule for the control-fabric zone against it, and a machine without
  # it is outside the boundary the zone declares.
  tags = concat(var.network_tags, ["event-fabric-broker", "internal-only"])

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
  }

  # SEC-070: Fabric compute boots shielded or not at all. The broker is the
  # sole writer of every partition's batch chain (ADR 0100 §2), so a boot
  # chain nobody can attest is a chain of custody nobody can attest either.
  shielded_instance_config {
    enable_secure_boot          = true
    enable_vtpm                 = true
    enable_integrity_monitoring = true
  }

  lifecycle {
    create_before_destroy = true
  }
}

# FABRIC-089: each broker's address is a reserved internal address in the
# control-fabric subnet, not only a literal handed to the interface. A literal
# on `network_ip` is held for exactly as long as the machine exists: replace
# the broker and the address is free for anything in the subnet to take while
# the A record still names it. Reserved here, the address outlives the
# machine, and Google refuses to hand it to anything else.
resource "google_compute_address" "event_fabric_broker" {
  for_each = local.brokers

  name         = "qip-event-fabric-broker-${replace(each.value, ".", "-")}"
  project      = var.project_id
  region       = var.region
  subnetwork   = var.subnet_id
  address_type = "INTERNAL"
  purpose      = "GCE_ENDPOINT"
  address      = each.value
  description  = "Stable internal address of an event-fabric broker; the fabric SRV/A records name it."
}

# The brokers: one machine per stable address the DNS records name. Not a
# managed instance group: a group's surge creates a replacement before
# retiring the original, outside any lease the broker takes, and on the local
# fabric two brokers on one chain are two writers (ADR 0100 §2). With the
# managed log the region's two or three instances contend for one Spanner
# lease and only its holder writes (ADR 0111). All share `var.zone` today —
# spreading them across zones (FABRIC-008) is a variable this module does not
# have yet.
resource "google_compute_instance_from_template" "event_fabric_broker" {
  for_each = local.brokers

  name    = "qip-event-fabric-broker-${replace(each.value, ".", "-")}"
  project = var.project_id
  zone    = var.zone

  source_instance_template = google_compute_instance_template.event_fabric_broker[0].self_link_unique

  network_interface {
    network    = var.network_id
    subnetwork = var.subnet_id
    network_ip = google_compute_address.event_fabric_broker[each.value].address
    stack_type = "IPV4_ONLY"
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
