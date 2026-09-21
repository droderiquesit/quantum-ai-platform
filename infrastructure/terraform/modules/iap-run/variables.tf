variable "project_id" {
  type        = string
  description = "The environment's own project. Never one another environment uses."
}

variable "project_number" {
  type        = string
  description = "The project's numeric id. It is half of the Google-issued `run.app` hostname — `qip-dev-portal-95200532413.us-east4.run.app` — so a wrong number here is a URL that belongs to somebody else's project and resolves to nothing."

  validation {
    condition     = can(regex("^[0-9]+$", var.project_number))
    error_message = "project_number is the project's numeric id, digits only. `algorik-dev` is the project *id* and goes in project_id."
  }
}

variable "environment" {
  type        = string
  description = "dev, test, stage or prod. Reaches no resource name — the service name already carries it — and is here so a message can say which environment's door this is."
}

variable "region" {
  type        = string
  description = "The region the Cloud Run service runs in. It is both half of the `run.app` hostname and the `location` the IAP binding is made in: IAP on Cloud Run is addressed per region, and a binding made in the wrong one applies cleanly and admits nobody to the service anybody is actually trying to reach."
}

variable "service_name" {
  type        = string
  description = <<-EOT
    The Cloud Run service this door guards, by its full name —
    `qip-dev-portal`, not `portal`.

    The service itself is a Config Connector `RunService` under
    `gitops/envs/<env>/`, not a resource this module creates. What is named
    here is the string the IAP binding and the derived URL are built from, so
    a name that does not match the manifest is an access list on a service
    that does not exist — which applies cleanly and reports nothing.
  EOT

  validation {
    condition     = can(regex("^[a-z]([a-z0-9-]*[a-z0-9])?$", var.service_name))
    error_message = "A Cloud Run service name is lower case, starts with a letter, and contains only letters, digits and hyphens."
  }
}

variable "trust_zone" {
  type        = string
  description = <<-EOT
    The trust zone the fronted service sits in.

    Not decoration and not documentation. Blueprint §40.5 is explicit that
    customer traffic and trading traffic never share a load balancer, an
    identity, a credential or a route, and §40.14 lists what a client may
    never reach. This module refuses, at plan time, to put an internet-facing
    door in front of any zone but the two §46.1 marks client-reachable — the
    same refusal `modules/iap-edge` makes, because the two modules can each
    publish a Cloud Run service and a rule true of only one of them is not a
    rule.

    An identity check does not change that answer. IAP decides *who* may pass;
    it says nothing about what is on the other side.
  EOT
}

variable "iap_members" {
  type        = list(string)
  description = <<-EOT
    Exactly who may pass Identity-Aware Proxy and reach this service, as IAM
    members (`user:someone@example.com`, `group:...`).

    **Per service, which is the thing ADR 0094 could not have.** The
    load-balancer door's list had to be granted at the project level — a GKE
    Gateway's backend service is named by its controller and has no Terraform
    address — and project-level IAM is inherited, so that one grant admitted a
    person to Argo CD, to Kargo and to the console at once. IAP on Cloud Run
    is addressable per service, so this list narrows rather than widens, and
    somebody who may operate the control plane is no longer automatically
    somebody who may read the book.

    Empty by default and empty in every environment. An IAM member is an
    account identifier and this repository does not carry those; the door
    comes up admitting nobody, which is the correct posture for a console
    whose readers have not been named yet, and an operator is granted out of
    band. `allUsers` and `allAuthenticatedUsers` are refused below: the first
    is the public internet and the second is every Google account in
    existence, and either one turns the gate this module exists to hold into
    a formality.
  EOT

  default = []

  validation {
    condition = alltrue([
      for member in var.iap_members :
      !contains(["allUsers", "allAuthenticatedUsers"], member)
    ])
    error_message = "iap_members may not contain allUsers or allAuthenticatedUsers. The first admits the public internet to the console and the second admits every Google account in existence; neither is ever the right answer for a surface that renders the platform's book."
  }
}
