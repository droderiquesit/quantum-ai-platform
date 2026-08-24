variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "network_id" {
  description = "The VPC the attachments terminate in and the endpoint answers from."
  type        = string
}

# --- Partner interconnect ---------------------------------------------------

variable "enable_partner_interconnect" {
  description = <<-EOT
    Whether to create the Cloud Router and the VLAN attachments.

    Default false, and this is the flag most worth leaving alone until a
    circuit has actually been ordered. An attachment Terraform creates sits in
    `PENDING_PARTNER` until a partner provisions the other half; once they do,
    it bills hourly whether or not anything routes over it. The larger cost is
    not the bill: an attachment that exists reads, to anyone looking at the
    project or a diagram, as a private path that works. It does not work until
    a partner, a circuit, a pairing key and a VLAN attachment on their side all
    exist, and Terraform can create none of those. NOT-ORDERED.md lists what a
    deployment has to arrange first.
  EOT
  type        = bool
  default     = false
}

variable "partner_interconnects" {
  description = <<-EOT
    The VLAN attachments to create, keyed by a short name that becomes part of
    the resource name — `chicago-a`, `chicago-b`, `newyork-a`.

    Empty by default. The three cells this exists for — `chicago-1`,
    `newyork-1` and `dubai-1` — are the ones `environments/prod/terraform.tfvars` records as being
    400, 300 and 380 kilometres from the venues they trade, and a colocated
    cage with a circuit back to the VPC is the first of the three honest
    answers `docs/operations/deploying-an-edge-cell.md` names. Naming them here
    does not create the cage.

    Two entries per site, in different edge availability domains, or the pair
    is one circuit twice: a single metro maintenance window takes both, and the
    redundancy that was paid for was never there.

      * `region` is the Google Cloud region the attachment terminates in. It
        must be a region the chosen colocation facility actually reaches; the
        partner's own coverage map decides this, not this file.
      * `edge_availability_domain` is which of that region's two domains.
      * `admin_enabled` defaults false, so a circuit the partner activates does
        not start carrying routes before somebody looks at what is on the far
        end.
  EOT

  type = map(object({
    region                   = string
    edge_availability_domain = string
    admin_enabled            = optional(bool, false)
    description              = optional(string, "")
  }))

  default = {}

  validation {
    condition = alltrue([
      for attachment in values(var.partner_interconnects) :
      contains(["AVAILABILITY_DOMAIN_1", "AVAILABILITY_DOMAIN_2"], attachment.edge_availability_domain)
    ])
    error_message = "Name the edge availability domain: AVAILABILITY_DOMAIN_1 or AVAILABILITY_DOMAIN_2. AVAILABILITY_DOMAIN_ANY lets Google place both halves of a redundant pair in the same domain."
  }
}

variable "cloud_router_asn" {
  description = <<-EOT
    The Cloud Router's BGP ASN, on the VPC side of the session.

    A private ASN, and not the one the colocated equipment uses: two ends
    claiming the same ASN never establish a session, and the symptom is an
    attachment that stays `PENDING_PARTNER` for days after the partner reports
    the circuit is up.
  EOT
  type        = number
  default     = 64514

  validation {
    condition = (
      (var.cloud_router_asn >= 64512 && var.cloud_router_asn <= 65534) ||
      (var.cloud_router_asn >= 4200000000 && var.cloud_router_asn <= 4294967294)
    )
    error_message = "The ASN must be private: 64512-65534, or 4200000000-4294967294 for a 32-bit one. A public ASN this project does not hold is somebody else's."
  }
}

# --- Private Service Connect ------------------------------------------------

variable "enable_private_service_connect" {
  description = <<-EOT
    Whether to create an internal endpoint that answers for Google APIs.

    Default false, for the same reason as everything else optional here, plus
    one specific to this platform: an endpoint alone changes nothing. Traffic
    reaches it only when something resolves a Google API hostname to its
    address, and the DNS that does so is out of band — on the far end of the
    interconnect, in the colocated site's own resolver. See NOT-ORDERED.md,
    which also explains why repointing `*.googleapis.com` *inside* this VPC
    would break every workload rather than help it.
  EOT
  type        = bool
  default     = false
}

variable "private_service_connect_address" {
  description = <<-EOT
    The internal address the endpoint answers on.

    No default: it must not overlap any subnet range in this VPC or any range
    the far end of the interconnect uses, and only the deployment knows both.
    An address chosen here as a convenience would be the one that collides.
  EOT
  type        = string
  default     = ""

  validation {
    condition     = var.private_service_connect_address == "" || can(regex("^([0-9]{1,3}\\.){3}[0-9]{1,3}$", var.private_service_connect_address))
    error_message = "The endpoint address is a single IPv4 address, not a range."
  }

  validation {
    condition     = !var.enable_private_service_connect || var.private_service_connect_address != ""
    error_message = "Private Service Connect is enabled with no address. Choose one that overlaps neither this VPC's subnets nor the far end of the interconnect."
  }
}

variable "private_service_connect_target" {
  description = <<-EOT
    Which bundle of Google APIs the endpoint reaches.

    `vpc-sc` by default: the restricted bundle, the same set the VPC's existing
    `199.36.153.8/30` route reaches, and the only one a VPC Service Controls
    perimeter can protect. `all-apis` additionally reaches APIs no perimeter
    covers — which is precisely the hole
    `docs/operations/external-dependencies.md` describes when it explains why
    private Google access alone does not stop the fast brain reaching a model
    API.
  EOT
  type        = string
  default     = "vpc-sc"

  validation {
    condition     = contains(["vpc-sc", "all-apis"], var.private_service_connect_target)
    error_message = "The target is vpc-sc (restricted) or all-apis (everything Google publishes)."
  }
}
