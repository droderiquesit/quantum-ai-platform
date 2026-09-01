# One execution node.
#
# The Algorik blueprint's §41.2 and §41.4 describe the only workload on this
# platform that is not a container: one Rust binary on one dedicated C3 or C3D,
# supervised by systemd, with no container runtime between it and the kernel, no
# external address, and cores 2 upwards isolated so that the hot path is never
# descheduled by something that could have run elsewhere.
#
# ADR 0020 is why this module exists and is also why nothing calls it. The
# repository runs that workload as a Kubernetes Deployment today
# (`infrastructure/helm/qip/templates/edge-cell.yaml`), the migration is
# sequenced, and step 3 — standing a node up in shadow mode — requires recorded
# human approval naming that step before it begins. So this is written,
# reviewable and inert: `infrastructure/terraform/main.tf` has no `module
# "execution_node"` block, and adding one is its own change with its own plan
# and its own evidence. A module that exists is not a node that exists.
#
# # What this module can and cannot enforce
#
# Terraform can create a machine, a network path and an identity. It cannot set
# a kernel parameter. `isolcpus`, the huge pages, the absence of swap and the
# absence of a container runtime are properties of the image named in
# `boot_image`, and the startup script below **verifies** them at boot and
# refuses to start the unit when they are missing. That is the honest shape of
# the guarantee: this configuration does not make the kernel isolate anything,
# it makes a machine that did not isolate anything decline to trade. README.md
# has the full contract.
#
# # Binary Authorization has no analogue here
#
# The repository requires Binary Authorization on every deployed image, and the
# enforcement is `evaluation_mode = "PROJECT_SINGLETON_POLICY_ENFORCE"` in
# `modules/cluster/main.tf` — a GKE admission controller. Cloud Run has an
# equivalent. A bare Compute Engine instance has none: nothing sits between the
# image and the boot to evaluate a policy, because §41.4's whole point is that
# nothing sits between the binary and the kernel.
#
# This module does not enforce it and must not be read as doing so. What it can
# enforce, and does: an image pinned to one self-link rather than a family
# (`boot_image`, with a validation refusing a family), secure boot, vTPM and
# integrity monitoring. What it cannot: that the image was built from a signed
# artefact. The signing half of the chain in `.github/workflows/deploy.yml` is
# registry-side and survives the topology change; the admission half does not,
# and the substitute is the image build attesting what it packaged. That is a
# contract on the image, stated in README.md, not a control in this file.
#
# # Shadow mode is structural
#
# `shadow_mode` defaults to true, and with it on there is no venue egress rule
# and no venue-credential binding. The node is *unable* to open a venue session
# rather than configured not to. Nothing here accepts an autonomy ceiling: the
# ceiling is decided in three places already (see
# .claude/rules/01-security-and-safety.md) and a fourth would weaken all three.

locals {
  name = "qip-${var.environment}-exec-${var.node_id}"

  # The tag every rule in this module targets, and the tag the instance
  # template applies. Exported so nothing else has to guess it — a rule
  # targeting a tag nothing carries is a rule that does nothing, and it does
  # nothing silently.
  node_tag = "qip-exec-${var.node_id}"

  # The shape's vCPU count, read off the machine type rather than asked for
  # separately. Two sources for one fact disagree eventually, and the
  # disagreement here would be an `isolcpus` range naming cores the machine
  # does not have — which the kernel ignores, leaving a node that boots, passes
  # every check anybody wrote, and schedules the hot path wherever it likes.
  vcpus = tonumber(regex("[0-9]+$", var.machine_type))

  # §41.3 assigns cores 0–1 to the OS, telemetry drainer and control-plane
  # client, and everything else to an isolated core. So the isolated range is
  # "2 to the last core" and it is derived, not configured. On a 16-vCPU shape
  # this is the blueprint's literal `2-15`; on an 8-vCPU shape it is `2-7` and
  # the thread assignment in §41.3 does not fit — see the README.
  isolated_cpus = "2-${local.vcpus - 1}"

  # The venue list the binary is configured with. Present in shadow mode too:
  # `qip-edge-node` refuses an empty `QIP_VENUES`, and what the node may
  # actually reach is the firewall's business rather than this string's.
  venue_ids = join(",", sort(keys(var.venues)))

  # The venue credential is bound only where the environment could use it *and*
  # the node has been let out of shadow mode. Two conditions rather than one:
  # the first is the environment's autonomy ceiling, which `modules/secrets`
  # already applies to the fast brain, and the second is this node's own
  # standing. A node nobody has observed yet holding a credential it could
  # authenticate with is precisely the ordering ADR 0020 step 3 forbids.
  venue_credential_bound = (
    var.venue_credential_readable
    && !var.shadow_mode
    && var.venue_credential_secret_id != null
  )
}

# --- where it lives ---------------------------------------------------------

resource "google_compute_subnetwork" "node" {
  project = var.project_id
  name    = local.name
  region  = var.region
  network = var.network_id

  ip_cidr_range = var.subnet_cidr

  # The node has no external address at all, so this is the whole of its route
  # to Secret Manager. Without it the machine boots, fails to fetch the capital
  # envelope key and never serves — which is the correct failure, but a slow one
  # to diagnose.
  private_ip_google_access = true

  # "What did it talk to" is the first question after an incident on a machine
  # that holds venue sessions, and half a percent is enough to answer it.
  log_config {
    aggregation_interval = "INTERVAL_5_SEC"
    flow_sampling        = 0.5
    metadata             = "INCLUDE_ALL_METADATA"
  }
}

# Egress for a machine with no external address, in a region that has none.
#
# Off by default; see the variable for why a second NAT in the primary region is
# a conflict rather than a redundancy.
resource "google_compute_router" "egress" {
  count = var.create_egress_nat ? 1 : 0

  project = var.project_id
  name    = "${local.name}-router"
  region  = var.region
  network = var.network_id
}

resource "google_compute_router_nat" "egress" {
  count = var.create_egress_nat ? 1 : 0

  project = var.project_id
  name    = "${local.name}-nat"
  router  = google_compute_router.egress[0].name
  region  = var.region

  nat_ip_allocate_option = "AUTO_ONLY"

  # This subnetwork and no other. `ALL_SUBNETWORKS_ALL_IP_RANGES` would make
  # this NAT the egress path for anything else that later lands in the region,
  # which is a widening performed by a module that has no business widening
  # anything.
  source_subnetwork_ip_ranges_to_nat = "LIST_OF_SUBNETWORKS"

  subnetwork {
    name                    = google_compute_subnetwork.node.id
    source_ip_ranges_to_nat = ["ALL_IP_RANGES"]
  }

  log_config {
    enable = true
    filter = "ALL"
  }
}

# --- who it is --------------------------------------------------------------

resource "google_service_account" "node" {
  project = var.project_id
  # A service account id is 6 to 30 characters. `qip-exec-` is 9, so a node id
  # and environment totalling more than 21 fails at apply, after the subnet and
  # the template exist. The precondition moves that to plan time.
  account_id = "qip-exec-${var.node_id}-${var.environment}"

  lifecycle {
    precondition {
      condition     = length("qip-exec-${var.node_id}-${var.environment}") <= 30
      error_message = "The derived service account id qip-exec-${var.node_id}-${var.environment} is ${length("qip-exec-${var.node_id}-${var.environment}")} characters; Google allows 30. Shorten the node id."
    }
  }
  display_name = "qip execution node ${var.node_id} (${var.environment})"
  description  = "The identity of the ${var.node_id} execution node. Federated, never keyed."
}

# There is no `google_service_account_key` in this module and there must never
# be one. The instance authenticates as the account above through the metadata
# server, which is Workload Identity Federation by another name and leaves
# nothing on disk to steal. A downloaded key would survive the machine, and the
# machine is the only thing this identity is for.

# The node verifies the signature on every capital envelope it is handed, and
# `qip-edge-node` refuses to start without the key. This is the one grant the
# node cannot run without.
resource "google_secret_manager_secret_iam_member" "capital_envelope_key" {
  project   = var.project_id
  secret_id = var.capital_envelope_secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.node.email}"
}

# The venue credential, bound only under both conditions in `locals`. Not a
# grant this module ever makes by default, and not one it makes at all for a
# node still in shadow mode.
resource "google_secret_manager_secret_iam_member" "venue_credential" {
  count = local.venue_credential_bound ? 1 : 0

  project   = var.project_id
  secret_id = var.venue_credential_secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.node.email}"
}

# Shadow mode's whole purpose is producing evidence — a node "matching the pod's
# decisions", in ADR 0020's words — and evidence that cannot leave the machine
# is not evidence anyone can check. These two are the narrowest roles that let
# it leave.
resource "google_project_iam_member" "telemetry" {
  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.node.email}"
}

resource "google_project_iam_member" "logging" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.node.email}"
}

# Object creation and nothing else, and only where a bucket is named. See
# modules/evidence: a writer that can delete makes an append-only store a store
# nobody has deleted from yet.
resource "google_storage_bucket_iam_member" "evidence" {
  count = var.evidence_bucket == null ? 0 : 1

  bucket = var.evidence_bucket
  role   = "roles/storage.objectCreator"
  member = "serviceAccount:${google_service_account.node.email}"
}

# Deliberately no Artifact Registry grant. §41.4 says no container runtime, so
# there is no image to pull at run time — the binary arrives in `boot_image`.
# A registry reader role here would be a permission held for a fetch that never
# happens, which is how a least-privilege identity stops being one.

# --- the machine ------------------------------------------------------------

# Compact placement: the instances this group creates sit as close together as
# the zone allows.
#
# For one instance this looks like it buys nothing, and it is here for the
# replacement rather than the steady state: blue-green brings a second machine
# up beside the first, and a replacement that lands in a distant part of the
# zone silently changes the number the whole architecture is built around.
resource "google_compute_resource_policy" "placement" {
  project = var.project_id
  name    = "${local.name}-placement"
  region  = var.region

  group_placement_policy {
    collocation = "COLLOCATED"
  }
}

resource "google_compute_health_check" "node" {
  project = var.project_id
  name    = "${local.name}-health"

  # Ten seconds and three failures before the group replaces the instance.
  # Faster than that and a garbage-collection pause becomes a machine
  # replacement; slower and a node that is not serving keeps its sessions.
  check_interval_sec  = 10
  timeout_sec         = 5
  healthy_threshold   = 2
  unhealthy_threshold = 3

  http_health_check {
    port         = var.health_port
    request_path = "/health"
  }

  log_config {
    enable = true
  }
}

resource "google_compute_instance_template" "node" {
  project = var.project_id

  # `name_prefix` with `create_before_destroy`, because a template is immutable
  # once created: any change to the image, the startup script or the machine
  # type makes a new template, and the group is pointed at it. That is the
  # mechanism blue-green replacement rests on.
  name_prefix = "${local.name}-"

  machine_type = var.machine_type
  labels       = var.labels
  tags         = [local.node_tag]

  lifecycle {
    create_before_destroy = true

    precondition {
      condition     = length(var.venues) > 0
      error_message = "No venue is configured. `qip-edge-node` refuses an empty QIP_VENUES, so this node would boot, fail and restart for ever. Name the venues it is configured for — in shadow mode it still will not reach them."
    }

    precondition {
      condition     = local.vcpus >= 8
      error_message = "A shape with ${local.vcpus} vCPUs leaves fewer than six isolated cores for the twenty-three modules in §41.2. The permitted machine types all satisfy this; a new one that does not is a shape to reject rather than a check to relax."
    }
  }

  disk {
    source_image = var.boot_image
    auto_delete  = true
    boot         = true
    disk_type    = "pd-balanced"
    disk_size_gb = 100
  }

  network_interface {
    subnetwork = google_compute_subnetwork.node.id

    # gVNIC, per §41.4. The virtio driver's interrupt behaviour is the
    # difference between a tail latency this architecture can explain and one it
    # cannot.
    nic_type = "GVNIC"

    # No `access_config` block, and its absence is the control. A machine with
    # no external address cannot be reached from the internet, which is a
    # stronger statement than any firewall rule — the same argument
    # modules/network makes for the cluster's nodes.
  }

  # TIER_1 networking, per §41.4. Google refuses this tier on shapes below the
  # family's vCPU threshold, and the refusal arrives at apply. The answer to it
  # is a larger machine, not a lower tier: the tier is part of the latency
  # argument the node exists to make.
  network_performance_config {
    total_egress_bandwidth_tier = "TIER_1"
  }

  service_account {
    email = google_service_account.node.email

    # `cloud-platform` is the scope, and the IAM bindings above are the
    # permissions. Scopes are the older, coarser mechanism and narrowing them
    # here would hide which of the two is actually granting anything; the
    # account holds four roles and nothing it could reach with a wider scope.
    scopes = ["cloud-platform"]
  }

  # The placement policy above. One policy, and the API permits only one.
  resource_policies = [google_compute_resource_policy.placement.id]

  scheduling {
    # Live migration moves a running machine between hosts, and the pause it
    # takes is invisible to everything except the workload whose entire purpose
    # is microseconds. `TERMINATE` instead: the instance stops, the group
    # notices and replaces it, and a replacement is a thing the deployment can
    # see.
    on_host_maintenance = "TERMINATE"
    automatic_restart   = true
    preemptible         = false
  }

  shielded_instance_config {
    enable_secure_boot          = true
    enable_vtpm                 = true
    enable_integrity_monitoring = true
  }

  metadata = {
    # OS Login, project-wide keys blocked, serial console off. The node holds
    # venue sessions; the set of people who may open a shell on it is defined by
    # IAM and nothing else, and the serial port is the one door that bypasses
    # that.
    enable-oslogin         = "TRUE"
    block-project-ssh-keys = "TRUE"
    serial-port-enable     = "FALSE"

    startup-script = templatefile("${path.module}/templates/startup.sh.tftpl", {
      project_id                 = var.project_id
      node_id                    = var.node_id
      region                     = var.region
      venue_ids                  = local.venue_ids
      health_port                = var.health_port
      egress_endpoint            = var.egress_proxy.endpoint
      shadow_mode                = var.shadow_mode
      isolated_cpus              = local.isolated_cpus
      required_hugepages_gb      = var.required_hugepages_gb
      watchdog_seconds           = var.watchdog_seconds
      capital_envelope_secret_id = var.capital_envelope_secret_id
      venue_credential_secret_id = local.venue_credential_bound ? var.venue_credential_secret_id : ""
    })
  }
}

# The group, and the mechanism of §41.4's blue-green replacement.
#
# Zonal rather than regional: a compact placement policy is a claim about
# proximity within one zone, and a regional group would undo it.
resource "google_compute_instance_group_manager" "node" {
  project = var.project_id
  name    = local.name
  zone    = var.zone

  base_instance_name = local.name
  target_size        = var.node_count

  version {
    instance_template = google_compute_instance_template.node.id
  }

  named_port {
    name = "health"
    port = var.health_port
  }

  # Blue-green, spelled out.
  #
  # `max_surge_fixed = 1` with `max_unavailable_fixed = 0` and
  # `replacement_method = "SUBSTITUTE"` means: bring the new machine up, wait
  # for it to pass the health check, then take the old one away. The old node
  # keeps serving until the new one is proven, which is the only ordering that
  # makes a replacement reversible.
  update_policy {
    type                           = "PROACTIVE"
    minimal_action                 = "REPLACE"
    most_disruptive_allowed_action = "REPLACE"
    max_surge_fixed                = 1
    max_unavailable_fixed          = 0
    replacement_method             = "SUBSTITUTE"
  }

  auto_healing_policies {
    health_check = google_compute_health_check.node.id

    # Five minutes. The node fetches secrets, verifies the image contract and
    # opens its sessions before it answers; a shorter delay replaces machines
    # that were starting normally, and a group that recreates a booting instance
    # never converges.
    initial_delay_sec = 300
  }

  lifecycle {
    # The group's size is a deployment decision, not a drift to correct: an
    # operator who scales to zero during an incident should not have Terraform
    # scale it back on the next apply.
    ignore_changes = [target_size]
  }
}

# --- what it may reach ------------------------------------------------------

# Everything out is denied. Declared at a priority below the allows, so a rule
# that is deleted leaves the node unable to connect rather than able to reach
# anything.
resource "google_compute_firewall" "deny_egress" {
  project = var.project_id
  name    = "${local.name}-deny-egress"
  network = var.network_id

  direction = "EGRESS"
  priority  = 65000

  deny {
    protocol = "all"
  }

  destination_ranges = ["0.0.0.0/0"]
  target_tags        = [local.node_tag]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# Google APIs, over the restricted VIP or a Private Service Connect endpoint.
# This is how the node reads its capital envelope key and writes its telemetry,
# and it is the only path that is not optional.
resource "google_compute_firewall" "google_apis" {
  project = var.project_id
  name    = "${local.name}-google-apis"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = ["443"]
  }

  destination_ranges = [var.google_apis_range]
  target_tags        = [local.node_tag]
}

# The TLS-terminating reverse proxy, without which every outbound adapter in
# the binary refuses at construction. One rule, one address, one port: the
# proxy chooses the destination, so a wider rule here would not buy the node a
# single extra host it could ask for.
resource "google_compute_firewall" "egress_proxy" {
  project = var.project_id
  name    = "${local.name}-egress-proxy"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [tostring(var.egress_proxy.port)]
  }

  destination_ranges = [var.egress_proxy.cidr]
  target_tags        = [local.node_tag]
}

# The central plane: capital envelopes in, evidence and exposure out. A node
# that has lost this keeps working inside the envelope it already holds, which
# is the property ADR 0008 exists to provide — so this is a path the node uses,
# not one it depends on.
resource "google_compute_firewall" "central_plane" {
  count = length(var.central_plane_ranges) > 0 ? 1 : 0

  project = var.project_id
  name    = "${local.name}-central-plane"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = ["443", "8080"]
  }

  destination_ranges = var.central_plane_ranges
  target_tags        = [local.node_tag]
}

# One rule per venue, named after the venue, and **none at all in shadow mode**.
#
# This is where shadow mode stops being a claim. A node that has not been
# observed has no route to a venue: not a disabled route, not a route behind a
# flag the binary reads, no route. Turning `shadow_mode` off is what creates
# these rules, and it is a diff somebody reviews.
#
# A single rule listing every venue range and every venue port would permit each
# venue's port to every venue's address, which is a cross product nobody asked
# for and nobody would notice.
resource "google_compute_firewall" "venue" {
  for_each = var.shadow_mode ? {} : var.venues

  project = var.project_id
  name    = "${local.name}-venue-${lower(each.key)}"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [tostring(each.value.port)]
  }

  destination_ranges = [each.value.cidr]
  target_tags        = [local.node_tag]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# --- what may reach it ------------------------------------------------------

# Health checks from Google's own ranges, on the health port and nothing else.
# The managed instance group's auto-healing is what makes blue-green
# replacement wait for a working machine, and without this rule every probe
# fails, every new instance is judged unhealthy, and the group replaces
# machines for ever.
resource "google_compute_firewall" "health_checks" {
  project = var.project_id
  name    = "${local.name}-health-checks"
  network = var.network_id

  direction = "INGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [tostring(var.health_port)]
  }

  # The documented ranges Google health checks originate from.
  source_ranges = ["35.191.0.0/16", "130.211.0.0/22"]
  target_tags   = [local.node_tag]
}

# Everything else inbound is denied. The VPC denies ingress by default and this
# rule changes nothing about that — it is here so a reviewer can see the posture
# on the machine that holds the venue sessions rather than infer it from a
# module two directories away.
resource "google_compute_firewall" "deny_ingress" {
  project = var.project_id
  name    = "${local.name}-deny-ingress"
  network = var.network_id

  direction = "INGRESS"
  priority  = 65100

  deny {
    protocol = "all"
  }

  source_ranges = ["0.0.0.0/0"]
  target_tags   = [local.node_tag]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}
