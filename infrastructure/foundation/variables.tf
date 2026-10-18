# Inputs.
#
# Three of them — `org_id`, `billing_account_id`, `prefix` — are passed at run
# time by `scripts/bootstrap-foundation.sh` and are never committed: an
# organisation id and a billing account id are account identifiers, and the
# repository holds none (.claude/rules/00-enterprise-governance.md). Every
# other input has its committed value in `terraform.tfvars` beside this file.

variable "org_id" {
  description = <<-EOT
    The numeric Google Cloud organisation id the platform folder is created
    under, or null for an account with no organisation.

    Null is the documented no-org mode, not a fallback: a personal Google
    account has no organisation, so there is no folder to create and no
    organisation policy to set. Projects are then created with no parent and
    held together by the billing account, their labels and their naming. Each
    org-only control this root would have set is listed by the
    `controls_not_applied` output, so the gap is a plan-time fact rather than
    something a reader infers.
  EOT
  type        = string
  default     = null

  validation {
    condition     = var.org_id == null || can(regex("^[0-9]{1,20}$", var.org_id))
    error_message = "org_id is the organisation's numeric id (gcloud organizations list), without the `organizations/` prefix, or null for an account with no organisation."
  }
}

variable "billing_account_id" {
  description = "The billing account every project is linked to and every budget is held on: `XXXXXX-XXXXXX-XXXXXX` (gcloud billing accounts list)."
  type        = string

  validation {
    condition     = can(regex("^[0-9A-F]{6}-[0-9A-F]{6}-[0-9A-F]{6}$", var.billing_account_id))
    error_message = "billing_account_id is three groups of six upper-case hex characters separated by hyphens, as `gcloud billing accounts list` prints it, without the `billingAccounts/` prefix."
  }
}

variable "prefix" {
  description = <<-EOT
    The project-id prefix: every project is `<prefix>-<environment>` and the
    seed is `<prefix>-seed`.

    4 to 24 characters so that the longest id, `<prefix>-stage`, stays inside
    Google's 30-character limit; a lowercase letter first and no trailing
    hyphen, which with the suffix gives an id Google accepts. Refused, never
    shortened: a prefix silently truncated is a project nobody named.
  EOT
  type        = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{2,22}[a-z0-9]$", var.prefix))
    error_message = "prefix must be 4-24 characters: a lowercase letter, then lowercase letters, digits and hyphens, not ending in a hyphen."
  }
}

variable "folder_display_name" {
  description = "Display name of the platform folder under the organisation. Ignored in no-org mode."
  type        = string
  default     = "algorik-platform"

  validation {
    condition     = can(regex("^[A-Za-z0-9][A-Za-z0-9 _-]{1,28}[A-Za-z0-9]$", var.folder_display_name))
    error_message = "A folder display name is 3-30 letters, digits, spaces, hyphens and underscores, starting and ending with a letter or digit."
  }
}

variable "environments" {
  description = <<-EOT
    Every environment the platform root knows, declared here whether or not
    it has a project. `enabled = false` declares the environment and creates
    nothing for it: no project, no budget, no bill.

    The four keys are exactly the values the platform root's
    `var.environment` validates, so a project this root creates is always one
    the platform can be pointed at, and an environment the platform refuses
    cannot be given a project here by a typo.

    `budget_amount` is a whole number in the billing account's currency.
  EOT
  type = map(object({
    enabled       = bool
    budget_amount = number
  }))

  validation {
    condition     = length(setsubtract(keys(var.environments), ["dev", "test", "stage", "prod"])) == 0 && length(var.environments) == 4
    error_message = "environments must declare exactly dev, test, stage and prod — the values the platform root's var.environment accepts — each with enabled and budget_amount."
  }

  validation {
    condition     = alltrue([for e in values(var.environments) : e.budget_amount > 0 && floor(e.budget_amount) == e.budget_amount])
    error_message = "Every budget_amount must be a positive whole number. A zero budget is not 'no limit'; it alerts on the first cent and is then ignored."
  }
}

variable "seed_budget_amount" {
  description = "Monthly budget for the seed project, which holds the foundation's state bucket and nothing that runs."
  type        = number
  default     = 5

  validation {
    condition     = var.seed_budget_amount > 0 && floor(var.seed_budget_amount) == var.seed_budget_amount
    error_message = "seed_budget_amount must be a positive whole number."
  }
}

variable "budget_thresholds" {
  description = "Fractions of each budget at which the billing account's administrators are emailed, on actual spend."
  type        = list(number)
  default     = [0.5, 0.9, 1.0]

  validation {
    condition     = length(var.budget_thresholds) > 0 && alltrue([for t in var.budget_thresholds : t > 0 && t <= 1])
    error_message = "budget_thresholds must be a non-empty list of fractions in (0, 1]."
  }
}

variable "labels" {
  description = <<-EOT
    The `owner` and `cost-center` labels every project carries. `environment`
    and `managed-by` are set by this root and cannot be overridden here.

    Values are a team or cost-centre name, never a person's address: labels
    are copied into the billing export and visible to everyone with billing
    viewer on the account.
  EOT
  type = object({
    owner       = string
    cost_center = string
  })

  validation {
    condition = alltrue([
      for v in [var.labels.owner, var.labels.cost_center] : can(regex("^[a-z][a-z0-9_-]{0,62}$", v))
    ])
    error_message = "Label values are 1-63 lowercase letters, digits, hyphens and underscores, starting with a letter (Google's rule). No @ — a label is not the place for an address."
  }
}

variable "org_policies_enabled" {
  description = <<-EOT
    Whether to set the organisation policies on the platform folder. Needs
    `roles/orgpolicy.policyAdmin` at the organisation; an owner without it
    sets this false and the `controls_not_applied` output says what is
    missing. Ignored in no-org mode, where there is nothing to set them on.
  EOT
  type        = bool
  default     = true
}

variable "allowed_policy_member_customer_ids" {
  description = <<-EOT
    Optional domain-restricted sharing: the Workspace / Cloud Identity
    customer ids (`C0xxxxxxx`) whose principals may be granted IAM roles in
    the folder. Empty sets no policy.

    Off by default because it is the one policy here that can lock the owner
    out: a list that does not include the owner's own directory refuses every
    later grant to the owner's account, including the ones a recovery needs.
    Set it only to ids you have read back from `gcloud organizations list`.
  EOT
  type        = list(string)
  default     = []

  validation {
    condition     = alltrue([for id in var.allowed_policy_member_customer_ids : can(regex("^C0[0-9a-z]{7,}$", id))])
    error_message = "Each entry is a directory customer id, `C0` followed by letters and digits (gcloud organizations list --format='value(owner.directoryCustomerId)')."
  }
}

# The APIs each environment project starts with.
#
# This is `local.always` in `infrastructure/terraform/modules/services/main.tf`,
# key for key, and `qip-acceptance`'s `tenant_foundation` suite fails if the
# two differ. The platform root enables these itself on its first apply — but
# `scripts/bootstrap-deploy.sh` runs before that apply and needs IAM, STS and
# Storage on to create the bootstrap identity and the state bucket, so a
# project this root creates must not arrive bare. The flagged APIs (Cloud
# Build, GKE, BigQuery, …) are left to the platform root, which enables each
# only when its switch is on.
variable "platform_apis" {
  description = "APIs enabled in every environment project: the platform root's unconditional set."
  type        = list(string)
  default = [
    "serviceusage.googleapis.com",
    "cloudresourcemanager.googleapis.com",
    "iam.googleapis.com",
    "iamcredentials.googleapis.com",
    "sts.googleapis.com",
    "orgpolicy.googleapis.com",
    "compute.googleapis.com",
    "run.googleapis.com",
    "dns.googleapis.com",
    "cloudkms.googleapis.com",
    "secretmanager.googleapis.com",
    "pubsub.googleapis.com",
    "artifactregistry.googleapis.com",
    "storage.googleapis.com",
    "monitoring.googleapis.com",
    "logging.googleapis.com",
    "binaryauthorization.googleapis.com",
    "containeranalysis.googleapis.com",
    "containerscanning.googleapis.com",
    "cloudasset.googleapis.com",
  ]
}
