# Whether the execution nodes an environment declares are provisioned (W4.3).
#
# A node can now be *declared* in committed configuration — reviewed, read,
# planned by the harness — without anything being created for it. The
# declaration and the decision to provision are two different acts by two
# different people: an agent may write the first, and only the owner may
# take the second, by flipping this to true in a reviewed diff and then
# reading the plan that diff produces before anybody applies it.
#
# The flag cannot be flipped on a declaration that is not finished. A node
# needs a boot image that exists and a capital allocation a person chose,
# and neither has a default anywhere: a plausible image self-link plans
# cleanly and fails at apply after the subnet, the identity and four IAM
# bindings exist, and a defaulted allocation is the one number in a cell's
# envelope no reviewer read. So turning this on with either still null is
# refused at plan time, naming the missing value, rather than planned.
#
# Validations here refer to `var.execution_nodes`, which Terraform 1.9 permits
# (cross-variable validation); a failure stops the plan before any module is
# evaluated, which is the point — the refusal must name the act that is
# missing, not surface as a regex failure inside the node module.
variable "execution_nodes_enabled" {
  description = <<-EOT
    Whether the entries in `execution_nodes` are provisioned. False by
    default and in every environment: dev declares ADR 0035's one node with
    this off, so nothing is created until the owner flips it. True requires
    at least one declared node, and every declared node to name its baked
    `boot_image` and a positive decimal `region_allocation`.
  EOT
  type        = bool
  default     = false
  nullable    = false

  # A node verifies the centre against the committed trust bundle and speaks
  # mutual TLS on a CA Service certificate, and holds no shared key to fall
  # back to (ADR 0110, ADR 0112). Enabling one where the environment has no
  # bundle, no pinned Google roots or no CA would create a machine whose
  # binary refuses to start, restarting for ever under `Restart=always` — so
  # it is refused here, naming what is missing.
  validation {
    condition = !var.execution_nodes_enabled || (
      var.artefact_trust_bundle_file != null
      && var.google_tls_roots_file != null
      && var.certificate_authority != null
    )
    error_message = "execution_nodes_enabled is true and the environment lacks what every node needs to start: a committed trust bundle (artefact_trust_bundle_file), the pinned Google roots (google_tls_roots_file) and a CA Service pool (certificate_authority). A node holds no shared key to fall back to."
  }

  # Enabling nothing is a mistake that reads as success: the owner believes a
  # node was turned on and the plan shows no change.
  validation {
    condition     = !var.execution_nodes_enabled || length(var.execution_nodes) > 0
    error_message = "execution_nodes_enabled is true but execution_nodes declares no node. Declare the node, or leave the flag false."
  }

  validation {
    condition = !var.execution_nodes_enabled || alltrue([
      for node in values(var.execution_nodes) : node.boot_image != null
    ])
    error_message = "execution_nodes_enabled is true but a declared node has no boot_image. Dispatch .github/workflows/image.yml for that node's machine_type, write the self-link it prints into the node's entry, and only then enable it."
  }

  # A positive decimal string, never zero and never an exponent: the binary
  # parses QIP_REGION_ALLOCATION as a `Decimal`, and a value it refuses at
  # start-up is a node that restarts for ever under `Restart=always`.
  validation {
    condition = !var.execution_nodes_enabled || alltrue([
      for node in values(var.execution_nodes) :
      node.region_allocation == null ? false : (
        can(regex("^[0-9]+(\\.[0-9]+)?$", node.region_allocation)) &&
        !can(regex("^0+(\\.0+)?$", node.region_allocation))
      )
    ])
    error_message = "execution_nodes_enabled is true but a declared node has no region_allocation, or one that is not a positive decimal. It is the ceiling on the capital the cell may reserve; a person chooses it (ADR 0045 proposes one) and nothing defaults it."
  }
}
