# One workload from the blueprint's Cloud Run catalogue — a service or a job.
#
# ADR 0022 makes the Algorik blueprint the architecture of record and its
# §41.6 names roughly seventy Rust binaries on Cloud Run and Cloud Run Jobs,
# every one of them scaling to zero, across twelve planes. ADR 0020 fixes the
# order that migration takes and forbids removing anything before the cutover
# evidence exists. This module is the target shape, built alongside the GKE
# stack rather than instead of it.
#
# **It is not wired into the root module, and that is deliberate.** An unwired
# module changes no plan, which is what makes it reviewable on its own merits
# rather than as part of a change that also moves traffic. NOT-WIRED.md says
# exactly what a later pass has to do, and what evidence ADR 0020 requires
# before that pass is allowed to happen.
#
# Seventy services are seventy chances to get one of them wrong, and the wrong
# one is the one nobody reads. So the properties that must hold for all of
# them are held here, once:
#
#   * an identity per workload, created here, holding telemetry and the
#     secrets this workload mounts and nothing else;
#   * secrets as files, never as environment values;
#   * the image pinned by digest, with Binary Authorization evaluating the
#     project policy on every deployment;
#   * every packet out through the VPC, so the firewall rules and flow logs
#     are the ones the rest of the platform already has;
#   * no ingress from the internet to the workload's own URL, under any input
#     this module accepts;
#   * a floor of zero instances unless somebody wrote down why not.
#
# What this module is not: the execution node. §41.4's one permitted VM runs
# bare under systemd on a C3, is always on, and is out of scope here — the
# `plane` variable refuses to name it for that reason.

terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

locals {
  name       = "qip-${var.environment}-${var.name}"
  is_service = var.kind == "service"
  is_job     = var.kind == "job"

  # A trust zone is the plane's own unless the caller says otherwise. See the
  # variable: a value written twice is a value that will disagree with itself.
  trust_zone = coalesce(var.trust_zone, var.plane)

  # Cloud Run has two ingress settings that are not the internet, and this is
  # the closed one of the two. There is no input to this module that produces
  # INGRESS_TRAFFIC_ALL; see `ingress_posture` for why that value is absent
  # rather than merely defaulted away from.
  ingress = var.ingress_posture == "public-edge" ? "INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER" : "INGRESS_TRAFFIC_INTERNAL_ONLY"

  # Where the mounted secrets appear. The same directory the CSI driver
  # projects into on GKE, so a binary moved from a pod to a service reads the
  # same paths it read before.
  secret_root = "/var/run/secrets/qip"

  # Each secret gets a directory of its own, because Cloud Run mounts a volume
  # at a path and two volumes may not share one. The process never learns
  # that: it reads the path in the generated _FILE variable.
  secret_files = {
    for key, mount in var.secret_mounts :
    key => "${local.secret_root}/${key}/${mount.file_name}"
  }

  # The environment the container actually gets: the caller's non-secret
  # settings, plus one path per mounted secret. Values never appear here.
  environment = merge(
    var.env,
    {
      for key, mount in var.secret_mounts :
      mount.env_file_variable => local.secret_files[key]
    },
  )

  labels = merge(
    var.labels,
    {
      "qip-plane"         = var.plane
      "qip-trust-zone"    = local.trust_zone
      "qip-traffic-class" = var.traffic_class
      "qip-workload"      = var.name
    },
  )
}

# --- the workload's own identity --------------------------------------------

# One service account per workload, and never the project's default compute
# identity.
#
# The default account is shared by everything in the project that does not name
# one; a grant given to it for one workload is a grant given to all of them,
# and `modules/secrets` already records what that cost when the portal was
# deployed under it. With seventy workloads coming, an account per workload is
# the difference between a compromise that holds one service's permissions and
# one that holds the catalogue's.
#
# It is also half of the rule that customer traffic and trading traffic never
# share an identity: they cannot, because no two workloads share one at all.
resource "google_service_account" "workload" {
  project = var.project_id

  # 6 to 30 characters, enforced by Google at apply time — which is after the
  # image has been built and pushed. The precondition moves that to plan time,
  # exactly as `modules/edge-cell` does for a cell id.
  account_id   = "qip-${var.name}-${var.environment}"
  display_name = "qip ${var.name} (${var.environment})"
  description  = "Runs the ${var.name} ${var.kind} on Cloud Run, in the ${var.plane} plane."

  # Every cross-variable refusal in this module is here rather than in a
  # `validation` block, for one reason: this resource exists whether the
  # workload is a service or a job, so a bad combination is refused in the plan
  # either way. A precondition fails during plan, so `terraform plan` on a
  # rejected configuration prints the message and produces no diff.
  lifecycle {
    precondition {
      condition     = length("qip-${var.name}-${var.environment}") <= 30
      error_message = "The derived service account id qip-${var.name}-${var.environment} is ${length("qip-${var.name}-${var.environment}")} characters; Google allows 30. Shorten the workload name."
    }

    # The public edge carries customer traffic and nothing else.
    #
    # This is the load-balancer half of the separation the blueprint requires:
    # a trading workload that is reachable through the customer edge shares a
    # load balancer, a WAF configuration and an ingress path with the console,
    # and the day one of those is loosened for a browser it is loosened for the
    # order path too.
    precondition {
      condition     = var.ingress_posture != "public-edge" || var.traffic_class == "customer"
      error_message = "A ${var.traffic_class} workload may not sit on the public edge. Customer traffic and trading traffic do not share a load balancer or a route; put this one behind the internal posture and give the customer-facing service its own deployment."
    }

    # Said again for the trading class specifically, and deliberately so.
    #
    # The rule above already implies it. Two checks rather than one is the
    # house habit for a boundary that matters: the day somebody adds a fourth
    # traffic class and relaxes the first condition to admit it, this one still
    # refuses to publish the order path.
    precondition {
      condition     = var.traffic_class != "trading" || var.ingress_posture == "internal"
      error_message = "The trading path is reachable from inside the VPC only. There is no ingress posture that publishes it."
    }

    # A job has no URL to publish.
    precondition {
      condition     = var.kind == "service" || var.ingress_posture == "internal"
      error_message = "A Cloud Run job answers no requests, so an ingress posture other than internal describes something that does not exist. Leave it at the default."
    }

    # A floor above zero needs a reason in the file, not in a review comment.
    precondition {
      condition     = var.min_instances == 0 || trimspace(var.always_on_justification) != ""
      error_message = "min_instances is ${var.min_instances}, which bills a warm instance whether or not a request arrives. Set always_on_justification to why this workload cannot start cold, or leave the floor at zero."
    }

    # A justification with nothing to justify is a comment that will outlive
    # the reason for it and be read as approval by whoever raises the floor
    # next.
    precondition {
      condition     = trimspace(var.always_on_justification) == "" || var.min_instances > 0
      error_message = "always_on_justification is set but min_instances is zero. Remove the justification, or raise the floor it was written for."
    }

    precondition {
      condition     = var.max_instances >= var.min_instances
      error_message = "max_instances (${var.max_instances}) is below min_instances (${var.min_instances}); Cloud Run would refuse this at apply."
    }

    # The venue credential belongs to the trading zone and reaches nothing
    # else.
    #
    # `modules/secrets` already makes it unreadable in an environment whose
    # ceiling is paper trading. This is the neighbouring failure: an
    # environment where it is readable, and a customer-facing workload mounting
    # it because the mount list was copied from the service next door. Matched
    # on the secret's own prefix rather than a substring of anything, because
    # this repository has already been bitten by a check that matched a
    # neighbouring token.
    precondition {
      condition = var.traffic_class != "customer" || alltrue([
        for mount in values(var.secret_mounts) :
        !startswith(mount.secret_id, "qip-venue-credential")
      ])
      error_message = "A customer-facing workload may not read the venue credential. Customer traffic and trading traffic share no credential; the workload that needs it is in the trading class and is reached over the VPC."
    }

    # Two mounts producing the same file path silently leave one of the two
    # secrets unreadable, and the process fails later looking like a bad
    # credential rather than a bad mount.
    precondition {
      condition     = length(values(local.secret_files)) == length(distinct(values(local.secret_files)))
      error_message = "Two secret mounts resolve to the same file path. Give each mount its own key and file name."
    }

    # The generated _FILE variables must not collide with the caller's own
    # settings: merge would silently keep one, and the one it keeps would be
    # the caller's plaintext.
    precondition {
      condition = length(setintersection(
        keys(var.env),
        [for mount in values(var.secret_mounts) : mount.env_file_variable],
      )) == 0
      error_message = "An entry in `env` has the same name as a variable generated for a mounted secret. Remove it from `env`; the path is written from secret_mounts."
    }
  }
}

# The minimum every workload needs beyond its own secrets, and no more.
#
# The same two grants `modules/secrets` gives the GKE deployables, for the same
# reason: a workload that cannot write telemetry is a workload nobody can
# operate, and everything past that is a decision about one particular service
# rather than about all of them. This module deliberately takes no
# `additional_roles` parameter — a list of extra roles on a module instantiated
# seventy times is a place for a wide grant to arrive quietly. A workload that
# needs to read a bucket gets that grant where the bucket is, named, in a file
# somebody reviews.
resource "google_project_iam_member" "telemetry" {
  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.workload.email}"
}

resource "google_project_iam_member" "logging" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.workload.email}"
}

# Read exactly the secrets this workload mounts.
#
# Scoped to the secret rather than granted at the project, which is the
# difference between a workload that can read its own credential and one that
# can read every credential in the deployment. No secret here is created by
# this module and none has a value in Terraform.
resource "google_secret_manager_secret_iam_member" "mounted" {
  for_each = var.secret_mounts

  project   = var.project_id
  secret_id = each.value.secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.workload.email}"
}

# --- the service ------------------------------------------------------------

resource "google_cloud_run_v2_service" "workload" {
  count = local.is_service ? 1 : 0

  project  = var.project_id
  name     = local.name
  location = var.region
  ingress  = local.ingress
  labels   = local.labels

  # A service deleted by a plan nobody read is an outage with a Terraform
  # commit for a cause. Removing one is then a deliberate two-step.
  deletion_protection = true

  # Binary Authorization, evaluating the project's policy — the same policy
  # `modules/binaryauthorization` sets to REQUIRE_ATTESTATION with
  # ENFORCED_BLOCK_AND_AUDIT_LOG, and the same attestor the pipeline signs
  # with. `use_default` rather than a policy named here, so that a workload
  # cannot be given a policy of its own that admits what the platform's
  # refuses. `breakglass_justification` is deliberately never set: it deploys
  # an image the policy rejects and it is the one field that turns this control
  # off from inside a service definition.
  binary_authorization {
    use_default = true
  }

  template {
    service_account = google_service_account.workload.email

    # Direct VPC egress, and all of it.
    #
    # PRIVATE_RANGES_ONLY sends anything with a public destination straight out
    # of Google's edge, bypassing the VPC — so the egress firewall, Cloud NAT's
    # single set of addresses and the flow logs all stop describing what the
    # workload actually did. There is no variable for that here: a workload
    # that needs a different arrangement is a change to this file, argued.
    vpc_access {
      egress = "ALL_TRAFFIC"

      network_interfaces {
        network    = var.egress_network
        subnetwork = var.egress_subnet
      }
    }

    scaling {
      min_instance_count = var.min_instances
      max_instance_count = var.max_instances
    }

    max_instance_request_concurrency = var.concurrency
    timeout                          = "${var.request_timeout_seconds}s"
    execution_environment            = "EXECUTION_ENVIRONMENT_GEN2"
    encryption_key                   = var.encryption_key

    containers {
      image = var.image_digest

      ports {
        container_port = var.container_port
      }

      resources {
        limits = {
          cpu    = var.cpu
          memory = var.memory
        }

        # Throttle the CPU between requests wherever the workload scales to
        # zero, which is the arrangement that makes scale-to-zero worth having.
        # A workload with a floor is one that was argued for, and it keeps its
        # CPU.
        cpu_idle          = var.min_instances == 0
        startup_cpu_boost = true
      }

      dynamic "env" {
        for_each = local.environment

        content {
          name  = env.key
          value = env.value
        }
      }

      dynamic "volume_mounts" {
        for_each = var.secret_mounts

        content {
          name       = volume_mounts.key
          mount_path = "${local.secret_root}/${volume_mounts.key}"
        }
      }

      # Nothing is routed here until the process says it is ready, and what
      # these binaries report at /health is real readiness — storage proven
      # writable, ports bound — rather than liveness. A process that reported
      # healthy and then discovered its journal had nowhere to go was running
      # with no record for however long that took.
      startup_probe {
        initial_delay_seconds = 2
        period_seconds        = 3
        timeout_seconds       = 2
        failure_threshold     = 10

        http_get {
          path = var.health_path
          port = var.container_port
        }
      }

      liveness_probe {
        period_seconds    = 30
        timeout_seconds   = 5
        failure_threshold = 3

        http_get {
          path = var.health_path
          port = var.container_port
        }
      }
    }

    # Secrets as files. The value never enters the environment, and the
    # environment carries the path instead.
    dynamic "volumes" {
      for_each = var.secret_mounts

      content {
        name = volumes.key

        secret {
          secret = volumes.value.secret_id

          # 0400 in octal: readable by the identity the container runs as, and
          # by nothing else on the filesystem.
          default_mode = 256

          items {
            path    = volumes.value.file_name
            version = volumes.value.version
            mode    = 256
          }
        }
      }
    }
  }

  # One revision serving, and it is the one that was just deployed. A split
  # would mean two digests answering the same URL, and an incident where the
  # answer to "which image served this request" is "one of these two".
  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }
}

# Who may call the service. Nothing, unless the caller names someone.
resource "google_cloud_run_v2_service_iam_member" "invokers" {
  for_each = local.is_service ? toset(var.invokers) : toset([])

  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.workload[0].name
  role     = "roles/run.invoker"
  member   = each.value
}

# --- the job ----------------------------------------------------------------

resource "google_cloud_run_v2_job" "workload" {
  count = local.is_job ? 1 : 0

  project  = var.project_id
  name     = local.name
  location = var.region
  labels   = local.labels

  binary_authorization {
    use_default = true
  }

  template {
    parallelism = var.task_parallelism
    task_count  = var.task_count

    template {
      service_account = google_service_account.workload.email
      max_retries     = var.task_max_retries
      timeout         = "${var.task_timeout_seconds}s"
      encryption_key  = var.encryption_key

      vpc_access {
        egress = "ALL_TRAFFIC"

        network_interfaces {
          network    = var.egress_network
          subnetwork = var.egress_subnet
        }
      }

      containers {
        image = var.image_digest

        resources {
          limits = {
            cpu    = var.cpu
            memory = var.memory
          }
        }

        dynamic "env" {
          for_each = local.environment

          content {
            name  = env.key
            value = env.value
          }
        }

        dynamic "volume_mounts" {
          for_each = var.secret_mounts

          content {
            name       = volume_mounts.key
            mount_path = "${local.secret_root}/${volume_mounts.key}"
          }
        }
      }

      dynamic "volumes" {
        for_each = var.secret_mounts

        content {
          name = volumes.key

          secret {
            secret       = volumes.value.secret_id
            default_mode = 256

            items {
              path    = volumes.value.file_name
              version = volumes.value.version
              mode    = 256
            }
          }
        }
      }
    }
  }
}

# Who may run the job. The same empty default as a service's invokers: a job
# nobody can start is a scheduling problem, and one anybody can start is not.
resource "google_cloud_run_v2_job_iam_member" "invokers" {
  for_each = local.is_job ? toset(var.invokers) : toset([])

  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_job.workload[0].name
  role     = "roles/run.invoker"
  member   = each.value
}
