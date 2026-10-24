# The development and research fleet (ADR 0102), first slice: one Cloud Run
# Job, the one identity it runs as, and the one bucket it reads packets from
# and writes outputs to.
#
# This is a root of its own, with its own state, on purpose. ADR 0098 asked for
# the fleet's resources to sit where `terraform plan` on the platform's root
# cannot propose to change them and the reverse. The project boundary ADR 0098
# wanted is gone (the owner chose the dev project), so the fence is what the
# worker identity is *not* able to do, and that is checkable by listing its
# bindings: `output.worker_roles` is that list and `tests/fleet.tftest.hcl`
# fails if it grows.
#
# The Job's egress is the VPC below, which reaches Google APIs and nothing
# else (ADR 0102 amendment of 2026-10-11).
#
# Nothing here touches the trading platform. No Secret Manager, KMS, Artifact
# Registry write, Cloud Run, Binary Authorization or git right is granted to the
# worker, so a worker cannot start a worker, read the venue credential, or push.

terraform {
  required_version = ">= 1.9.0"

  # State lives in the fleet's own bucket (`<project>-fleet-tfstate`), passed
  # at init with `-backend-config`. Not the platform's state bucket: an
  # identity that plans the fleet then needs no access to the platform's
  # state, and a plan here cannot see the platform's resources at all.
  backend "gcs" {
    prefix = "fleet"
  }

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

provider "google" {
  project = var.project_id
  region  = var.region
}

locals {
  labels = {
    env         = var.environment
    service     = "fleet"
    owner       = var.owner
    cost-center = var.cost_center
  }

  # The whole of the worker's project-level authority. Vertex AI, and read-only
  # telemetry (ADR 0102 amendment of 2026-10-11, item 3), and nothing else.
  # `roles/monitoring.viewer` is held whatever `worker_may_call_models` says: it
  # reads and cannot spend, so the harder stop has nothing in it to remove.
  # Object access to the bucket is bound on the bucket below, never on the
  # project, so it cannot reach another bucket.
  worker_project_roles = concat(
    ["roles/monitoring.viewer"],
    var.worker_may_call_models ? ["roles/aiplatform.user"] : [],
  )

  # The worker's bucket authority, narrowed by object prefix (ADR 0102
  # amendment, item 3). It reads its packets and the kill switch, and it
  # writes only under `output/` and `ledger/`. It holds no right on `HALT`
  # beyond reading it and none on `packets/` beyond reading, so a worker that
  # was talked into deleting the switch or rewriting a sibling's packet has no
  # permission to. The ledger prefix also carries the list right, because the
  # day's total is a listing of it; the list request names its prefix in an
  # attribute and not in a resource name, which is why the writes condition
  # has a third clause.
  bucket_objects = "projects/_/buckets/${var.project_id}-fleet/objects/"
  worker_reads_condition = join(" || ", [
    "resource.name.startsWith(\"${local.bucket_objects}packets/\")",
    "resource.name == \"${local.bucket_objects}HALT\"",
  ])
  worker_writes_condition = join(" || ", [
    "resource.name.startsWith(\"${local.bucket_objects}output/\")",
    "resource.name.startsWith(\"${local.bucket_objects}ledger/\")",
    "api.getAttribute(\"storage.googleapis.com/objectListPrefix\", \"\").startsWith(\"ledger/\")",
  ])

  # The only addresses the worker's subnet may reach (ADR 0102 amendment, item
  # 2): `private.googleapis.com`, which Private Google Access serves, and
  # nothing on the internet.
  google_apis_range     = "199.36.153.8/30"
  google_apis_addresses = ["199.36.153.8", "199.36.153.9", "199.36.153.10", "199.36.153.11"]

  # Vertex AI, Cloud Run, the bucket, the identity, the network and its DNS
  # zone. Enabling is a human act by default (`enable_apis = false`); see
  # README.md.
  apis = ["aiplatform.googleapis.com", "run.googleapis.com", "storage.googleapis.com", "iam.googleapis.com", "artifactregistry.googleapis.com", "compute.googleapis.com", "dns.googleapis.com", "monitoring.googleapis.com"]
}

resource "google_project_service" "api" {
  for_each = var.enable_apis ? toset(local.apis) : toset([])

  service            = each.value
  disable_on_destroy = false
}

resource "google_service_account" "worker" {
  account_id   = "fleet-worker"
  display_name = "Fleet worker (ADR 0102): Vertex AI and one bucket, nothing else"
}

# Absent on purpose: google_service_account_key. The credential is the metadata
# server's short-lived token (01-security-and-safety.md).

resource "google_project_iam_member" "worker" {
  for_each = toset(local.worker_project_roles)

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.worker.email}"
}

resource "google_storage_bucket" "fleet" {
  name                        = "${var.project_id}-fleet"
  location                    = var.region
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
  labels                      = local.labels

  # Bounded retention. HALT is deliberately outside every prefix named here,
  # because a kill switch an age rule can delete is a switch that quietly
  # un-halts the fleet a month later.
  lifecycle_rule {
    condition {
      age            = var.retention_days
      matches_prefix = ["packets/", "output/", "ledger/"]
    }
    action {
      type = "Delete"
    }
  }
}

resource "google_storage_bucket_iam_member" "worker_reads" {
  bucket = google_storage_bucket.fleet.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.worker.email}"

  condition {
    title       = "packets-and-halt-read-only"
    description = "Read the packets and the kill switch; nothing else in the bucket."
    expression  = local.worker_reads_condition
  }
}

resource "google_storage_bucket_iam_member" "worker_writes" {
  bucket = google_storage_bucket.fleet.name
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.worker.email}"

  condition {
    title       = "output-and-ledger-only"
    description = "Write under output/ and ledger/ (and list ledger/ for the day's total); no right on packets/ or HALT."
    expression  = local.worker_writes_condition
  }
}

# Egress restricted to Google APIs (ADR 0102 amendment, item 2). The Job's
# traffic all goes through this VPC, whose subnet has Private Google Access, and
# whose firewall admits TCP 443 to `private.googleapis.com` and denies every
# other destination. A prompt-injected worker therefore has no route to an
# attacker's host; what it can reach is Google's own APIs under its own
# identity, which the bindings above bound.
#
# The default rules of a new network allow all egress, so the deny is a rule of
# its own at a lower priority than the one allow. No custom route is declared:
# the network's default route reaches the Private Google Access range, and the
# acceptance suite refuses a custom route here. `googleapis.com` resolves to
# the restricted range from a private zone on this network only.
resource "google_compute_network" "fleet" {
  name                    = "fleet"
  auto_create_subnetworks = false

  depends_on = [google_project_service.api]
}

resource "google_compute_subnetwork" "fleet" {
  name                     = "fleet-${var.region}"
  region                   = var.region
  network                  = google_compute_network.fleet.id
  ip_cidr_range            = "10.77.0.0/24"
  private_ip_google_access = true
}

resource "google_compute_firewall" "allow_google_apis_egress" {
  name      = "fleet-allow-google-apis-egress"
  network   = google_compute_network.fleet.name
  direction = "EGRESS"
  priority  = 1000

  destination_ranges = [local.google_apis_range]

  allow {
    protocol = "tcp"
    ports    = ["443"]
  }
}

resource "google_compute_firewall" "deny_all_egress" {
  name      = "fleet-deny-all-egress"
  network   = google_compute_network.fleet.name
  direction = "EGRESS"
  priority  = 65000

  destination_ranges = ["0.0.0.0/0"]

  deny {
    protocol = "all"
  }
}

resource "google_dns_managed_zone" "google_apis" {
  name       = "fleet-googleapis"
  dns_name   = "googleapis.com."
  visibility = "private"
  labels     = local.labels

  private_visibility_config {
    networks {
      network_url = google_compute_network.fleet.id
    }
  }

  depends_on = [google_project_service.api]
}

resource "google_dns_record_set" "private_googleapis" {
  managed_zone = google_dns_managed_zone.google_apis.name
  name         = "private.googleapis.com."
  type         = "A"
  ttl          = 300
  rrdatas      = local.google_apis_addresses
}

resource "google_dns_record_set" "wildcard_googleapis" {
  managed_zone = google_dns_managed_zone.google_apis.name
  name         = "*.googleapis.com."
  type         = "CNAME"
  ttl          = 300
  rrdatas      = ["private.googleapis.com."]
}

# The one repository the worker image is pushed to. The worker identity holds
# no role on it: Cloud Run's own service agent pulls the image, so a worker
# can neither push an image nor read another one.
resource "google_artifact_registry_repository" "fleet" {
  repository_id = "fleet"
  location      = var.region
  format        = "DOCKER"
  description   = "The fleet worker image (ADR 0102). Pushed by a person or CI, never by a worker."
  labels        = local.labels

  # Immutable, as modules/registry's `images` repository is (R12-CICD-10203).
  # The Job runs an image pinned by digest (variables.tf refuses a tag), but a
  # tag in this repository could still be moved onto new bytes, and "the
  # worker image a person pushed" would then name whatever was pushed last.
  docker_config {
    immutable_tags = true
  }

  depends_on = [google_project_service.api]
}

# Created only once an image exists. The first apply has to create the
# repository before anything can be pushed to it, and a Job naming an image
# that is not there fails at create with a message about the image, not about
# the order.
resource "google_cloud_run_v2_job" "fleet" {
  count = var.image == null ? 0 : 1

  name                = "fleet"
  location            = var.region
  labels              = local.labels
  deletion_protection = false

  template {
    # One packet per task. The dispatcher sets the task count per run; the
    # parallelism is the ladder's current rung and is capped at 40 by variable.
    parallelism = var.parallelism
    task_count  = 1
    labels      = local.labels

    template {
      service_account = google_service_account.worker.email
      # The platform default retries a task blindly, and retrying a wrong
      # answer buys a differently wrong answer at full price (ADR 0102 §2).
      max_retries = 0
      timeout     = "${var.task_timeout_seconds}s"

      vpc_access {
        egress = "ALL_TRAFFIC"
        network_interfaces {
          network    = google_compute_network.fleet.name
          subnetwork = google_compute_subnetwork.fleet.name
        }
      }

      containers {
        image = var.image

        resources {
          limits = {
            cpu    = "1"
            memory = "512Mi"
          }
        }

        env {
          name  = "FLEET_BUCKET"
          value = google_storage_bucket.fleet.name
        }
        env {
          name  = "FLEET_DAILY_USD_CEILING"
          value = tostring(var.daily_usd_ceiling)
        }
        env {
          name  = "FLEET_SLOT_USD_CAP"
          value = tostring(var.per_slot_usd_cap)
        }
      }
    }
  }

  lifecycle {
    # The dispatcher sets these per run.
    ignore_changes = [client, client_version]
  }

  depends_on = [google_project_service.api]
}
