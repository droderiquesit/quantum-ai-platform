variable "project_id" {
  type        = string
  description = "The environment's own project. Never one another environment uses."
}

variable "environment" {
  type        = string
  description = "dev, test, stage or prod. It reaches the zone's resource name, so two environments that both created a zone for the same domain would at least be distinguishable in the console — but they must not both create one at all, and the root's `count` plus `a_single_environment_owns_the_dns_zone_for_the_domain` in the infrastructure suite is what stops that."
}

variable "labels" {
  type        = map(string)
  description = "The platform labels, for the zone."
  default     = {}
}

variable "domain" {
  type        = string
  description = <<-EOT
    The domain this zone is authoritative for, without a trailing dot —
    `algorik.ai`.

    **Setting this is a delegation, not a record.** Creating the zone changes
    nothing on the internet; the domain keeps answering from whatever
    nameservers the registrar names until somebody replaces those with the four
    in this module's `nameservers` output. From that moment every name under
    the domain is answered out of this state file, which is the whole point and
    also the reason the variable is empty in every environment but the one that
    owns it.
  EOT

  validation {
    # The value is concatenated with a trailing dot to make a Cloud DNS
    # `dns_name`, and it is the suffix every record in the zone is checked
    # against. A scheme, a path, a port or an upper-case letter would reach
    # the API as part of the zone's name; Google refuses some of those and
    # accepts others as a zone nothing will ever be delegated to, which is
    # worse, because it applies cleanly and never answers.
    condition     = can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$", var.domain))
    error_message = "domain must be a dotted lowercase DNS name with no scheme, port, path, wildcard or trailing dot — algorik.ai, not https://algorik.ai/ and not algorik.ai. (with the dot). The trailing dot is added by this module; supplied here it becomes `algorik.ai..`, which is a zone name nothing resolves."
  }
}

variable "a_records" {
  type = map(object({
    address     = string
    ttl_seconds = number
  }))

  description = <<-EOT
    The A records this zone serves, keyed by the fully qualified name without a
    trailing dot — `portal.algorik.ai` — and valued by the IPv4 address and the
    time to live chosen for it.

    **Every address here comes from the output of the module that reserved it.**
    A literal pasted in is a second claim about the same fact, and the two
    disagree the first time an address is released and re-reserved: the record
    keeps resolving, the certificate keeps serving, and the name points at
    somebody else's load balancer. `the_dns_records_take_their_addresses_from_the_modules_that_reserved_them`
    in the infrastructure suite refuses a literal in the root's wiring.

    The time to live is per record and has no default, deliberately. A TTL is a
    promise about how long a mistake lasts, and a promise nobody made is one
    nobody can be held to; the root states the number and the reason beside the
    record it applies to.
  EOT

  default = {}

  validation {
    # The key becomes a record name. A wildcard, a scheme or an upper-case
    # letter reaches Cloud DNS as a name, and the ones it accepts are the
    # dangerous half: `*.algorik.ai` applies cleanly and answers for every
    # host nobody enumerated, including the ones a future front door will
    # want to own.
    condition = alltrue([
      for name in keys(var.a_records) :
      can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$", name))
    ])
    error_message = "every a_records key must be a dotted lowercase DNS name with no scheme, port, path, wildcard or trailing dot — portal.algorik.ai. A wildcard is refused with the rest: it applies cleanly and then answers for every name nobody enumerated."
  }

  validation {
    # An `A` rrdata that is not a dotted quad is refused by Cloud DNS at
    # apply. That refusal arrives after the zone has been created and, if it
    # is the only failure in the run, after the delegation has already been
    # made — so the domain is live and the name is missing. This refuses it
    # while it is still a plan.
    #
    # `cidrhost` rather than a four-octet regex: a regex that admits `999` is
    # the usual shape of this check, and an octet out of range is exactly what
    # a truncated copy-paste produces. `cidrhost("$addr/32", 0)` parses the
    # address the way the provider will and fails on anything else.
    #
    # The digits-and-dots test in front of it is not redundant. `cidrhost`
    # parses an IPv6 prefix quite happily, so `2001:db8::1` passes it alone —
    # and an IPv6 address in an `A` record is refused by Cloud DNS at exactly
    # the apply-time moment this validation exists to move.
    condition = alltrue([
      for record in values(var.a_records) :
      can(regex("^[0-9]+(\\.[0-9]+){3}$", record.address)) && can(cidrhost("${record.address}/32", 0))
    ])
    error_message = "every a_records address must be an IPv4 address — 203.0.113.10. Cloud DNS refuses anything else when the record is created, which is after the zone exists and possibly after the domain has been delegated to it, so the name is simply missing."
  }

  validation {
    # The window, and both ends of it are a decision.
    #
    # Below sixty seconds most resolvers floor the value anyway, so the number
    # stops describing what actually happens — and a record nobody can cache is
    # a query bill for a name three people type.
    #
    # Above a day a wrong record is wrong for a day. The names in this zone
    # front Google-managed certificates, which stay in PROVISIONING until the
    # name resolves to the address on the load balancer; a stale record cached
    # for 86,401 seconds is a certificate that does not issue for a day and an
    # operator with no way to tell whether the fix worked.
    condition = alltrue([
      for record in values(var.a_records) :
      record.ttl_seconds >= 60 && record.ttl_seconds <= 86400
    ])
    error_message = "every a_records ttl_seconds must be between 60 and 86400. Under a minute most resolvers floor it and the number stops describing anything; over a day a wrong record is wrong for a day, and a Google-managed certificate waiting on it stays in PROVISIONING for that day."
  }
}

variable "dnssec_enabled" {
  type = bool

  description = <<-EOT
    Whether Cloud DNS signs this zone.

    **On, and the reason is the asymmetry between signing and validating.**
    Signing costs nothing an unsigned zone does not already cost and changes
    nothing a resolver does, because a resolver validates only when the parent
    publishes a DS record — and the parent here is the registrar, which this
    repository cannot reach. So a signed zone with no DS resolves exactly as an
    unsigned one does. Turning signing on later, on a zone the domain is
    already delegated to, means a key rollover on a live zone, which is not the
    afternoon anybody wants to first read Cloud DNS's key model.

    The DS record is the half that is not reversible, and it is deliberately
    left to the owner. `ds_record` below is what to publish; publishing it
    couples the *whole domain's* resolution to this zone's continued existence
    with these keys, and `infra.yml down` destroys this zone. A DS at the
    registrar pointing at keys that no longer exist takes `algorik.ai` dark for
    every validating resolver — mail included — and the fix is at the
    registrar, the one place nothing here can act. So sign now, publish the DS
    when dev has stopped being a thing that gets torn down.
  EOT

  default = true
}
