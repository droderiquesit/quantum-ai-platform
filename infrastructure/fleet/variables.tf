variable "project_id" {
  description = "The Google Cloud project the fleet runs in (algorik-platform-dev, from the environment's tfvars)."
  type        = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.project_id))
    error_message = "The project id must be a valid Google Cloud project identifier."
  }

  # The placeholder an environment carries before a project exists. A plausible
  # id pointing at nothing fails much later, with a message about something else.
  validation {
    condition     = var.project_id != "unprovisioned"
    error_message = "project_id is 'unprovisioned': there is no project to put the fleet in. Create one and set its id."
  }
}

variable "region" {
  description = "Region for the Job and the bucket."
  type        = string
  default     = "us-central1"
}

variable "environment" {
  description = "The environment label. prod is refused: the fleet is development tooling (ADR 0102)."
  type        = string
  default     = "dev"

  validation {
    condition     = contains(["dev", "test", "stage"], var.environment)
    error_message = "environment must be dev, test or stage. The fleet is never in prod."
  }
}

variable "owner" {
  description = "The owner label."
  type        = string
  default     = "research-desk"

  validation {
    condition     = can(regex("^[a-z][a-z0-9_-]{0,62}$", var.owner))
    error_message = "owner must be a valid label value: lower case letters, digits, hyphen, underscore."
  }
}

variable "cost_center" {
  description = "The cost-center label."
  type        = string
  default     = "fleet"

  validation {
    condition     = can(regex("^[a-z][a-z0-9_-]{0,62}$", var.cost_center))
    error_message = "cost_center must be a valid label value: lower case letters, digits, hyphen, underscore."
  }
}

variable "image" {
  description = "The worker image, pinned by digest: a policy that trusts a tag trusts whoever can push it."
  type        = string

  validation {
    condition     = can(regex("@sha256:[0-9a-f]{64}$", var.image))
    error_message = "image must be pinned by digest (name@sha256:<64 hex>), never by tag."
  }
}

variable "parallelism" {
  description = "Tasks run at once: the ladder's current rung (ADR 0102 decision 1). Starts at 8; 40 is the owner's ceiling."
  type        = number
  default     = 8

  validation {
    condition     = var.parallelism >= 1 && var.parallelism <= 40 && floor(var.parallelism) == var.parallelism
    error_message = "parallelism must be a whole number from 1 to 40. Forty is the owner's ceiling and is refused, not clamped, above it."
  }
}

variable "daily_usd_ceiling" {
  description = "The fleet-attributable spend ceiling per day, in USD. Handed to the dispatcher; it is not enforced by Google."
  type        = number
  default     = 25

  validation {
    condition     = var.daily_usd_ceiling > 0 && var.daily_usd_ceiling <= 25
    error_message = "daily_usd_ceiling must be above 0 and at most 25 (ADR 0102 decision 5). Raising it is an amendment to the record, not an edit here."
  }
}

variable "per_slot_usd_cap" {
  description = "The per-slot daily cap in USD: the 20 USD allowance over 40 slots."
  type        = number
  default     = 0.5

  validation {
    condition     = var.per_slot_usd_cap > 0 && var.per_slot_usd_cap <= 0.5
    error_message = "per_slot_usd_cap must be above 0 and at most 0.50 (ADR 0102 decision 5)."
  }
}

variable "task_timeout_seconds" {
  description = "Per-task wall-clock limit. A task that reaches it is stopped and its partial output judged as it is."
  type        = number
  default     = 600

  validation {
    condition     = var.task_timeout_seconds >= 30 && var.task_timeout_seconds <= 3600
    error_message = "task_timeout_seconds must be between 30 and 3600."
  }
}

variable "retention_days" {
  description = "Days packets, outputs and ledger objects are kept. HALT is never aged out."
  type        = number
  default     = 30

  validation {
    condition     = var.retention_days >= 1
    error_message = "retention_days must be at least 1."
  }
}

variable "worker_may_call_models" {
  description = "The harder stop (ADR 0102 decision 5): false removes the worker's aiplatform.user binding and stops spend at the source."
  type        = bool
  default     = true
}

variable "enable_apis" {
  description = "Whether this root enables the required APIs. False: enabling is a separate, human-run step (README.md)."
  type        = bool
  default     = false
}
