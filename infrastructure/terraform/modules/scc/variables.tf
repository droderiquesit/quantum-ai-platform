variable "project_id" {
  type = string
}

variable "enable_security_command_center" {
  description = <<-EOT
    Whether to create this project's Security Command Center resources.

    **Off**, and the reason is not timidity about the resources themselves —
    they are free, and two of them are detectors this platform would genuinely
    benefit from.

    It is off because everything here only ever runs if Security Command Center
    is **activated at the organisation this project belongs to**, at the
    Premium or Enterprise tier. Activation is not a project-level act, there is
    no `org_id` in this configuration, and no resource here can check it.
    Turning this on inside an organisation that has not activated SCC creates
    two custom detectors that are accepted, stored, never evaluated, and appear
    in the console as though the project is being watched.

    That is the failure this default prevents, and it is worse than the gap it
    replaces: an absent control is visibly absent, and a control that exists
    and never fires reads as a clean result.

    `ORGANISATION-SCOPED.md` lists what has to be true first, and what stays out
    of reach afterwards. Turn this on once somebody has confirmed the
    organisation's activation and tier — and check that findings actually
    appear, because "no findings" and "no detector running" look identical from
    here.
  EOT
  type        = bool
  default     = false
}

variable "location" {
  description = <<-EOT
    The Security Command Center location these resources live in.

    `global` unless a data-residency requirement says otherwise. SCC's
    regionalised endpoints exist for jurisdictions that require findings to
    stay in a region, and choosing one narrows which findings the project can
    see: a regional configuration does not observe the global one and vice
    versa. Pick this deliberately or leave it alone.
  EOT
  type        = string
  default     = "global"
}

variable "muted_findings" {
  description = <<-EOT
    Findings this deployment has decided not to act on, keyed by mute config id.

    Empty by default, and it should stay small. Every entry is a class of
    finding that will stop being shown to anybody, so the `description` is the
    load-bearing field: it is the only record of who decided and why, and it is
    what a reviewer reads when the muted thing turns out to have mattered.

    `filter` is an SCC findings filter, for example
    `category="PUBLIC_BUCKET_ACL" AND resource.name:"qip-prod-public-docs"`.
    Prefer a filter that names the specific resource. A filter matching a whole
    category mutes findings on resources that do not exist yet — including ones
    created by someone who never read this file.

    `type` is `STATIC` or `DYNAMIC`. `STATIC` mutes matching findings for good;
    `DYNAMIC` mutes them while the filter matches and lets them reappear when
    it stops. `DYNAMIC` is the default here because a mute that expires when
    the situation changes is a mute that cannot outlive its own reasoning.
  EOT

  type = map(object({
    filter      = string
    description = string
    type        = optional(string, "DYNAMIC")
  }))

  default = {}

  validation {
    condition = alltrue([
      for mute in values(var.muted_findings) : contains(["STATIC", "DYNAMIC"], mute.type)
    ])
    error_message = "A mute config type must be STATIC or DYNAMIC."
  }

  validation {
    condition = alltrue([
      for mute in values(var.muted_findings) : length(trimspace(mute.description)) >= 20
    ])
    error_message = "Every mute needs a description saying why the finding is being ignored. A mute with no reason is indistinguishable from a finding nobody saw."
  }
}
