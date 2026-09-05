variable "environment" {
  description = "Which environment this posture is for: dev, test, stage or prod. An input rather than a label, because the refusal on `access_posture` reads it."
  type        = string

  # No default and never null: the prod refusal is only as good as this value,
  # and a null here would make `contains` fail on its argument instead of the
  # posture failing on its environment.
  nullable = false

  validation {
    condition     = contains(["dev", "test", "stage", "prod"], var.environment)
    error_message = "The environment must be dev, test, stage or prod."
  }
}

variable "access_posture" {
  description = <<-EOT
    How OpenObserve is reached from outside the VPC. ADR 0033's decision, as
    the one value somebody has to read to answer "who can see the platform's
    telemetry".

      * `authenticated` — the service sits behind Identity-Aware Proxy on a
        load balancer, its own `run.app` URL stops answering the internet, and
        `roles/run.invoker` is held by the principals in `access_principals`.
        ADR 0033 requires this before the first byte of platform telemetry
        reaches the store, and it is the default here.
      * `anonymous` — ADR 0030's posture: `INGRESS_TRAFFIC_ALL` and the
        anonymous invoker, on the owner's instruction, argued for a service
        holding nothing. This module names no anonymous principal; the binding
        is the manifest's, under the record that argued it, and this value is
        what makes that choice legible in the Terraform rather than inferable
        from a manifest.

    Defaulted to the restrictive value, because the two postures are not
    symmetrical. Anonymous was argued once, for an empty service, in one
    environment; a caller that has not made that argument has not made a
    choice, and the default it falls into should be the one that needs no
    record.
  EOT

  type    = string
  default = "authenticated"

  # A caller passing `null` — a lookup that missed, an optional attribute
  # nobody set — gets the default rather than a null, and the refusals below
  # then read a real value. Without this the first plan carrying a null ends
  # on `contains(...)` with a null argument: an internal error naming a
  # function, in place of the sentence that says which posture is missing.
  # That failure has happened in this tree once already, on a validation
  # dereferencing a variable at its own default of null.
  nullable = false

  validation {
    condition     = contains(["authenticated", "anonymous"], var.access_posture)
    error_message = "The OpenObserve access posture is `authenticated` (ADR 0033: Identity-Aware Proxy, named principals) or `anonymous` (ADR 0030: reachable by anyone). A third spelling is refused rather than mapped onto one of them, because the mapping would be a guess about who may read this platform's telemetry."
  }

  validation {
    # The refusal ADR 0033 makes at plan time. `prod` never had ADR 0030's
    # argument: that record priced anonymous exposure of an empty dev service
    # and named the trigger that ends it. A production store holds cycle
    # counts, refusals by gate, limit breaches and fills — a description of
    # how this desk trades — behind a write path anyone could reach, and an
    # unauthenticated write path makes every claim read out of it
    # unfalsifiable.
    #
    # Written as a refusal of the one combination rather than as a whitelist
    # of environments, so a fifth environment does not silently inherit
    # permission nobody granted it.
    condition     = !(var.environment == "prod" && var.access_posture == "anonymous")
    error_message = "access_posture is `anonymous` for prod. ADR 0033 moves OpenObserve to the authenticated posture before it holds telemetry, and ADR 0030's anonymous exposure was argued for an empty dev service and for nothing else. Leave access_posture at its default of `authenticated`. If prod must genuinely be reachable without a credential, that is a new record amending ADR 0033 and this refusal, not a value passed here."
  }
}

variable "access_principals" {
  description = <<-EOT
    Who may invoke OpenObserve under the authenticated posture, as IAM
    members: `user:`, `group:`, `serviceAccount:`, `domain:` or `principalSet:`.

    Empty by default, which grants nobody. That is the fail-closed end of ADR
    0033's cost — "access becomes a grant, which is a small administrative act
    each time" — and it is deliberately not a default naming an operator: a
    default principal is a grant nobody reviewed, and the first person to
    notice it would be whoever found they could read the store.

    Ignored under the anonymous posture, where the invoker is the manifest's.
  EOT

  type    = list(string)
  default = []

  # Null is the empty grant, not an iteration error. Both refusals below walk
  # this list, and a `for` expression over a null value fails with "a null
  # value cannot be used as the collection", which names neither the input nor
  # the decision it is missing.
  nullable = false

  validation {
    # Structural: every IAM member form that names a principal carries a
    # prefix, and the two that name everybody carry none. A member that
    # matches this cannot be either of them.
    condition     = alltrue([for member in var.access_principals : can(regex("^(user|group|serviceAccount|domain|principal|principalSet):.+$", member))])
    error_message = "An OpenObserve access principal is an IAM member with its prefix: user:, group:, serviceAccount:, domain:, principal: or principalSet:. A bare token is refused, because the two bare tokens IAM accepts are the ones that name everybody."
  }

  validation {
    # And by name, beside the structural rule rather than instead of it. The
    # structural rule already excludes both; this one exists so the error a
    # caller reads names the mistake they made — reaching for the anonymous
    # principal under the posture that exists to end it — rather than
    # reporting a malformed member.
    condition     = length([for member in var.access_principals : member if member == "allUsers" || member == "allAuthenticatedUsers"]) == 0
    error_message = "An OpenObserve access principal may not be `allUsers` or `allAuthenticatedUsers`. The first is the binding ADR 0033 exists to close; the second reads as authenticated in an audit and admits every Google account in existence. Name the operators, the group, or the service account that needs it."
  }
}
