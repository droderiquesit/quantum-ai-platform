# What the execution node's boot image is baked on.
#
# `modules/execution-node` names an image in `boot_image` and refuses anything
# that is not a full self-link, because there is no admission controller
# between that value and a process on the hot path. Until now nothing in this
# repository produced such an image: the module's README said so under "No
# image bake exists", and `environments/dev/terraform.tfvars` carried
# `boot_image = "projects/algorik-dev/global/images/<the baked image>"` in a
# comment because no real value could be written.
#
# `.github/workflows/image.yml` is the bake. This module is the three things
# that bake needs to exist in the project before it can run, and nothing else:
#
#   * a bucket the payload is staged in, so the builder machine reads one
#     object and reaches nothing else;
#   * an identity for the builder machine holding exactly one grant — read
#     that bucket — because a builder that can do more than read its payload
#     is a machine with a token on it that can do more than build an image;
#   * a subnet with private Google access, a deny-everything egress rule and
#     one allow to the restricted VIP, so the builder has no route to the
#     internet at all.
#
# **It creates none of them unless an environment names a range.**
# `image_bake_subnet_cidr` is null everywhere, exactly as `console_egress_cidr`
# is null in an environment whose console has no route: a subnet created
# because a variable had a default is a subnet nobody decided to create. The
# workflow's preflight refuses with the file to edit rather than failing on a
# bucket that is not there.
#
# What this module deliberately does not hold: the image. An image is not a
# Terraform resource here and must not become one. Terraform would then own
# its lifecycle, and a `terraform destroy` — including `infra.yml`'s targeted
# `down` — could delete the image a node's instance template still names,
# which is a machine that cannot be replaced and a group that cannot heal.
# The bake creates the image; a person names it in the tfvars; nothing in this
# configuration can remove it.
#
# ## What nothing here reclaims, said plainly rather than discovered on a bill
#
# The staging bucket has a lifecycle rule and the payload expires. Two things
# beside it do not, and neither has an owner yet:
#
#   * **Every image the bake ever produced stays.** Roughly 20 GB each, billed
#     for as long as the project exists, including the ones from a bake that
#     was retried and the ones for a machine shape nobody chose in the end.
#     A lifecycle rule is not the answer and must not be added here: an image
#     a node's instance template names cannot be deleted by a schedule that
#     does not read the tfvars. What is needed is a person reading
#     `gcloud compute images list --filter="labels.qip-environment=<env>"`
#     against the `boot_image` values in `environments/*/terraform.tfvars`
#     and deleting the rest by hand, with the approval this repository
#     requires for deleting anything in a cloud project.
#   * **A runner that dies rather than fails leaks a builder.** `image.yml`
#     deletes the instance and the disk in steps marked `if: always()`, which
#     covers a failed step and a cancelled job — but not a runner that is
#     killed, loses its network, or has its token expire mid-job. What is left
#     is an `e2-standard-2` and a 20 GB pd-balanced disk, both labelled
#     `qip-role=image-bake`, and nothing watches for them. The cheap
#     mitigation is `--max-run-duration` with
#     `--instance-termination-action=DELETE` on the builder, which would bound
#     the instance half at Compute Engine rather than in the workflow; it is
#     not applied here because it has never been exercised against a real
#     dispatch and a wrong flag would fail the first bake at the point where
#     the machine is created. Until then the check is
#     `gcloud compute instances list --filter="labels.qip-role=image-bake"`,
#     which should be empty between bakes.

locals {
  name = "qip-${var.environment}-image-bake"

  # The tag both firewall rules target and the tag `image.yml` puts on the
  # builder. A rule targeting a tag no instance carries binds nothing and does
  # nothing silently — the failure `modules/network` records for the console's
  # own rules — so the workflow reads this name out of this module's output
  # rather than spelling it a second time.
  builder_tag = "${local.name}-builder"

  # The restricted VIP: the same /30 `modules/network`'s private zone resolves
  # every `*.googleapis.com` to, and the default `modules/trust-zones` and
  # `modules/execution-node` take. One literal per repository would be better
  # than four; four modules naming the same /30 is what exists, and this is the
  # fourth rather than a fifth spelling of it.
  restricted_vip_range = "199.36.153.8/30"
}

# --- where the payload is staged --------------------------------------------

# One object per bake, named by the payload's own sha256.
#
# Content-addressed for a reason that is not aesthetic. `image.yml`
# authenticates as `qip-infra-<env>`, whose custom storage role in
# `modules/cicd` carries `storage.objects.create` and deliberately not
# `storage.objects.delete` — so that nothing it runs can delete from the
# evidence bucket. Overwriting an object in Cloud Storage requires the delete
# permission, so a bake that wrote `payload.tar` twice would fail the second
# time with a 403 naming a permission the account is never going to hold. A
# name derived from the bytes is never overwritten, and the same payload
# uploaded twice is the same object.
#
# The bucket expires them instead, because nothing else can: the payload is a
# staging artefact whose durable record is the image's own labels and the
# run's log, and a bucket that only ever grows is a bill nobody decided on.
resource "google_storage_bucket" "payload" {
  project  = var.project_id
  name     = "${local.name}-${var.project_id}"
  location = var.region

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false

  # No versioning. A content-addressed object never changes, so a version
  # history of it is a second copy of the same bytes and a lifecycle rule that
  # never reaches the live one.
  lifecycle_rule {
    condition {
      age = var.payload_retention_days
    }
    action {
      type = "Delete"
    }
  }

  labels = var.labels
}

# --- who reads it -----------------------------------------------------------

# The builder machine's identity, and the whole of what it may do.
#
# One grant, on one bucket. Not a project-level role, not Artifact Registry:
# the runner extracts the two binaries from the attested container images and
# stages them, so the builder never talks to the registry and never needs a
# credential that could. A machine that exists for eight minutes and then
# becomes a disk image should carry the smallest token that can finish the
# job, because whatever it carries is what an image with a bug in its
# provisioning script carries too.
#
# No key, and none may ever be created. The machine authenticates through the
# metadata server.
resource "google_service_account" "builder" {
  project      = var.project_id
  account_id   = "qip-${var.environment}-image-builder"
  display_name = "qip execution-node image builder (${var.environment})"
  description  = "Runs the throwaway machine .github/workflows/image.yml bakes the execution node's boot image on. Reads one staging bucket and nothing else; it has no key."
}

resource "google_storage_bucket_iam_member" "builder_reads_the_payload" {
  bucket = google_storage_bucket.payload.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.builder.email}"
}

# --- what it may reach ------------------------------------------------------

# Its own range, private Google access on, no external address anywhere.
#
# The builder downloads one object from Cloud Storage over the restricted VIP
# and installs a Debian package the runner already put in that object. It
# fetches nothing from the internet, and the rules below are what make that a
# property of the network rather than a claim about the script: a provisioning
# script that grew a `curl https://…` would fail to connect instead of quietly
# pulling something nobody reviewed into the image a trading node boots.
resource "google_compute_subnetwork" "builder" {
  project = var.project_id
  name    = local.name
  region  = var.region
  network = var.network_id

  ip_cidr_range = var.subnet_cidr

  private_ip_google_access = true

  log_config {
    aggregation_interval = "INTERVAL_5_SEC"
    flow_sampling        = 0.5
    metadata             = "INCLUDE_ALL_METADATA"
  }
}

# Everything out is denied, at a priority below the one allow. The zone
# module's rule, copied in shape and priority rather than invented, so a
# reader comparing this subnet to a trust zone finds the same posture.
resource "google_compute_firewall" "builder_deny_egress" {
  project = var.project_id
  name    = "${local.name}-deny-egress"
  network = var.network_id

  direction = "EGRESS"
  priority  = 65000

  deny {
    protocol = "all"
  }

  destination_ranges = ["0.0.0.0/0"]
  target_tags        = [local.builder_tag]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# The one allow: TCP 443 to the restricted VIP. Cloud Storage for the payload,
# and the metadata server for the token — which is link-local and subject to
# no firewall rule at all.
resource "google_compute_firewall" "builder_google_apis" {
  project = var.project_id
  name    = "${local.name}-google-apis"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = ["443"]
  }

  destination_ranges = [local.restricted_vip_range]
  target_tags        = [local.builder_tag]
}

# Nothing reaches the builder. It serves no port, and the workflow reads its
# progress off the serial console through the Compute API rather than over the
# network. The VPC denies ingress by default and this rule changes nothing
# about that; it is here so the posture is visible on the machine rather than
# inferred from a module two directories away.
resource "google_compute_firewall" "builder_deny_ingress" {
  project = var.project_id
  name    = "${local.name}-deny-ingress"
  network = var.network_id

  direction = "INGRESS"
  priority  = 65000

  deny {
    protocol = "all"
  }

  source_ranges = ["0.0.0.0/0"]
  target_tags   = [local.builder_tag]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}
