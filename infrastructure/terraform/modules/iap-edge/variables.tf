variable "project_id" {
  type        = string
  description = "The environment's own project. Never one another environment uses."
}

variable "environment" {
  type        = string
  description = "dev, test, stage or prod. Reaches no resource name directly — the service name already carries it — and is here so a label or a message can say which environment's door this is."
}

variable "region" {
  type        = string
  description = "The region the serverless network endpoint group is created in. It must be the region the Cloud Run service runs in: a serverless NEG is regional and cannot name a service in another, and the failure is an apply-time 'resource not found' that reads as a missing service rather than a misplaced group."
}

variable "labels" {
  type        = map(string)
  description = "The platform labels, for the one resource here that takes them."
  default     = {}
}

variable "hostname" {
  type        = string
  description = "The public name this door answers on, e.g. portal.algorik.ai. Its A record is created at the registrar by hand and must resolve to this module's address before the managed certificate can provision."

  validation {
    # A hostname reaches a certificate's SAN list and, through IAP, an OAuth
    # redirect. Both are places where an unvalidated value is somebody else's
    # name. The same expression `modules/gitops-gateway` validates
    # `argocd_hostname` with, deliberately: two front doors validating a
    # hostname two different ways is two definitions of what a hostname is.
    condition     = can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$", var.hostname))
    error_message = "hostname must be a dotted lowercase DNS name, with no scheme, port or path — portal.algorik.ai, not https://portal.algorik.ai/ and not PORTAL.ALGORIK.AI. A wildcard is refused with everything else: Google's managed certificates do not issue for one, and a wildcard here would name hosts nobody enumerated."
  }
}

variable "service_name" {
  type        = string
  description = <<-EOT
    The Cloud Run service this door fronts, by its full name — `qip-dev-portal`,
    not `portal`.

    Every resource in this module is named `<service_name>-iap`, so the name
    is the whole identity of the door as well as the backend it points at. A
    serverless NEG names a service in its own project and region and nothing
    else; there is no way to front two, and this module does not pretend
    otherwise.
  EOT

  validation {
    condition     = can(regex("^[a-z]([a-z0-9-]*[a-z0-9])?$", var.service_name))
    error_message = "A Cloud Run service name is lower case, starts with a letter, and contains only letters, digits and hyphens."
  }

  validation {
    # Compute resource names are capped at 63 characters and every resource
    # here appends `-iap`, with the forwarding rule appending `-https` on top.
    # Google refuses that at apply, which is after the certificate has been
    # ordered and the address reserved; this refuses it at plan.
    condition     = length("${var.service_name}-iap-https") <= 63
    error_message = "The derived resource name ${var.service_name}-iap-https is longer than the 63 characters Compute allows. Shorten the service name."
  }
}

variable "trust_zone" {
  type        = string
  description = <<-EOT
    The trust zone the catalogue places the fronted service in.

    Not decoration and not documentation. Blueprint §40.5 is explicit that
    customer traffic and trading traffic never share a load balancer, an
    identity, a credential or a route, and §40.14 lists what a client may
    never reach. This module refuses, at plan time, to put a backend in front
    of any zone but the two §46.1 marks client-reachable.

    An identity check does not change that answer. IAP decides *who* may pass;
    it says nothing about what is on the other side, and a door with a lock on
    it is still a door into the room it opens onto.
  EOT
}

variable "rate_limit_requests_per_minute" {
  type        = number
  description = <<-EOT
    How many requests one client address may make in a minute before Cloud
    Armor bans it for ten.

    This binds an *admitted* caller: Cloud Armor is attached to the backend
    service, so IAP has already authenticated whoever reaches it. The number
    is therefore about a session that has started behaving like a script, not
    about an anonymous flood — Google's front end absorbs that before this
    policy is consulted.
  EOT

  default = 600

  validation {
    condition     = var.rate_limit_requests_per_minute >= 1 && var.rate_limit_requests_per_minute <= 100000
    error_message = "The rate limit is at least one request a minute and at most a hundred thousand. A limit of zero refuses every client, and a limit large enough not to bind is a control that reads as protection and cannot fire."
  }
}
