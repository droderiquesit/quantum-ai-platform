# The tenant: one platform folder, one project per enabled environment, and
# the seed project that holds this root's own state (ADR 0118).
#
# What this root creates bills nothing while idle. A folder, a project, an
# API enablement, a budget and an organisation policy are free; the only
# stored bytes are this root's state, in the seed's bucket, which
# `scripts/bootstrap-foundation.sh` creates because state cannot bootstrap
# itself. Everything that runs — the control-plane cluster, execution nodes,
# the build pool, Cloud Run — is the platform root's, behind the owner's
# switches in `infrastructure/environments/<env>/terraform.tfvars`, and none of
# it is reachable from here.

locals {
  org_mode = var.org_id != null

  seed_project_id = "${var.prefix}-seed"

  # The state bucket the bootstrap script creates in the seed. Named from the
  # seed so the next operator can find it from the prefix alone.
  state_bucket = "${local.seed_project_id}-foundation-tfstate"

  enabled = { for name, environment in var.environments : name => environment if environment.enabled }

  # `environment` and `managed-by` are this root's, merged last so a caller
  # cannot mislabel a project as another environment's or as unmanaged.
  base_labels = {
    owner         = var.labels.owner
    "cost-center" = var.labels.cost_center
    "managed-by"  = "terraform"
  }

  folder_id = local.org_mode ? google_folder.platform[0].folder_id : null

  policies_applied = local.org_mode && var.org_policies_enabled

  # The APIs the bootstrap script turns on in the seed before this root can
  # run, adopted here so the record is in state rather than only in a script.
  seed_apis = [
    "serviceusage.googleapis.com",
    "cloudresourcemanager.googleapis.com",
    "cloudbilling.googleapis.com",
    "billingbudgets.googleapis.com",
    "orgpolicy.googleapis.com",
    "iam.googleapis.com",
    "storage.googleapis.com",
  ]

  # Every project this root holds a budget for: the seed and each enabled
  # environment. One budget per project, never one across the account, so the
  # alert names the project that is spending.
  #
  # Keyed on `local.enabled`, never on `google_project.environment` itself.
  # The keys are the same, but `terraform import` — which the bootstrap runs
  # to adopt the seed before the first plan — evaluates this map with every
  # project that is not yet in state as a wholly unknown value, and a `for`
  # over an unknown collection is unknown, so the budget's `for_each` was
  # refused as "known only after apply" on the owner's first run
  # (2026-10-11), before anything had been applied. A map whose keys come
  # from a variable is known in every walk; the unknown project numbers are
  # values, which `for_each` allows.
  budgeted = merge(
    { seed = { project_number = google_project.seed.number, project_id = google_project.seed.project_id, amount = var.seed_budget_amount } },
    { for name, environment in local.enabled : name => {
      project_number = google_project.environment[name].number
      project_id     = google_project.environment[name].project_id
      amount         = environment.budget_amount
    } },
  )
}

# --- The folder ---------------------------------------------------------------

resource "google_folder" "platform" {
  count = local.org_mode ? 1 : 0

  display_name = var.folder_display_name
  parent       = "organizations/${var.org_id}"

  # A folder holding projects cannot be deleted anyway; this makes the plan
  # say so instead of the API.
  deletion_protection = true
}

# --- Organisation policy on the folder ----------------------------------------
#
# Set on the folder, not the organisation: the owner's organisation may hold
# other things this repository did not create and must not touch
# (.claude/rules/domains/infrastructure.md). Each is inherited by every project
# below, the seed included.

locals {
  boolean_policies = local.policies_applied ? {
    # No downloaded service-account keys, ever (01-security-and-safety.md).
    # The platform root sets the same two on its project; here they also hold
    # the seed and any project somebody adds to the folder by hand.
    "iam.disableServiceAccountKeyCreation" = "no service-account key can be created, so none can be downloaded"
    "iam.disableServiceAccountKeyUpload"   = "no externally generated key can be attached to an account either"
    # Every bucket is IAM-only; an object ACL is a second permission system
    # nobody reviews.
    "storage.uniformBucketLevelAccess" = "every bucket uses uniform bucket-level access"
    # A new project gets no `default` VPC with its permissive firewall rules;
    # the platform root builds its own network (modules/network).
    "compute.skipDefaultNetworkCreation" = "no default VPC appears in a new project"
  } : {}
}

resource "google_org_policy_policy" "boolean" {
  for_each = local.boolean_policies

  name   = "folders/${local.folder_id}/policies/${each.key}"
  parent = "folders/${local.folder_id}"

  spec {
    rules {
      enforce = "TRUE"
    }
  }
}

# No VM in the folder may have an external address. Nothing in the platform
# root declares one — execution nodes, fabricd and ledgerd all omit
# `access_config` deliberately — so this turns that convention into a refusal.
resource "google_org_policy_policy" "vm_external_ip" {
  count = local.policies_applied ? 1 : 0

  name   = "folders/${local.folder_id}/policies/compute.vmExternalIpAccess"
  parent = "folders/${local.folder_id}"

  spec {
    rules {
      deny_all = "TRUE"
    }
  }
}

# Domain-restricted sharing, only when the owner names the directories.
resource "google_org_policy_policy" "allowed_member_domains" {
  count = local.policies_applied && length(var.allowed_policy_member_customer_ids) > 0 ? 1 : 0

  name   = "folders/${local.folder_id}/policies/iam.allowedPolicyMemberDomains"
  parent = "folders/${local.folder_id}"

  spec {
    rules {
      values {
        allowed_values = var.allowed_policy_member_customer_ids
      }
    }
  }
}

# --- Hierarchical firewall policy on the folder (R12-SEC-9131) ---------------
#
# A project's own VPC rules are only as strong as whoever holds
# `compute.securityAdmin` in it: one allow rule from 0.0.0.0/0 and a machine is
# on the internet. A folder policy is evaluated before every VPC rule in every
# project below it, and no project-level role can edit it, so the internet
# deny here holds whatever a project does to its own rules.
#
# It denies only what no platform path uses. Private and shared address space
# and Google's own front ends (health checks, the HTTPS load balancer's proxies,
# Identity-Aware Proxy) are handed on to the project's VPC rules with
# `goto_next`, which decide them exactly as today; every other IPv4 and IPv6
# source is denied before those rules are read. Nothing in the platform root
# exposes a VM directly — `compute.vmExternalIpAccess` above refuses the
# address — so the deny changes no working path and refuses the next one
# somebody opens by hand.

resource "google_compute_firewall_policy" "platform" {
  count = local.firewall_applied ? 1 : 0

  parent      = "folders/${local.folder_id}"
  short_name  = "${var.prefix}-platform"
  description = "Denies internet ingress to every project in the platform folder before any VPC rule is read."

  depends_on = [google_project_service.seed_compute]
}

locals {
  firewall_applied = local.org_mode && var.hierarchical_firewall_enabled

  # Handed on to the project's own rules, never allowed here.
  firewall_delegated_ranges = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "35.191.0.0/16",   # Google health checks and proxy load balancers
    "130.211.0.0/22",  # Google health checks and proxy load balancers
    "35.235.240.0/20", # Identity-Aware Proxy TCP forwarding
  ]
}

resource "google_compute_firewall_policy_rule" "delegate_private_and_google" {
  count = local.firewall_applied ? 1 : 0

  firewall_policy = google_compute_firewall_policy.platform[0].name
  description     = "Private address space and Google front ends: the project's VPC rules decide."
  priority        = 1000
  direction       = "INGRESS"
  action          = "goto_next"

  match {
    src_ip_ranges = local.firewall_delegated_ranges
    layer4_configs {
      ip_protocol = "all"
    }
  }
}

resource "google_compute_firewall_policy_rule" "deny_internet_ingress_ipv4" {
  count = local.firewall_applied ? 1 : 0

  firewall_policy = google_compute_firewall_policy.platform[0].name
  description     = "Every other IPv4 source: denied before any VPC rule can admit it."
  priority        = 2000
  direction       = "INGRESS"
  action          = "deny"
  enable_logging  = true

  match {
    src_ip_ranges = ["0.0.0.0/0"]
    layer4_configs {
      ip_protocol = "all"
    }
  }
}

resource "google_compute_firewall_policy_rule" "deny_internet_ingress_ipv6" {
  count = local.firewall_applied ? 1 : 0

  firewall_policy = google_compute_firewall_policy.platform[0].name
  description     = "Every IPv6 source: denied before any VPC rule can admit it."
  priority        = 2001
  direction       = "INGRESS"
  action          = "deny"
  enable_logging  = true

  match {
    src_ip_ranges = ["::/0"]
    layer4_configs {
      ip_protocol = "all"
    }
  }
}

resource "google_compute_firewall_policy_association" "platform" {
  count = local.firewall_applied ? 1 : 0

  name              = "${var.prefix}-platform-folder"
  firewall_policy   = google_compute_firewall_policy.platform[0].id
  attachment_target = google_folder.platform[0].name
}

# Hierarchical firewall policies are Compute Engine resources, and this root
# bills its calls to the seed (`versions.tf`), so the seed needs the Compute
# API the bootstrap script does not turn on. Kept out of `seed_apis`, whose
# list is the script's.
resource "google_project_service" "seed_compute" {
  count = local.firewall_applied ? 1 : 0

  project            = google_project.seed.project_id
  service            = "compute.googleapis.com"
  disable_on_destroy = false
}

# --- The seed project ----------------------------------------------------------
#
# Created by `scripts/bootstrap-foundation.sh` — under the organisation, or
# with no parent in no-org mode — because this root's state bucket must exist
# before `terraform init`. The script then runs `terraform import` on this
# address once, before the first plan, so the seed is adopted rather than
# created twice: labelled, budgeted and, in org mode, moved into the platform
# folder like every other project.
#
# Not an `import` block: a block re-imports nothing once the address is in
# state, but it cannot be planned against a mocked provider, and the tests
# below are the only evidence this root has before the owner applies it.

resource "google_project" "seed" {
  project_id      = local.seed_project_id
  name            = local.seed_project_id
  folder_id       = local.folder_id
  billing_account = var.billing_account_id
  labels          = merge(local.base_labels, { environment = "seed" })

  auto_create_network = false
  deletion_policy     = "PREVENT"

  lifecycle {
    # Create-only in the API, so an imported project reads back the provider
    # default and would plan a meaningless change for ever.
    ignore_changes = [auto_create_network]
  }
}

resource "google_project_service" "seed" {
  for_each = toset(local.seed_apis)

  project = google_project.seed.project_id
  service = each.key

  disable_on_destroy         = false
  disable_dependent_services = false
}

# --- One project per enabled environment ---------------------------------------

resource "google_project" "environment" {
  for_each = local.enabled

  project_id      = "${var.prefix}-${each.key}"
  name            = "${var.prefix}-${each.key}"
  folder_id       = local.folder_id
  billing_account = var.billing_account_id
  labels          = merge(local.base_labels, { environment = each.key })

  # No default VPC: the platform root's network is the only one.
  auto_create_network = false

  # A project deletion takes every key ring, bucket and audit log with it,
  # and its id can never be used again. Retiring an environment is the
  # owner's act (docs/ops/runbooks/new-project-from-scratch.md), not a
  # side-effect of flipping `enabled`.
  deletion_policy = "PREVENT"

  # The folder's policies are in force before the first project lands, so a
  # project never exists for a moment without them.
  depends_on = [
    google_org_policy_policy.boolean,
    google_org_policy_policy.vm_external_ip,
    google_org_policy_policy.allowed_member_domains,
  ]
}

resource "google_project_service" "environment" {
  for_each = {
    for pair in setproduct(keys(local.enabled), var.platform_apis) :
    "${pair[0]}/${pair[1]}" => { environment = pair[0], service = pair[1] }
  }

  project = google_project.environment[each.value.environment].project_id
  service = each.value.service

  # Never off from here: the platform root manages the same APIs, and a
  # disable from either state deletes resources the other one owns
  # (modules/services/main.tf says why that is an outage, not a cleanup).
  disable_on_destroy         = false
  disable_dependent_services = false
}

# --- Budgets -------------------------------------------------------------------
#
# A budget alerts; it does not stop spending. Each one emails the billing
# account's administrators at every threshold, on actual spend, for its own
# project only.

resource "google_billing_budget" "project" {
  for_each = local.budgeted

  billing_account = var.billing_account_id
  display_name    = "${each.value.project_id} monthly"

  budget_filter {
    projects        = ["projects/${each.value.project_number}"]
    calendar_period = "MONTH"
  }

  amount {
    specified_amount {
      # Currency omitted: the billing account's own is used, and naming a
      # different one is refused by the API.
      units = tostring(each.value.amount)
    }
  }

  dynamic "threshold_rules" {
    for_each = var.budget_thresholds
    content {
      threshold_percent = threshold_rules.value
      spend_basis       = "CURRENT_SPEND"
    }
  }
}
