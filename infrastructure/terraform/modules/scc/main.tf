# Security Command Center, the part of it that is reachable from here.
#
# SCC is named twice in the platform's technology stack and had nothing behind
# it. Most of what people mean by "we use Security Command Center" is
# organisation-scoped: the activation itself, the tier that decides which
# detectors run, Event Threat Detection, Container Threat Detection, the
# built-in Security Health Analytics library, and the notification and export
# configurations that carry findings out. This configuration has a project id
# and no organisation id, deliberately — every environment is a separate
# project so a blast radius stops at a project boundary — and inventing an
# `org_id` variable would let this module look like it does things it cannot.
#
# ORGANISATION-SCOPED.md draws that line item by item. What is below is the
# part that genuinely is a project-level resource.
#
# # Custom detectors
#
# The two modules here are Security Health Analytics custom modules, and they
# are the honest contribution this repository can make to SCC. The acceptance
# suite already refuses a configuration that turns off private endpoints or
# Binary Authorization enforcement — but it checks the **repository**, and a
# cluster is changed in the console by someone who never opens the repository.
# Terraform notices at the next plan, which may be a week away and may be run
# by nobody. These notice in the project, continuously, and raise a finding
# with a severity rather than a diff.
#
# They are deliberately not copies of built-in detectors. SCC already ships
# public-IP and open-firewall detectors, and a custom module duplicating one
# produces two findings for one fact, which is how a findings list becomes
# something people close without reading.

# The cluster's admission control, turned off.
#
# `modules/binaryauthorization` exists because the cluster was set to
# `PROJECT_SINGLETON_POLICY_ENFORCE` with no policy for it to enforce, so
# Google evaluated the implicit `ALWAYS_ALLOW` default and the cluster refused
# nothing while reading as though it did. This detector is the other half of
# that failure: enforcement switched back off, on a cluster that still looks
# correct in every diagram, with unsigned images admitted from that moment on
# and no event anywhere that says so.
resource "google_scc_management_project_security_health_analytics_custom_module" "binary_authorization" {
  count = var.enable_security_command_center ? 1 : 0

  project  = var.project_id
  location = var.location

  display_name     = "qip_binary_authorization_not_enforcing"
  enablement_state = "ENABLED"

  custom_config {
    severity       = "CRITICAL"
    description    = "A GKE cluster in this project is not enforcing the project's Binary Authorization policy, so an image the deploy pipeline never signed can be admitted."
    recommendation = "Set the cluster's binary authorization evaluation mode back to PROJECT_SINGLETON_POLICY_ENFORCE. If this was intentional, it is a change to what may run in a project that trades, and belongs in a reviewed Terraform apply rather than in the console."

    resource_selector {
      resource_types = ["container.googleapis.com/Cluster"]
    }

    predicate {
      # `has()` first: a cluster with the field absent has never had
      # enforcement configured, which is the same exposure as one that has had
      # it removed. Without the guard the expression errors on that cluster and
      # the detector silently skips exactly the case it exists for.
      expression  = "!has(resource.binaryAuthorization) || resource.binaryAuthorization.evaluationMode != \"PROJECT_SINGLETON_POLICY_ENFORCE\""
      title       = "Binary Authorization is not enforcing"
      description = "The cluster admits images without checking them against the project's Binary Authorization policy."
    }
  }
}

# The control plane, made reachable.
#
# `enable_private_endpoint` is the setting that keeps the Kubernetes API off
# the public internet, and `master_authorized_networks_config` is the second
# control behind it. Turning the first off is a single field in the console and
# converts a private cluster into one whose control plane answers from
# anywhere the authorised networks permit — and the authorised network list is
# empty in every committed environment, which stops being a safe default the
# moment somebody adds a range while debugging.
resource "google_scc_management_project_security_health_analytics_custom_module" "private_endpoint" {
  count = var.enable_security_command_center ? 1 : 0

  project  = var.project_id
  location = var.location

  display_name     = "qip_cluster_control_plane_is_public"
  enablement_state = "ENABLED"

  custom_config {
    severity       = "HIGH"
    description    = "A GKE cluster in this project has a public control-plane endpoint. A private cluster with a public control plane is private in name only."
    recommendation = "Set enable_private_endpoint on the cluster. Reaching the control plane should require a path into the VPC; see modules/cluster."

    resource_selector {
      resource_types = ["container.googleapis.com/Cluster"]
    }

    predicate {
      expression  = "!has(resource.privateClusterConfig) || resource.privateClusterConfig.enablePrivateEndpoint != true"
      title       = "The cluster control plane has a public endpoint"
      description = "The Kubernetes API server is reachable from outside the VPC."
    }
  }
}

# Findings this deployment has decided not to act on.
#
# Empty by default. A mute is a decision to stop looking at something, and the
# reason it is here rather than left to the console is that a mute clicked in a
# console has no author, no date and no argument attached to it — it is
# indistinguishable a year later from a finding nobody ever saw. A mute with a
# `description` in a file that gets reviewed is a decision; the same mute in the
# console is an absence.
resource "google_scc_v2_project_mute_config" "muted" {
  for_each = var.enable_security_command_center ? var.muted_findings : {}

  project        = var.project_id
  location       = var.location
  mute_config_id = each.key
  filter         = each.value.filter
  description    = each.value.description
  type           = each.value.type
}
