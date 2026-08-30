variable "enabled" {
  description = "Whether this environment runs customer identity at all. Off by default so a plan against an environment that has not opted in is a no-op."
  type        = bool
  default     = false
}

variable "project_id" {
  description = "Project hosting Identity Platform. The customer directory lives and dies with this project."
  type        = string
}

variable "authorized_domains" {
  description = "Domains authentication may redirect back to. Real deployment outputs and algorik.ai domains only — a wildcard here would authorize redirects to hosts nobody reviewed."
  type        = list(string)
  default     = ["localhost"]

  validation {
    condition     = !contains([for d in var.authorized_domains : length(trimspace(d)) > 0], false)
    error_message = "An empty authorized domain authorizes nothing and masks a templating bug upstream. Remove it."
  }
}

variable "enable_email_password" {
  description = "Email and password sign-in. The launch method; disable only once every account has another way in."
  type        = bool
  default     = true
}

variable "mfa_state" {
  description = "Multi-factor posture: OFF, ENABLED (optional for users), or MANDATORY. MANDATORY locks out every unenrolled account the moment it applies — flip it deliberately, with an enrolment campaign behind it."
  type        = string
  default     = "ENABLED"

  validation {
    condition     = contains(["OFF", "ENABLED", "MANDATORY"], var.mfa_state)
    error_message = "mfa_state must be OFF, ENABLED, or MANDATORY."
  }
}

variable "sign_up_quota_per_hour" {
  description = "New-account ceiling per hour. A bot burst that outruns review is cheaper to refuse at the platform than to clean out of the directory afterwards."
  type        = number
  default     = 100

  validation {
    condition     = var.sign_up_quota_per_hour > 0 && var.sign_up_quota_per_hour <= 10000
    error_message = "The sign-up quota must be positive, and above 10000/hour it is not a guard."
  }
}
