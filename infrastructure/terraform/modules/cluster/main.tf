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

  # How the cluster autoscaler behaves, stated rather than inherited.
  #
  # `enabled = false` is node auto-provisioning, which is a different feature
  # from the node pool autoscaling below: it lets GKE invent node pools with
  # machine types and service accounts nobody reviewed. Everything this
  # configuration does to constrain a node — the pool's own service account,
  # shielded boot, `GKE_METADATA`, the customer-managed key — would be absent
  # from a pool the autoscaler created for itself.
  #
  # `BALANCED` is the profile, and the alternative is the reason to write it
  # down. `OPTIMIZE_UTILIZATION` removes an underused node aggressively, which
  # means draining the pods on it to pack them elsewhere. On a platform whose
  # fast path sets requests equal to limits so it is never throttled or
  # evicted, a bin-packing eviction in the middle of a session is the exact
  # event the pod specifications are written to avoid. Paying for some idle
  # capacity is the cheaper side of that trade.
  cluster_autoscaling {
    enabled             = false
    autoscaling_profile = "BALANCED"
  }

  # The Backup for GKE agent.
  #
  # A backup plan cannot be created against a cluster that is not running this,
  # so it lives here rather than in modules/backup: the plan takes its
  # dependency through the cluster id it names, and the agent is what makes
  # that dependency mean something. See modules/backup for what it covers and
  # why the edge cell journals are the reason it exists.
  addons_config {
    gke_backup_agent_config {
      enabled = true
    }

    # The persistent disk CSI driver, asserted rather than assumed.
    #
    # `infrastructure/kubernetes/base/journal-storage.yaml` provisions the
    # journal volumes through `pd.csi.storage.gke.io` and says, correctly, that
    # this module declared no `addons_config` at all — so whether that driver
    # was present was a GKE default this repository never stated. It is on by
    # default on a current cluster, and the failure if it is not is quiet and
    # confusing: the StorageClass provisions nothing, and a cell's journal claim
    # sits `Pending` for a reason that reads as capacity.
    #
    # It is also what makes a volume snapshot possible at all, so both halves of
    # the journal's durability depend on this line being true.
    gce_persistent_disk_csi_driver_config {
      enabled = true
    }
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
  #
  # Sunday 02:00-06:00 UTC: no venue this platform trades is open, which is the
  # property that matters and the reason the window is not simply "overnight"
  # for whoever wrote it.
  maintenance_policy {
    recurring_window {
      start_time = "2026-01-01T02:00:00Z"
      end_time   = "2026-01-01T06:00:00Z"
      recurrence = "FREQ=WEEKLY;BYDAY=SU"
    }

    # Dated periods when even the window above is refused.
    #
    # Empty by default, and it cannot be otherwise: a maintenance exclusion is
    # a fixed pair of timestamps, not a recurring rule, so there is no way to
    # express "never during a quarterly roll" or "not while the exchange is in
    # a settlement window" as a standing configuration. Something has to name
    # the dates, and the only honest place for that is a deployment that knows
    # the calendar.
    #
    # The failure this prevents is narrow and real: a control-plane or node
    # upgrade landing inside a period somebody had already decided was
    # change-frozen, because the weekly window happened to fall in it.
    dynamic "maintenance_exclusion" {
      for_each = var.maintenance_exclusions
      content {
        exclusion_name = maintenance_exclusion.key
        start_time     = maintenance_exclusion.value.start_time
        end_time       = maintenance_exclusion.value.end_time

        exclusion_options {
          scope = maintenance_exclusion.value.scope
        }
      }
    }
  }

  # Confidential computing on the nodes, and it is off.
  #
  # This is a decision rather than a default, and the reasoning is in
  # modules/data/NOT-PROVISIONED.md. Briefly: AMD SEV memory encryption on the
  # nodes is a real hardening step, and `crates/libs/qip-confidential` is not
  # confidential computing — it is statistical disclosure control, and its own
  # documentation says in its first paragraph that there is no enclave and no
  # attestation. Turning this on next to a crate with that name would let the
  # two together imply a guarantee neither provides.
  #
  # A dynamic block rather than `enabled = var.…`, because
  # `confidential_nodes` forces replacement: an absent block and a block
  # reading `enabled = false` are the same cluster, and only one of them is
  # guaranteed to stay that way across a provider upgrade. See the machine-type
  # precondition on the node pool — the families that support this are a short
  # list, and neither of this repository's configured machine types is on it.
  dynamic "confidential_nodes" {
    for_each = var.enable_confidential_nodes ? [1] : []
    content {
      enabled = true
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

  # The size at creation, and only at creation. The autoscaler owns it after
  # that — see the `ignore_changes` below, which is what stops a one-character
  # edit to `node_count` in a tfvars file destroying and recreating the whole
  # pool.
  initial_node_count = var.node_count

  # Nodes are added when a pod cannot be scheduled, and this is the change that
  # makes the API's autoscaler mean something.
  #
  # Before this, the pool was a fixed `node_count` and `qip-api`'s
  # HorizontalPodAutoscaler had `maxReplicas: 6` with nothing able to add a
  # node. The ceiling was therefore capacity rather than policy: past the point
  # the committed nodes could hold, the autoscaler's answer to load was a pod
  # in `Pending`, which looks like a scheduling problem and is a sizing one.
  #
  # Per-zone bounds rather than `total_min_node_count` / `total_max_node_count`,
  # deliberately. A regional total can be satisfied entirely inside one zone,
  # and a pool that drifts into one zone quietly falsifies what every
  # PodDisruptionBudget and `topologySpreadConstraint` in the manifests assumes
  # about having somewhere to fail to. Per-zone limits balance by construction:
  # this is a regional cluster, so the real range is three times each number.
  autoscaling {
    min_node_count = var.min_node_count
    max_node_count = var.max_node_count
  }

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

  # Both on, and the market-hours question is answered by the maintenance
  # policy above rather than by turning either off.
  #
  # `auto_upgrade` is bound by that policy: GKE defers a node upgrade to the
  # stated window, and to nothing at all inside a `maintenance_exclusion`. The
  # alternative — auto-upgrade off — is a pool that falls out of support and
  # eventually gets upgraded by Google anyway, at a time nobody chose, which is
  # strictly the worse version of the same event.
  #
  # `auto_repair` is not deferred the same way, and that is correct rather than
  # an oversight. It acts on a node that has already failed its health checks
  # for several consecutive minutes: the pods on it are not serving, so the
  # thing being protected from an untimely repair is a node that is already not
  # working. Leaving it in the pool holds capacity the autoscaler believes it
  # has.
  management {
    auto_repair  = true
    auto_upgrade = true
  }

  # Add a node before taking one away, always.
  #
  # `max_unavailable = 0` is the line that matters during a session: an upgrade
  # never reduces the pool below its current size, so the capacity a
  # HorizontalPodAutoscaler was given is the capacity it keeps. `max_surge = 1`
  # makes the upgrade slow — one node at a time across the region — and slow is
  # the correct trade for a pool whose fast path sets requests equal to limits
  # precisely so it is never squeezed.
  #
  # Raising `max_surge` shortens the upgrade and widens the window in which
  # several nodes are being drained at once. Raising `max_unavailable` above
  # zero is how an upgrade becomes an outage.
  upgrade_settings {
    max_surge       = 1
    max_unavailable = 0
  }

  lifecycle {
    # `initial_node_count` forces replacement. Without this, editing
    # `node_count` in a tfvars file — the most ordinary-looking change in this
    # repository — destroys the node pool and recreates it, draining every pod
    # in the cluster at once, in a plan whose summary reads "1 to add, 1 to
    # destroy". Once the pool exists, its size is the autoscaler's answer and
    # not this variable's.
    ignore_changes = [initial_node_count]

    # The autoscaler refuses a pool whose starting size is outside its own
    # bounds, and it refuses it at apply, after the cluster exists. Here it is
    # a plan-time failure with the three numbers in the message.
    precondition {
      condition     = var.node_count >= var.min_node_count && var.node_count <= var.max_node_count
      error_message = "node_count is ${var.node_count} per zone, outside the autoscaling bounds ${var.min_node_count}-${var.max_node_count}. The initial size must sit inside the range the autoscaler may move within."
    }

    # Confidential nodes need AMD SEV, which needs an AMD machine family. This
    # configuration's two machine types — the `n2-standard-4` default and the
    # `e2-standard-16` production uses — are both Intel, so turning the flag on
    # without also changing the machine type produces a cluster that fails at
    # apply with a message about the machine type rather than about
    # confidential computing. This says which knob was actually wrong.
    precondition {
      condition     = !var.enable_confidential_nodes || contains(["n2d", "c2d", "c3d"], split("-", var.machine_type)[0])
      error_message = "enable_confidential_nodes is true and machine_type is ${var.machine_type}, which has no AMD SEV support. Confidential nodes need an n2d, c2d or c3d machine type; see modules/data/NOT-PROVISIONED.md for whether you want them at all."
    }
  }
}
