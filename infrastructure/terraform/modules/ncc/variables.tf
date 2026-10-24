variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type    = map(string)
  default = {}
}

# The latency mesh (R12-GCP-8057): one VPC spoke per region's Reflex/Fabric
# network, each exporting only the narrow prefixes its peer endpoints and
# broker ports live on. Empty declares no hub.
variable "latency_spokes" {
  description = "Reflex/Fabric VPC spokes of the mesh-topology hub, by name: the VPC network URI and the narrow prefixes it may export."
  type = map(object({
    network_uri = string
    export      = list(string)
  }))
  default  = {}
  nullable = false

  validation {
    condition     = alltrue([for s in values(var.latency_spokes) : length(s.export) > 0])
    error_message = "Every latency spoke names the prefixes it exports; a spoke exporting everything would carry the whole VPC across regions."
  }

  validation {
    # Narrow means narrow: the peer endpoints and broker ports of one
    # region fit in a /24. A wider prefix is a subnet's worth of machines
    # reachable across the mesh, which is exactly what the hub exists not to
    # carry.
    condition = alltrue(flatten([for s in values(var.latency_spokes) : [
      for p in s.export : can(cidrnetmask(p)) && tonumber(split("/", p)[1]) >= 24
    ]]))
    error_message = "A latency spoke exports only /24 or narrower IPv4 prefixes."
  }
}

# The service/data star (R12-GCP-8058): centre spokes (the inspection and
# management VPCs) and edge spokes (GKE, data, AI). Edge spokes reach each
# other only through the centre.
variable "star_spokes" {
  description = "Spokes of the star-topology hub, by name: the VPC network URI and its group, center or edge."
  type = map(object({
    network_uri = string
    group       = string
  }))
  default  = {}
  nullable = false

  validation {
    condition     = alltrue([for s in values(var.star_spokes) : contains(["center", "edge"], s.group)])
    error_message = "A star spoke's group is center or edge."
  }

  validation {
    condition = length(var.star_spokes) == 0 || (
      anytrue([for s in values(var.star_spokes) : s.group == "center"]) &&
      anytrue([for s in values(var.star_spokes) : s.group == "edge"])
    )
    error_message = "A star hub needs at least one center spoke, where inspection happens, and one edge spoke; without a center every edge reaches every other directly."
  }
}
