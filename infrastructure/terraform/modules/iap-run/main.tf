# Identity-Aware Proxy on a Cloud Run service itself, with no load balancer,
# no reserved address, no certificate and no DNS record anywhere.
#
# ADR 0095, which narrows ADR 0094 rather than replacing it. Read the fact
# first, because the whole module rests on it and it is a fact about Google
# rather than a preference of this repository:
#
#   Cloud Run enforces IAP **on the service**, across every ingress path,
#   including the Google-issued `run.app` URL. Google's own page says it in
#   one sentence — "By enabling IAP on Cloud Run directly, you can secure
#   traffic with a single click from all ingress paths, including default
#   run.app URLs and load balancers" (cloud.google.com/run/docs/securing/
#   identity-aware-proxy-cloud-run, read 2026-09-21).
#
# A `run.app` URL is Google-issued, answers on a Google-managed certificate
# that nobody here orders or renews, and needs no registrar, no zone and no
# delegated nameserver. So an environment that owns no domain still gets a
# front door that a browser can reach and that IAP guards — which is the whole
# point of this module and the reason ADR 0094's edge is no longer the only
# shape a console door can take.
#
# ## What this module does and, more importantly, what it does not
#
# It holds **the access list and nothing else**: who may pass IAP and reach
# one named Cloud Run service. It does not create the service, does not enable
# IAP on it, and does not grant the IAP service agent the invoker role. Each
# of those lives somewhere else in this tree and each is named here, because a
# module that quietly did not do a thing a reader assumed it did is how a door
# ends up open:
#
#   * **The service** is `gitops/envs/<env>/portal.yaml`, a Config Connector
#     `RunService` since ADR 0036. Terraform releases it from state rather
#     than owning it.
#   * **The invoker grant** to IAP's service agent is the
#     `qip-<env>-portal-invoker-iap` `IAMPolicyMember` in
#     `gitops/envs/<env>/invokers.yaml`. It is the same grant Google's page
#     asks for — `roles/run.invoker` to
#     `service-<project-number>@gcp-sa-iap.iam.gserviceaccount.com` — and it
#     was already there for the load-balancer door, because IAP forwards as
#     its own agent whichever side it sits on.
#   * **The enable bit is not expressible in a `RunService` and that is an
#     upstream gap, stated rather than worked around.** Config Connector's
#     `RunService` CRD carries no `iapEnabled` field in v1.156.0, the version
#     `bootstrap/config-connector-operator` pins, nor on that project's
#     `master` — checked by fetching the CRD and grepping it, 2026-09-21. The
#     reason is mechanical: that CRD is generated from the **GA** `google`
#     provider, and `iap_enabled` exists only in `google-beta`
#     (`terraform providers schema -json` at 6.50.0: present in google-beta's
#     `google_cloud_run_v2_service`, absent from google's). So the enable bit
#     is set by `infra.yml`'s `apps` stage with `gcloud run services update
#     --iap`, which then reads `iapEnabled` back and fails the job if it is
#     not true, and this module's README says what closes the gap.
#
# ## Why the access list is here at all, when ADR 0094 refused to hold one
#
# ADR 0094 decision 3 refused a per-resource members list, and the reason was
# sound and is now spent. A GKE Gateway's backend service is named by its
# controller at reconcile time and has no Terraform address, so
# `roles/iap.httpsResourceAccessor` had to be granted at the **project**
# level; project-level IAM is inherited by every IAP-protected resource in the
# project, so a second list on the portal's own backend could only ever widen
# that set and never narrow it. A list that cannot exclude anybody is not an
# access list.
#
# IAP on Cloud Run is addressable per service. `google_iap_web_cloud_run_service_iam_member`
# takes a project, a location and a service name — three strings, none of them
# a resource this module has to own — so the portal's list can be exactly the
# people who may reach the portal, and admitting somebody to Argo CD stops
# admitting them to the console at the same time. ADR 0094's own reversal
# condition was "the backend service becoming addressable, then the grant
# narrows and decision 3 reverses on its own terms". This is that, by a route
# the ADR did not anticipate.

locals {
  # The zones a client may terminate in, from blueprint §46.1's
  # `client_reachable_zones`. Repeated here rather than passed in, exactly as
  # `modules/iap-edge` and `modules/public-edge` repeat it and for the same
  # reason: a list this module is *given* is a list the caller can widen, and
  # the point of the refusal below is that widening it is an edit to this file
  # that a reviewer sees.
  client_reachable_zones = ["public-edge", "application-identity"]

  # The service's own URL, as Cloud Run has assigned it deterministically
  # since 2024: the service name, the project number, the region. The same
  # expression `modules/cloudrun` computes `uri` with, and deliberately the
  # same one — two derivations of one address agree the day they are written
  # and disagree the first time either is edited.
  url = "https://${var.service_name}-${var.project_number}.${var.region}.run.app"
}

# The refusal §40.5 is built on, and the same one `modules/iap-edge` makes.
#
# An identity check does not change which room a door opens onto. A client
# reaches the public edge and the authenticated application surfaces — never
# Spanner, Pub/Sub, an execution node, a venue, IBM, custody or signing
# material (§40.14). A `terraform_data` rather than a `validation` block
# because there is no other resource here for a `precondition` to hang on: the
# members list is empty in every environment, so `google_iap_web_cloud_run_service_iam_member`
# has zero instances and a precondition on it would be evaluated zero times —
# a refusal that cannot fire, which is the exact defect this repository names
# `MaxExpectedShortfall` after.
resource "terraform_data" "client_reachable_zone" {
  input = var.trust_zone

  lifecycle {
    precondition {
      condition     = contains(local.client_reachable_zones, var.trust_zone)
      error_message = "The Cloud Run IAP door would front a service in the ${var.trust_zone} zone. A client reaches the public edge and the authenticated application surfaces — never Spanner, Pub/Sub, an execution node, a venue, IBM, custody or signing material (§40.14). An identity check in front of a trading zone is still a route into it, so this is refused rather than narrowed. Move the surface into application-identity, or serve it from somewhere that is not the public internet."
    }
  }
}

# The access list.
#
# `for_each` over the members rather than a `google_iap_web_cloud_run_service_iam_binding`
# over the list, and the difference is what happens to a grant this
# configuration does not know about. A `_binding` is authoritative: it removes
# every principal not in its list, so an operator granted by hand during an
# incident is revoked by the next apply, silently, with nothing in the plan
# naming them. A `_member` is additive per principal, so this file is exactly
# the set of people this repository put on the list and an apply says so one
# line at a time.
#
# Empty in every environment, and that is a posture rather than an omission:
# an IAM member is an account identifier and
# `.claude/rules/00-enterprise-governance.md` refuses one in a committed file.
# The door comes up admitting nobody, and an operator is granted by name
# afterwards — the README has the command.
resource "google_iap_web_cloud_run_service_iam_member" "console" {
  for_each = toset(var.iap_members)

  project                = var.project_id
  location               = var.region
  cloud_run_service_name = var.service_name

  role   = "roles/iap.httpsResourceAccessor"
  member = each.value
}
