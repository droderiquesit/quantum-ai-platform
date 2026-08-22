# The Kubernetes cluster.
#
# Private nodes, a private control plane, workload identity, shielded nodes,
# customer-managed encryption for etcd, and network policy on. Each of those
# is a line here and a class of incident that cannot happen.

resource "google_container_cluster" "primary" {
  project  = var.project_id
  name     = "qip-${var.environment}"
  location = var.region

  # The default pool is removed immediately and replaced with a managed one:
  # the default pool cannot be configured with the node settings below.
  remove_default_node_pool = true
  initial_node_count       = 1

  network    = var.network_id
  subnetwork = var.subnet_id

  # Pods get addresses from the subnet's secondary range, which is what makes
  # network policy meaningful for them.
  ip_allocation_policy {
    cluster_secondary_range_name  = var.pod_range
    services_secondary_range_name = var.service_range
  }

  private_cluster_config {
    # No public addresses on nodes at all.
    enable_private_nodes = true
    # The control plane has no public endpoint either. Reaching it needs a
    # path into the VPC.
    enable_private_endpoint = true
    master_ipv4_cidr_block  = "172.16.0.0/28"
  }

  # Even with a private endpoint, the ranges that may reach it are named. Two
  # controls rather than one, because the first is a configuration flag and
  # configuration flags get changed.
  master_authorized_networks_config {
    dynamic "cidr_blocks" {
      for_each = var.authorised_networks
      content {
        cidr_block   = cidr_blocks.value.cidr_block
        display_name = cidr_blocks.value.display_name
      }
    }
  }

  # Pod-to-pod traffic is denied unless a NetworkPolicy permits it. Without
  # this, a compromised research pod can reach the execution pod directly.
  network_policy {
    enabled  = true
    provider = "CALICO"
  }

  # Workload identity: pods authenticate as Google service accounts without a
  # key file. A key file is a credential that lives on disk and never expires.
  workload_identity_config {
    workload_pool = "${var.project_id}.svc.id.goog"
  }

  # etcd encrypted with a key we control, so revoking the key revokes access
  # to the data.
  database_encryption {
    state    = "ENCRYPTED"
    key_name = var.kms_key_id
  }

  # Binary authorisation: only images signed by our pipeline run.
  binary_authorization {
    evaluation_mode = "PROJECT_SINGLETON_POLICY_ENFORCE"
  }

  # The managed intrusion detection and vulnerability scanning.
  security_posture_config {
    mode               = "BASIC"
    vulnerability_mode = "VULNERABILITY_ENTERPRISE"
  }

  # Logging and monitoring for the control plane as well as the workloads: an
  # audit trail that omits the control plane omits exactly the events an
  # attacker would generate.
  logging_config {
    enable_components = [
      "SYSTEM_COMPONENTS",
      "WORKLOADS",
      "APISERVER",
      "CONTROLLER_MANAGER",
      "SCHEDULER",
    ]
  }

  monitoring_config {
    enable_components = [
      "SYSTEM_COMPONENTS",
      "APISERVER",
      "CONTROLLER_MANAGER",
      "SCHEDULER",
      "STORAGE",
      "POD",
      "DAEMONSET",
      "DEPLOYMENT",
      "STATEFULSET",
    ]

    managed_prometheus {
      enabled = true
    }
  }

  # Upgrades happen in a stated window rather than whenever Google decides.
  maintenance_policy {
    recurring_window {
      start_time = "2026-01-01T02:00:00Z"
      end_time   = "2026-01-01T06:00:00Z"
      recurrence = "FREQ=WEEKLY;BYDAY=SU"
    }
  }

  # Deleting a cluster that holds a live book should take deliberate effort.
  deletion_protection = true

  resource_labels = var.labels
}

resource "google_container_node_pool" "primary" {
  project  = var.project_id
  name     = "qip-${var.environment}-primary"
  location = var.region
  cluster  = google_container_cluster.primary.name

  node_count = var.node_count

  node_config {
    machine_type = var.machine_type
    disk_type    = "pd-ssd"
    disk_size_gb = 100

    # The pool's own service account, not the default compute one, which has
    # far more permission than any workload needs.
    service_account = var.service_account
    oauth_scopes    = ["https://www.googleapis.com/auth/cloud-platform"]

    # A read-only root filesystem and a verified boot chain.
    shielded_instance_config {
      enable_secure_boot          = true
      enable_integrity_monitoring = true
    }

    workload_metadata_config {
      # Pods cannot read the node's metadata server, which is how a
      # compromised pod would otherwise obtain the node's credentials.
      mode = "GKE_METADATA"
    }

    labels = var.labels
    tags   = ["qip-node"]

    metadata = {
      # The legacy metadata endpoints are an authentication bypass.
      disable-legacy-endpoints = "true"
    }
  }

  management {
    auto_repair  = true
    auto_upgrade = true
  }

  upgrade_settings {
    max_surge       = 1
    max_unavailable = 0
  }
}
