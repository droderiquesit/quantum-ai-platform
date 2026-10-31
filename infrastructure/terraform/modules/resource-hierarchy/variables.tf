# The organisation, or a folder, the platform's folders are created under.
variable "parent" {
  description = "organizations/<number> or folders/<number> the platform folder is created under."
  type        = string

  validation {
    condition     = can(regex("^(organizations|folders)/[0-9]+$", var.parent))
    error_message = "parent is organizations/<number> or folders/<number>."
  }
}

variable "billing_account" {
  description = "The billing account every project is attached to, as XXXXXX-XXXXXX-XXXXXX."
  type        = string

  validation {
    condition     = can(regex("^[0-9A-F]{6}-[0-9A-F]{6}-[0-9A-F]{6}$", var.billing_account))
    error_message = "billing_account is the 18-character account id, XXXXXX-XXXXXX-XXXXXX."
  }
}

# A short token every project id starts with, so ids are globally unique
# without being guessable from the environment name alone.
variable "prefix" {
  type = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9]{1,4}$", var.prefix))
    error_message = "prefix is 2 to 5 lower-case letters and digits, starting with a letter."
  }
}

# Every environment gets the same planes (R12-GCP-8052): non-production
# mirrors production's boundaries at lower scale, so a boundary that only
# prod has is a boundary nobody tested.
variable "environments" {
  description = "Environment name to the execution regions it runs in, e.g. { prod = [\"us-east4\", \"europe-west2\"], dev = [\"us-east4\"] }."
  type        = map(list(string))

  validation {
    condition     = contains(keys(var.environments), "prod")
    error_message = "environments names prod; the hierarchy is production's boundaries mirrored downward, not the reverse."
  }

  validation {
    condition     = alltrue([for name in keys(var.environments) : can(regex("^[a-z]{2,8}$", name))])
    error_message = "An environment name is 2 to 8 lower-case letters."
  }

  validation {
    condition     = alltrue([for regions in values(var.environments) : length(regions) > 0 && length(regions) == length(distinct(regions))])
    error_message = "Every environment names at least one region, each once."
  }

  validation {
    # A project id is at most 30 characters: <prefix>-<env>-<plane>-<region>,
    # with "fabric" the longest regional plane token and "ledger" the longest
    # global one. Refused here rather than truncated: a shortened id is a
    # name nobody chose, and the provider's own check would fail mid-plan.
    condition = alltrue(flatten([for env, regions in var.environments : concat(
      [length("${var.prefix}-${env}-ledger") <= 30],
      [for r in regions : length("${var.prefix}-${env}-fabric-${r}") <= 30]
    )]))
    error_message = "A plane's project id would be longer than 30 characters; choose a shorter prefix or environment name."
  }

  validation {
    condition = alltrue(flatten([for regions in values(var.environments) : [
      for r in regions : can(regex("^[a-z]+-[a-z]+[0-9]$", r))
    ]]))
    error_message = "A region is a Google Cloud region name such as us-east4."
  }
}

variable "labels" {
  type    = map(string)
  default = {}
}
